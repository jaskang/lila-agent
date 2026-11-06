//! lilad: the only writer of Lila's state. Requests come from the CLI over a
//! Unix socket, terminal facts from herdr-core's event stream; both are
//! handled one at a time on one thread, so there is no locking.

mod flows;
mod requests;
mod review;
mod server;
mod session;
mod watch;

pub use server::serve;

use crate::check::{CheckJob, CheckResult};
use crate::commit::{CommitJob, CommitResult};
use crate::paths::{Paths, which};
use crate::roles;
use crate::sessions::{self, Contexts};
use crate::usage::{Scanner, Sources};
use anyhow::{Context, Result, anyhow, bail};
use lila_core::dispatch::{self, AgentStatus, MemberView, QueuedTask, StallAction};
use lila_core::flow::{self, Flow, Next};
use lila_core::message::StepHint;
use lila_core::route::{self, DoneRoute};
use lila_core::{Actor, Brief, Outcome, ProjectConfig, Task, TaskState, Team, Transition, inbox, message, prompt, runtime};
use lila_store::{NewTask, Run, RunState, SessionRow, Store};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// What the daemon needs from herdr-core. A fake stands in for it in tests.
pub trait Terminals {
    /// Live terminals carrying this project's label.
    fn list(&mut self, project: &str) -> Result<Vec<Value>>;
    /// Starts a program; returns its terminal address.
    fn run(&mut self, req: Value) -> Result<String>;
    /// Types text and presses Enter. `only_if`: only when the status word is exactly this.
    fn send(&mut self, terminal: &str, text: &str, only_if: Option<&str>) -> Result<()>;
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

    fn send(&mut self, terminal: &str, text: &str, only_if: Option<&str>) -> Result<()> {
        let mut p = json!({"terminal": terminal, "text": text});
        if let Some(s) = only_if {
            p["if"] = json!(s);
        }
        self.0.call("terminal.send", p)?;
        Ok(())
    }

    fn kill(&mut self, terminal: &str) -> Result<()> {
        self.0.call("terminal.kill", json!({"terminal": terminal}))?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct Live {
    terminal: Option<String>,
    status: AgentStatus,
    kind: Option<String>,
    msg: String,
    /// Has reported a status at least once since it started.
    reported: bool,
    free_since: i64,
    started_ms: Option<i64>,
    /// `Member::launch_key` of what it was started with (unknown for one
    /// started before lilad was), to tell it runs on settings since changed.
    launched: Option<u64>,
    /// The chain (root task) of the last task this session was given; unknown
    /// for a session started before lilad was.
    chain: Option<String>,
    /// The CLI session the terminal runs (the store's `sessions`), once known:
    /// at start for Claude Code, when its rollout turns up for Codex.
    session: Option<String>,
    /// When it last started working (a turn began).
    busy_since: i64,
}

impl Default for Live {
    fn default() -> Self {
        Live { terminal: None, status: AgentStatus::Gone, kind: None, msg: String::new(), reported: false, free_since: 0, started_ms: None, launched: None, chain: None, session: None, busy_since: 0 }
    }
}

struct Proj {
    cwd: PathBuf,
    team: Team,
    live: BTreeMap<String, Live>,
}

pub struct Daemon<T: Terminals> {
    pub store: Store,
    term: T,
    paths: Paths,
    projects: BTreeMap<String, Proj>,
    /// PATH the members get (the CLI's PATH when it started the daemon).
    path_env: String,
    /// The `lila` binary, so members can run it.
    lila_bin: PathBuf,
    pub log: Vec<String>,
    /// The last delivery error per task, so a stuck delivery is logged once, not every tick.
    failed_sends: BTreeMap<String, String>,
    /// Goes up when what members are doing changes (status, words, terminals): not in the database.
    live_version: u64,
    /// Checks to start (`take_checks`), and the runs whose check is under way.
    checks: Vec<CheckJob>,
    checking: BTreeSet<(String, String)>,
    /// Commits of accepted chains to make (`take_commits`), and the chains whose commit is under way.
    commits: Vec<CommitJob>,
    committing: BTreeSet<(String, String)>,
    /// Where the CLIs record their sessions, and the context sizes read from them.
    sources: Sources,
    contexts: Contexts,
    /// Token use per member and window, from the same records (budgets, quotas).
    scanner: Scanner,
    /// Per project: when Codex's quota was last read, and until when it is used up.
    quota_checked: BTreeMap<String, i64>,
    codex_paused: BTreeMap<String, i64>,
    /// The reset time the person was told about: once per time Codex runs out.
    quota_told: BTreeMap<String, i64>,
    /// magpie's command line, when it is installed (set by `serve`; tests leave it out).
    pub magpie: Option<PathBuf>,
}

/// How a member gets work from another chain than its session's.
#[derive(Debug, Clone, PartialEq)]
enum Switch {
    /// Its context is still small: the same session takes it.
    Keep,
    /// An earlier session of it did this chain: back to that one.
    Resume(String),
    /// A new, empty session.
    Fresh,
}

/// The actor lilad writes as: it creates a flow's later steps.
const LILAD: &str = "lilad";

/// A duration ahead, for people: "in 3 days 4 h", "in 2 h 5 min", "in 12 min".
fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
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

fn strings(p: &Value, key: &str) -> Vec<String> {
    p.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
}

pub fn load_team(cwd: &Path, roles_dir: &Path) -> Result<Team> {
    let path = cwd.join(".lila").join("project.toml");
    let text = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let cfg = ProjectConfig::parse(&text).map_err(anyhow::Error::msg)?;
    let roles = roles::load(roles_dir)?;
    let mut team = Team::build(&cfg, &roles).map_err(|e| anyhow!("{}: {e}", path.display()))?;
    for spec in &cfg.members {
        if let Some(file) = &spec.prompt_file {
            let p = cwd.join(".lila").join(file);
            let prompt = std::fs::read_to_string(&p).with_context(|| format!("member {}: read prompt {}", spec.id, p.display()))?;
            team.set_prompt(spec, &prompt);
        }
    }
    Ok(team)
}

fn terminal_name(project: &str, member: &str) -> String {
    let mut n = format!("lila-{project}-{member}");
    n.truncate(32);
    n
}

impl<T: Terminals> Daemon<T> {
    pub fn new(store: Store, term: T, paths: Paths, path_env: String, lila_bin: PathBuf) -> Daemon<T> {
        let mut d = Daemon {
            store,
            term,
            paths,
            projects: BTreeMap::new(),
            path_env,
            lila_bin,
            log: Vec::new(),
            failed_sends: BTreeMap::new(),
            live_version: 0,
            checks: Vec::new(),
            checking: BTreeSet::new(),
            commits: Vec::new(),
            committing: BTreeSet::new(),
            sources: Sources::from_env(),
            contexts: Contexts::default(),
            scanner: Scanner::default(),
            quota_checked: BTreeMap::new(),
            codex_paused: BTreeMap::new(),
            quota_told: BTreeMap::new(),
            magpie: None,
        };
        let rows = d.store.projects().unwrap_or_default();
        for p in rows {
            if let Err(e) = d.load_project(&p.id, Path::new(&p.cwd), now_ms()) {
                d.say(format!("project {}: not loaded: {e:#}", p.id));
            }
        }
        // A check that was running when lilad stopped runs again.
        let pids: Vec<String> = d.projects.keys().cloned().collect();
        for pid in pids {
            for run in d.store.runs(&pid).unwrap_or_default() {
                if run.state == RunState::Checking {
                    d.queue_check(&pid, &run.anchor);
                }
            }
        }
        d
    }

    /// Changes when anything a viewer shows changed: database writes or live member state.
    pub fn fingerprint(&self) -> (u64, u64) {
        (self.store.total_changes(), self.live_version)
    }

    fn say(&mut self, line: String) {
        eprintln!("[lilad] {line}");
        self.log.push(line);
        if self.log.len() > 500 {
            self.log.drain(..100);
        }
    }

    /// Reads a project's team. Members it no longer lists are stopped and their
    /// open tasks cancelled (also when they left while lilad was not running).
    fn load_project(&mut self, id: &str, cwd: &Path, now: i64) -> Result<Vec<String>> {
        let team = load_team(cwd, &self.paths.roles())?;
        if team.project != id {
            bail!("project.toml now names it {:?}; it was registered as {id:?}", team.project);
        }
        // Reloading a running project keeps what we know about its terminals.
        let mut live = self.projects.remove(id).map(|p| p.live).unwrap_or_default();
        if live.is_empty() {
            for m in self.store.members(id)? {
                if team.member(&m.id).is_some() {
                    // Until herdr-core's snapshot says otherwise, a recorded terminal is presumed alive.
                    let status = if m.terminal.is_some() { AgentStatus::Unknown } else { AgentStatus::Gone };
                    let chain = m.session.as_deref().and_then(|s| self.store.last_chain(id, s).ok().flatten());
                    live.insert(m.id.clone(), Live { terminal: m.terminal, started_ms: m.started_ms, status, session: m.session, chain, ..Live::default() });
                }
            }
        }
        let in_team = |m: &str| team.member(m).is_some();
        let mut terminals: BTreeMap<String, Option<String>> = live.iter().filter(|(m, _)| !in_team(m)).map(|(k, v)| (k.clone(), v.terminal.clone())).collect();
        live.retain(|mid, _| in_team(mid));
        for m in &team.members {
            live.entry(m.id.clone()).or_default();
        }
        for row in self.store.members(id)? {
            if !in_team(&row.id) {
                let t = terminals.remove(&row.id).flatten().or(row.terminal);
                terminals.insert(row.id, t);
            }
        }
        for (t, _) in self.store.tasks(id)? {
            if t.state.is_open() && !in_team(&t.to) {
                terminals.entry(t.to).or_default();
            }
        }
        self.projects.insert(id.to_owned(), Proj { cwd: cwd.to_path_buf(), team, live });
        let removed: Vec<String> = terminals.keys().cloned().collect();
        for (mid, terminal) in terminals {
            self.member_removed(id, &mid, terminal, now);
        }
        if !removed.is_empty() {
            self.pump(id, now);
        }
        Ok(removed)
    }

    // ---------------- herdr-core facts ----------------

    fn find_member(&self, terminal: &str) -> Option<(String, String)> {
        self.projects.iter().find_map(|(pid, p)| p.live.iter().find(|(_, l)| l.terminal.as_deref() == Some(terminal)).map(|(m, _)| (pid.clone(), m.clone())))
    }

    /// A whole terminal record (snapshot, `created`, `restarted`).
    pub fn on_terminal(&mut self, rec: &Value, now: i64) {
        let (Some(pid), Some(mid), Some(addr)) = (rec["labels"]["lila.project"].as_str(), rec["labels"]["lila.member"].as_str(), rec["address"].as_str()) else {
            return;
        };
        let Some(live) = self.projects.get_mut(pid).and_then(|p| p.live.get_mut(mid)) else { return };
        let (pid, mid) = (pid.to_owned(), mid.to_owned());
        if rec["state"] == "live" {
            if live.terminal.as_deref() != Some(addr) {
                live.terminal = Some(addr.to_owned());
                live.started_ms.get_or_insert(now);
                self.live_version += 1;
            }
            let live = self.projects.get_mut(&pid).unwrap().live.get_mut(&mid).unwrap();
            live.reported |= rec["reported"].as_bool().unwrap_or(false);
            if live.status == AgentStatus::Gone {
                live.status = AgentStatus::Unknown;
            }
            let status = rec.get("status").cloned().unwrap_or(Value::Null);
            self.set_status(&pid, &mid, &status, now);
        } else if live.terminal.as_deref() == Some(addr) {
            self.member_gone(&pid, &mid, rec["reason"].as_str().unwrap_or("finished"), now, true);
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
                        if let Some((pid, mid)) = self.find_member(terminal) {
                            if p["data"]["reported"].as_bool() == Some(true) {
                                self.projects.get_mut(&pid).unwrap().live.get_mut(&mid).unwrap().reported = true;
                            }
                            self.set_status(&pid, &mid, &p["data"]["status"], now);
                        }
                    }
                    Some("finished" | "removed" | "lost") => {
                        if let Some((pid, mid)) = self.find_member(terminal) {
                            let why = p["data"]["reason"].as_str().unwrap_or("ended").to_owned();
                            self.member_gone(&pid, &mid, &why, now, true);
                            self.pump(&pid, now);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn set_status(&mut self, pid: &str, mid: &str, status: &Value, now: i64) {
        let live = self.projects.get_mut(pid).unwrap().live.get_mut(mid).unwrap();
        let old = live.status;
        let new = if status.is_null() { AgentStatus::Unknown } else { AgentStatus::from_word(status["state"].as_str()) };
        let (kind, msg) = (status["kind"].as_str().map(String::from), status["msg"].as_str().unwrap_or_default().to_owned());
        let changed = old != new || live.kind != kind || live.msg != msg;
        live.status = new;
        live.kind = kind;
        live.msg = msg;
        if changed {
            self.live_version += 1;
        }
        let live = self.projects.get_mut(pid).unwrap().live.get_mut(mid).unwrap();
        if new.is_free() && !old.is_free() {
            live.free_since = now;
        }
        if new == AgentStatus::Working && old != AgentStatus::Working {
            live.busy_since = now;
        }
        let back_free = new.is_free() && !old.is_free();
        if new != AgentStatus::Unknown {
            live.reported = true;
        }
        let (msg, kind, reported) = (live.msg.clone(), live.kind.clone(), live.reported);
        if reported {
            self.resolve_open(pid, "startup", mid, now);
        }
        if back_free {
            self.resolve_open(pid, "runaway", mid, now);
        }
        if new == AgentStatus::Blocked && old != AgentStatus::Blocked && self.store.open_inbox_for(pid, "terminal", mid).map(|v| v.is_empty()).unwrap_or(false) {
            let what = if msg.is_empty() { kind.as_deref().unwrap_or("input").to_owned() } else { msg };
            let _ = self.store.add_inbox(pid, "terminal", Some(mid), None, &inbox::terminal(mid, &what), &[], now);
        }
        if old == AgentStatus::Blocked && new != AgentStatus::Blocked {
            self.resolve_open(pid, "terminal", mid, now);
        }
        if old != new {
            self.pump(pid, now);
        }
    }

    fn resolve_open(&mut self, pid: &str, kind: &str, mid: &str, now: i64) {
        for item in self.store.open_inbox_for(pid, kind, mid).unwrap_or_default() {
            let _ = self.store.resolve_inbox(pid, &item.id, None, now);
        }
    }

    fn member_gone(&mut self, pid: &str, mid: &str, why: &str, now: i64, unexpected: bool) {
        let Some(live) = self.projects.get_mut(pid).and_then(|p| p.live.get_mut(mid)) else { return };
        if live.status == AgentStatus::Gone && live.terminal.is_none() {
            return;
        }
        let terminal = live.terminal.take();
        live.status = AgentStatus::Gone;
        self.live_version += 1;
        let live = self.projects.get_mut(pid).unwrap().live.get_mut(mid).unwrap();
        live.reported = false;
        let role = self.projects[pid].team.member(mid).map(|m| m.role.id.clone()).unwrap_or_default();
        let _ = self.store.set_member(pid, mid, &role, None, None);
        self.resolve_open(pid, "terminal", mid, now);
        self.resolve_open(pid, "startup", mid, now);
        let mut requeued = Vec::new();
        for (t, _) in self.store.tasks(pid).unwrap_or_default() {
            if t.to == mid
                && matches!(t.state, TaskState::Delivered | TaskState::InProgress)
                && self.store.transition(pid, &t.id, Transition::Requeue, &Actor::Member("lilad".into()), &format!("{mid} ended: {why}"), now).is_ok()
            {
                requeued.push(t.id);
            }
        }
        let _ = self.store.event(pid, None, "lilad", "member_gone", &format!("{mid} ({}): {why}", terminal.unwrap_or_default()), now);
        self.say(format!("{pid}: {mid} ended ({why}); requeued {requeued:?}"));
        if unexpected {
            let tail = if requeued.is_empty() { String::new() } else { format!(" Its tasks {} went back to the queue.", requeued.join(", ")) };
            let _ = self.store.add_inbox(pid, "lost", Some(mid), None, &inbox::lost(mid, why, &requeued), &[], now);
            // The PM hears only when there is something for it to do (work to hand
            // elsewhere): every note is a turn, and the person sees it in the PM's terminal.
            let owner = self.projects[pid].team.owner.clone();
            if owner != mid && !requeued.is_empty() {
                let _ = self.store.push_outbox(pid, &owner, &message::note(mid, &format!("{mid} ended ({why}).{tail} The person was told.")), now);
            }
        }
    }

    // ---------------- the pump: notes, deliveries, timeouts ----------------

    /// Everything that can move now: pending notes, queued tasks.
    pub fn pump(&mut self, pid: &str, now: i64) {
        if !self.projects.contains_key(pid) {
            return;
        }
        // Notes first: they are information the member should have before new work.
        // A member's notes go out as one message: one turn when it is idle.
        let mut ready: Vec<(String, Vec<lila_store::OutMessage>)> = Vec::new();
        for msg in self.store.outbox(pid).unwrap_or_default() {
            if !self.projects[pid].live.contains_key(&msg.member) {
                let _ = self.store.pop_outbox(msg.id);
                continue;
            }
            match ready.iter_mut().find(|(m, _)| *m == msg.member) {
                Some((_, msgs)) => msgs.push(msg),
                None => ready.push((msg.member.clone(), vec![msg])),
            }
        }
        let mut noted: Vec<String> = Vec::new();
        for (member, msgs) in ready {
            let live = &self.projects[pid].live[&member];
            let (Some(t), true) = (live.terminal.clone(), live.status.accepts_notes()) else { continue };
            let text = msgs.iter().map(|m| m.text.as_str()).collect::<Vec<_>>().join("\n");
            noted.push(member.clone());
            match self.term.send(&t, &text, None) {
                Ok(()) => {
                    for m in &msgs {
                        let _ = self.store.pop_outbox(m.id);
                    }
                }
                Err(e) => self.say(format!("{pid}: note to {member} not sent yet: {e:#}")),
            }
        }

        if self.store.project(pid).ok().flatten().is_some_and(|p| p.paused) {
            return;
        }
        let tasks = self.store.tasks(pid).unwrap_or_default();
        let proj = &self.projects[pid];
        let views: Vec<MemberView> = proj
            .team
            .members
            .iter()
            .map(|m| {
                let held = |f: fn(TaskState) -> bool| tasks.iter().any(|(t, _)| t.to == m.id && f(t.state));
                MemberView {
                    id: m.id.clone(),
                    is_owner: m.is_owner,
                    status: proj.live[&m.id].status,
                    holds_task: held(TaskState::is_active),
                    busy: held(|s| matches!(s, TaskState::Delivered | TaskState::InProgress)),
                }
            })
            .collect();
        let codex_until = self.codex_paused.get(pid).copied().filter(|t| *t > now);
        let folder = self.held_for_folder(pid, &tasks);
        let queued: Vec<QueuedTask> = tasks
            .iter()
            .filter(|(t, _)| t.state == TaskState::Queued)
            .filter(|(t, _)| !self.held_for_review(pid, t) && !folder.contains(&t.id))
            .filter(|(t, _)| codex_until.is_none() || proj.team.member(&t.to).is_none_or(|m| m.role.file.runtime != lila_core::Runtime::Codex))
            .map(|(t, meta)| QueuedTask { id: t.id.clone(), to: t.to.clone(), priority: t.priority, seq: meta.seq })
            .collect();
        let picks = dispatch::pick_deliveries(&views, &queued, proj.team.settings.max_working);
        for (tid, mid) in picks {
            let Some(task) = tasks.iter().find(|(t, _)| t.id == tid).map(|(t, _)| t.clone()) else { continue };
            let live = self.projects[pid].live[&mid].clone();
            let Some(terminal) = live.terminal.clone() else { continue };
            let ancestors = self.store.ancestors(pid, &tid).unwrap_or_default();
            let chain = ancestors.last().map(|t| t.id.clone()).unwrap_or_else(|| tid.clone());
            let proj = &self.projects[pid];
            let fresh = proj.team.settings.fresh_session && proj.team.member(&mid).is_some_and(|m| !m.is_owner);
            if fresh && live.chain.as_ref().is_some_and(|c| *c != chain) {
                let plan = self.switch_plan(pid, &mid, &live, &chain);
                if plan != Switch::Keep {
                    // A note just typed would be lost with the session: wait until it is read.
                    if !noted.contains(&mid) {
                        self.new_session(pid, &mid, &tid, plan, now);
                    }
                    continue;
                }
            }
            // The request the chain serves: the person's words from nearest the root,
            // and every "done when" and "out of scope" set on the way down.
            let origin = chain_brief(&ancestors);
            let origin = origin.as_ref().map(|(ids, file, brief)| message::ChainBrief { id: ids, file, brief });
            let hint = self.step_hint(pid, &task);
            let text = message::task_delivery(&task, &task_file_rel(&task.id), origin, hint.as_ref());
            // Type only if it is still idle. hc's `if` knows `idle` and `working`
            // only; `done` (finished while unseen) takes input like idle, unguarded.
            let guard = (live.status == AgentStatus::Idle).then_some("idle");
            match self.term.send(&terminal, &text, guard) {
                Ok(()) => {
                    self.failed_sends.remove(&tid);
                    let _ = self.store.transition(pid, &tid, Transition::Deliver, &Actor::Member("lilad".into()), &format!("to {terminal}"), now);
                    let live = self.projects.get_mut(pid).unwrap().live.get_mut(&mid).unwrap();
                    live.free_since = now;
                    live.chain = Some(chain.clone());
                    if let Some(sid) = live.session.clone() {
                        let _ = self.store.touch_chain(pid, &sid, &chain, now);
                    }
                    self.say(format!("{pid}: delivered {tid} to {mid}"));
                }
                Err(e) => {
                    let e = format!("{e:#}");
                    if self.failed_sends.get(&tid) != Some(&e) {
                        self.say(format!("{pid}: {tid} not delivered to {mid} yet: {e}"));
                        self.failed_sends.insert(tid.clone(), e);
                    }
                }
            }
        }
    }

}


fn split_brief(brief: &str) -> (String, String) {
    let brief = brief.trim();
    let first = brief.lines().next().unwrap_or_default();
    let title: String = first.chars().take(80).collect();
    // A one-line brief is all title: the body would only say it again.
    let body = if title == brief { String::new() } else { brief.to_owned() };
    (title, body)
}

fn ensure_gitignore(root: &Path) -> Result<()> {
    // Task files and the spool are run state; project.toml is configuration and stays tracked.
    let path = root.join(".lila").join(".gitignore");
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    let before = text.clone();
    for line in ["tasks/", "spool/"] {
        if !text.lines().any(|l| l.trim() == line) {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(line);
            text.push('\n');
        }
    }
    if text != before {
        std::fs::write(path, text)?;
    }
    Ok(())
}

/// `.lila/spool/` in a project: the file channel for sandboxed members (codex
/// workspace-write) that may not open the lilad socket. `<id>.req.json` in,
/// `<id>.reply.json` out; both written to a temporary name and renamed.
pub fn spool_dir(root: &Path) -> PathBuf {
    root.join(".lila").join("spool")
}

impl<T: Terminals> Daemon<T> {
    /// Answers every pending spool request in every project.
    pub fn process_spool(&mut self, now: i64) {
        let roots: Vec<PathBuf> = self.projects.values().map(|p| p.cwd.clone()).collect();
        for root in roots {
            let dir = spool_dir(&root);
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            let mut reqs: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.to_string_lossy().ends_with(".req.json")).collect();
            reqs.sort();
            for path in reqs {
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                let _ = std::fs::remove_file(&path);
                let reply = match serde_json::from_str::<Value>(&text) {
                    Ok(req) => {
                        let method = req["method"].as_str().unwrap_or_default();
                        if method == "stop" {
                            json!({"error": "lilad can't be stopped through the spool"})
                        } else {
                            match self.handle(method, &req["params"], now) {
                                Ok(v) => json!({"result": v}),
                                Err(e) => json!({"error": format!("{e:#}")}),
                            }
                        }
                    }
                    Err(e) => json!({"error": format!("bad spool request: {e}")}),
                };
                let name = path.file_name().unwrap().to_string_lossy().replace(".req.json", "");
                let tmp = dir.join(format!("{name}.reply.tmp"));
                if std::fs::write(&tmp, reply.to_string()).is_ok() {
                    let _ = std::fs::rename(&tmp, dir.join(format!("{name}.reply.json")));
                }
            }
        }
    }
}

/// The brief a task's ancestors (nearest first) set: ids, the file of the words, the brief.
fn chain_brief(ancestors: &[Task]) -> Option<(String, String, Brief)> {
    let set: Vec<(String, Brief)> = ancestors.iter().rev().map(|a| (a.id.clone(), a.chain_brief())).filter(|(_, b)| !b.is_empty()).collect();
    let (first, _) = set.first()?;
    let mut merged = Brief::default();
    let mut file = task_file_rel(first);
    for (id, b) in &set {
        if merged.quote.is_none() && b.quote.is_some() {
            merged.quote = b.quote.clone();
            file = task_file_rel(id);
        }
        for (into, from) in [(&mut merged.accept, &b.accept), (&mut merged.out_of_scope, &b.out_of_scope)] {
            for item in from {
                if !into.contains(item) {
                    into.push(item.clone());
                }
            }
        }
    }
    let ids = set.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>().join(", ");
    Some((ids, file, merged))
}

pub fn task_file_rel(id: &str) -> String {
    format!(".lila/tasks/{id}.md")
}

fn write_task_file(cwd: &Path, t: &Task) -> Result<()> {
    let dir = cwd.join(".lila").join("tasks");
    std::fs::create_dir_all(&dir)?;
    let parent = t.parent.as_deref().map(|p| format!(" · parent {p}")).unwrap_or_default();
    let mut text = format!("# {} · {}\n\nfrom {} → {}{parent}\n\n{}\n", t.id, t.title, t.from, t.to, t.body.trim());
    let b = &t.brief;
    if let Some(q) = b.quote.as_deref() {
        text.push_str("\n## The person's words\n\n");
        text.extend(q.lines().map(|l| format!("> {l}\n")));
    }
    for (head, items) in [("Done when", &b.accept), ("Out of scope", &b.out_of_scope)] {
        if !items.is_empty() {
            text.push_str(&format!("\n## {head}\n\n"));
            text.extend(items.iter().map(|i| format!("- {i}\n")));
        }
    }
    std::fs::write(dir.join(format!("{}.md", t.id)), text)?;
    Ok(())
}

pub fn task_json(t: &Task) -> Value {
    json!({
        "id": t.id, "title": t.title, "body": t.body, "state": t.state.as_str(), "from": t.from.as_str(), "to": t.to,
        "parent": t.parent, "priority": t.priority, "summary": t.summary, "block_reason": t.block_reason,
        "created_ms": t.created_ms, "updated_ms": t.updated_ms,
        "quote": t.brief.quote, "accept": t.brief.accept, "out_of_scope": t.brief.out_of_scope,
        "decisions": t.outcome.decisions, "unverified": t.outcome.unverified,
        "verdict": t.outcome.passed.map(|p| if p { "pass" } else { "fail" }),
        "run": t.run.as_ref().map(|(a, _)| a), "run_step": t.run.as_ref().map(|(_, s)| s),
    })
}

fn inbox_json(i: &lila_store::InboxItem) -> Value {
    json!({
        "id": i.id, "kind": i.kind, "member": i.member, "task": i.task, "text": i.text, "options": i.options,
        "answer": i.answer, "created_ms": i.created_ms, "resolved_ms": i.resolved_ms,
    })
}

impl<T: Terminals> Daemon<T> {
    /// After a fresh snapshot: members pointing at terminals that no longer exist ended while we were away.
    /// Members started after the snapshot was taken are not in it and are left alone.
    pub fn reconcile(&mut self, live_terminals: &[String], taken_ms: i64, now: i64) {
        let gone: Vec<(String, String)> = self
            .projects
            .iter()
            .flat_map(|(pid, p)| {
                p.live
                    .iter()
                    .filter(move |(_, l)| l.started_ms.is_none_or(|s| s < taken_ms - 2000))
                    .filter(|(_, l)| l.terminal.as_ref().is_some_and(|t| !live_terminals.contains(t)))
                    .map(move |(m, _)| (pid.clone(), m.clone()))
            })
            .collect();
        for (pid, mid) in gone {
            self.member_gone(&pid, &mid, "its terminal no longer exists", now, true);
            self.pump(&pid, now);
        }
    }
}

#[cfg(test)]
mod tests;
