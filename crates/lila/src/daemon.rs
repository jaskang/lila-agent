//! lilad: the only writer of Lila's state. Requests come from the CLI over a
//! Unix socket, terminal facts from herdr-core's event stream; both are
//! handled one at a time on one thread, so there is no locking.

use crate::paths::{Paths, which};
use crate::roles;
use anyhow::{Context, Result, anyhow, bail};
use lila_core::dispatch::{self, AgentStatus, MemberView, QueuedTask, StallAction};
use lila_core::{Actor, ProjectConfig, Task, TaskState, Team, Transition, message, prompt, runtime};
use lila_store::{NewTask, Store};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What the daemon needs from herdr-core. A fake stands in for it in tests.
pub trait Terminals {
    /// Starts a program; returns its terminal address.
    fn run(&mut self, req: Value) -> Result<String>;
    /// Types text and presses Enter. `only_if`: only when the status word is exactly this.
    fn send(&mut self, terminal: &str, text: &str, only_if: Option<&str>) -> Result<()>;
    fn kill(&mut self, terminal: &str) -> Result<()>;
}

pub struct HcTerminals(pub hc_client::Hc);

impl Terminals for HcTerminals {
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
}

impl Default for Live {
    fn default() -> Self {
        Live { terminal: None, status: AgentStatus::Gone, kind: None, msg: String::new(), reported: false, free_since: 0, started_ms: None }
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
    Team::build(&cfg, &roles).map_err(|e| anyhow!("{}: {e}", path.display()))
}

fn terminal_name(project: &str, member: &str) -> String {
    let mut n = format!("lila-{project}-{member}");
    n.truncate(32);
    n
}

impl<T: Terminals> Daemon<T> {
    pub fn new(store: Store, term: T, paths: Paths, path_env: String, lila_bin: PathBuf) -> Daemon<T> {
        let mut d = Daemon { store, term, paths, projects: BTreeMap::new(), path_env, lila_bin, log: Vec::new(), failed_sends: BTreeMap::new(), live_version: 0 };
        let rows = d.store.projects().unwrap_or_default();
        for p in rows {
            if let Err(e) = d.load_project(&p.id, Path::new(&p.cwd)) {
                d.say(format!("project {}: not loaded: {e:#}", p.id));
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

    fn load_project(&mut self, id: &str, cwd: &Path) -> Result<()> {
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
                    live.insert(m.id.clone(), Live { terminal: m.terminal, started_ms: m.started_ms, status, ..Live::default() });
                }
            }
        }
        live.retain(|mid, _| team.member(mid).is_some());
        for m in &team.members {
            live.entry(m.id.clone()).or_default();
        }
        self.projects.insert(id.to_owned(), Proj { cwd: cwd.to_path_buf(), team, live });
        Ok(())
    }

    // ---------------- requests ----------------

    /// One CLI request. `params.caller` = {project?, member?, cwd}.
    pub fn handle(&mut self, method: &str, params: &Value, now: i64) -> Result<Value> {
        let caller = params.get("caller").cloned().unwrap_or(Value::Null);
        match method {
            "ping" => Ok(json!({"pong": true, "pid": std::process::id()})),
            "project.up" => self.project_up(&caller, params, now),
            "project.down" => self.project_down(&caller, now),
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
        self.load_project(&pid, &root)?;
        std::fs::create_dir_all(root.join(".lila").join("tasks"))?;
        std::fs::create_dir_all(spool_dir(&root))?;
        ensure_gitignore(&root)?;

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
            let t = self.create_task(&pid, NewTask { title, body, from: Actor::Human, to: owner, parent: None, priority: 0 }, now)?;
            brief_task = Some(t.id);
        }
        self.pump(&pid, now);
        Ok(json!({"project": pid, "cwd": root_s, "started": started, "running": running, "brief_task": brief_task}))
    }

    fn start_member(&mut self, pid: &str, mid: &str, now: i64) -> Result<String> {
        let proj = &self.projects[pid];
        let member = proj.team.member(mid).expect("member").clone();
        let cwd = proj.cwd.to_string_lossy().to_string();
        let text = prompt::render(&proj.team, &member, &cwd);
        let dir = self.paths.prompts(pid);
        std::fs::create_dir_all(&dir)?;
        let prompt_path = dir.join(format!("{mid}.md"));
        std::fs::write(&prompt_path, &text)?;

        let mut launch = runtime::launch(&member.role, &prompt_path.to_string_lossy(), &text);
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
        let live = self.projects.get_mut(pid).unwrap().live.get_mut(mid).unwrap();
        *live = Live { terminal: Some(addr.clone()), status: AgentStatus::Unknown, started_ms: Some(now), free_since: now, ..Live::default() };
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
        let name = runtime::launch(role, "", "").argv[0].clone();
        let program = which(&name, &path).ok_or_else(|| anyhow!("{name:?} is not on PATH for role {}; add its folder to the role's env.PATH", role.id))?;
        env.insert("PATH".into(), path);
        Ok((program, env))
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

    fn status(&self, caller: &Value) -> Result<Value> {
        let (pid, _) = self.who(caller)?;
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
                json!({
                    "id": m.id, "role": m.role.file.name, "runtime": m.role.file.runtime.to_string(), "model": m.role.file.model,
                    "owner": m.is_owner, "terminal": live.terminal, "status": live.status.as_str(), "kind": live.kind, "msg": live.msg,
                    "task": current, "queued": queued,
                })
            })
            .collect();
        let open: Vec<Value> = tasks.iter().filter(|(t, _)| t.state.is_open()).map(|(t, _)| task_json(t)).collect();
        let inbox = self.store.inbox(&pid, false)?.len();
        let paused = self.store.project(&pid)?.map(|p| p.paused).unwrap_or(false);
        Ok(json!({"project": pid, "cwd": proj.cwd, "paused": paused, "members": members, "open_tasks": open, "inbox": inbox}))
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
        let to = req(params, "to")?.to_owned();
        let team = &self.projects[&pid].team;
        team.check_assign(&actor, &to).map_err(anyhow::Error::msg)?;
        let max_depth = team.settings.max_chain_depth;
        let parent = s(params, "parent").map(|p| p.to_ascii_uppercase());
        if let Some(p) = &parent {
            self.store.task(&pid, p)?.ok_or_else(|| anyhow!("no task {p} to use as parent"))?;
            if self.store.depth(&pid, p)? + 1 >= max_depth {
                bail!("task chain under {p} is already {max_depth} deep; report to the owner with `lila task block` instead of creating more subtasks");
            }
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
                    "{from} and {to} have already sent this work back and forth {max} times; stop and run `lila task block {p} --reason \"<what keeps failing>\"` so the owner can decide"
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
        };
        let task = self.create_task(&pid, new, now)?;
        self.pump(&pid, now);
        let task = self.store.task(&pid, &task.id)?.expect("exists");
        Ok(task_json(&task))
    }

    fn task_move(&mut self, caller: &Value, params: &Value, t: Transition, now: i64) -> Result<Value> {
        let (pid, actor) = self.who(caller)?;
        let id = req(params, "id")?;
        let task = self.store.task(&pid, id)?.ok_or_else(|| anyhow!("no task {id} in project {pid}"))?;
        let owner = self.projects[&pid].team.owner.clone();
        match (&actor, t) {
            (Actor::Human, _) => {}
            (Actor::Member(m), Transition::Cancel) if *m == owner || task.from == actor => {}
            (Actor::Member(_), Transition::Cancel) => bail!("only {} (who assigned it) or the owner can cancel {}", task.from, task.id),
            (Actor::Member(m), _) if *m != task.to => bail!("{} is assigned to {}, not you ({m})", task.id, task.to),
            _ => {}
        }
        let detail = match t {
            Transition::Done => req(params, "summary").context("give a one-line --summary")?.to_owned(),
            Transition::Block => req(params, "reason").context("say what you need with --reason")?.to_owned(),
            _ => s(params, "reason").unwrap_or_default().to_owned(),
        };
        let was_active = task.state.is_active();
        let task = self.store.transition(&pid, &task.id, t, &actor, &detail, now)?;
        let files = strings(params, "files");
        if !files.is_empty() {
            self.store.add_artifacts(&pid, &task.id, &files)?;
        }
        match t {
            Transition::Done => {
                let follow_ups: Vec<Task> = self.store.tasks(&pid)?.into_iter().map(|(t, _)| t).filter(|t| t.parent.as_deref() == Some(&task.id)).collect();
                self.report(&pid, &task, message::done_note(&task, &files, &follow_ups), now)?
            }
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

    /// Tells whoever assigned the task (and the owner, when a blocker skips it).
    fn report(&mut self, pid: &str, task: &Task, text: String, now: i64) -> Result<()> {
        let owner = self.projects[pid].team.owner.clone();
        match &task.from {
            Actor::Human => {
                let what = if task.state == TaskState::Done { "done" } else { "blocked" };
                let body = format!("{} {what}: {}", task.id, task.summary.clone().or(task.block_reason.clone()).unwrap_or_default());
                self.store.add_inbox(pid, "report", Some(&task.to), Some(&task.id), &body, &[], now)?;
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

    /// Changes a task you assigned. Queued: rewritten in place. Already delivered:
    /// what was typed can't change, so `append` goes to the member as an update.
    fn task_edit(&mut self, caller: &Value, params: &Value, now: i64) -> Result<Value> {
        let (pid, actor) = self.who(caller)?;
        let id = req(params, "id")?;
        let task = self.store.task(&pid, id)?.ok_or_else(|| anyhow!("no task {id} in project {pid}"))?;
        let owner = self.projects[&pid].team.owner.clone();
        if actor != Actor::Human && actor != Actor::Member(owner.clone()) && actor != task.from {
            bail!("only {} (who assigned it) or the owner can edit {}", task.from, task.id);
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
        let Actor::Member(m) = &actor else { bail!("`lila ask` is how the owner asks you; you are the person") };
        if *m != owner {
            bail!("only the owner ({owner}) asks the person; report to your assigner with `lila task block` instead");
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
        let text = s(params, "text");
        if item.kind == "question" {
            let text = text.ok_or_else(|| anyhow!("{} is a question; give an answer", item.id))?;
            let to = item.member.clone().unwrap_or_else(|| self.projects[&pid].team.owner.clone());
            self.store.push_outbox(&pid, &to, &message::answer(&item.id, &item.text, text), now)?;
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
        if new != AgentStatus::Unknown {
            live.reported = true;
        }
        let (msg, kind, reported) = (live.msg.clone(), live.kind.clone(), live.reported);
        if reported {
            self.resolve_open(pid, "startup", mid, now);
        }
        if new == AgentStatus::Blocked && old != AgentStatus::Blocked && self.store.open_inbox_for(pid, "terminal", mid).map(|v| v.is_empty()).unwrap_or(false) {
            let what = if msg.is_empty() { format!("waiting in its terminal ({})", kind.as_deref().unwrap_or("input")) } else { msg };
            let _ = self.store.add_inbox(pid, "terminal", Some(mid), None, &format!("{mid} needs you in its terminal: {what}"), &[], now);
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
            let _ = self.store.add_inbox(pid, "lost", Some(mid), None, &format!("{mid} ended unexpectedly ({why}). Run `lila up` to restart it.{tail}"), &[], now);
            let owner = self.projects[pid].team.owner.clone();
            if owner != mid {
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
        for msg in self.store.outbox(pid).unwrap_or_default() {
            let Some(live) = self.projects[pid].live.get(&msg.member).cloned() else {
                let _ = self.store.pop_outbox(msg.id);
                continue;
            };
            let (Some(t), true) = (live.terminal.as_deref(), live.status.accepts_notes()) else { continue };
            match self.term.send(t, &msg.text, None) {
                Ok(()) => {
                    let _ = self.store.pop_outbox(msg.id);
                }
                Err(e) => self.say(format!("{pid}: note to {} not sent yet: {e:#}", msg.member)),
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
        let queued: Vec<QueuedTask> = tasks
            .iter()
            .filter(|(t, _)| t.state == TaskState::Queued)
            .map(|(t, meta)| QueuedTask { id: t.id.clone(), to: t.to.clone(), priority: t.priority, seq: meta.seq })
            .collect();
        let picks = dispatch::pick_deliveries(&views, &queued, proj.team.settings.max_working);
        for (tid, mid) in picks {
            let Some(task) = tasks.iter().find(|(t, _)| t.id == tid).map(|(t, _)| t.clone()) else { continue };
            let live = self.projects[pid].live[&mid].clone();
            let Some(terminal) = live.terminal else { continue };
            let text = message::task_delivery(&task, &task_file_rel(&task.id));
            // Type only if it is still idle. hc's `if` knows `idle` and `working`
            // only; `done` (finished while unseen) takes input like idle, unguarded.
            let guard = (live.status == AgentStatus::Idle).then_some("idle");
            match self.term.send(&terminal, &text, guard) {
                Ok(()) => {
                    self.failed_sends.remove(&tid);
                    let _ = self.store.transition(pid, &tid, Transition::Deliver, &Actor::Member("lilad".into()), &format!("to {terminal}"), now);
                    self.projects.get_mut(pid).unwrap().live.get_mut(&mid).unwrap().free_since = now;
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

    /// Every few seconds: startup timeouts, quiet members, then the pump.
    pub fn tick(&mut self, now: i64) {
        let pids: Vec<String> = self.projects.keys().cloned().collect();
        for pid in pids {
            let settings = self.projects[&pid].team.settings.clone();
            let lives: Vec<(String, Live)> = self.projects[&pid].live.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            for (mid, live) in &lives {
                if live.terminal.is_some()
                    && !live.reported
                    && live.started_ms.is_some_and(|s| now - s >= settings.startup_timeout_secs as i64 * 1000)
                    && self.store.open_inbox_for(&pid, "startup", mid).map(|v| v.is_empty()).unwrap_or(false)
                {
                    let text = format!(
                        "{mid} has not reported a status since it started. It may be waiting in its terminal (a trust prompt, a login). Open terminal {} and check.",
                        live.terminal.as_deref().unwrap_or("?")
                    );
                    let _ = self.store.add_inbox(&pid, "startup", Some(mid), None, &text, &[], now);
                    self.say(format!("{pid}: {mid} has not reported a status"));
                }
            }
            let stall_ms = settings.stall_minutes as i64 * 60_000;
            for (t, meta) in self.store.tasks(&pid).unwrap_or_default() {
                if !matches!(t.state, TaskState::Delivered | TaskState::InProgress) {
                    continue;
                }
                let Some((_, live)) = lives.iter().find(|(m, _)| *m == t.to) else { continue };
                if !live.status.is_free() {
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
                            let _ = self.store.add_inbox(&pid, "stalled", Some(&t.to), Some(&t.id), &text, &[], now);
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

pub fn task_file_rel(id: &str) -> String {
    format!(".lila/tasks/{id}.md")
}

fn write_task_file(cwd: &Path, t: &Task) -> Result<()> {
    let dir = cwd.join(".lila").join("tasks");
    std::fs::create_dir_all(&dir)?;
    let parent = t.parent.as_deref().map(|p| format!(" · parent {p}")).unwrap_or_default();
    let text = format!("# {} · {}\n\nfrom {} → {}{parent}\n\n{}\n", t.id, t.title, t.from, t.to, t.body.trim());
    std::fs::write(dir.join(format!("{}.md", t.id)), text)?;
    Ok(())
}

pub fn task_json(t: &Task) -> Value {
    json!({
        "id": t.id, "title": t.title, "body": t.body, "state": t.state.as_str(), "from": t.from.as_str(), "to": t.to,
        "parent": t.parent, "priority": t.priority, "summary": t.summary, "block_reason": t.block_reason,
        "created_ms": t.created_ms, "updated_ms": t.updated_ms,
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
    let mut daemon = Daemon::new(store, HcTerminals(hc), paths.clone(), path_env, lila_bin);
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
            Input::Stop => break,
        }
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
