//! Members whose agent runs through a protocol (architecture v3 §7.2): their
//! terminal runs `lila host <runtime>`, which attaches to lilad. Words for
//! them go to the host as commands; what they do comes back as events.

use super::*;
use lila_agent::{AgentEvent, Command, PermOption, PermissionRequest, ToolKind};
use std::sync::mpsc::Sender;

/// An open permission card: whose request it is and the answers it maps to.
pub(super) struct Asking {
    member: String,
    request: String,
    options: Vec<PermOption>,
}

impl<T: Terminals> Daemon<T> {
    /// The member's agent runs through `lila host` (Codex: `codex app-server`).
    pub(super) fn hosted(&self, pid: &str, mid: &str) -> bool {
        self.projects.get(pid).and_then(|p| p.team.member(mid)).is_some_and(|m| m.role.file.runtime == lila_core::Runtime::Codex)
    }

    pub fn host_attach(&mut self, pid: &str, mid: &str, tx: Sender<String>, now: i64) {
        if !self.projects.get(pid).is_some_and(|p| p.live.contains_key(mid)) {
            return;
        }
        self.hosts.insert((pid.to_owned(), mid.to_owned()), tx);
        self.say(format!("{pid}: {mid}'s host attached"));
        self.pump(pid, now);
    }

    pub fn host_gone(&mut self, pid: &str, mid: &str) {
        self.hosts.remove(&(pid.to_owned(), mid.to_owned()));
    }

    /// A command for a hosted member.
    pub(super) fn host_command(&mut self, pid: &str, mid: &str, cmd: &Command) -> Result<()> {
        let key = (pid.to_owned(), mid.to_owned());
        let tx = self.hosts.get(&key).ok_or_else(|| anyhow!("{mid} is starting (its host has not attached yet)"))?;
        if tx.send(serde_json::to_string(cmd)?).is_err() {
            self.hosts.remove(&key);
            bail!("{mid}'s host went away");
        }
        Ok(())
    }

    /// Words for a member: to its host, or typed into its terminal (`guard`: only while that status).
    pub(super) fn tell(&mut self, pid: &str, mid: &str, terminal: &str, text: &str, guard: Option<&str>) -> Result<()> {
        if self.hosted(pid, mid) {
            self.host_command(pid, mid, &Command::Prompt { text: text.to_owned() })
        } else {
            self.term.send(terminal, text, guard)
        }
    }

    pub fn host_event(&mut self, pid: &str, mid: &str, ev: AgentEvent, now: i64) {
        if !self.projects.get(pid).is_some_and(|p| p.live.contains_key(mid)) {
            return;
        }
        match ev {
            AgentEvent::Session { id } => {
                let live = self.projects.get_mut(pid).unwrap().live.get_mut(mid).unwrap();
                if live.session.as_deref() == Some(id.as_str()) {
                    return;
                }
                live.session = Some(id.clone());
                let (chain, started) = (live.chain.clone(), live.started_ms.unwrap_or(now));
                self.live_version += 1;
                let row = SessionRow { id: id.clone(), member: mid.to_owned(), runtime: "codex".into(), file: None, started_ms: started };
                if self.store.add_session(pid, &row).is_ok()
                    && let Some(c) = chain
                {
                    let _ = self.store.touch_chain(pid, &id, &c, now);
                }
            }
            AgentEvent::Item { item } => {
                let Some(session) = self.projects[pid].live[mid].session.clone() else { return };
                let row = (item.id.clone(), item.at_ms, serde_json::to_string(&item).unwrap_or_default());
                if let Err(e) = self.store.put_items(pid, mid, &session, &[row]) {
                    self.say(format!("{pid}: conversation of {mid}: {e:#}"));
                }
            }
            AgentEvent::Permission { request } => self.ask_permission(pid, mid, request, now),
            AgentEvent::PermissionResolved { id } => {
                let done: Vec<String> = self.asking.iter().filter(|(k, a)| k.0 == pid && a.member == mid && a.request == id).map(|(k, _)| k.1.clone()).collect();
                for q in done {
                    self.asking.remove(&(pid.to_owned(), q.clone()));
                    if self.store.inbox_item(pid, &q).ok().flatten().is_some_and(|i| i.resolved_ms.is_none()) {
                        let _ = self.store.resolve_inbox(pid, &q, None, now);
                    }
                }
            }
            AgentEvent::TurnEnded { failed: Some(why), .. } => {
                let _ = self.store.event(pid, None, mid, "turn_failed", &why, now);
            }
            AgentEvent::Limits { used_percent, resets_ms } => {
                match resets_ms.filter(|t| used_percent >= 100.0 && *t > now) {
                    Some(t) => {
                        self.codex_paused.insert(pid.to_owned(), t);
                    }
                    None => {
                        self.codex_paused.remove(pid);
                    }
                }
            }
            AgentEvent::Notice { text } => {
                self.say(format!("{pid}: {mid}: {text}"));
                let _ = self.store.event(pid, None, mid, "notice", &text, now);
            }
            AgentEvent::TurnStarted { .. } | AgentEvent::TurnEnded { .. } | AgentEvent::Usage { .. } => {}
        }
    }

    /// A member asks before acting: Lila's own protocol goes ahead; the rest is a card.
    fn ask_permission(&mut self, pid: &str, mid: &str, r: PermissionRequest, now: i64) {
        if r.tool == ToolKind::Execute && (r.title == "lila" || r.title.starts_with("lila ")) {
            let allow = r.options.first().map(|o| o.id.clone()).unwrap_or_else(|| "accept".into());
            let _ = self.host_command(pid, mid, &Command::Answer { permission: r.id.clone(), option: allow });
            let _ = self.store.event(pid, None, "lilad", "allowed", &format!("{mid}: {}", r.title), now);
            return;
        }
        let what = match r.tool {
            ToolKind::Execute => "运行",
            ToolKind::Edit => "修改",
            ToolKind::Delete => "删除",
            _ => "执行",
        };
        let text = if r.detail.is_empty() { format!("{mid} 想{what}：{}", r.title) } else { format!("{mid} 想{what}：{}\n{}", r.title, r.detail) };
        let labels: Vec<String> = r.options.iter().map(|o| o.label.clone()).collect();
        // Asked again (lilad came back, the host repeated it): the card already open.
        let open = self.store.open_inbox_for(pid, "permission", mid).unwrap_or_default().into_iter().find(|i| i.text == text);
        let q = match open {
            Some(item) => item.id,
            None => {
                let task = self.store.tasks(pid).unwrap_or_default().into_iter().find(|(t, _)| t.to == mid && t.state.is_active()).map(|(t, _)| t.id);
                match self.store.add_inbox(pid, "permission", Some(mid), task.as_deref(), &text, &labels, now) {
                    Ok(item) => item.id,
                    Err(e) => {
                        self.say(format!("{pid}: permission card for {mid}: {e:#}"));
                        return;
                    }
                }
            }
        };
        self.asking.insert((pid.to_owned(), q), Asking { member: mid.to_owned(), request: r.id, options: r.options });
    }

    /// The person's answer to a permission card: the option whose label it is (else the last: no).
    pub(super) fn permission_answer(&mut self, pid: &str, item: &lila_store::InboxItem, text: Option<&str>) -> Result<()> {
        let key = (pid.to_owned(), item.id.clone());
        let Some(a) = self.asking.remove(&key) else { bail!("{} is no longer waiting for an answer", item.id) };
        let option = text.and_then(|t| a.options.iter().find(|o| o.label == t || o.id == t)).or(a.options.last()).map(|o| o.id.clone()).unwrap_or_else(|| "decline".into());
        if let Err(e) = self.host_command(pid, &a.member, &Command::Answer { permission: a.request.clone(), option }) {
            self.asking.insert(key, a);
            return Err(e);
        }
        Ok(())
    }
}
