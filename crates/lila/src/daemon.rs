//! lilad: the only writer of Lila's state. Requests come from the CLI over a
//! Unix socket, terminal facts from herdr-core's event stream; both are
//! handled one at a time on one thread, so there is no locking.

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

    // ---------------- requests ----------------

    /// One CLI request. `params.caller` = {project?, member?, cwd}.
    pub fn handle(&mut self, method: &str, params: &Value, now: i64) -> Result<Value> {
        let caller = params.get("caller").cloned().unwrap_or(Value::Null);
        match method {
            "ping" => Ok(json!({"pong": true, "pid": std::process::id()})),
            "project.up" => self.project_up(&caller, params, now),
            "project.down" => self.project_down(&caller, now),
            "project.remove" => self.project_remove(&caller, now),
            "project.reload" => self.project_reload(&caller, now),
            "member.restart" => self.member_restart(&caller, params, now),
            "projects" => Ok(json!(self.projects.iter().map(|(id, p)| json!({"id": id, "cwd": p.cwd})).collect::<Vec<_>>())),
            "status" => self.status(&caller),
            "task.new" => self.task_new(&caller, params, now),
            "task.start" => self.task_move(&caller, params, Transition::Start, now),
            "task.done" => self.task_move(&caller, params, Transition::Done, now),
            "task.block" => self.task_move(&caller, params, Transition::Block, now),
            "task.cancel" => self.task_move(&caller, params, Transition::Cancel, now),
            "task.edit" => self.task_edit(&caller, params, now),
            "task.show" => self.task_show(&caller, params),
            "task.ls" => self.task_ls(&caller, params),
            "ask" => self.ask(&caller, params, now),
            "inbox" => self.inbox(&caller, params),
            "answer" => self.answer(&caller, params, now),
            "log" => self.events(&caller, params),
            "pause" | "resume" => {
                let (pid, _) = self.who(&caller)?;
                self.store.set_paused(&pid, method == "pause")?;
                if method == "resume" {
                    self.pump(&pid, now);
                }
                Ok(json!({"project": pid, "paused": method == "pause"}))
            }
            other => Err(anyhow!("unknown request {other}")),
        }
    }

    /// The project and actor a request comes from.
    fn who(&self, caller: &Value) -> Result<(String, Actor)> {
        let member = s(caller, "member");
        let pid = match s(caller, "project") {
            Some(p) => p.to_owned(),
            None => {
                let cwd = req(caller, "cwd")?;
                let root = crate::paths::find_project_root(Path::new(cwd)).ok_or_else(|| anyhow!("no .lila/project.toml in {cwd} or above; run `lila init`"))?;
                let root = root.to_string_lossy().to_string();
                self.store.project_by_cwd(&root)?.map(|p| p.id).ok_or_else(|| anyhow!("project in {root} is not running yet; run `lila up` there"))?
            }
        };
        let proj = self.projects.get(&pid).ok_or_else(|| anyhow!("unknown project {pid}; run `lila up` in its folder"))?;
        let actor = match member {
            None => Actor::Human,
            Some(m) => {
                if proj.team.member(m).is_none() {
                    bail!("{m} is not a member of project {pid}");
                }
                Actor::Member(m.to_owned())
            }
        };
        Ok((pid, actor))
    }

    /// The registered project the caller's folder belongs to, loaded or not.
    fn registered(&self, caller: &Value) -> Option<(String, PathBuf)> {
        let root = crate::paths::find_project_root(Path::new(s(caller, "cwd")?))?;
        let p = self.store.project_by_cwd(&root.to_string_lossy()).ok()??;
        Some((p.id, root))
    }

    fn project_up(&mut self, caller: &Value, params: &Value, now: i64) -> Result<Value> {
        if s(caller, "member").is_some() {
            bail!("only the person starts a project");
        }
        let cwd = req(caller, "cwd")?;
        let root = crate::paths::find_project_root(Path::new(cwd)).ok_or_else(|| anyhow!("no .lila/project.toml in {cwd} or above; run `lila init` first"))?;
        let team = load_team(&root, &self.paths.roles())?;
        let pid = team.project.clone();
        let root_s = root.to_string_lossy().to_string();
        self.store.upsert_project(&pid, &root_s, now)?;
        self.load_project(&pid, &root, now)?;
        std::fs::create_dir_all(root.join(".lila").join("tasks"))?;
        std::fs::create_dir_all(spool_dir(&root))?;
        ensure_gitignore(&root)?;

        // A project may have been absent from the database when the event
        // snapshot arrived. Recover its existing team before starting anyone.
        for rec in self.term.list(&pid)? {
            if rec["state"] != "live" || rec["labels"]["lila.project"] != pid || rec["cwd"] != root_s {
                continue;
            }
            let (Some(mid), Some(addr)) = (s(&rec["labels"], "lila.member"), s(&rec, "address")) else { continue };
            let Some(member) = self.projects[&pid].team.member(mid) else { continue };
            let started_ms = self.projects[&pid].live[mid].started_ms.unwrap_or(now);
            self.store.set_member(&pid, mid, &member.role.id, Some(addr), Some(started_ms))?;
            self.on_terminal(&rec, now);
        }

        let mut started = Vec::new();
        let mut running = Vec::new();
        let members: Vec<_> = self.projects[&pid].team.members.clone();
        let to_start: Vec<_> = members
            .iter()
            .filter(|m| {
                let live = &self.projects[&pid].live[&m.id];
                live.terminal.is_none() || live.status == AgentStatus::Gone
            })
            .collect();
        // Check every program first, so a missing one doesn't leave half a team running.
        for m in &to_start {
            self.member_env(&m.role)?;
        }
        for m in &members {
            let live = &self.projects[&pid].live[&m.id];
            if !to_start.iter().any(|s| s.id == m.id) {
                running.push(json!({"member": m.id, "terminal": live.terminal}));
                continue;
            }
            let addr = self.start_member(&pid, &m.id, now).with_context(|| format!("start {}", m.id))?;
            started.push(json!({"member": m.id, "terminal": addr, "runtime": m.role.file.runtime.to_string(), "model": m.role.file.model}));
        }

        let mut brief_task = None;
        if let Some(brief) = s(params, "brief") {
            let owner = self.projects[&pid].team.owner.clone();
            let (title, body) = split_brief(brief);
            let t = self.create_task(&pid, NewTask { title, body, from: Actor::Human, to: owner, parent: None, priority: 0, brief: Brief::default(), run: None }, now)?;
            brief_task = Some(t.id);
        }
        self.pump(&pid, now);
        Ok(json!({"project": pid, "cwd": root_s, "started": started, "running": running, "brief_task": brief_task}))
    }

    fn start_member(&mut self, pid: &str, mid: &str, now: i64) -> Result<String> {
        self.start_session(pid, mid, runtime::Session::New(None), now)
    }

    /// Starts a member's CLI in a new session or an earlier one (`Session::Resume`).
    fn start_session(&mut self, pid: &str, mid: &str, session: runtime::Session, now: i64) -> Result<String> {
        let proj = &self.projects[pid];
        let member = proj.team.member(mid).expect("member").clone();
        let cwd = proj.cwd.to_string_lossy().to_string();
        let text = prompt::render(&proj.team, &member, &cwd);
        let dir = self.paths.prompts(pid);
        std::fs::create_dir_all(&dir)?;
        let prompt_path = dir.join(format!("{mid}.md"));
        std::fs::write(&prompt_path, &text)?;

        let rt = member.role.file.runtime;
        // Claude Code takes the id Lila picks, so the session is known from the start.
        let session = match session {
            runtime::Session::New(None) if rt == lila_core::Runtime::Claude => runtime::Session::New(Some(sessions::new_uuid())),
            other => other,
        };
        let mut launch = runtime::launch(&member.role, &prompt_path.to_string_lossy(), &text, &session);
        let (program, mut env) = self.member_env(&member.role)?;
        launch.argv[0] = program.to_string_lossy().to_string();
        env.insert("LILA_PROJECT".into(), pid.to_owned());
        env.insert("LILA_ROOT".into(), cwd.clone());
        env.insert("LILA_MEMBER".into(), mid.to_owned());
        if let Some(h) = std::env::var_os("LILA_HOME") {
            env.insert("LILA_HOME".into(), h.to_string_lossy().to_string());
        }

        let name = terminal_name(pid, mid);
        let addr = self.term.run(json!({
            "argv": launch.argv,
            "cwd": cwd,
            "env": env,
            "labels": {"lila.project": pid, "lila.member": mid},
            "name": name,
            "no_parent": true,
            "no_notify": true,
        }))?;
        self.store.set_member(pid, mid, &member.role.id, Some(&addr), Some(now))?;
        let known = match &session {
            runtime::Session::New(Some(id)) | runtime::Session::Resume(id) => Some(id.clone()),
            runtime::Session::New(None) => None,
        };
        let mut chain = None;
        if let Some(id) = &known {
            let file = (rt == lila_core::Runtime::Claude).then(|| sessions::claude_file(&self.live_sources(), &cwd, id).to_string_lossy().to_string());
            self.store.add_session(pid, &SessionRow { id: id.clone(), member: mid.to_owned(), runtime: rt.to_string(), file, started_ms: now })?;
            chain = self.store.last_chain(pid, id)?;
        }
        let live = self.projects.get_mut(pid).unwrap().live.get_mut(mid).unwrap();
        *live = Live {
            terminal: Some(addr.clone()),
            status: AgentStatus::Unknown,
            started_ms: Some(now),
            free_since: now,
            launched: Some(member.launch_key()),
            session: known,
            chain,
            busy_since: now,
            ..Live::default()
        };
        self.live_version += 1;
        let shown: Vec<String> = launch.argv.iter().map(|a| if a.chars().count() > 80 { format!("{}…", a.chars().take(40).collect::<String>()) } else { a.clone() }).collect();
        self.store.event(pid, None, "lilad", "member_started", &format!("{mid} in {addr}: {}", shown.join(" ")), now)?;
        self.say(format!("{pid}: started {mid} in {addr}"));
        Ok(addr)
    }

    /// The absolute program for a role and the environment its terminal gets
    /// (PATH = role PATH + lila's folder + the PATH lilad was started with).
    fn member_env(&self, role: &lila_core::Role) -> Result<(PathBuf, BTreeMap<String, String>)> {
        let lila_dir = self.lila_bin.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        let mut path = format!("{lila_dir}:{}", self.path_env);
        let mut env: BTreeMap<String, String> = BTreeMap::new();
        for (k, v) in &role.file.env {
            if k == "PATH" {
                path = format!("{v}:{path}");
            } else {
                env.insert(k.clone(), v.clone());
            }
        }
        // Claude Code draws inline by default: its input box follows the output
        // and the wheel scrolls the whole terminal. Full-screen (alternate screen,
        // the wheel to Claude) keeps the input at the bottom. A role's env.CLAUDE_CODE_NO_FLICKER = "0" turns it off.
        if role.file.runtime == lila_core::Runtime::Claude {
            env.entry("CLAUDE_CODE_NO_FLICKER".into()).or_insert_with(|| "1".into());
        }
        let name = runtime::launch(role, "", "", &runtime::Session::New(None)).argv[0].clone();
        let program = which(&name, &path).ok_or_else(|| anyhow!("{name:?} is not on PATH for role {}; add its folder to the role's env.PATH", role.id))?;
        env.insert("PATH".into(), path);
        // Members run on their CLI's current account (`accounts`): its own config folder.
        if let Some((var, dir)) = self.account(role.file.runtime).and_then(|a| a.env()) {
            env.insert(var.into(), dir);
        }
        Ok((program, env))
    }

    /// A CLI's current subscription account (`~/.config/lila/accounts.toml`).
    fn account(&self, r: lila_core::Runtime) -> Option<crate::accounts::Account> {
        let list = crate::accounts::load(&self.paths.accounts()).ok()?;
        crate::accounts::current(&list, r).cloned()
    }

    /// Where the CLIs record sessions now: the current accounts' folders, or the
    /// defaults (`sources`) for an account in the CLI's own folder.
    fn live_sources(&self) -> Sources {
        let pick = |r: lila_core::Runtime, default: &PathBuf| self.account(r).and_then(|a| a.dir).unwrap_or_else(|| default.clone());
        Sources { claude: pick(lila_core::Runtime::Claude, &self.sources.claude), codex: pick(lila_core::Runtime::Codex, &self.sources.codex) }
    }

    /// Reads project.toml and the roles again (after the settings changed).
    /// Running members keep running; `status` marks those on old settings `stale`.
    fn project_reload(&mut self, caller: &Value, now: i64) -> Result<Value> {
        let (pid, actor) = match self.who(caller) {
            Ok(w) => w,
            // Registered but not loaded (its project.toml was missing when lilad
            // started, and the window has just written it again): load it now.
            Err(e) => {
                let Some((pid, root)) = self.registered(caller) else { return Err(e) };
                if s(caller, "member").is_some() {
                    return Err(e);
                }
                self.load_project(&pid, &root, now)?;
                self.live_version += 1;
                self.say(format!("{pid}: loaded"));
                return Ok(json!({"project": pid, "stale": [], "removed": [], "loaded": true}));
            }
        };
        if actor != Actor::Human {
            bail!("only the person reloads a project");
        }
        let cwd = self.projects[&pid].cwd.clone();
        let removed = self.load_project(&pid, &cwd, now)?;
        self.live_version += 1;
        let stale: Vec<String> = self.stale_members(&pid);
        Ok(json!({"project": pid, "stale": stale, "removed": removed}))
    }

    /// A member left the team (the settings removed it): its CLI stops, and its
    /// open tasks are cancelled, which whoever assigned them hears about.
    fn member_removed(&mut self, pid: &str, mid: &str, terminal: Option<String>, now: i64) {
        if let Some(t) = &terminal
            && let Err(e) = self.term.kill(t)
        {
            self.say(format!("{pid}: kill removed {mid} ({t}): {e:#}"));
        }
        let _ = self.store.remove_member(pid, mid);
        let owner = self.projects[pid].team.owner.clone();
        let mut cancelled = Vec::new();
        for (t, _) in self.store.tasks(pid).unwrap_or_default() {
            if t.to != mid || !t.state.is_open() {
                continue;
            }
            let why = format!("{mid} was removed from the team");
            if self.store.transition(pid, &t.id, Transition::Cancel, &Actor::Human, &why, now).is_err() {
                continue;
            }
            if let Actor::Member(from) = &t.from
                && *from != owner
                && self.projects[pid].team.member(from).is_some()
            {
                let _ = self.store.push_outbox(pid, from, &message::note(&t.id, &format!("{} \"{}\" was cancelled: {why}. Hand it to someone else if it is still needed.", t.id, t.title)), now);
            }
            cancelled.push(format!("{} \"{}\" (from {})", t.id, t.title, t.from));
        }
        for kind in ["terminal", "startup", "lost", "stalled", "runaway"] {
            self.resolve_open(pid, kind, mid, now);
        }
        let detail = if cancelled.is_empty() { String::new() } else { format!("; cancelled {}", cancelled.join(", ")) };
        let _ = self.store.event(pid, None, "lilad", "member_removed", &format!("{mid}{detail}"), now);
        self.say(format!("{pid}: {mid} removed{detail}"));
        if !cancelled.is_empty() {
            let text = format!("{mid} was removed from the team. Its open tasks were cancelled: {}. Re-assign what is still needed.", cancelled.join("; "));
            let _ = self.store.push_outbox(pid, &owner, &message::note(mid, &text), now);
        }
    }

    /// Running members started with other settings than the project has now.
    fn stale_members(&self, pid: &str) -> Vec<String> {
        let proj = &self.projects[pid];
        proj.team.members.iter().filter(|m| proj.live.get(&m.id).is_some_and(|l| l.terminal.is_some() && l.launched.is_some_and(|k| k != m.launch_key()))).map(|m| m.id.clone()).collect()
    }

    /// Stops one member and starts it again with the current settings. Its
    /// open task goes back to the queue and is delivered again.
    fn member_restart(&mut self, caller: &Value, params: &Value, now: i64) -> Result<Value> {
        let (pid, actor) = self.who(caller)?;
        if actor != Actor::Human {
            bail!("only the person restarts a member");
        }
        let mid = req(params, "member")?.to_owned();
        let member = self.projects[&pid].team.member(&mid).cloned().ok_or_else(|| anyhow!("no member {mid:?} in project {pid}"))?;
        self.member_env(&member.role)?;
        if let Some(t) = self.projects[&pid].live[&mid].terminal.clone() {
            if let Err(e) = self.term.kill(&t) {
                self.say(format!("{pid}: kill {mid} ({t}): {e:#}"));
            }
            self.member_gone(&pid, &mid, "restarted with new settings", now, false);
        }
        let addr = self.start_member(&pid, &mid, now)?;
        if member.is_owner {
            let digest = self.digest(&pid)?;
            self.store.push_outbox(&pid, &mid, &digest, now)?;
        }
        self.pump(&pid, now);
        Ok(json!({"project": pid, "member": mid, "terminal": addr}))
    }

    fn project_down(&mut self, caller: &Value, now: i64) -> Result<Value> {
        let (pid, actor) = self.who(caller)?;
        if actor != Actor::Human {
            bail!("only the person stops a project");
        }
        let mut stopped = Vec::new();
        let ids: Vec<String> = self.projects[&pid].live.keys().cloned().collect();
        for mid in ids {
            let terminal = self.projects[&pid].live[&mid].terminal.clone();
            if let Some(t) = terminal {
                if let Err(e) = self.term.kill(&t) {
                    self.say(format!("{pid}: kill {mid} ({t}): {e:#}"));
                }
                self.member_gone(&pid, &mid, "stopped by `lila down`", now, false);
                stopped.push(mid);
            }
        }
        Ok(json!({"project": pid, "stopped": stopped}))
    }

    /// Stops a project's team and forgets it: tasks, events, inbox, sessions.
    /// Its folder and `.lila/` stay, so it can be added again (without its history).
    fn project_remove(&mut self, caller: &Value, now: i64) -> Result<Value> {
        if s(caller, "member").is_some() {
            bail!("only the person removes a project");
        }
        let pid = match s(caller, "project") {
            Some(p) => p.to_owned(),
            None => self.registered(caller).map(|(id, _)| id).ok_or_else(|| anyhow!("this folder is not a registered project"))?,
        };
        if self.store.project(&pid)?.is_none() {
            bail!("no project {pid}");
        }
        let mut stopped = json!([]);
        if self.projects.contains_key(&pid) {
            stopped = self.project_down(&json!({"project": pid}), now)?["stopped"].take();
            self.projects.remove(&pid);
        }
        self.checks.retain(|j| j.project != pid);
        self.checking.retain(|(p, _)| *p != pid);
        self.commits.retain(|j| j.project != pid);
        self.committing.retain(|(p, _)| *p != pid);
        for m in [&mut self.quota_checked, &mut self.codex_paused, &mut self.quota_told] {
            m.remove(&pid);
        }
        self.store.remove_project(&pid)?;
        self.live_version += 1;
        self.say(format!("{pid}: removed"));
        Ok(json!({"project": pid, "removed": true, "stopped": stopped}))
    }

    fn status(&mut self, caller: &Value) -> Result<Value> {
        let (pid, _) = self.who(caller)?;
        // How full each member's context is (their session's latest turn), against
        // where Lila starts it afresh: `pm_refresh_tokens` for the PM, else `fresh_after_tokens`.
        let ids: Vec<(String, bool, Option<String>)> = self.projects[&pid].team.members.iter().map(|m| (m.id.clone(), m.is_owner, self.projects[&pid].live[&m.id].session.clone())).collect();
        let mut contexts: BTreeMap<String, Option<u64>> = BTreeMap::new();
        for (mid, _, session) in &ids {
            let n = if self.projects[&pid].live[mid].terminal.is_some() { self.context_of(&pid, session.as_deref()) } else { None };
            contexts.insert(mid.clone(), n);
        }
        let proj = &self.projects[&pid];
        let tasks = self.store.tasks(&pid)?;
        let members: Vec<Value> = proj
            .team
            .members
            .iter()
            .map(|m| {
                let live = &proj.live[&m.id];
                let current = tasks.iter().find(|(t, _)| t.to == m.id && t.state.is_active()).map(|(t, _)| json!({"id": t.id, "title": t.title, "state": t.state.as_str()}));
                let queued = tasks.iter().filter(|(t, _)| t.to == m.id && t.state == TaskState::Queued).count();
                let stale = live.terminal.is_some() && live.launched.is_some_and(|k| k != m.launch_key());
                json!({
                    "id": m.id, "role": m.role.file.name, "runtime": m.role.file.runtime.to_string(), "model": m.role.file.model,
                    "owner": m.is_owner, "terminal": live.terminal, "status": live.status.as_str(), "kind": live.kind, "msg": live.msg,
                    "task": current, "queued": queued, "stale": stale, "effort": m.role.file.effort,
                    "context": contexts.get(&m.id).copied().flatten(),
                    "context_limit": if m.is_owner { proj.team.settings.pm_refresh_tokens } else { proj.team.settings.fresh_after_tokens },
                })
            })
            .collect();
        let open: Vec<Value> = tasks.iter().filter(|(t, _)| t.state.is_open()).map(|(t, _)| task_json(t)).collect();
        let inbox = self.store.inbox(&pid, false)?.len();
        let paused = self.store.project(&pid)?.map(|p| p.paused).unwrap_or(false);
        let runs: Vec<Value> = self
            .store
            .runs(&pid)?
            .into_iter()
            .filter(|r| r.state.is_open())
            .map(|r| {
                let title = tasks.iter().find(|(t, _)| t.id == r.anchor).map(|(t, _)| t.title.clone()).unwrap_or_default();
                let steps: Vec<String> = proj.team.flows.get(&r.flow).map(|f| f.steps.iter().map(|s| s.action.clone()).collect()).unwrap_or_default();
                let step = steps.get(r.step).cloned().unwrap_or_default();
                json!({
                    "anchor": r.anchor, "title": title, "flow": r.flow, "step": step, "round": r.round, "state": r.state.as_str(),
                    "steps": steps, "step_index": r.step, "max_rounds": proj.team.settings.max_rounds,
                })
            })
            .collect();
        let held = tasks.iter().filter(|(t, _)| t.state == TaskState::Queued && self.held_for_review(&pid, t)).count();
        let codex_until = self.codex_paused.get(&pid).copied();
        let plain: Vec<Task> = tasks.iter().map(|(t, _)| t.clone()).collect();
        Ok(json!({
            "project": pid, "cwd": proj.cwd, "paused": paused, "members": members, "open_tasks": open, "inbox": inbox, "runs": runs,
            "held_for_review": held, "unreviewed": self.unreviewed(&pid), "codex_paused_until": codex_until,
            "open_chains": self.open_chains(&pid, &plain), "held_for_folder": self.held_for_folder(&pid, &tasks).len(),
        }))
    }

    fn create_task(&mut self, pid: &str, new: NewTask, now: i64) -> Result<Task> {
        let task = self.store.create_task(pid, new, now)?;
        let cwd = self.projects[pid].cwd.clone();
        write_task_file(&cwd, &task)?;
        self.say(format!("{pid}: {} {} → {}: {}", task.id, task.from, task.to, task.title));
        Ok(task)
    }

    fn task_new(&mut self, caller: &Value, params: &Value, now: i64) -> Result<Value> {
        let (pid, actor) = self.who(caller)?;
        let parent = s(params, "parent").map(|p| p.to_ascii_uppercase());
        if let Some(p) = &parent {
            self.store.task(&pid, p)?.ok_or_else(|| anyhow!("no task {p} to use as parent"))?;
        }
        let to = self.resolve_to(&pid, &actor, req(params, "to")?, parent.as_deref())?;
        let team = &self.projects[&pid].team;
        team.check_assign(&actor, &to).map_err(anyhow::Error::msg)?;
        // Inside a flow, lilad hands the work on: a member doing it too would double it.
        if let (Some(p), Actor::Member(m)) = (&parent, &actor)
            && *m != team.owner
            && let Some(pt) = self.store.task(&pid, p)?
            && let Some((run, f, step)) = self.current_step(&pid, &pt)
            && f.steps.iter().any(|st| st.by.as_deref().is_some_and(|by| team.covers(by, &to)))
        {
            let how = if f.steps[step].wants_verdict() {
                format!("finish {p} with `lila task done {p} --verdict fail --summary \"<what fails>\"`: Lila sends the fix back")
            } else {
                format!("finish {p} with `lila task done {p}`: Lila {}", flow::later(&f, step, self.check_cmd(&pid).as_deref()).join(", then "))
            };
            bail!("{p} is a step of the {} flow; {how}", run.flow);
        }
        let flow_name = team.flow_for(s(params, "flow"), &to, &actor).map_err(anyhow::Error::msg)?;
        let budget = params.get("budget").and_then(Value::as_u64).filter(|b| *b > 0);
        if budget.is_some() && flow_name.is_none() {
            bail!("a budget is for a flow run; this task runs no flow (give --flow)");
        }
        let max_depth = team.settings.max_chain_depth;
        if let Some(p) = &parent
            && self.store.depth(&pid, p)? + 1 >= max_depth
        {
            bail!("task chain under {p} is already {max_depth} deep; report to the PM with `lila task block` instead of creating more subtasks");
        }
        if let (Some(p), Actor::Member(from)) = (&parent, &actor) {
            let chain: Vec<(String, String)> = self.store.ancestors(&pid, p)?.into_iter().map(|t| (t.from.to_string(), t.to)).collect();
            let parent_task = self.store.task(&pid, p)?.expect("checked above");
            let mut chain_with_parent = vec![(parent_task.from.to_string(), parent_task.to.clone())];
            chain_with_parent.extend(chain);
            let max = self.projects[&pid].team.settings.max_rounds;
            let rounds = lila_core::task::rounds(from, &to, &chain_with_parent);
            if rounds > max {
                bail!(
                    "{from} and {to} have already sent this work back and forth {max} times; stop and run `lila task block {p} --reason \"<what keeps failing>\"` so the PM can decide"
                );
            }
        }
        let new = NewTask {
            title: req(params, "title")?.trim().to_owned(),
            body: s(params, "body").unwrap_or_default().to_owned(),
            from: actor,
            to,
            parent,
            priority: params.get("priority").and_then(Value::as_i64).unwrap_or(0),
            brief: Brief { quote: s(params, "quote").map(String::from), accept: strings(params, "accept"), out_of_scope: strings(params, "out_of_scope") },
            run: None,
        };
        let task = self.create_task(&pid, new, now)?;
        if let Some(f) = &flow_name {
            self.store.start_run(&pid, &task.id, f, budget, now)?;
            self.store.event(&pid, Some(&task.id), LILAD, "flow", &format!("{f} flow starts"), now)?;
        }
        self.pump(&pid, now);
        let task = self.store.task(&pid, &task.id)?.expect("exists");
        let mut v = task_json(&task);
        if task.state == TaskState::Queued && self.held_for_review(&pid, &task) {
            v["held"] = json!(format!("{} results wait for the person's review; new work starts once they look at them", self.unreviewed(&pid)));
        } else if task.state == TaskState::Queued && self.held_for_folder(&pid, &self.store.tasks(&pid)?).contains(&task.id) {
            let open: Vec<String> = self.open_chains(&pid, &self.store.tasks(&pid)?.into_iter().map(|(t, _)| t).collect::<Vec<_>>()).into_iter().collect();
            v["held"] = json!(format!(
                "the project folder holds chain {} until the person signs it off; this chain starts after that",
                open.join(", ")
            ));
        }
        Ok(v)
    }

    /// A pool id (`engineer` with `count = 2`) becomes one of its members: the one
    /// that last worked in the parent's chain, else the least loaded (`dispatch::pick_slot`).
    fn resolve_to(&self, pid: &str, actor: &Actor, to: &str, parent: Option<&str>) -> Result<String> {
        let proj = &self.projects[pid];
        let Some(slots) = proj.team.pool(to) else { return Ok(to.to_owned()) };
        let slots: Vec<String> = slots.iter().filter(|m| actor.as_str() != m.as_str()).cloned().collect();
        if slots.is_empty() {
            return Ok(to.to_owned());
        }
        let tasks: Vec<Task> = self.store.tasks(pid)?.into_iter().map(|(t, _)| t).collect();
        let affinity = parent.and_then(|p| {
            let root = route::ancestors(&tasks, p).last().map(|t| t.id.clone()).unwrap_or_else(|| p.to_owned());
            let in_chain = |t: &Task| t.id == root || route::ancestors(&tasks, &t.id).iter().any(|a| a.id == root);
            tasks.iter().rev().find(|t| slots.contains(&t.to) && in_chain(t)).map(|t| t.to.clone())
        });
        let load = |m: &str| (tasks.iter().filter(|t| t.to == m && t.state.is_open()).count(), proj.live.get(m).is_some_and(|l| l.status.is_free()));
        Ok(dispatch::pick_slot(&slots, affinity.as_deref(), load).to_owned())
    }

    fn task_move(&mut self, caller: &Value, params: &Value, t: Transition, now: i64) -> Result<Value> {
        let (pid, actor) = self.who(caller)?;
        let id = req(params, "id")?;
        let task = self.store.task(&pid, id)?.ok_or_else(|| anyhow!("no task {id} in project {pid}"))?;
        let owner = self.projects[&pid].team.owner.clone();
        match (&actor, t) {
            (Actor::Human, _) => {}
            (Actor::Member(m), Transition::Cancel) if *m == owner || task.from == actor => {}
            (Actor::Member(_), Transition::Cancel) => bail!("only {} (who assigned it) or the PM can cancel {}", task.from, task.id),
            (Actor::Member(m), _) if *m != task.to => bail!("{} is assigned to {}, not you ({m})", task.id, task.to),
            _ => {}
        }
        let detail = match t {
            Transition::Done => req(params, "summary").context("give a one-line --summary")?.to_owned(),
            Transition::Block => req(params, "reason").context("say what you need with --reason")?.to_owned(),
            _ => s(params, "reason").unwrap_or_default().to_owned(),
        };
        // The current step of a flow run: lilad starts the next one.
        let step = if t == Transition::Done { self.current_step(&pid, &task) } else { None };
        let passed = match (&step, s(params, "verdict")) {
            (Some((_, f, i)), v) if f.steps[*i].wants_verdict() => match v {
                Some("pass") => Some(true),
                Some("fail") => Some(false),
                _ => bail!("{} is a verification: finish with `lila task done {} --verdict pass|fail --summary \"…\"`", task.id, task.id),
            },
            (_, Some(v)) => bail!("{} asks for no verdict; drop --verdict {v}", task.id),
            _ => None,
        };
        let was_active = task.state.is_active();
        let mut task = self.store.transition(&pid, &task.id, t, &actor, &detail, now)?;
        let outcome = Outcome { decisions: strings(params, "decisions"), unverified: strings(params, "unverified"), passed };
        if t == Transition::Done && !outcome.is_empty() {
            self.store.set_outcome(&pid, &task.id, &outcome)?;
            task = self.store.task(&pid, &task.id)?.expect("exists");
        }
        let files = strings(params, "files");
        if !files.is_empty() {
            self.store.add_artifacts(&pid, &task.id, &files)?;
        }
        if t == Transition::Cancel
            && let Some((anchor, _)) = &task.run
            && let Some(run) = self.store.run(&pid, anchor)?
            && run.state.is_open()
        {
            self.store.set_run(&pid, anchor, run.step, run.round, RunState::Cancelled, now)?;
            self.store.event(&pid, Some(anchor), actor.as_str(), "flow_cancelled", &format!("with {}", task.id), now)?;
        }
        match t {
            Transition::Done => match &step {
                Some((run, _, i)) => {
                    let failure = message::fix_from_verdict(&task, &files);
                    self.advance(&pid, &run.anchor, *i, passed != Some(false), &task, &failure, now)?;
                }
                None => self.report_done(&pid, &task, &files, now)?,
            },
            Transition::Block => self.report(&pid, &task, message::blocked_note(&task), now)?,
            Transition::Cancel if was_active && actor != Actor::Member(task.to.clone()) => {
                let text = message::note(&task.id, &format!("{} was cancelled by {actor}. Stop working on it. {detail}", task.id));
                self.store.push_outbox(&pid, &task.to, &text, now)?;
            }
            _ => {}
        }
        self.say(format!("{pid}: {} {} by {actor}", task.id, t.as_str()));
        self.pump(&pid, now);
        Ok(task_json(&self.store.task(&pid, &task.id)?.expect("exists")))
    }

    /// Tells the member whose work waits on a finished task, once nothing it set
    /// off is still open (`lila_core::route`). Each note costs that member a turn.
    fn report_done(&mut self, pid: &str, task: &Task, files: &[String], now: i64) -> Result<()> {
        let owner = self.projects[pid].team.owner.clone();
        let tasks: Vec<Task> = self.store.tasks(pid)?.into_iter().map(|(t, _)| t).collect();
        match route::route_done(&tasks, &task.id, &owner) {
            DoneRoute::Inbox => self.add_review(pid, task, now)?,
            DoneRoute::Hold | DoneRoute::Nobody => {}
            DoneRoute::Tell { member, waiting, path } => {
                let path: Vec<(&Task, Vec<String>)> =
                    path.iter().filter_map(|id| route::find(&tasks, id)).map(|t| (t, self.store.artifacts(pid, &t.id).unwrap_or_default())).collect();
                let waiting = waiting.as_deref().and_then(|id| route::find(&tasks, id));
                let root = route::ancestors(&tasks, &task.id).last().map(|t| t.id.clone()).unwrap_or_else(|| task.id.clone());
                let open = route::open_under(&tasks, waiting.map_or(root.as_str(), |w| w.id.as_str()));
                self.store.push_outbox(pid, &member, &message::done_note(task, files, &path, waiting, &open), now)?;
            }
        }
        Ok(())
    }

    /// Tells whoever assigned a blocked task (and the owner, when a blocker skips it).
    fn report(&mut self, pid: &str, task: &Task, text: String, now: i64) -> Result<()> {
        let owner = self.projects[pid].team.owner.clone();
        match &self.assigner(pid, task) {
            Actor::Human => {
                self.store.add_inbox(pid, "report", Some(&task.to), Some(&task.id), &inbox::blocked(task), &[], now)?;
            }
            Actor::Member(m) => {
                self.store.push_outbox(pid, m, &text, now)?;
                // The owner coordinates: it hears about work members hand each other,
                // or it waits on a chain nobody reports back up (seen in the M1 run).
                if *m != owner && task.to != owner {
                    self.store.push_outbox(pid, &owner, &text, now)?;
                }
            }
        }
        Ok(())
    }

    /// Who hears about a task: whoever assigned it, or for a step lilad created,
    /// whoever started its flow run.
    fn assigner(&self, pid: &str, task: &Task) -> Actor {
        if task.from.as_str() == LILAD
            && let Some((anchor, _)) = &task.run
            && let Ok(Some(a)) = self.store.task(pid, anchor)
        {
            return a.from;
        }
        task.from.clone()
    }

    fn check_cmd(&self, pid: &str) -> Option<String> {
        let c = self.projects[pid].team.settings.check.trim();
        (!c.is_empty()).then(|| c.to_owned())
    }

    /// The open run `task` is the step under way of: the run, its flow and the step.
    fn current_step(&self, pid: &str, task: &Task) -> Option<(Run, Flow, usize)> {
        let (anchor, step) = task.run.clone()?;
        let run = self.store.run(pid, &anchor).ok()??;
        if run.state != RunState::Running || run.step != step {
            return None;
        }
        let f = self.projects[pid].team.flows.get(&run.flow)?.clone();
        (step < f.steps.len()).then_some((run, f, step))
    }

    /// What a member doing a step is told about the rest of its flow.
    fn step_hint(&self, pid: &str, task: &Task) -> Option<StepHint> {
        let (anchor, step) = task.run.as_ref()?;
        let run = self.store.run(pid, anchor).ok()??;
        let f = self.projects[pid].team.flows.get(&run.flow)?;
        let st = f.steps.get(*step)?;
        Some(if st.wants_verdict() { StepHint::Verdict } else { StepHint::HandOn(flow::later(f, *step, self.check_cmd(pid).as_deref())) })
    }

    /// Step `at` of `anchor`'s run ended (`passed`: its verdict or check). Starts what
    /// comes next (`flow::after`). `last`: the newest step task, which the next hangs
    /// under. `failure`: what goes back to the member on a fail.
    #[allow(clippy::too_many_arguments)]
    fn advance(&mut self, pid: &str, anchor: &str, at: usize, passed: bool, last: &Task, failure: &str, now: i64) -> Result<()> {
        let Some(run) = self.store.run(pid, anchor)? else { return Ok(()) };
        let anchor_task = self.store.task(pid, anchor)?.context("a flow run without its task")?;
        let (f, max) = {
            let team = &self.projects[pid].team;
            (team.flows.get(&run.flow).filter(|f| at < f.steps.len()).cloned(), team.settings.max_rounds)
        };
        let Some(f) = f else {
            self.store.set_run(pid, anchor, at, run.round, RunState::Done, now)?;
            self.store.event(pid, Some(anchor), LILAD, "flow_done", &format!("the {} flow is gone from the project: stopped here", run.flow), now)?;
            let files = self.store.artifacts(pid, &last.id)?;
            return self.report_done(pid, last, &files, now);
        };
        let check = self.check_cmd(pid);
        let next = flow::after(&f, at, passed, run.round, max, check.is_some());
        if !matches!(next, Next::Finished | Next::GiveUp)
            && let Some(budget) = run.budget
        {
            let spent = self.run_tokens(pid, &run, now);
            if spent >= budget {
                return self.hold_run(pid, &run, &anchor_task, (at, passed, &last.id, failure), spent, budget, now);
            }
        }
        match next {
            Next::Finished => {
                self.store.set_run(pid, anchor, at, run.round, RunState::Done, now)?;
                self.store.event(pid, Some(anchor), LILAD, "flow_done", &format!("{} flow passed", run.flow), now)?;
                self.finish_run(pid, &anchor_task, &run.flow, last, now)?;
            }
            Next::GiveUp => {
                self.store.set_run(pid, anchor, at, run.round, RunState::Failed, now)?;
                self.store.event(pid, Some(anchor), LILAD, "flow_failed", &format!("still failing after {} rounds", run.round), now)?;
                let text = message::gave_up(&anchor_task, &run.flow, run.round, failure);
                let person = inbox::gave_up(&anchor_task, run.round, failure);
                self.tell_lead(pid, &anchor_task, &text, &person, now)?;
            }
            Next::Check { step } => {
                self.store.set_run(pid, anchor, step, run.round, RunState::Checking, now)?;
                self.store.event(pid, Some(anchor), LILAD, "check", &format!("running `{}`", check.as_deref().unwrap_or("")), now)?;
                self.queue_check(pid, anchor);
            }
            Next::Member { step } => {
                let st = f.steps[step].clone();
                let to = self.resolve_to(pid, &Actor::Member(LILAD.into()), st.by.as_deref().unwrap_or_default(), Some(&last.id))?;
                let files = self.store.artifacts(pid, &last.id)?;
                let ran = if f.steps[at].is_check() { check.as_deref() } else { None };
                let title = if st.wants_verdict() { format!("Verify: {}", anchor_task.title) } else { format!("{}: {}", capitalized(&st.action), anchor_task.title) };
                let body = message::step_body(last, &files, ran, st.wants_verdict());
                self.store.set_run(pid, anchor, step, run.round, RunState::Running, now)?;
                self.create_step(pid, anchor, step, &to, &last.id, title, body, now)?;
            }
            Next::Back { step, round } => {
                // To whoever did that step last: its session may still hold the work.
                let to = self
                    .store
                    .tasks(pid)?
                    .into_iter()
                    .rev()
                    .map(|(t, _)| t)
                    .find(|t| t.run.as_ref().is_some_and(|(a, s)| a == anchor && *s == step))
                    .map(|t| t.to)
                    .context("nobody did the step to send the work back to")?;
                self.store.set_run(pid, anchor, step, round, RunState::Running, now)?;
                self.store.event(pid, Some(anchor), LILAD, "sent_back", &format!("round {round} of {max}, to {to}"), now)?;
                self.create_step(pid, anchor, step, &to, &last.id, format!("Fix: {}", anchor_task.title), failure.to_owned(), now)?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn create_step(&mut self, pid: &str, anchor: &str, step: usize, to: &str, parent: &str, title: String, body: String, now: i64) -> Result<Task> {
        let new = NewTask {
            title,
            body,
            from: Actor::Member(LILAD.into()),
            to: to.to_owned(),
            parent: Some(parent.to_owned()),
            priority: 0,
            brief: Brief::default(),
            run: Some((anchor.to_owned(), step)),
        };
        self.create_task(pid, new, now)
    }

    /// Every step passed: whoever started the run hears once, like for any finished chain.
    fn finish_run(&mut self, pid: &str, anchor: &Task, flow_name: &str, last: &Task, now: i64) -> Result<()> {
        if anchor.from == Actor::Human {
            let _ = flow_name;
            return self.add_review(pid, anchor, now);
        }
        let files = self.store.artifacts(pid, &last.id)?;
        self.report_done(pid, last, &files, now)
    }

    /// News about a run for whoever started it (`person`: what the person reads), and the PM.
    fn tell_lead(&mut self, pid: &str, anchor: &Task, text: &str, person: &str, now: i64) -> Result<()> {
        let owner = self.projects[pid].team.owner.clone();
        if anchor.from == Actor::Human {
            self.store.add_inbox(pid, "report", Some(&anchor.to), Some(&anchor.id), person, &[], now)?;
        } else if anchor.from.as_str() != owner {
            self.store.push_outbox(pid, anchor.from.as_str(), text, now)?;
        }
        self.store.push_outbox(pid, &owner, text, now)?;
        Ok(())
    }

    fn queue_check(&mut self, pid: &str, anchor: &str) {
        let Some(cmd) = self.check_cmd(pid) else { return };
        if !self.checking.insert((pid.to_owned(), anchor.to_owned())) {
            return;
        }
        let p = &self.projects[pid];
        self.checks.push(CheckJob {
            project: pid.to_owned(),
            anchor: anchor.to_owned(),
            cwd: p.cwd.clone(),
            cmd,
            timeout: std::time::Duration::from_secs(p.team.settings.check_timeout_secs.max(1)),
            path_env: self.path_env.clone(),
        });
    }

    /// Checks to start now; each result comes back through `on_check_done`.
    pub fn take_checks(&mut self) -> Vec<CheckJob> {
        std::mem::take(&mut self.checks)
    }

    /// A check finished: on to the next step, or the work goes back.
    pub fn on_check_done(&mut self, pid: &str, anchor: &str, r: &CheckResult, now: i64) {
        self.checking.remove(&(pid.to_owned(), anchor.to_owned()));
        if !self.projects.contains_key(pid) {
            return;
        }
        let Ok(Some(run)) = self.store.run(pid, anchor) else { return };
        if run.state != RunState::Checking {
            return;
        }
        let detail = match r.exit {
            Some(0) => "passed".to_owned(),
            Some(c) => format!("failed (exit {c})"),
            None => "did not finish".to_owned(),
        };
        let _ = self.store.event(pid, Some(anchor), LILAD, "check", &detail, now);
        self.say(format!("{pid}: check for {anchor} {detail}"));
        let tasks = self.store.tasks(pid).unwrap_or_default();
        let Some(last) = tasks.into_iter().rev().map(|(t, _)| t).find(|t| t.run.as_ref().is_some_and(|(a, _)| a == anchor)) else { return };
        let failure = message::fix_from_check(&self.check_cmd(pid).unwrap_or_default(), r.exit, &r.tail);
        if let Err(e) = self.advance(pid, anchor, run.step, r.passed(), &last, &failure, now) {
            self.say(format!("{pid}: after the check for {anchor}: {e:#}"));
        }
        self.pump(pid, now);
    }

    /// Changes a task you assigned. Queued: rewritten in place. Already delivered:
    /// what was typed can't change, so `append` goes to the member as an update.
    fn task_edit(&mut self, caller: &Value, params: &Value, now: i64) -> Result<Value> {
        let (pid, actor) = self.who(caller)?;
        let id = req(params, "id")?;
        let task = self.store.task(&pid, id)?.ok_or_else(|| anyhow!("no task {id} in project {pid}"))?;
        let owner = self.projects[&pid].team.owner.clone();
        if actor != Actor::Human && actor != Actor::Member(owner.clone()) && actor != task.from {
            bail!("only {} (who assigned it) or the PM can edit {}", task.from, task.id);
        }
        if !task.state.is_open() {
            bail!("{} is {}; create a new task instead", task.id, task.state);
        }
        let (title, body, append) = (s(params, "title"), s(params, "body"), s(params, "append"));
        if title.is_none() && body.is_none() && append.is_none() {
            bail!("nothing to change: give --title, --body or --append");
        }
        let delivered = task.state != TaskState::Queued;
        if delivered && (title.is_some() || body.is_some()) {
            bail!("{} was already delivered to {}; its text can't be replaced. Use --append to send them an update", task.id, task.to);
        }
        let mut new_body = body.map(String::from).unwrap_or_else(|| task.body.clone());
        if let Some(a) = append {
            new_body = format!("{}\n\n## Update from {actor}\n\n{}", new_body.trim_end(), a.trim());
        }
        let new_title = title.map(|t| t.trim().to_owned()).unwrap_or_else(|| task.title.clone());
        self.store.set_task_text(&pid, &task.id, &new_title, &new_body, now)?;
        let updated = self.store.task(&pid, &task.id)?.expect("exists");
        write_task_file(&self.projects[&pid].cwd, &updated)?;
        let detail = append.map(|a| format!("appended: {a}")).unwrap_or_else(|| "rewrote the task".into());
        self.store.event(&pid, Some(&task.id), actor.as_str(), "edited", &detail, now)?;
        if delivered && let Some(a) = append {
            let text = message::note(&task.id, &format!("Update to {} \"{}\" from {actor}: {}", task.id, new_title, a.trim()));
            self.store.push_outbox(&pid, &task.to, &text, now)?;
        }
        self.pump(&pid, now);
        let mut v = task_json(&updated);
        v["update_sent"] = json!(delivered);
        Ok(v)
    }

    fn task_show(&self, caller: &Value, params: &Value) -> Result<Value> {
        let (pid, _) = self.who(caller)?;
        let id = req(params, "id")?;
        let task = self.store.task(&pid, id)?.ok_or_else(|| anyhow!("no task {id}"))?;
        let mut v = task_json(&task);
        v["file"] = json!(task_file_rel(&task.id));
        v["artifacts"] = json!(self.store.artifacts(&pid, &task.id)?);
        v["children"] = json!(self.store.tasks(&pid)?.iter().filter(|(t, _)| t.parent.as_deref() == Some(&task.id)).map(|(t, _)| task_json(t)).collect::<Vec<_>>());
        v["events"] = json!(
            self.store.events(&pid, Some(&task.id), 100)?.iter().map(|e| json!({"at_ms": e.at_ms, "actor": e.actor, "kind": e.kind, "detail": e.detail})).collect::<Vec<_>>()
        );
        // The brief its chain works to (Lila carries it down), and where the person's words come from.
        let mut chain = vec![task.clone()];
        chain.extend(self.store.ancestors(&pid, &task.id)?);
        if let Some((_, _, b)) = chain_brief(&chain) {
            let from = chain.iter().rev().find(|t| t.chain_brief().quote.is_some()).map(|t| t.id.clone());
            v["brief"] = json!({"quote": b.quote, "quote_from": from, "accept": b.accept, "out_of_scope": b.out_of_scope});
        }
        // The flow run it is a step of: steps, where it is, and its last check.
        if let Some((anchor, _)) = &task.run
            && let Some(run) = self.store.run(&pid, anchor)?
        {
            let team = &self.projects[&pid].team;
            let steps: Vec<String> = team.flows.get(&run.flow).map(|f| f.steps.iter().map(|s| s.action.clone()).collect()).unwrap_or_default();
            let check = self.store.events(&pid, Some(anchor), 100)?.into_iter().filter(|e| e.kind == "check" && !e.detail.starts_with("running")).max_by_key(|e| e.at_ms);
            v["flow"] = json!({
                "anchor": anchor, "flow": run.flow, "steps": steps, "step": run.step, "round": run.round, "max_rounds": team.settings.max_rounds,
                "state": run.state.as_str(), "check": self.check_cmd(&pid), "last_check": check.map(|e| json!({"at_ms": e.at_ms, "detail": e.detail})),
            });
        }
        Ok(v)
    }

    fn task_ls(&self, caller: &Value, params: &Value) -> Result<Value> {
        let (pid, actor) = self.who(caller)?;
        let mine = params.get("mine").and_then(Value::as_bool).unwrap_or(false);
        let all = params.get("all").and_then(Value::as_bool).unwrap_or(false);
        let state = s(params, "state").map(|st| TaskState::parse(st).ok_or_else(|| anyhow!("unknown state {st}"))).transpose()?;
        let tasks: Vec<Value> = self
            .store
            .tasks(&pid)?
            .into_iter()
            .map(|(t, _)| t)
            .filter(|t| all || state.is_some() || t.state.is_open())
            .filter(|t| state.is_none_or(|st| t.state == st))
            .filter(|t| !mine || Actor::Member(t.to.clone()) == actor || t.from == actor)
            .map(|t| task_json(&t))
            .collect();
        Ok(json!(tasks))
    }

    fn ask(&mut self, caller: &Value, params: &Value, now: i64) -> Result<Value> {
        let (pid, actor) = self.who(caller)?;
        let owner = self.projects[&pid].team.owner.clone();
        let Actor::Member(m) = &actor else { bail!("`lila ask` is how the PM asks you; you are the person") };
        if *m != owner {
            bail!("only the PM ({owner}) asks the person; report to your assigner with `lila task block` instead");
        }
        let item = self.store.add_inbox(&pid, "question", Some(m), s(params, "task"), req(params, "question")?, &strings(params, "options"), now)?;
        self.say(format!("{pid}: {} asked {}: {}", m, item.id, item.text));
        Ok(json!({"id": item.id, "note": "The person will answer in their inbox; the answer arrives as <lila answer>. Keep working on anything that does not depend on it."}))
    }

    fn inbox(&self, caller: &Value, params: &Value) -> Result<Value> {
        let (pid, _) = self.who(caller)?;
        let all = params.get("all").and_then(Value::as_bool).unwrap_or(false);
        Ok(json!(self.store.inbox(&pid, all)?.iter().map(inbox_json).collect::<Vec<_>>()))
    }

    fn answer(&mut self, caller: &Value, params: &Value, now: i64) -> Result<Value> {
        let (pid, actor) = self.who(caller)?;
        if actor != Actor::Human {
            bail!("only the person answers inbox items");
        }
        let id = req(params, "id")?;
        let item = self.store.inbox_item(&pid, id)?.ok_or_else(|| anyhow!("no inbox item {id}"))?;
        // A second click (or a second window) on something already handled: nothing to do.
        if item.resolved_ms.is_some() {
            return Ok(json!({"id": item.id, "resolved": true, "already": true}));
        }
        let text = s(params, "text");
        match item.kind.as_str() {
            "question" => {
                let text = text.ok_or_else(|| anyhow!("{} is a question; give an answer", item.id))?;
                let to = item.member.clone().unwrap_or_else(|| self.projects[&pid].team.owner.clone());
                self.store.push_outbox(&pid, &to, &message::answer(&item.id, &item.text, text), now)?;
            }
            "review" => self.review_answer(&pid, &item, text, now)?,
            "budget" => self.budget_answer(&pid, &item, text, now)?,
            _ => {}
        }
        self.store.resolve_inbox(&pid, &item.id, text, now)?;
        self.store.event(&pid, item.task.as_deref(), "human", "answered", &format!("{}: {}", item.id, text.unwrap_or("dismissed")), now)?;
        self.pump(&pid, now);
        Ok(json!({"id": item.id, "resolved": true}))
    }

    fn events(&self, caller: &Value, params: &Value) -> Result<Value> {
        let (pid, _) = self.who(caller)?;
        let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(30) as usize;
        Ok(json!(
            self.store
                .events(&pid, s(params, "task"), limit)?
                .iter()
                .map(|e| json!({"at_ms": e.at_ms, "task": e.task, "actor": e.actor, "kind": e.kind, "detail": e.detail}))
                .collect::<Vec<_>>()
        ))
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

    /// How `mid` (its session on another chain) takes work from `chain`: back to
    /// the session that did this chain before, the same session while its
    /// context is small (`fresh_after_tokens`), else a new one.
    fn switch_plan(&mut self, pid: &str, mid: &str, live: &Live, chain: &str) -> Switch {
        if let Ok(Some(s)) = self.store.session_for_chain(pid, mid, chain)
            && live.session.as_ref() != Some(&s.id)
            && s.file.as_deref().is_some_and(|f| Path::new(f).exists())
        {
            return Switch::Resume(s.id);
        }
        let limit = self.projects[pid].team.settings.fresh_after_tokens;
        match self.context_of(pid, live.session.as_deref()) {
            Some(n) if limit > 0 && n < limit => Switch::Keep,
            _ => Switch::Fresh,
        }
    }

    /// The context of a member's session's latest turn, in tokens, when its record is readable.
    fn context_of(&mut self, pid: &str, session: Option<&str>) -> Option<u64> {
        let file = self.store.session(pid, session?).ok()??.file?;
        self.contexts.of(Path::new(&file))
    }

    /// Starts a free member's CLI again for `tid` (from another chain): in the
    /// session that did that chain before, or an empty one. It holds no open task,
    /// so nothing goes back to the queue; `tid` goes out once the session reports idle.
    fn new_session(&mut self, pid: &str, mid: &str, tid: &str, plan: Switch, now: i64) {
        let Some(old) = self.projects[pid].live[mid].terminal.clone() else { return };
        if let Err(e) = self.term.kill(&old) {
            self.say(format!("{pid}: kill {mid} ({old}): {e:#}"));
        }
        self.member_gone(pid, mid, &format!("new session for {tid}"), now, false);
        let (session, kind, what) = match plan {
            Switch::Resume(id) => (runtime::Session::Resume(id.clone()), "resumed_session", format!("{mid} goes back to session {id} for {tid}")),
            _ => (runtime::Session::New(None), "fresh_session", format!("{mid} starts a new session for {tid}")),
        };
        let _ = self.store.event(pid, Some(tid), "lilad", kind, &what, now);
        if let Err(e) = self.start_session(pid, mid, session, now) {
            self.say(format!("{pid}: {mid} could not be started again for {tid}: {e:#}"));
            let _ = self.store.add_inbox(pid, "lost", Some(mid), None, &inbox::restart_failed(mid, &format!("{e:#}")), &[], now);
        }
    }

    /// The PM in a new session: its context grew past `pm_refresh_tokens`. The
    /// digest is what it needs to carry on; the details stay a `lila task show` away.
    fn refresh_pm(&mut self, pid: &str, tokens: u64, now: i64) {
        let owner = self.projects[pid].team.owner.clone();
        let Some(old) = self.projects[pid].live[&owner].terminal.clone() else { return };
        let digest = match self.digest(pid) {
            Ok(d) => d,
            Err(e) => return self.say(format!("{pid}: no digest for the PM: {e:#}")),
        };
        if let Err(e) = self.term.kill(&old) {
            self.say(format!("{pid}: kill {owner} ({old}): {e:#}"));
        }
        self.member_gone(pid, &owner, "new session", now, false);
        let detail = format!("its context reached {} tokens; it starts again with a digest of the project", crate::usage::short(tokens));
        let _ = self.store.event(pid, None, "lilad", "pm_refreshed", &detail, now);
        self.say(format!("{pid}: {owner}: {detail}"));
        if let Err(e) = self.start_member(pid, &owner, now) {
            let _ = self.store.add_inbox(pid, "lost", Some(&owner), None, &inbox::restart_failed(&owner, &format!("{e:#}")), &[], now);
            return;
        }
        let _ = self.store.push_outbox(pid, &owner, &digest, now);
    }

    /// What the PM needs in a new session, from the store: no model summarizes it,
    /// so nothing the person decided is lost on the way.
    fn digest(&self, pid: &str) -> Result<String> {
        let tasks: Vec<Task> = self.store.tasks(pid)?.into_iter().map(|(t, _)| t).collect();
        let runs: Vec<Run> = self.store.runs(pid)?.into_iter().filter(|r| r.state.is_open()).collect();
        let team = &self.projects[pid].team;
        let decided: Vec<String> = self
            .store
            .inbox(pid, true)?
            .into_iter()
            .filter(|i| i.kind == "question" && i.answer.is_some())
            .map(|i| format!("{} {} → {}", i.id, i.text.trim(), i.answer.unwrap_or_default().trim()))
            .collect();
        let under = |root: &str, id: &str| id == root || route::ancestors(&tasks, id).iter().any(|a| a.id == root);
        let mut open = Vec::new();
        let mut closed = Vec::new();
        for r in tasks.iter().filter(|t| t.parent.is_none()) {
            let below = route::open_under(&tasks, &r.id);
            let flows: Vec<&Run> = runs.iter().filter(|x| under(&r.id, &x.anchor)).collect();
            if r.state.is_open() || !below.is_empty() || !flows.is_empty() {
                let mut line = format!("{} from {} → {} [{}] \"{}\"", r.id, r.from, r.to, r.state, r.title);
                for t in &below {
                    line.push_str(&format!("\n    {} → {} [{}] {}", t.id, t.to, t.state, t.title));
                }
                for x in flows {
                    let step = team.flows.get(&x.flow).and_then(|f| f.steps.get(x.step)).map(|s| s.action.as_str()).unwrap_or("?");
                    line.push_str(&format!("\n    {} flow of {}: {step} ({}, fix round {})", x.flow, x.anchor, x.state.as_str(), x.round));
                }
                open.push(line);
            } else if r.state == TaskState::Done {
                closed.push(format!("{} \"{}\": {}", r.id, r.title, r.summary.as_deref().unwrap_or("").chars().take(160).collect::<String>()));
            }
        }
        let waiting = self.store.inbox(pid, false)?.len();
        Ok(message::digest(&decided, &open, &closed[closed.len().saturating_sub(8)..], waiting))
    }

    // ---------------- the person's review ----------------

    /// Results waiting for the person to sign off.
    fn unreviewed(&self, pid: &str) -> usize {
        self.store.inbox(pid, false).map(|v| v.iter().filter(|i| i.kind == "review").count()).unwrap_or(0)
    }

    /// New work the PM starts waits while `max_unreviewed` results wait for the
    /// person: more output than anyone checks is hoping, not reviewing. The
    /// person's own requests and work within a chain go on.
    fn held_for_review(&self, pid: &str, task: &Task) -> bool {
        let max = self.projects[pid].team.settings.max_unreviewed as usize;
        max > 0 && task.parent.is_none() && task.from != Actor::Human && self.unreviewed(pid) >= max
    }

    // ---------------- one chain in the folder at a time ----------------

    /// Chains with work in the project folder: a member other than the PM has had
    /// work in them, and they are not signed off yet (work or a flow is open, a
    /// review card waits, or their commit is being made). By root task.
    fn open_chains(&self, pid: &str, tasks: &[Task]) -> BTreeSet<String> {
        let owner = &self.projects[pid].team.owner;
        let root = |id: &str| route::ancestors(tasks, id).last().map(|t| t.id.clone()).unwrap_or_else(|| id.to_owned());
        let started: BTreeSet<String> = tasks.iter().filter(|t| t.to != *owner && t.state != TaskState::Queued).map(|t| root(&t.id)).collect();
        let mut busy: BTreeSet<String> = tasks.iter().filter(|t| !matches!(t.state, TaskState::Done | TaskState::Cancelled)).map(|t| root(&t.id)).collect();
        busy.extend(self.store.runs(pid).unwrap_or_default().into_iter().filter(|r| r.state.is_open()).map(|r| root(&r.anchor)));
        busy.extend(self.store.inbox(pid, false).unwrap_or_default().into_iter().filter(|i| i.kind == "review").filter_map(|i| i.task).map(|t| root(&t)));
        busy.extend(self.committing.iter().filter(|(p, _)| p == pid).map(|(_, c)| c.clone()));
        started.intersection(&busy).cloned().collect()
    }

    /// Queued tasks that wait for the folder: members share one, so at most
    /// `max_chains` chains have work in it. Chains already in it go on; new ones
    /// come in the order their work was queued. The PM's own tasks never wait.
    fn held_for_folder(&self, pid: &str, tasks: &[(Task, lila_store::TaskMeta)]) -> BTreeSet<String> {
        let team = &self.projects[pid].team;
        let max = team.settings.max_chains as usize;
        if max == 0 {
            return BTreeSet::new();
        }
        let plain: Vec<Task> = tasks.iter().map(|(t, _)| t.clone()).collect();
        let root = |id: &str| route::ancestors(&plain, id).last().map(|t| t.id.clone()).unwrap_or_else(|| id.to_owned());
        let mut admitted = self.open_chains(pid, &plain);
        let mut held = BTreeSet::new();
        let mut queued: Vec<&(Task, lila_store::TaskMeta)> = tasks.iter().filter(|(t, _)| t.state == TaskState::Queued && t.to != team.owner).collect();
        queued.sort_by_key(|(_, m)| m.seq);
        for (t, _) in queued {
            let chain = root(&t.id);
            if admitted.contains(&chain) {
                continue;
            }
            if admitted.len() < max && !self.held_for_review(pid, t) {
                admitted.insert(chain);
            } else {
                held.insert(t.id.clone());
            }
        }
        held
    }

    /// The person accepted `id`: its chain's changes become one commit (`commit_on_accept`).
    fn queue_commit(&mut self, pid: &str, id: &str) {
        let p = &self.projects[pid];
        if !p.team.settings.commit_on_accept {
            return;
        }
        let tasks: Vec<Task> = self.store.tasks(pid).unwrap_or_default().into_iter().map(|(t, _)| t).collect();
        let Some(task) = tasks.iter().find(|t| t.id == id) else { return };
        // The chain's title and the person's words; what came of it from the accepted task (a rework's).
        let mut what = route::ancestors(&tasks, id).last().copied().unwrap_or(task).clone();
        what.brief = what.chain_brief();
        what.summary = task.summary.clone();
        what.outcome = task.outcome.clone();
        let chain = what.id.clone();
        if !self.committing.insert((pid.to_owned(), chain.clone())) {
            return;
        }
        self.commits.push(CommitJob { project: pid.to_owned(), chain, cwd: p.cwd.clone(), message: message::commit_message(&what), path_env: self.path_env.clone() });
    }

    /// Commits to make now; each result comes back through `on_commit_done`.
    pub fn take_commits(&mut self) -> Vec<CommitJob> {
        std::mem::take(&mut self.commits)
    }

    /// A chain's commit is made (or not): the folder is free for the next chain.
    pub fn on_commit_done(&mut self, pid: &str, chain: &str, r: &CommitResult, now: i64) {
        self.committing.remove(&(pid.to_owned(), chain.to_owned()));
        if !self.projects.contains_key(pid) {
            return;
        }
        let (kind, detail) = match r {
            CommitResult::Committed(sha) => ("committed", format!("{chain}'s changes are commit {sha}")),
            CommitResult::Nothing => ("committed", format!("{chain} changed nothing to commit")),
            CommitResult::NotRepo => ("committed", "the project folder is not a git repository; nothing committed".to_owned()),
            CommitResult::Failed(why) => ("commit_failed", format!("{chain}'s changes could not be committed: {why}")),
        };
        let _ = self.store.event(pid, Some(chain), LILAD, kind, &detail, now);
        if let CommitResult::Failed(why) = r {
            let _ = self.store.add_inbox(pid, "report", None, Some(chain), &inbox::commit_failed(chain, why), &[], now);
        }
        self.say(format!("{pid}: {detail}"));
        self.pump(pid, now);
    }

    /// The person's work is finished: a card for them to sign off, with what each
    /// step reported and every check Lila ran under it.
    fn add_review(&mut self, pid: &str, task: &Task, now: i64) -> Result<()> {
        let tasks: Vec<Task> = self.store.tasks(pid)?.into_iter().map(|(t, _)| t).collect();
        let under = |id: &str| id == task.id || route::ancestors(&tasks, id).iter().any(|a| a.id == task.id);
        let mut lines: Vec<(i64, String)> = Vec::new();
        for t in tasks.iter().filter(|t| t.id != task.id && t.state == TaskState::Done && under(&t.id)) {
            let files = self.store.artifacts(pid, &t.id)?;
            lines.push((t.updated_ms, inbox::step_done(t, &files)));
        }
        let check = self.check_cmd(pid).unwrap_or_default();
        for e in self.store.events(pid, None, 1000)? {
            if e.kind == "check" && !e.detail.starts_with("running") && e.task.as_deref().is_some_and(under) {
                lines.push((e.at_ms, inbox::step_check(&check, &e.task.unwrap_or_default(), &e.detail)));
            }
        }
        lines.sort_by_key(|(at, _)| *at);
        let steps: Vec<String> = lines.into_iter().map(|(_, l)| l).collect();
        let text = inbox::review(task, &steps);
        self.store.add_inbox(pid, "review", Some(&task.to), Some(&task.id), &text, &["通过".to_owned()], now)?;
        Ok(())
    }

    /// Accept (no text, or 通过/accept/ok), or anything else: what to do again,
    /// which goes to the PM as a new task in the same chain.
    fn review_answer(&mut self, pid: &str, item: &lila_store::InboxItem, text: Option<&str>, now: i64) -> Result<()> {
        let id = item.task.clone().context("a review without its task")?;
        let accepted = text.is_none_or(|t| matches!(t.trim().to_lowercase().as_str(), "通过" | "accept" | "accepted" | "ok" | "pass" | "yes"));
        if accepted {
            self.store.event(pid, Some(&id), "human", "accepted", &format!("{id} signed off"), now)?;
            self.queue_commit(pid, &id);
            return Ok(());
        }
        let task = self.store.task(pid, &id)?.context("the reviewed task is gone")?;
        let owner = self.projects[pid].team.owner.clone();
        let comment = text.unwrap_or_default().trim().to_owned();
        let new = NewTask {
            title: format!("Rework: {}", task.title),
            body: format!("The person did not accept {id} \"{}\":\n\n{comment}", task.title),
            from: Actor::Human,
            to: owner,
            parent: Some(id.clone()),
            priority: 0,
            brief: Brief::default(),
            run: None,
        };
        let rework = self.create_task(pid, new, now)?;
        self.store.event(pid, Some(&id), "human", "rejected", &format!("{comment} → {}", rework.id), now)?;
        Ok(())
    }

    // ---------------- budgets ----------------

    /// Tokens a run's step tasks used: each member's use while it held its task.
    fn run_tokens(&mut self, pid: &str, run: &Run, now: i64) -> u64 {
        let cwd = self.projects[pid].cwd.to_string_lossy().to_string();
        let tasks: Vec<Task> = self.store.tasks(pid).unwrap_or_default().into_iter().map(|(t, _)| t).filter(|t| t.run.as_ref().is_some_and(|(a, _)| *a == run.anchor)).collect();
        let mut spent = 0;
        let src = self.live_sources();
        for t in tasks {
            let events = self.store.events(pid, Some(&t.id), 200).unwrap_or_default();
            let Some(from) = events.iter().filter(|e| e.kind == "delivered").map(|e| e.at_ms).min() else { continue };
            let to = events.iter().filter(|e| matches!(e.kind.as_str(), "done" | "cancelled")).map(|e| e.at_ms).max().unwrap_or(now);
            spent += self.scanner.tokens_between(&src, pid, &cwd, &t.to, from, to);
        }
        spent
    }

    /// Over budget before its next step: the run holds what it would do next until the person decides.
    #[allow(clippy::too_many_arguments)]
    fn hold_run(&mut self, pid: &str, run: &Run, anchor: &Task, next: (usize, bool, &str, &str), spent: u64, budget: u64, now: i64) -> Result<()> {
        let (at, passed, last, failure) = next;
        let hold = json!({"at": at, "passed": passed, "last": last, "failure": failure}).to_string();
        self.store.set_run_hold(pid, &run.anchor, run.budget, Some(&hold))?;
        self.store.set_run(pid, &run.anchor, at, run.round, RunState::Held, now)?;
        let text = message::over_budget(anchor, &run.flow, spent, budget);
        self.store.event(pid, Some(&run.anchor), LILAD, "flow_held", &text, now)?;
        let person = inbox::over_budget(anchor, &message::short_tokens(spent), &message::short_tokens(budget));
        self.store.add_inbox(pid, "budget", Some(&anchor.to), Some(&run.anchor), &person, &["继续".to_owned(), "取消".to_owned()], now)?;
        let owner = self.projects[pid].team.owner.clone();
        self.store.push_outbox(pid, &owner, &message::note(&run.anchor, &format!("{text} You hear when they do.")), now)?;
        Ok(())
    }

    /// Go on (with twice the budget, or more than it spent) or cancel the run.
    fn budget_answer(&mut self, pid: &str, item: &lila_store::InboxItem, text: Option<&str>, now: i64) -> Result<()> {
        let anchor = item.task.clone().context("a budget item without its run")?;
        let Some(run) = self.store.run(pid, &anchor)? else { return Ok(()) };
        if run.state != RunState::Held {
            return Ok(());
        }
        let owner = self.projects[pid].team.owner.clone();
        if text.is_some_and(|t| matches!(t.trim().to_lowercase().as_str(), "取消" | "cancel" | "stop")) {
            self.store.set_run_hold(pid, &anchor, run.budget, None)?;
            self.store.set_run(pid, &anchor, run.step, run.round, RunState::Cancelled, now)?;
            self.store.event(pid, Some(&anchor), "human", "flow_cancelled", "over budget", now)?;
            self.store.push_outbox(pid, &owner, &message::note(&anchor, &format!("The person stopped the {} flow of {anchor} over its budget.", run.flow)), now)?;
            return Ok(());
        }
        let spent = self.run_tokens(pid, &run, now);
        let old = run.budget.unwrap_or(0);
        let budget = (old * 2).max(spent + old.max(1));
        let hold: Value = serde_json::from_str(run.hold.as_deref().unwrap_or("{}")).unwrap_or_default();
        self.store.set_run_hold(pid, &anchor, Some(budget), None)?;
        self.store.set_run(pid, &anchor, run.step, run.round, RunState::Running, now)?;
        self.store.event(pid, Some(&anchor), "human", "budget", &format!("raised to {}", message::short_tokens(budget)), now)?;
        let last = hold["last"].as_str().and_then(|id| self.store.task(pid, id).ok().flatten()).context("the held run lost its last step")?;
        let at = hold["at"].as_u64().unwrap_or(run.step as u64) as usize;
        self.advance(pid, &anchor, at, hold["passed"].as_bool().unwrap_or(true), &last, hold["failure"].as_str().unwrap_or(""), now)
    }

    // ---------------- the watchdog ----------------

    /// A member working `runaway_minutes` without a `lila` call is reminded once;
    /// if it still says nothing, the person hears. Busy is not progress.
    fn watch_runaways(&mut self, pid: &str, now: i64) {
        let limit = self.projects[pid].team.settings.runaway_minutes as i64 * 60_000;
        if limit == 0 {
            return;
        }
        let working: Vec<(String, i64)> =
            self.projects[pid].live.iter().filter(|(_, l)| l.status == AgentStatus::Working && l.terminal.is_some()).map(|(m, l)| (m.clone(), l.busy_since)).collect();
        if working.is_empty() {
            return;
        }
        let tasks = self.store.tasks(pid).unwrap_or_default();
        let events = self.store.events(pid, None, 500).unwrap_or_default();
        for (mid, busy_since) in working {
            let Some(t) = tasks.iter().map(|(t, _)| t).find(|t| t.to == mid && matches!(t.state, TaskState::Delivered | TaskState::InProgress)) else { continue };
            let last_call = events.iter().filter(|e| e.actor == mid).map(|e| e.at_ms).max().unwrap_or(0);
            let delivered = events.iter().filter(|e| e.kind == "delivered" && e.task.as_deref() == Some(&t.id)).map(|e| e.at_ms).max().unwrap_or(0);
            let since = busy_since.max(last_call).max(delivered);
            let nudged = events.iter().filter(|e| e.kind == "runaway" && e.task.as_deref() == Some(&t.id) && e.at_ms >= since).map(|e| e.at_ms).max();
            let minutes = (limit / 60_000) as u64;
            match nudged {
                None if now - since >= limit => {
                    let _ = self.store.push_outbox(pid, &mid, &message::runaway(t, minutes), now);
                    let _ = self.store.event(pid, Some(&t.id), LILAD, "runaway", &format!("{mid} worked {minutes} min without a lila call; reminded"), now);
                }
                Some(at) if now - at >= limit && self.store.open_inbox_for(pid, "runaway", &mid).map(|v| v.is_empty()).unwrap_or(false) => {
                    let _ = self.store.add_inbox(pid, "runaway", Some(&mid), Some(&t.id), &inbox::runaway(&mid, t, 2 * minutes), &[], now);
                }
                _ => {}
            }
        }
    }

    /// Codex's usage window, from its own records once a minute: used up, work
    /// for Codex members waits until it resets, and the person hears once.
    fn watch_quota(&mut self, pid: &str, now: i64) {
        let codex: Vec<String> =
            self.projects[pid].team.members.iter().filter(|m| m.role.file.runtime == lila_core::Runtime::Codex).map(|m| m.id.clone()).collect();
        if codex.is_empty() || now - self.quota_checked.get(pid).copied().unwrap_or(i64::MIN / 2) < 60_000 {
            return;
        }
        self.quota_checked.insert(pid.to_owned(), now);
        let cwd = self.projects[pid].cwd.to_string_lossy().to_string();
        // magpie reads the account Codex is signed in to now; a session record may be
        // an earlier account's (one magpie switched away from), which would hold
        // Codex's members back until that account resets.
        let active = self.magpie.as_ref().and_then(|b| crate::magpie::accounts(b).ok()).and_then(|l| l.into_iter().find(|m| m.runtime == lila_core::Runtime::Codex && m.active));
        let until = match active {
            Some(m) => m.usage.windows.iter().filter(|w| w.used_percent >= 100.0 && w.resets_ms > now).map(|w| w.resets_ms).max(),
            None => {
                let src = self.live_sources();
                let usage = self.scanner.usage(&src, pid, &cwd, Some(now));
                usage.codex_limits.iter().filter(|l| l.used_percent >= 100.0 && l.resets_at_ms > now).map(|l| l.resets_at_ms).max()
            }
        };
        let open: Vec<String> = self.store.inbox(pid, false).unwrap_or_default().into_iter().filter(|i| i.kind == "quota").map(|i| i.id).collect();
        match until {
            Some(t) => {
                self.codex_paused.insert(pid.to_owned(), t);
                if open.is_empty() && self.quota_told.get(pid) != Some(&t) {
                    self.quota_told.insert(pid.to_owned(), t);
                    let _ = self.store.add_inbox(pid, "quota", None, None, &inbox::quota(t - now, &codex), &[], now);
                    let _ = self.store.event(pid, None, LILAD, "quota", &format!("used up; resets in {} min; {} wait", (t - now) / 60_000, codex.join(", ")), now);
                }
            }
            None => {
                self.codex_paused.remove(pid);
                for id in open {
                    let _ = self.store.resolve_inbox(pid, &id, None, now);
                }
            }
        }
    }

    /// Codex picks its own session id: look for the rollout of members that
    /// started without one known, for the first half hour.
    fn find_sessions(&mut self, pid: &str, now: i64) {
        let cwd = self.projects[pid].cwd.to_string_lossy().to_string();
        let team = &self.projects[pid].team;
        let missing: Vec<(String, i64, Option<String>)> = self.projects[pid]
            .live
            .iter()
            .filter(|(m, l)| l.terminal.is_some() && l.session.is_none() && team.member(m).is_some_and(|x| x.role.file.runtime == lila_core::Runtime::Codex))
            .filter_map(|(m, l)| l.started_ms.filter(|s| now - s < 30 * 60_000).map(|s| (m.clone(), s, l.chain.clone())))
            .collect();
        for (mid, started, chain) in missing {
            let Some((id, file)) = sessions::find_codex(&self.live_sources(), pid, &mid, &cwd, started) else { continue };
            let row = SessionRow { id: id.clone(), member: mid.clone(), runtime: "codex".into(), file: Some(file.to_string_lossy().to_string()), started_ms: started };
            if self.store.add_session(pid, &row).is_ok() {
                if let Some(c) = &chain {
                    let _ = self.store.touch_chain(pid, &id, c, now);
                }
                if let Some(l) = self.projects.get_mut(pid).and_then(|p| p.live.get_mut(&mid)) {
                    l.session = Some(id);
                }
            }
        }
    }

    /// The PM gets a new session once its context is past `pm_refresh_tokens` and
    /// it has sat idle `pm_refresh_idle_minutes` with nothing waiting to be typed.
    fn maybe_refresh_pm(&mut self, pid: &str, now: i64) {
        let team = &self.projects[pid].team;
        let (limit, idle) = (team.settings.pm_refresh_tokens, team.settings.pm_refresh_idle_minutes as i64 * 60_000);
        let owner = team.owner.clone();
        let live = self.projects[pid].live[&owner].clone();
        if limit == 0 || live.terminal.is_none() || !live.status.is_free() || now - live.free_since < idle {
            return;
        }
        if self.store.outbox(pid).unwrap_or_default().iter().any(|m| m.member == owner) {
            return;
        }
        if let Some(n) = self.context_of(pid, live.session.as_deref())
            && n >= limit
        {
            self.refresh_pm(pid, n, now);
        }
    }

    /// Every few seconds: startup timeouts, quiet members, then the pump.
    pub fn tick(&mut self, now: i64) {
        let pids: Vec<String> = self.projects.keys().cloned().collect();
        for pid in &pids {
            self.find_sessions(pid, now);
            self.maybe_refresh_pm(pid, now);
            self.watch_runaways(pid, now);
            self.watch_quota(pid, now);
        }
        for pid in pids {
            let settings = self.projects[&pid].team.settings.clone();
            let lives: Vec<(String, Live)> = self.projects[&pid].live.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            for (mid, live) in &lives {
                if live.terminal.is_some()
                    && !live.reported
                    && live.started_ms.is_some_and(|s| now - s >= settings.startup_timeout_secs as i64 * 1000)
                    && self.store.open_inbox_for(&pid, "startup", mid).map(|v| v.is_empty()).unwrap_or(false)
                {
                    let _ = self.store.add_inbox(&pid, "startup", Some(mid), None, &inbox::startup(mid), &[], now);
                    self.say(format!("{pid}: {mid} has not reported a status"));
                }
            }
            let stall_ms = settings.stall_minutes as i64 * 60_000;
            let tasks = self.store.tasks(&pid).unwrap_or_default();
            let all: Vec<Task> = tasks.iter().map(|(t, _)| t.clone()).collect();
            let open_runs: Vec<String> = self.store.runs(&pid).unwrap_or_default().into_iter().filter(|r| r.state.is_open()).map(|r| r.anchor).collect();
            for (t, meta) in tasks {
                if !matches!(t.state, TaskState::Delivered | TaskState::InProgress) {
                    continue;
                }
                let Some((_, live)) = lives.iter().find(|(m, _)| *m == t.to) else { continue };
                if !live.status.is_free() {
                    continue;
                }
                // It handed work on and waits for it: quiet is expected, and it hears when that finishes.
                if !route::open_under(&all, &t.id).is_empty() {
                    continue;
                }
                // Or a flow under it is between steps (lilad runs the check).
                if open_runs.iter().any(|a| *a == t.id || route::ancestors(&all, a).iter().any(|x| x.id == t.id)) {
                    continue;
                }
                match dispatch::stall_check(now, live.free_since, meta.nudged_ms, meta.stall_reported, stall_ms) {
                    StallAction::None => {}
                    StallAction::Nudge => {
                        let _ = self.store.push_outbox(&pid, &t.to, &message::nudge(&t), now);
                        let _ = self.store.set_nudged(&pid, &t.id, now);
                        let _ = self.store.event(&pid, Some(&t.id), "lilad", "nudged", &format!("{} quiet for {} min", t.to, settings.stall_minutes), now);
                    }
                    StallAction::Report => {
                        let _ = self.store.set_stall_reported(&pid, &t.id);
                        let _ = self.store.event(&pid, Some(&t.id), "lilad", "stalled", &format!("{} did not answer the reminder", t.to), now);
                        let owner = self.projects[&pid].team.owner.clone();
                        let text = format!("{} went quiet on {} \"{}\" and did not answer a reminder.", t.to, t.id, t.title);
                        if t.to == owner {
                            let _ = self.store.add_inbox(&pid, "stalled", Some(&t.to), Some(&t.id), &inbox::stalled(&t), &[], now);
                        } else {
                            let _ = self.store.push_outbox(&pid, &owner, &message::note(&t.id, &format!("{text} Check its terminal, re-assign, or cancel it.")), now);
                        }
                    }
                }
            }
            self.pump(&pid, now);
        }
    }
}

fn split_brief(brief: &str) -> (String, String) {
    let brief = brief.trim();
    let first = brief.lines().next().unwrap_or_default();
    let title: String = first.chars().take(80).collect();
    (title, brief.to_owned())
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

// ---------------- the server ----------------

enum Input {
    Request(String, Value, std::sync::mpsc::Sender<Value>),
    Hc(Value),
    /// A fresh `events.subscribe` snapshot and when it was taken.
    Snapshot(Value, i64),
    Tick,
    /// Look for spool requests (fast: sandboxed members wait on them).
    Spool,
    /// A flow's check finished: project, run anchor, result.
    CheckDone(String, String, CheckResult),
    /// An accepted chain's commit finished: project, chain, result.
    CommitDone(String, String, CommitResult),
    Stop,
}

/// Runs until `lila daemon stop` or the socket is removed.
pub fn serve(paths: Paths, hc_bin: String) -> Result<()> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::{Arc, Mutex, mpsc};

    paths.ensure()?;
    let sock = paths.socket()?;
    if std::os::unix::net::UnixStream::connect(&sock).is_ok() {
        bail!("lilad is already running on {}", sock.display());
    }
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).with_context(|| format!("bind {}", sock.display()))?;

    let store = Store::open(&paths.db())?;
    let hc = hc_client::Hc::connect(&hc_bin)?;
    let path_env = std::env::var("PATH").unwrap_or_default();
    let lila_bin = std::env::current_exe()?;
    let mut daemon = Daemon::new(store, HcTerminals(hc), paths.clone(), path_env.clone(), lila_bin);
    daemon.magpie = crate::magpie::find(&path_env);
    eprintln!("[lilad] listening on {} (pid {})", sock.display(), std::process::id());

    let (tx, rx) = mpsc::channel::<Input>();
    // Watchers (lila-app) that asked to hear when anything changes.
    let watchers: Arc<Mutex<Vec<mpsc::Sender<String>>>> = Arc::default();

    // Socket: one JSON request per line, one reply line per request.
    // `subscribe` turns the connection into a stream of `{"event":"changed"}` lines.
    let tx_sock = tx.clone();
    let watchers_sock = watchers.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(conn) = conn else { continue };
            let tx = tx_sock.clone();
            let watchers = watchers_sock.clone();
            std::thread::spawn(move || {
                let mut writer = match conn.try_clone() {
                    Ok(w) => w,
                    Err(_) => return,
                };
                for line in BufReader::new(conn).lines().map_while(Result::ok) {
                    let reply = match serde_json::from_str::<Value>(&line) {
                        Ok(req) if req["method"] == "subscribe" => {
                            let (wtx, wrx) = mpsc::channel::<String>();
                            watchers.lock().unwrap().push(wtx);
                            if writeln!(writer, "{}", json!({"result": {"subscribed": true}})).is_err() {
                                return;
                            }
                            for event in wrx {
                                if writeln!(writer, "{event}").is_err() {
                                    return;
                                }
                            }
                            return;
                        }
                        Ok(req) => {
                            let (rtx, rrx) = mpsc::channel();
                            let method = req["method"].as_str().unwrap_or_default().to_owned();
                            if tx.send(Input::Request(method, req["params"].clone(), rtx)).is_err() {
                                break;
                            }
                            rrx.recv().unwrap_or_else(|_| json!({"error": "daemon stopped"}))
                        }
                        Err(e) => json!({"error": format!("bad request: {e}")}),
                    };
                    if writeln!(writer, "{reply}").is_err() {
                        break;
                    }
                }
            });
        }
    });

    // herdr-core events, resubscribing when the core restarts.
    let tx_ev = tx.clone();
    std::thread::spawn(move || {
        loop {
            match hc_client::Events::subscribe(&hc_bin, json!({})) {
                Ok(mut ev) => {
                    if tx_ev.send(Input::Snapshot(ev.snapshot.clone(), now_ms())).is_err() {
                        return;
                    }
                    while let Ok(line) = ev.next_line() {
                        if tx_ev.send(Input::Hc(line)).is_err() {
                            return;
                        }
                    }
                    eprintln!("[lilad] lost herdr-core events; resubscribing");
                }
                Err(e) => eprintln!("[lilad] cannot subscribe to herdr-core events: {e:#}"),
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });

    let tx_spool = tx.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(250));
            if tx_spool.send(Input::Spool).is_err() {
                return;
            }
        }
    });

    let tx_tick = tx.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            if tx_tick.send(Input::Tick).is_err() {
                return;
            }
        }
    });

    spawn_checks(&mut daemon, &tx);
    let mut seen = daemon.fingerprint();
    let mut seq: u64 = 0;
    for input in rx {
        let now = now_ms();
        match input {
            Input::Request(method, params, reply) => {
                if method == "stop" {
                    let _ = reply.send(json!({"result": {"stopped": true}}));
                    let _ = tx.send(Input::Stop);
                    continue;
                }
                let out = match daemon.handle(&method, &params, now) {
                    Ok(v) => json!({"result": v}),
                    Err(e) => json!({"error": format!("{e:#}")}),
                };
                let _ = reply.send(out);
            }
            Input::Snapshot(snap, taken_ms) => {
                for rec in snap["terminals"].as_array().cloned().unwrap_or_default() {
                    daemon.on_terminal(&rec, now);
                }
                // Members whose terminal is not in the snapshot are gone.
                let known: Vec<String> = snap["terminals"].as_array().into_iter().flatten().filter(|t| t["state"] == "live").filter_map(|t| t["address"].as_str().map(String::from)).collect();
                daemon.reconcile(&known, taken_ms, now);
            }
            Input::Hc(line) => daemon.on_event(&line, now),
            Input::Tick => daemon.tick(now),
            Input::Spool => daemon.process_spool(now),
            Input::CheckDone(pid, anchor, r) => daemon.on_check_done(&pid, &anchor, &r, now),
            Input::CommitDone(pid, chain, r) => daemon.on_commit_done(&pid, &chain, &r, now),
            Input::Stop => break,
        }
        spawn_checks(&mut daemon, &tx);
        let fp = daemon.fingerprint();
        if fp != seen {
            seen = fp;
            seq += 1;
            let event = json!({"event": "changed", "seq": seq}).to_string();
            watchers.lock().unwrap().retain(|w| w.send(event.clone()).is_ok());
        }
    }
    let _ = std::fs::remove_file(&sock);
    eprintln!("[lilad] stopped");
    Ok(())
}

/// Runs each check and commit lilad asked for on its own thread.
fn spawn_checks<T: Terminals>(daemon: &mut Daemon<T>, tx: &std::sync::mpsc::Sender<Input>) {
    for job in daemon.take_checks() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let r = crate::check::run(&job);
            let _ = tx.send(Input::CheckDone(job.project, job.anchor, r));
        });
    }
    for job in daemon.take_commits() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let r = crate::commit::run(&job);
            let _ = tx.send(Input::CommitDone(job.project, job.chain, r));
        });
    }
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
#[path = "daemon_tests.rs"]
mod tests;
