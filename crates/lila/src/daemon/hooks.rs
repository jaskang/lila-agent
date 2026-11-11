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

/// The events Lila hooks, and how long Claude Code waits for each (seconds).
pub const HOOKED: [(&str, u64); 6] = [("PreToolUse", 10), ("PermissionRequest", 600), ("MessageDisplay", 10), ("UserPromptSubmit", 30), ("Stop", 10), ("StopFailure", 10)];

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
            "PermissionRequest" => {
                self.next_hook += 1;
                let id = format!("hook-{}", self.next_hook);
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
                    Decided::Card => json!({"wait": id}),
                })
            }
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
                self.refresh_changes(&tid);
                Ok(json!({}))
            }
            "StopFailure" => {
                let why = payload["error"].as_str().or(payload["message"].as_str()).or(payload["reason"].as_str()).map(String::from).unwrap_or_else(|| payload.to_string());
                self.follow_transcripts();
                self.writing.remove(&tid);
                self.store.event(&tid, "claude", "turn_failed", &why, now)?;
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

    /// `hook.answer {thread, id}`: the person's answer to a hook's permission request, once given.
    pub(super) fn hook_answer(&mut self, params: &Value) -> Result<Value> {
        let id = req(params, "id")?;
        if let Some(answer) = self.hook_answers.remove(id) {
            return Ok(match answer {
                Some((option, message)) => json!({"decision": option, "message": message}),
                None => json!({"decision": null}),
            });
        }
        let open = self.asking.iter().any(|a| a.request.id == id);
        Ok(if open { json!({"pending": true}) } else { json!({"decision": null}) })
    }

    /// What a Claude thread is writing right now (from `MessageDisplay`), until the transcript has it.
    pub(super) fn writing_of(&self, tid: &str) -> Option<&str> {
        self.writing.get(tid).map(|(_, t)| t.as_str()).filter(|t| !t.trim().is_empty())
    }
}
