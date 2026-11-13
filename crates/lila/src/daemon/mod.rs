//! lilad: the only writer of Lila's state. Requests come from the CLI and the window over a
//! Unix socket, facts from the omp of each thread over channels; all are handled one at a time
//! on one thread, so there is no locking.
//!
//! The model is projects and their threads. A thread is a conversation with omp; omp is a child
//! process of lilad (`runner`), so it ends with lilad.

mod agents;
mod checkpoints;
mod conversation;
mod jobs;
mod permissions;
mod questions;
mod requests;
mod runner;
mod server;
mod sources;
mod threads;

pub use agents::{Agents, Fact, Spawn, Started, has_session, omp_args};
pub use jobs::{Begin, Job, Output};
pub use runner::OmpAgents;
pub use server::serve;
pub use sources::{PollJob, PostJob};

use crate::paths::{Paths, which};
use std::sync::mpsc::Sender;
use anyhow::{Context, Result, anyhow, bail};
use lila_core::{AgentStatus, ProjectConfig, Thread, ThreadState, Tier};
use lila_store::Store;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// What lilad knows of a thread's running omp that the database does not hold.
#[derive(Debug, Clone)]
struct Live {
    status: AgentStatus,
    kind: Option<String>,
    msg: String,
    /// Which run of the thread's omp this is: the facts of another are let go.
    run: u64,
    /// When omp last became free to take a message.
    free_since: i64,
    /// The model and effort this thread runs on, as omp reported them (nothing yet: the stored ones),
    /// with the choices it lists. Starts over with omp.
    config: lila_agent::Config,
    /// The slash commands omp declared for this thread.
    commands: Vec<lila_agent::SlashCommand>,
}

impl Live {
    fn new(status: AgentStatus, started_ms: i64, run: u64) -> Live {
        Live { status, kind: None, msg: String::new(), run, free_since: started_ms, config: Default::default(), commands: Vec::new() }
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

pub struct Daemon<A: Agents> {
    pub store: Store,
    agents: A,
    paths: Paths,
    projects: BTreeMap<String, Proj>,
    /// Running threads' omp, by thread id.
    live: HashMap<String, Live>,
    /// The PATH omp is looked for on (lilad's own when it started).
    path_env: String,
    pub log: Vec<String>,
    /// The last delivery error per thread, so a stuck delivery is logged once, not every tick.
    failed_sends: BTreeMap<String, String>,
    /// Goes up when what threads are doing changes (status, words): not in the database.
    live_version: u64,
    /// Threads something changed in since the watchers were told (`take_dirty`).
    dirty: BTreeSet<String>,
    /// Projects that were added, changed or removed since the watchers were told.
    dirty_projects: BTreeSet<String>,
    /// Where the commands of each running omp go, by thread. Letting go of one ends that omp.
    senders: HashMap<String, Sender<lila_agent::Command>>,
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

impl<A: Agents> Daemon<A> {
    pub fn new(store: Store, agents: A, paths: Paths, path_env: String) -> Daemon<A> {
        let mut d = Daemon {
            store,
            agents,
            paths,
            projects: BTreeMap::new(),
            live: HashMap::new(),
            path_env,
            log: Vec::new(),
            failed_sends: BTreeMap::new(),
            live_version: 0,
            dirty: BTreeSet::new(),
            dirty_projects: BTreeSet::new(),
            senders: HashMap::new(),
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
        // Threads that were running when lilad last stopped: their omp ended with it.
        let now = now_ms();
        for t in d.store.live_threads().unwrap_or_default() {
            let _ = d.store.set_ended(&t.id, now);
            let _ = d.store.event(&t.id, "lilad", "ended", "lilad was not running", now);
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

    fn set_status(&mut self, tid: &str, status: &lila_agent::HostStatus, now: i64) {
        let Some(live) = self.live.get_mut(tid) else { return };
        let old = live.status;
        let (word, kind, msg) = status.parts();
        let (kind, msg) = (kind.map(String::from), msg.unwrap_or_default());
        let new = AgentStatus::from_word(Some(word));
        let changed = old != new || live.kind != kind || live.msg != msg;
        live.status = new;
        live.kind = kind;
        live.msg = msg;
        if new.is_free() && !old.is_free() {
            live.free_since = now;
        }
        // A turn ended: what it changed is counted again.
        let turn_ended = new.is_free() && matches!(old, AgentStatus::Working | AgentStatus::Blocked);
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

    /// The omp of a thread is gone: it exited, was stopped, or lilad is stopping. The thread stays, ended;
    /// its conversation is kept and it can be resumed. Its omp ends if it is still there (`senders`).
    fn thread_gone(&mut self, tid: &str, why: &str, now: i64) {
        let Ok(Some(t)) = self.store.thread(tid) else { return };
        if !self.live.contains_key(tid) {
            return;
        }
        let _ = self.store.set_ended(tid, now);
        self.live.remove(tid);
        self.senders.remove(tid);
        self.asking.retain(|a| a.thread != tid);
        self.questions.retain(|a| a.thread != tid);
        let _ = self.store.event(tid, "lilad", "ended", why, now);
        self.say(format!("{}: thread {tid} ended ({why})", t.project));
        self.mark(tid);
    }

    /// Every few seconds: words waiting for a thread that takes them.
    pub fn tick(&mut self, now: i64) {
        self.pump(now);
    }

    // ---------------- what threads look like ----------------

    /// The state a thread shows, and what it waits for (`card`: a permission card or a question).
    fn state_of(&self, t: &Thread) -> (ThreadState, Option<&'static str>) {
        let live = self.live.get(&t.id);
        let status = live.map_or(AgentStatus::Gone, |l| l.status);
        let card = self.asking.iter().any(|a| a.thread == t.id) || self.questions.iter().any(|a| a.thread == t.id);
        let state = status.thread_state(card);
        (state, (state == ThreadState::Waiting).then_some("card"))
    }

    pub(super) fn thread_json(&self, t: &Thread) -> Value {
        let (state, waiting) = self.state_of(t);
        let live = self.live.get(&t.id);
        let cards = self.asking.iter().filter(|a| a.thread == t.id).count() + self.questions.iter().filter(|a| a.thread == t.id).count();
        json!({
            "id": t.id, "project": t.project, "title": t.title, "legacy": t.legacy,
            "model": live.and_then(|l| l.config.model.clone()).unwrap_or_else(|| t.model.clone()),
            "effort": live.and_then(|l| l.config.effort.clone()).or_else(|| t.effort.clone()),
            "permission": t.tier.as_str(), "cwd": t.cwd, "session": t.session,
            "state": state.as_str(), "status": live.map_or("gone", |l| l.status.as_str()), "waiting": waiting, "cards": cards,
            "msg": live.map(|l| l.msg.clone()).filter(|m| !m.is_empty()),
            "created_ms": t.created_ms, "active_ms": t.active_ms, "started_ms": t.started_ms, "ended_ms": t.ended_ms, "archived": t.archived,
            "worktree": t.worktree, "base_branch": t.base_branch, "base_commit": t.base_commit, "merged_ms": t.merged_ms,
            "merging": self.merging.contains(&t.id), "restoring": self.restoring.contains(&t.id), "changes": self.summary_of(&t.id).map(|c| c.json()),
        })
    }

    /// omp's program, on the PATH lilad was started with.
    pub(super) fn omp_program(&self) -> Result<PathBuf> {
        which(lila_core::runtime::CLI, &self.path_env).ok_or_else(|| anyhow!("{:?} is not on PATH; install it and sign in", lila_core::runtime::CLI))
    }
}

#[cfg(test)]
mod tests;
