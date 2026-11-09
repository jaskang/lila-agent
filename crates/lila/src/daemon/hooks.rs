//! Claude Code's hooks (architecture v3 §7.1, P0 S1): Lila gives each Claude
//! member a settings file whose hooks run `lila hook <event>`, which hands the
//! event to lilad. A permission request becomes a card the person answers
//! (the hook waits for it, `hook.answer`); the reply being written shows as it
//! comes (`MessageDisplay`).

use super::*;
use lila_agent::{PermOption, PermissionRequest};

/// The events Lila hooks, and how long Claude Code waits for each (seconds).
pub const HOOKED: [(&str, u64); 5] = [("PermissionRequest", 600), ("MessageDisplay", 10), ("UserPromptSubmit", 10), ("Stop", 10), ("StopFailure", 10)];

/// The settings file that hooks a Claude member's session: each event runs `lila hook <event>`.
pub fn settings(lila_bin: &Path) -> Value {
    let bin = format!("'{}'", lila_bin.to_string_lossy().replace('\'', "'\\''"));
    let hooks: serde_json::Map<String, Value> = HOOKED
        .iter()
        .map(|(event, timeout)| ((*event).to_owned(), json!([{"matcher": "*", "hooks": [{"type": "command", "command": format!("{bin} hook {event}"), "timeout": timeout}]}])))
        .collect();
    json!({ "hooks": hooks })
}

impl<T: Terminals> Daemon<T> {
    /// `hook {event, payload}` from a Claude member's session.
    pub(super) fn hook(&mut self, caller: &Value, params: &Value, now: i64) -> Result<Value> {
        let (pid, actor) = self.who(caller)?;
        let Actor::Member(mid) = actor else { bail!("hooks come from members") };
        let payload = &params["payload"];
        let key = (pid.clone(), mid.clone());
        match req(params, "event")? {
            "PermissionRequest" => {
                self.next_hook += 1;
                let name = payload["tool_name"].as_str().unwrap_or("tool");
                let (tool, title) = lila_agent::claude::describe(name, &payload["tool_input"]);
                let options = vec![PermOption { id: "allow".into(), label: "允许".into() }, PermOption { id: "deny".into(), label: "拒绝".into() }];
                let id = format!("hook-{}", self.next_hook);
                let detail = payload["tool_input"]["description"].as_str().unwrap_or_default().to_owned();
                let request = PermissionRequest { id: id.clone(), tool, title, detail, options };
                Ok(match self.ask_permission(&pid, &mid, request, now) {
                    Some(decision) => json!({"decision": decision}),
                    None => json!({"wait": id}),
                })
            }
            "MessageDisplay" => {
                let message = payload["message_id"].as_str().unwrap_or_default().to_owned();
                let delta = payload["delta"].as_str().unwrap_or_default();
                let entry = self.writing.entry(key).or_insert_with(|| (message.clone(), String::new()));
                if entry.0 != message {
                    *entry = (message, String::new());
                }
                entry.1.push_str(delta);
                self.live_version += 1;
                Ok(json!({}))
            }
            // A turn starts or ends: whatever was being written is in the transcript now.
            "UserPromptSubmit" | "Stop" => {
                if self.writing.remove(&key).is_some() {
                    self.live_version += 1;
                }
                Ok(json!({}))
            }
            "StopFailure" => {
                let why = payload["error"].as_str().or(payload["message"].as_str()).or(payload["reason"].as_str()).map(String::from).unwrap_or_else(|| payload.to_string());
                self.writing.remove(&key);
                self.store.event(&pid, None, &mid, "turn_failed", &why, now)?;
                Ok(json!({}))
            }
            _ => Ok(json!({})),
        }
    }

    /// `hook.answer {id}`: the person's answer to a hook's permission request, once given.
    pub(super) fn hook_answer(&mut self, caller: &Value, params: &Value) -> Result<Value> {
        let (pid, _) = self.who(caller)?;
        let id = req(params, "id")?;
        if let Some(answer) = self.hook_answers.remove(id) {
            return Ok(json!({"decision": answer}));
        }
        let open = self.asking.iter().any(|((p, _), a)| *p == pid && a.request() == id);
        Ok(if open { json!({"pending": true}) } else { json!({"decision": null}) })
    }

    /// What a Claude member is writing right now (from `MessageDisplay`), until the transcript has it.
    pub(super) fn writing_of(&self, pid: &str, mid: &str) -> Option<&str> {
        self.writing.get(&(pid.to_owned(), mid.to_owned())).map(|(_, t)| t.as_str()).filter(|t| !t.trim().is_empty())
    }
}
