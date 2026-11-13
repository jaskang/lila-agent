//! A thread's omp runs over its RPC mode (architecture v3 §7.2): its terminal runs `lila host omp`,
//! which attaches to lilad. Words for it go to the host as commands; what it does comes back as events.

use super::permissions::Decided;
use super::*;
use lila_agent::{AgentEvent, Attachment, Command};
use std::sync::mpsc::Sender;

impl<T: Terminals> Daemon<T> {
    pub fn host_attach(&mut self, tid: &str, tx: Sender<String>, now: i64) {
        if !self.live.contains_key(tid) {
            return;
        }
        self.hosts.insert(tid.to_owned(), tx);
        self.say(format!("thread {tid}'s host attached"));
        self.pump(now);
    }

    pub fn host_gone(&mut self, tid: &str) {
        self.hosts.remove(tid);
    }

    /// A command for a thread's host.
    pub(super) fn host_command(&mut self, tid: &str, cmd: &Command) -> Result<()> {
        let tx = self.hosts.get(tid).ok_or_else(|| anyhow!("{tid} is starting (its host has not attached yet)"))?;
        if tx.send(serde_json::to_string(cmd)?).is_err() {
            self.hosts.remove(tid);
            bail!("{tid}'s host went away");
        }
        Ok(())
    }

    /// Words with files, to the host.
    pub(super) fn tell_with(&mut self, tid: &str, text: &str, files: &[Attachment]) -> Result<()> {
        self.host_command(tid, &Command::Prompt { text: text.to_owned(), attachments: files.to_vec() })
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
                    Ok(Decided::Answer { option }) => {
                        let _ = self.host_command(tid, &Command::Answer { permission: id, option });
                    }
                    Ok(Decided::Card) => {}
                    Err(e) => self.say(format!("permission of {tid}: {e:#}")),
                }
            }
            AgentEvent::PermissionResolved { id } => self.request_resolved(tid, &id),
            AgentEvent::Question { question } => self.open_question(tid, question, now),
            AgentEvent::QuestionResolved { id } => self.question_resolved(tid, &id),
            AgentEvent::TurnEnded { failed, .. } => {
                if let Some(why) = failed {
                    let _ = self.store.event(tid, "host", "turn_failed", &why, now);
                    self.mark(tid);
                }
                self.refresh_changes(tid);
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
