//! Claude Code's hooks (architecture v3 §7.1, P0 S1): Lila gives each Claude thread a settings
//! file whose hooks run `lila hook <event>`, which hands the event to lilad. Every tool call is
//! judged by the thread's tier before it runs (`PreToolUse`: allow, deny, or ask); an ask makes
//! Claude Code show its dialog, which reaches lilad as a `PermissionRequest` and becomes a card the
//! person answers (the hook waits for it, `hook.answer`). The reply being written shows as it
//! comes (`MessageDisplay`).

use super::permissions::Decided;
use super::*;
use crate::policy::{self, Danger, Verdict};
use lila_agent::{PermKind, PermOption};
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};

/// How long Claude Code lets the `PermissionRequest` hook wait for the person (seconds). Claude Code 2.1.322 clamps a hook's
/// timeout to 2^31-1 ms (about 24.8 days, the most a timer can hold) and honours anything below it, so a card stays answerable
/// for as long as the thread lives; the hook process itself waits without a limit of its own (`client::hook`).
pub const PERMISSION_WAIT_SECS: u64 = 2_000_000;

/// The events Lila hooks, and how long Claude Code waits for each (seconds).
pub const HOOKED: [(&str, u64); 6] = [("PreToolUse", 10), ("PermissionRequest", PERMISSION_WAIT_SECS), ("MessageDisplay", 10), ("UserPromptSubmit", 30), ("Stop", 10), ("StopFailure", 10)];

/// A card whose hook has not asked after it for this long (ms) has lost its hook (Claude Code killed it because the person
/// answered in the terminal or interrupted the turn): the card goes. The hook asks every half second.
const HOOK_GONE_MS: i64 = 30_000;

/// How many tool calls' verdicts are kept per thread, and for how long (ms): a `PermissionRequest`
/// follows its `PreToolUse` within moments.
const JUDGED_KEEP: usize = 64;
const JUDGED_MS: i64 = 600_000;

/// What `PreToolUse` decided about one tool call: asked about (the reason and danger for the card),
/// or let through.
#[derive(Debug)]
pub(super) struct Judged {
    key: u64,
    at_ms: i64,
    ask: Option<(String, Option<Danger>)>,
}

/// Names a tool call across the two hooks: `PermissionRequest` carries no `tool_use_id`, but the
/// tool and its input are the same in both.
fn call_key(payload: &Value) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    payload["tool_name"].as_str().hash(&mut h);
    payload["tool_input"].to_string().hash(&mut h);
    h.finish()
}

fn options() -> Vec<PermOption> {
    vec![PermOption { id: "allow".into(), label: "允许".into(), kind: PermKind::Allow }, PermOption { id: "deny".into(), label: "拒绝".into(), kind: PermKind::Reject }]
}

/// The settings file that hooks a Claude thread's session: each event runs `lila hook <event>`.
pub fn settings(lila_bin: &Path) -> Value {
    let bin = format!("'{}'", lila_bin.to_string_lossy().replace('\'', "'\\''"));
    let hooks: serde_json::Map<String, Value> = HOOKED
        .iter()
        .map(|(event, timeout)| ((*event).to_owned(), json!([{"matcher": "*", "hooks": [{"type": "command", "command": format!("{bin} hook {event}"), "timeout": timeout}]}])))
        .collect();
    json!({ "hooks": hooks })
}

impl<T: Terminals> Daemon<T> {
    /// `hook {thread, event, payload}` from a Claude thread's session.
    pub(super) fn hook(&mut self, params: &Value, now: i64) -> Result<Value> {
        let t = self.thread_of(params)?;
        let tid = t.id.clone();
        let payload = &params["payload"];
        match req(params, "event")? {
            "PreToolUse" => Ok(self.pre_tool_use(&t, payload, now)?),
            "PermissionRequest" => self.permission_hook(&t, payload, None, now),
            "MessageDisplay" => {
                let message = payload["message_id"].as_str().unwrap_or_default().to_owned();
                let delta = payload["delta"].as_str().unwrap_or_default();
                let entry = self.writing.entry(tid.clone()).or_insert_with(|| (message.clone(), String::new()));
                if entry.0 != message {
                    *entry = (message, String::new());
                }
                entry.1.push_str(delta);
                self.mark(&tid);
                Ok(json!({}))
            }
            // A turn starts or ends: whatever was being written is in the transcript now.
            // The transcript is read now, not at the next round of following: what the turn wrote and the
            // state it ends in go out in the same push, so the reply is never later than the idle.
            "UserPromptSubmit" => {
                self.follow_transcripts();
                if self.writing.remove(&tid).is_some() {
                    self.mark(&tid);
                }
                Ok(json!({}))
            }
            // Claude Code can end the turn before its transcript has the reply: what it wrote stays
            // shown until following the transcript finds the reply there (`follow_transcripts`).
            "Stop" => {
                self.follow_transcripts();
                self.drop_hook_cards(&tid, "the turn ended", now);
                self.refresh_changes(&tid);
                Ok(json!({}))
            }
            "StopFailure" => {
                let why = payload["error"].as_str().or(payload["message"].as_str()).or(payload["reason"].as_str()).map(String::from).unwrap_or_else(|| payload.to_string());
                self.follow_transcripts();
                self.writing.remove(&tid);
                self.store.event(&tid, "claude", "turn_failed", &why, now)?;
                self.drop_hook_cards(&tid, "the turn failed", now);
                self.refresh_changes(&tid);
                self.mark(&tid);
                Ok(json!({}))
            }
            _ => Ok(json!({})),
        }
    }

    /// `PreToolUse`: the thread's tier judges the tool call before it runs. The reply is `allow`, `deny` with
    /// the reason the agent is told, or `ask`, which makes Claude Code show its dialog; that reaches
    /// `PermissionRequest`, and the card is opened there, so nothing waits in here. Looking at a file is let
    /// through without a word in the log.
    fn pre_tool_use(&mut self, t: &Thread, payload: &Value, now: i64) -> Result<Value> {
        let request = lila_agent::claude::permission_request("pre", payload, Vec::new());
        let reply = match policy::decide(t.tier, Path::new(&t.cwd), &request) {
            Verdict::Allow => {
                if !policy::reads(&request) {
                    self.auto_event(&t.id, t.tier, "auto_allowed", &request, now)?;
                }
                self.remember_judged(&t.id, call_key(payload), None, now);
                json!({"verdict": "allow"})
            }
            Verdict::Deny(reason) => {
                self.auto_event(&t.id, t.tier, "auto_denied", &request, now)?;
                json!({"verdict": "deny", "reason": reason})
            }
            Verdict::Ask { why, danger } => {
                // What the tier itself asks about may have been allowed "always" for this thread; a danger never is.
                if danger.is_none()
                    && let Some(by) = self.rules_covering(t, &request)
                {
                    self.rule_event(&t.id, t.tier, &request, &by, now)?;
                    self.remember_judged(&t.id, call_key(payload), None, now);
                    return Ok(json!({"verdict": "allow"}));
                }
                self.remember_judged(&t.id, call_key(payload), Some((why.clone(), danger)), now);
                json!({"verdict": "ask", "reason": why})
            }
        };
        Ok(reply)
    }

    fn remember_judged(&mut self, tid: &str, key: u64, ask: Option<(String, Option<Danger>)>, now: i64) {
        let list: &mut VecDeque<Judged> = self.judged.entry(tid.to_owned()).or_default();
        while list.front().is_some_and(|j| now - j.at_ms > JUDGED_MS) || list.len() >= JUDGED_KEEP {
            list.pop_front();
        }
        list.push_back(Judged { key, at_ms: now, ask });
    }

    /// The verdict `PreToolUse` gave this call (the oldest of equal calls), taken.
    fn take_judged(&mut self, tid: &str, key: u64) -> Option<Judged> {
        let list = self.judged.get_mut(tid)?;
        let at = list.iter().position(|j| j.key == key)?;
        list.remove(at)
    }

    /// A `PermissionRequest` from Claude Code's hook, or one the hook sends again after lilad forgot it (`again`: its id):
    /// answered by the tier at once, or a card the hook then waits for.
    fn permission_hook(&mut self, t: &Thread, payload: &Value, again: Option<&str>, now: i64) -> Result<Value> {
        let tid = t.id.clone();
        let id = match again {
            Some(id) => id.to_owned(),
            None => {
                self.next_hook += 1;
                format!("hook-{:x}-{}", self.boot, self.next_hook)
            }
        };
        let request = lila_agent::claude::permission_request(&id, payload, options());
        // A call `PreToolUse` already judged is not judged again: the card it asked for opens, or the
        // let-through is repeated, with nothing more in the log.
        let judged = self.take_judged(&tid, call_key(payload));
        let repeat = judged.as_ref().is_some_and(|j| j.ask.is_none()).then(|| request.pick(true).map(|o| o.id.clone())).flatten();
        let decided = match (judged, repeat) {
            (Some(Judged { ask: Some((why, danger)), .. }), _) => self.open_card(&tid, request, why, danger, now)?,
            (_, Some(option)) => Decided::Answer { option, message: None },
            _ => self.ask_permission(&tid, request, now)?,
        };
        Ok(match decided {
            Decided::Answer { option, message } => json!({"decision": option, "message": message}),
            Decided::Card => {
                if let Some(a) = self.asking.iter_mut().find(|a| a.thread == tid && a.request.id == id) {
                    a.polled_ms = Some(now);
                }
                json!({"wait": id})
            }
        })
    }

    /// `hook.answer {thread, id, payload?}`: the person's answer to a hook's permission request, once given. The hook asks every
    /// half second for as long as it waits. When lilad does not know the request (it was restarted meanwhile) it says so
    /// (`unknown`); the hook then sends the request again with its `payload`, and the card comes back.
    pub(super) fn hook_answer(&mut self, params: &Value, now: i64) -> Result<Value> {
        let t = self.thread_of(params)?;
        let id = req(params, "id")?;
        if let Some(answer) = self.hook_answers.remove(id) {
            return Ok(match answer {
                Some((option, message)) => json!({"decision": option, "message": message}),
                None => json!({"decision": null}),
            });
        }
        if let Some(a) = self.asking.iter_mut().find(|a| a.request.id == id) {
            a.polled_ms = Some(now);
            return Ok(json!({"pending": true}));
        }
        // A thread that is gone (stopped, ended) has nobody left to answer: nothing comes back.
        let alive = t.terminal.is_some() && self.live.contains_key(&t.id) && !t.engine.hosted();
        match params.get("payload").filter(|p| p.is_object()) {
            Some(payload) if alive => self.permission_hook(&t, payload, Some(id), now),
            _ => Ok(json!({"decision": null, "unknown": true})),
        }
    }

    /// Cards whose hook is gone (`HOOK_GONE_MS`) are dropped, so a card never outlives the question it stands for.
    pub(super) fn drop_abandoned_cards(&mut self, now: i64) {
        let gone: Vec<super::permissions::Asking> = self.asking.iter().filter(|a| a.polled_ms.is_some_and(|p| now - p > HOOK_GONE_MS)).cloned().collect();
        for a in gone {
            self.drop_card(&a, "its hook stopped asking (answered in the terminal, or Claude Code gave up)", now);
        }
    }

    /// A turn ended: no dialog of Claude Code's can still be open, so the cards its hooks stand for are closed.
    fn drop_hook_cards(&mut self, tid: &str, why: &str, now: i64) {
        for a in self.asking.iter().filter(|a| a.thread == tid && a.polled_ms.is_some()).cloned().collect::<Vec<_>>() {
            self.drop_card(&a, why, now);
        }
    }

    fn drop_card(&mut self, a: &super::permissions::Asking, why: &str, now: i64) {
        self.asking.retain(|x| x.id != a.id);
        self.hook_answers.remove(&a.request.id);
        let _ = self.store.event(&a.thread, "lilad", "dismissed", &format!("{} · {why}", a.request.title.lines().next().unwrap_or_default()), now);
        self.mark(&a.thread);
    }

    /// What a Claude thread is writing right now (from `MessageDisplay`), until the transcript has it.
    pub(super) fn writing_of(&self, tid: &str) -> Option<&str> {
        self.writing.get(tid).map(|(_, t)| t.as_str()).filter(|t| !t.trim().is_empty())
    }
}
