//! Threads whose agent runs through a protocol (architecture v3 §7.2): their terminal runs
//! `lila host <engine>`, which attaches to lilad. Words for them go to the host as commands;
//! what they do comes back as events.

use super::permissions::Decided;
use super::*;
use lila_agent::{AgentEvent, Attachment, AttachmentKind, Command};
use std::sync::mpsc::Sender;

impl<T: Terminals> Daemon<T> {
    pub fn host_attach(&mut self, tid: &str, tx: Sender<String>, now: i64) {
        if !self.live.contains_key(tid) {
            return;
        }
        self.hosts.insert(tid.to_owned(), tx);
        self.say(format!("thread {tid}'s host attached"));
        // The tier may have changed while it was away (a new host starts on the stored one, a surviving one is told).
        if let Ok(Some(t)) = self.store.thread(tid) {
            let (approval, sandbox) = lila_core::runtime::codex_settings(t.tier);
            if t.engine == Runtime::Codex {
                let _ = self.host_command(tid, &Command::SetPermission { approval: approval.into(), sandbox: sandbox.into() });
            }
        }
        self.pump(now);
    }

    pub fn host_gone(&mut self, tid: &str) {
        self.hosts.remove(tid);
    }

    /// A command for a hosted thread.
    pub(super) fn host_command(&mut self, tid: &str, cmd: &Command) -> Result<()> {
        let tx = self.hosts.get(tid).ok_or_else(|| anyhow!("{tid} is starting (its host has not attached yet)"))?;
        if tx.send(serde_json::to_string(cmd)?).is_err() {
            self.hosts.remove(tid);
            bail!("{tid}'s host went away");
        }
        Ok(())
    }

    /// Words with files: a host gets them as the engine's own inputs; Claude Code's terminal gets
    /// them as `@path` mentions after the words, which it reads (images too) when the message is sent.
    /// `guard`: only while the terminal has that status.
    pub(super) fn tell_with(&mut self, t: &Thread, terminal: &str, text: &str, files: &[Attachment], guard: Option<&str>) -> Result<()> {
        if t.engine.hosted() {
            self.host_command(&t.id, &Command::Prompt { text: text.to_owned(), attachments: files.to_vec() })
        } else {
            self.term.send(terminal, &claude_text(text, files), guard)
        }
    }

    pub fn host_event(&mut self, tid: &str, ev: AgentEvent, now: i64) {
        if !self.live.contains_key(tid) {
            return;
        }
        match ev {
            AgentEvent::Session { id } => {
                if self.store.set_session(tid, &id).is_ok() {
                    self.mark(tid);
                }
            }
            AgentEvent::Item { item } => {
                let row = (item.id.clone(), item.at_ms, serde_json::to_string(&item).unwrap_or_default());
                if let Err(e) = self.store.put_items(tid, &[row]) {
                    self.say(format!("conversation of {tid}: {e:#}"));
                }
                let _ = self.store.touch_thread(tid, item.at_ms.min(now));
                self.mark(tid);
            }
            AgentEvent::Permission { request } => {
                let id = request.id.clone();
                match self.ask_permission(tid, request, now) {
                    Ok(Decided::Answer { option, .. }) => {
                        let _ = self.host_command(tid, &Command::Answer { permission: id, option });
                    }
                    Ok(Decided::Card) => {}
                    Err(e) => self.say(format!("permission of {tid}: {e:#}")),
                }
            }
            AgentEvent::PermissionResolved { id } => self.request_resolved(tid, &id),
            AgentEvent::TurnEnded { failed, .. } => {
                if let Some(why) = failed {
                    let _ = self.store.event(tid, "host", "turn_failed", &why, now);
                    self.mark(tid);
                }
                self.refresh_changes(tid);
            }
            AgentEvent::Limits { used_percent, resets_ms } => {
                if used_percent >= 100.0 {
                    let _ = self.store.event(tid, "host", "limit", &format!("usage window used up; resets at {}", resets_ms.unwrap_or(0)), now);
                    self.mark(tid);
                }
            }
            AgentEvent::Notice { text } => {
                self.say(format!("thread {tid}: {text}"));
                let _ = self.store.event(tid, "host", "notice", &text, now);
                self.mark(tid);
            }
            AgentEvent::Config { .. } => {
                if let Some(l) = self.live.get_mut(tid) {
                    l.config.apply(&ev);
                }
                self.mark(tid);
            }
            AgentEvent::Commands { commands } => {
                if let Some(l) = self.live.get_mut(tid) {
                    l.commands = commands;
                }
            }
            AgentEvent::TurnStarted { .. } | AgentEvent::Usage { .. } => {}
        }
    }
}

/// The words as Claude Code's terminal gets them: the files after them as `@/absolute/path` mentions
/// (quoted when the path has spaces), which it reads when the message is sent. A skill is called by its words.
pub(super) fn claude_text(text: &str, files: &[Attachment]) -> String {
    let mut out = text.trim().to_owned();
    for f in files.iter().filter(|f| f.kind != AttachmentKind::Skill) {
        out.push(' ');
        if f.path.contains(char::is_whitespace) {
            out.push_str(&format!("@\"{}\"", f.path));
        } else {
            out.push_str(&format!("@{}", f.path));
        }
    }
    out.trim_start().to_owned()
}
