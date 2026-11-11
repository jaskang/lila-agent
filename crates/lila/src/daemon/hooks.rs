//! Claude Code's hooks (architecture v3 §7.1, P0 S1): Lila gives each Claude thread a settings
//! file whose hooks run `lila hook <event>`, which hands the event to lilad. A permission request
//! is answered by the thread's tier or becomes a card the person answers (the hook waits for it,
//! `hook.answer`); the reply being written shows as it comes (`MessageDisplay`).

use super::permissions::Decided;
use super::*;
use lila_agent::{PermKind, PermOption};

/// The events Lila hooks, and how long Claude Code waits for each (seconds).
pub const HOOKED: [(&str, u64); 5] = [("PermissionRequest", 600), ("MessageDisplay", 10), ("UserPromptSubmit", 10), ("Stop", 10), ("StopFailure", 10)];

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
            "PermissionRequest" => {
                self.next_hook += 1;
                let options = vec![
                    PermOption { id: "allow".into(), label: "允许".into(), kind: PermKind::Allow },
                    PermOption { id: "deny".into(), label: "拒绝".into(), kind: PermKind::Reject },
                ];
                let id = format!("hook-{}", self.next_hook);
                let request = lila_agent::claude::permission_request(&id, payload, options);
                Ok(match self.ask_permission(&tid, request, now)? {
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
                Ok(json!({}))
            }
            "StopFailure" => {
                let why = payload["error"].as_str().or(payload["message"].as_str()).or(payload["reason"].as_str()).map(String::from).unwrap_or_else(|| payload.to_string());
                self.follow_transcripts();
                self.writing.remove(&tid);
                self.store.event(&tid, "claude", "turn_failed", &why, now)?;
                self.mark(&tid);
                Ok(json!({}))
            }
            _ => Ok(json!({})),
        }
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
