//! A thread's omp runs over its RPC mode in a thread of lilad's own (`runner`): words for it go to it as
//! commands; what it does comes back as facts (events, its status, its end).

use super::permissions::Decided;
use super::*;
use lila_agent::{AgentEvent, Attachment, Command, HostStatus};
use std::sync::mpsc::Sender;

/// What starts omp: the program, its arguments, and the folder it runs in.
pub struct Spawn {
    pub thread: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

/// An omp that is running: which run of the thread it is, and where its commands go. Dropping `commands` ends it
/// (omp's stdin is closed; killed if it is still there 3 seconds later).
pub struct Started {
    pub run: u64,
    pub commands: Sender<Command>,
}

/// What comes back from a running omp.
#[derive(Debug, Clone, PartialEq)]
pub enum Fact {
    Event(AgentEvent),
    Status(HostStatus),
    /// The process is gone, and why (exit code and the last words it said).
    Exited(String),
}

/// What the daemon needs to run omp. A fake stands in for it in tests.
pub trait Agents {
    fn start(&mut self, spawn: Spawn) -> Result<Started>;
    /// Waits until every omp that was told to end is gone.
    fn wait(&mut self);
}

/// What `omp` is started with: its RPC mode with the tool dialogs (`ask` needs them), every write and every command
/// asked about (Lila answers them: `policy`), the thread's own folder of sessions, and the last session in it when
/// there is one. `extra` goes last.
pub fn omp_args(model: Option<&str>, session_dir: &Path, extra: &[String]) -> Vec<String> {
    let mut args: Vec<String> = ["--mode", "rpc-ui", "--approval-mode", "always-ask"].map(String::from).into();
    if let Some(m) = model.filter(|m| !m.is_empty()) {
        args.extend(["--model".to_owned(), m.to_owned()]);
    }
    args.extend(["--session-dir".to_owned(), session_dir.to_string_lossy().to_string()]);
    if has_session(session_dir) {
        args.push("--continue".into());
    }
    args.extend(extra.iter().cloned());
    args
}

/// Does the thread's folder of sessions hold one (omp writes each session as a `.jsonl` file)?
pub fn has_session(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|d| d.flatten().any(|e| e.path().extension().is_some_and(|x| x == "jsonl")))
}

impl<A: Agents> Daemon<A> {
    /// Starts a thread's omp.
    pub(super) fn start_thread(&mut self, t: &Thread, now: i64) -> Result<()> {
        let program = self.omp_program()?;
        let dir = self.paths.omp_sessions(&t.id);
        std::fs::create_dir_all(&dir).with_context(|| format!("make {}", dir.display()))?;
        let args = omp_args(Some(&t.model), &dir, &[]);
        let shown: Vec<String> = args.iter().map(|a| if a.chars().count() > 80 { format!("{}…", a.chars().take(40).collect::<String>()) } else { a.clone() }).collect();
        let started = self.agents.start(Spawn { thread: t.id.clone(), program: program.clone(), args, cwd: PathBuf::from(&t.cwd) })?;
        self.store.set_running(&t.id, now)?;
        self.live.insert(t.id.clone(), Live::new(AgentStatus::Unknown, now, started.run));
        self.senders.insert(t.id.clone(), started.commands);
        self.store.event(&t.id, "lilad", "started", &format!("{} {}", program.display(), shown.join(" ")), now)?;
        self.say(format!("{}: started thread {}", t.project, t.id));
        self.mark(&t.id);
        Ok(())
    }

    /// Ends every omp and waits for them (lilad is stopping).
    pub fn shutdown(&mut self, now: i64) {
        for tid in self.live.keys().cloned().collect::<Vec<_>>() {
            self.thread_gone(&tid, "lilad stopped", now);
        }
        self.agents.wait();
    }

    /// A fact from a thread's omp; one from an omp that is no longer the thread's current is let go.
    pub fn agent_fact(&mut self, tid: &str, run: u64, fact: Fact, now: i64) {
        if self.live.get(tid).is_none_or(|l| l.run != run) {
            return;
        }
        match fact {
            Fact::Event(ev) => self.agent_event(tid, ev, now),
            Fact::Status(status) => self.set_status(tid, &status, now),
            Fact::Exited(why) => self.thread_gone(tid, &why, now),
        }
    }

    /// A command for a thread's omp.
    pub(super) fn agent_command(&mut self, tid: &str, cmd: &Command) -> Result<()> {
        let tx = self.senders.get(tid).ok_or_else(|| anyhow!("{tid} is not running"))?;
        if tx.send(cmd.clone()).is_err() {
            self.senders.remove(tid);
            bail!("{tid}'s omp went away");
        }
        Ok(())
    }

    /// Words with files, to omp.
    pub(super) fn tell_with(&mut self, tid: &str, text: &str, files: &[Attachment]) -> Result<()> {
        self.agent_command(tid, &Command::Prompt { text: text.to_owned(), attachments: files.to_vec() })
    }

    pub fn agent_event(&mut self, tid: &str, ev: AgentEvent, now: i64) {
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
                        let _ = self.agent_command(tid, &Command::Answer { permission: id, option });
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
