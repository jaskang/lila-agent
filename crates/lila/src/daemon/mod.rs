//! lilad: the only writer of Lila's state. Requests come from the CLI and the window over a
//! Unix socket, terminal facts from herdr-core's event stream; both are handled one at a time
//! on one thread, so there is no locking.
//!
//! The model is projects and their threads (docs/architecture-v4.md). A thread is a
//! conversation with omp; omp runs behind `lila host omp` in an hc terminal of its own, so a lilad
//! restart does not end it.

mod checkpoints;
mod conversation;
mod host;
mod jobs;
mod permissions;
mod questions;
mod requests;
mod server;
mod sources;
mod threads;

pub use jobs::{Begin, Job, Output};
pub use server::serve;
pub use sources::{PollJob, PostJob};

use crate::paths::{Paths, which};
use anyhow::{Context, Result, anyhow, bail};
use lila_core::{AgentStatus, ProjectConfig, Thread, ThreadState, Tier};
use lila_store::Store;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// What the daemon needs from herdr-core. A fake stands in for it in tests.
pub trait Terminals {
    /// Live terminals carrying this project's label.
    fn list(&mut self, project: &str) -> Result<Vec<Value>>;
    /// Starts a program; returns its terminal address.
    fn run(&mut self, req: Value) -> Result<String>;
    fn kill(&mut self, terminal: &str) -> Result<()>;
}

pub struct HcTerminals(pub hc_client::Hc);

impl Terminals for HcTerminals {
    fn list(&mut self, project: &str) -> Result<Vec<Value>> {
        let r = self.0.call("terminal.list", json!({"labels": {"lila.project": project}}))?;
        r["terminals"].as_array().cloned().ok_or_else(|| anyhow!("terminal.list returned no terminals"))
    }

    fn run(&mut self, req: Value) -> Result<String> {
        let r = self.0.call("terminal.run", req)?;
        r["address"].as_str().map(String::from).ok_or_else(|| anyhow!("terminal.run returned no address: {r}"))
    }

    fn kill(&mut self, terminal: &str) -> Result<()> {
        self.0.call("terminal.kill", json!({"terminal": terminal}))?;
        Ok(())
    }
}

/// How long a host may run without reporting a status before lilad says so in its log.
const STARTUP_MS: i64 = 20_000;

/// What lilad knows of a thread's running omp that the database does not hold.
#[derive(Debug, Clone)]
struct Live {
    status: AgentStatus,
    kind: Option<String>,
    msg: String,
    /// Has reported a status at least once since it started.
    reported: bool,
    started_ms: i64,
    /// When omp last became free to take a message.
    free_since: i64,
    /// The model and effort this thread runs on, as omp reported them (nothing yet: the stored ones),
    /// with the choices it lists. Starts over with omp.
    config: lila_agent::Config,
    /// The slash commands omp declared for this thread.
    commands: Vec<lila_agent::SlashCommand>,
}

impl Live {
    fn new(status: AgentStatus, started_ms: i64) -> Live {
        Live { status, kind: None, msg: String::new(), reported: false, started_ms, free_since: started_ms, config: Default::default(), commands: Vec::new() }
    }
}

struct Proj {
    cwd: PathBuf,
    config: ProjectConfig,
    /// Why its `.lila/project.toml` could not be read (the folder is gone, the file is broken); the defaults stand in.
    error: Option<String>,
    /// The folder is the top of a git repository (so a thread there can have a worktree of its own). Asked once, when
    /// the project is loaded or reloaded: `project.list` runs in the main loop and must not start git.
    repo: bool,
}

pub struct Daemon<T: Terminals> {
    pub store: Store,
    term: T,
    paths: Paths,
    projects: BTreeMap<String, Proj>,
    /// Running threads' omp, by thread id.
    live: HashMap<String, Live>,
    /// PATH omp gets (the CLI's PATH when it started the daemon).
    path_env: String,
    /// The `lila` binary, so threads can run `lila host`.
    lila_bin: PathBuf,
    pub log: Vec<String>,
    /// The last delivery error per thread, so a stuck delivery is logged once, not every tick.
    failed_sends: BTreeMap<String, String>,
    /// Goes up when what threads are doing changes (status, words, terminals): not in the database.
    live_version: u64,
    /// Threads something changed in since the watchers were told (`take_dirty`).
    dirty: BTreeSet<String>,
    /// Projects that were added, changed or removed since the watchers were told.
    dirty_projects: BTreeSet<String>,
    /// Attached hosts (`host`), by thread: their command channel.
    hosts: HashMap<String, std::sync::mpsc::Sender<String>>,
    /// Open permission cards (`permissions`): what a thread asks the person.
    asking: Vec<permissions::Asking>,
    /// Open question cards (`questions`): what an agent asks the person.
    questions: Vec<questions::Asked>,
    /// The models omp offers, and when it was asked (`models.list`).
    models: Option<(i64, Vec<lila_agent::Choice>)>,
    next_card: u64,
    /// Work waiting to be run off the loop (`jobs`), taken by the server.
    jobs: Vec<jobs::Job>,
    /// What each thread has changed, counted after its turns end (`jobs`): `None` is a folder outside git.
    summaries: HashMap<String, Option<crate::changes::Summary>>,
    /// Threads whose changes are being counted now, and those asked again meanwhile.
    refreshing: BTreeSet<String>,
    refresh_again: BTreeSet<String>,
    /// Threads being merged: nothing is sent to them or started in them until it is done.
    merging: BTreeSet<String>,
    /// Threads whose files are being put back (`checkpoints`): the same.
    restoring: BTreeSet<String>,
    /// Threads whose words wait for the checkpoint taken before they go (`checkpoints`).
    snapshotting: BTreeSet<String>,
    /// The latest checkpoint number given out per thread, for those whose job has not finished (so none is given twice).
    checkpoint_numbers: HashMap<String, i64>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn s<'a>(p: &'a Value, key: &str) -> Option<&'a str> {
    p.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn req<'a>(p: &'a Value, key: &str) -> Result<&'a str> {
    s(p, key).ok_or_else(|| anyhow!("missing {key}"))
}

fn terminal_name(thread: &str) -> String {
    format!("lila-{thread}")
}

impl<T: Terminals> Daemon<T> {
    pub fn new(store: Store, term: T, paths: Paths, path_env: String, lila_bin: PathBuf) -> Daemon<T> {
        let mut d = Daemon {
            store,
            term,
            paths,
            projects: BTreeMap::new(),
            live: HashMap::new(),
            path_env,
            lila_bin,
            log: Vec::new(),
            failed_sends: BTreeMap::new(),
            live_version: 0,
            dirty: BTreeSet::new(),
            dirty_projects: BTreeSet::new(),
            hosts: HashMap::new(),
            asking: Vec::new(),
            questions: Vec::new(),
            models: None,
            next_card: 0,
            jobs: Vec::new(),
            summaries: HashMap::new(),
            refreshing: BTreeSet::new(),
            refresh_again: BTreeSet::new(),
            merging: BTreeSet::new(),
            restoring: BTreeSet::new(),
            snapshotting: BTreeSet::new(),
            checkpoint_numbers: HashMap::new(),
        };
        for p in d.store.projects().unwrap_or_default() {
            d.load_project(&p.id, Path::new(&p.cwd));
        }
        // Threads that had a terminal when lilad last stopped: presumed alive until herdr-core's snapshot says otherwise.
        let now = now_ms();
        for t in d.store.live_threads().unwrap_or_default() {
            d.live.insert(t.id, Live::new(AgentStatus::Unknown, now));
        }
        // What every thread has changed, counted in the background.
        for p in d.projects.keys().cloned().collect::<Vec<_>>() {
            for t in d.store.threads(&p, false).unwrap_or_default() {
                d.refresh_changes(&t.id);
            }
        }
        d
    }

    /// Changes when anything a viewer shows changed: database writes or live thread state.
    pub fn fingerprint(&self) -> (u64, u64) {
        (self.store.total_changes(), self.live_version)
    }

    /// Threads and projects that changed since the last call (the server tells its watchers).
    pub fn take_dirty(&mut self) -> (Vec<String>, Vec<String>) {
        (std::mem::take(&mut self.dirty).into_iter().collect(), std::mem::take(&mut self.dirty_projects).into_iter().collect())
    }

    /// The push for a thread that changed: its state and how far its conversation goes (`None`: it is gone).
    pub(super) fn thread_event(&self, tid: &str, seq: u64) -> Option<Value> {
        let t = self.store.thread(tid).ok()??;
        let (state, waiting) = self.state_of(&t);
        Some(json!({"event": "thread", "project": t.project, "thread": tid, "state": state.as_str(), "waiting": waiting, "items_seq": self.store.last_seq(tid).unwrap_or(0), "changes": self.summary_of(tid).map(|c| c.json()), "seq": seq}))
    }

    fn mark(&mut self, thread: &str) {
        self.live_version += 1;
        self.dirty.insert(thread.to_owned());
    }

    fn say(&mut self, line: String) {
        eprintln!("[lilad] {line}");
        self.log.push(line);
        if self.log.len() > 500 {
            self.log.drain(..100);
        }
    }

    /// Reads a project's `.lila/project.toml`. A project whose file cannot be read stays in the list with
    /// the default settings and the reason.
    fn load_project(&mut self, id: &str, cwd: &Path) {
        let (config, error) = match crate::project::load(cwd) {
            Ok(c) if c.name == id => (c, None),
            Ok(c) => (ProjectConfig::new(id), Some(format!("project.toml now names it {:?}; it was registered as {id:?}", c.name))),
            Err(e) => (ProjectConfig::new(id), Some(format!("{e:#}"))),
        };
        if let Some(e) = &error {
            self.say(format!("project {id}: {e}"));
        }
        let repo = crate::worktree::is_repo_root(cwd, &self.path_env);
        self.projects.insert(id.to_owned(), Proj { cwd: cwd.to_path_buf(), config, error, repo });
        self.dirty_projects.insert(id.to_owned());
        self.live_version += 1;
    }

    // ---------------- herdr-core facts ----------------

    fn find_thread(&self, terminal: &str) -> Option<String> {
        self.store.live_threads().ok()?.into_iter().find(|t| t.terminal.as_deref() == Some(terminal)).map(|t| t.id)
    }

    /// A whole terminal record (snapshot, `created`, `restarted`).
    pub fn on_terminal(&mut self, rec: &Value, now: i64) {
        let (Some(tid), Some(addr)) = (rec["labels"]["lila.thread"].as_str(), rec["address"].as_str()) else { return };
        let Ok(Some(t)) = self.store.thread(tid) else { return };
        if rec["state"] == "live" && t.legacy.is_some() {
            // A Claude Code or Codex thread from before omp was the only engine: nothing here runs it any more, so its terminal goes.
            let _ = self.term.kill(addr);
            return;
        }
        if rec["state"] == "live" {
            if t.terminal.as_deref() != Some(addr) {
                // A terminal this thread was not known to run in (lilad was away when it started): take it.
                if self.store.set_running(tid, addr, t.started_ms.unwrap_or(now)).is_err() {
                    return;
                }
                self.mark(tid);
            }
            let live = self.live.entry(tid.to_owned()).or_insert_with(|| Live::new(AgentStatus::Unknown, now));
            live.reported |= rec["reported"].as_bool().unwrap_or(false);
            if live.status == AgentStatus::Gone {
                live.status = AgentStatus::Unknown;
            }
            let status = rec.get("status").cloned().unwrap_or(Value::Null);
            self.set_status(tid, &status, now);
        } else if t.terminal.as_deref() == Some(addr) {
            self.thread_gone(tid, rec["reason"].as_str().unwrap_or("finished"), now);
        }
    }

    /// A line from `events.subscribe`.
    pub fn on_event(&mut self, line: &Value, now: i64) {
        let p = &line["params"];
        match line["method"].as_str() {
            Some("missed") => {
                for rec in p["terminals"].as_array().cloned().unwrap_or_default() {
                    self.on_terminal(&rec, now);
                }
            }
            Some("event") => {
                let Some(terminal) = p["terminal"].as_str() else { return };
                match p["type"].as_str() {
                    Some("created" | "restarted") => self.on_terminal(&p["data"], now),
                    Some("status") => {
                        if let Some(tid) = self.find_thread(terminal) {
                            if p["data"]["reported"].as_bool() == Some(true)
                                && let Some(l) = self.live.get_mut(&tid)
                            {
                                l.reported = true;
                            }
                            self.set_status(&tid, &p["data"]["status"], now);
                        }
                    }
                    Some("finished" | "removed" | "lost") => {
                        if let Some(tid) = self.find_thread(terminal) {
                            let why = p["data"]["reason"].as_str().unwrap_or("ended").to_owned();
                            self.thread_gone(&tid, &why, now);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn set_status(&mut self, tid: &str, status: &Value, now: i64) {
        let Some(live) = self.live.get_mut(tid) else { return };
        let old = live.status;
        let new = if status.is_null() { AgentStatus::Unknown } else { AgentStatus::from_word(status["state"].as_str()) };
        let (kind, msg) = (status["kind"].as_str().map(String::from), status["msg"].as_str().unwrap_or_default().to_owned());
        let changed = old != new || live.kind != kind || live.msg != msg;
        live.status = new;
        live.kind = kind;
        live.msg = msg;
        if new.is_free() && !old.is_free() {
            live.free_since = now;
        }
        // A turn ended: what it changed is counted again.
        let turn_ended = new.is_free() && matches!(old, AgentStatus::Working | AgentStatus::Blocked);
        if new != AgentStatus::Unknown {
            live.reported = true;
        }
        if changed {
            self.mark(tid);
        }
        if turn_ended {
            self.refresh_changes(tid);
        }
        if old != new {
            self.pump(now);
        }
    }

    /// The omp of a thread is gone: its terminal ended, was killed, or is no longer there. The
    /// thread stays, ended; its conversation is kept and it can be resumed.
    fn thread_gone(&mut self, tid: &str, why: &str, now: i64) {
        let Ok(Some(t)) = self.store.thread(tid) else { return };
        if t.terminal.is_none() && !self.live.contains_key(tid) {
            return;
        }
        let _ = self.store.set_ended(tid, now);
        self.live.remove(tid);
        self.hosts.remove(tid);
        self.asking.retain(|a| a.thread != tid);
        self.questions.retain(|a| a.thread != tid);
        let _ = self.store.event(tid, "lilad", "ended", &format!("{}: {why}", t.terminal.unwrap_or_default()), now);
        self.say(format!("{}: thread {tid} ended ({why})", t.project));
        self.mark(tid);
    }

    /// After a fresh snapshot: threads pointing at terminals that no longer exist ended while we were away.
    /// Threads started after the snapshot was taken are not in it and are left alone.
    pub fn reconcile(&mut self, live_terminals: &[String], taken_ms: i64, now: i64) {
        let gone: Vec<String> = self
            .store
            .live_threads()
            .unwrap_or_default()
            .into_iter()
            .filter(|t| t.started_ms.is_none_or(|s| s < taken_ms - 2000))
            .filter(|t| t.terminal.as_ref().is_some_and(|a| !live_terminals.contains(a)))
            .map(|t| t.id)
            .collect();
        for tid in gone {
            self.thread_gone(&tid, "its terminal no longer exists", now);
        }
    }

    /// Every few seconds: startup timeouts, then the pump.
    pub fn tick(&mut self, now: i64) {
        let slow: Vec<String> = self.live.iter().filter(|(_, l)| !l.reported && l.status == AgentStatus::Unknown && now - l.started_ms >= STARTUP_MS && now - l.started_ms < STARTUP_MS + 6000).map(|(t, _)| t.clone()).collect();
        for tid in slow {
            self.say(format!("thread {tid} has not reported a status"));
            self.mark(&tid);
        }
        self.pump(now);
    }

    // ---------------- what threads look like ----------------

    /// The state a thread shows, and what it waits for (`card`: a permission card or a question).
    fn state_of(&self, t: &Thread) -> (ThreadState, Option<&'static str>) {
        let live = self.live.get(&t.id).filter(|_| t.terminal.is_some());
        let status = live.map_or(AgentStatus::Gone, |l| l.status);
        let card = self.asking.iter().any(|a| a.thread == t.id) || self.questions.iter().any(|a| a.thread == t.id);
        let state = status.thread_state(card);
        (state, (state == ThreadState::Waiting).then_some("card"))
    }

    pub(super) fn thread_json(&self, t: &Thread) -> Value {
        let (state, waiting) = self.state_of(t);
        let live = self.live.get(&t.id).filter(|_| t.terminal.is_some());
        let cards = self.asking.iter().filter(|a| a.thread == t.id).count() + self.questions.iter().filter(|a| a.thread == t.id).count();
        json!({
            "id": t.id, "project": t.project, "title": t.title, "legacy": t.legacy,
            "model": live.and_then(|l| l.config.model.clone()).unwrap_or_else(|| t.model.clone()),
            "effort": live.and_then(|l| l.config.effort.clone()).or_else(|| t.effort.clone()),
            "permission": t.tier.as_str(), "cwd": t.cwd, "session": t.session,
            "state": state.as_str(), "status": live.map_or("gone", |l| l.status.as_str()), "waiting": waiting, "cards": cards,
            "msg": live.map(|l| l.msg.clone()).filter(|m| !m.is_empty()),
            "terminal": t.terminal,
            "created_ms": t.created_ms, "active_ms": t.active_ms, "started_ms": t.started_ms, "ended_ms": t.ended_ms, "archived": t.archived,
            "worktree": t.worktree, "base_branch": t.base_branch, "base_commit": t.base_commit, "merged_ms": t.merged_ms,
            "merging": self.merging.contains(&t.id), "restoring": self.restoring.contains(&t.id), "changes": self.summary_of(&t.id).map(|c| c.json()),
        })
    }

    /// omp's program on PATH and the environment its terminal gets (PATH = lila's folder + the
    /// PATH lilad was started with).
    pub(super) fn omp_env(&self) -> Result<(PathBuf, BTreeMap<String, String>)> {
        let lila_dir = self.lila_bin.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        let path = format!("{lila_dir}:{}", self.path_env);
        let program = which(lila_core::runtime::CLI, &path).ok_or_else(|| anyhow!("{:?} is not on PATH; install it and sign in", lila_core::runtime::CLI))?;
        Ok((program, BTreeMap::from([("PATH".to_owned(), path)])))
    }
}

#[cfg(test)]
mod tests;
