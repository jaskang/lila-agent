//! What lilad answers: every request from the CLI, members and the window (`Daemon::handle`).

use super::*;

impl<T: Terminals> Daemon<T> {
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
            "request.list" => self.request_list(&caller, params),
            "request.show" => self.request_show(&caller, params),
            "conversation.read" => self.conversation_read(&caller, params),
            "conversation.send" => self.conversation_send(&caller, params),
            "hook" => self.hook(&caller, params, now),
            "hook.answer" => self.hook_answer(&caller, params),
            "triage" => self.triage(&caller, params, now),
            "draft" => self.draft(&caller, params, now),
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
    pub(super) fn who(&self, caller: &Value) -> Result<(String, Actor)> {
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
    pub(super) fn registered(&self, caller: &Value) -> Option<(String, PathBuf)> {
        let root = crate::paths::find_project_root(Path::new(s(caller, "cwd")?))?;
        let p = self.store.project_by_cwd(&root.to_string_lossy()).ok()??;
        Some((p.id, root))
    }

    pub(super) fn project_up(&mut self, caller: &Value, params: &Value, now: i64) -> Result<Value> {
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
            let ours = self.dir_strings(&pid);
            if rec["state"] != "live" || rec["labels"]["lila.project"] != pid || !rec["cwd"].as_str().is_some_and(|c| ours.iter().any(|o| o == c)) {
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

    pub(super) fn start_member(&mut self, pid: &str, mid: &str, now: i64) -> Result<String> {
        let dir = self.home_dir(pid, mid);
        self.start_session(pid, mid, runtime::Session::New(None), &dir, now)
    }

    /// Starts a member's CLI in `dir` (the project folder or a chain's worktree),
    /// in a new session or an earlier one (`Session::Resume`).
    pub(super) fn start_session(&mut self, pid: &str, mid: &str, session: runtime::Session, dir: &Path, now: i64) -> Result<String> {
        let proj = &self.projects[pid];
        let member = proj.team.member(mid).expect("member").clone();
        let cwd = dir.to_string_lossy().to_string();
        let text = prompt::render(&proj.team, &member, &cwd);
        let prompts = self.paths.prompts(pid);
        std::fs::create_dir_all(&prompts)?;
        let prompt_path = prompts.join(format!("{mid}.md"));
        std::fs::write(&prompt_path, &text)?;

        let rt = member.role.file.runtime;
        // Claude Code takes the id Lila picks, so the session is known from the start.
        let session = match session {
            runtime::Session::New(None) if rt == lila_core::Runtime::Claude => runtime::Session::New(Some(sessions::new_uuid())),
            other => other,
        };
        let mut launch = runtime::launch(&member.role, &prompt_path.to_string_lossy(), &session);
        if rt == lila_core::Runtime::Claude {
            // Its hooks report to lilad (permission cards, the reply as it is written).
            let settings = prompts.join(format!("{mid}.settings.json"));
            std::fs::write(&settings, serde_json::to_string_pretty(&hooks::settings(&self.lila_bin))?)?;
            launch.argv.extend(["--settings".to_owned(), settings.to_string_lossy().to_string()]);
        }
        let (program, mut env) = self.member_env(&member.role)?;
        if launch.argv[0] == "lila" {
            // Driven through its protocol by `lila host <runtime>`, which starts the CLI.
            launch.argv[0] = self.lila_bin.to_string_lossy().to_string();
            launch.argv.splice(3..3, ["--program".to_owned(), program.to_string_lossy().to_string()]);
        } else {
            launch.argv[0] = program.to_string_lossy().to_string();
        }
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
            cwd: Some(dir.to_path_buf()),
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
    pub(super) fn member_env(&self, role: &lila_core::Role) -> Result<(PathBuf, BTreeMap<String, String>)> {
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
        let name = runtime::cli(role.file.runtime);
        let program = which(name, &path).ok_or_else(|| anyhow!("{name:?} is not on PATH for role {}; add its folder to the role's env.PATH", role.id))?;
        env.insert("PATH".into(), path);
        // Members run on their CLI's current account (`accounts`): its own config folder.
        if let Some((var, dir)) = self.account(role.file.runtime).and_then(|a| a.env()) {
            env.insert(var.into(), dir);
        }
        Ok((program, env))
    }

    /// A CLI's current subscription account (`~/.config/lila/accounts.toml`).
    pub(super) fn account(&self, r: lila_core::Runtime) -> Option<crate::accounts::Account> {
        let list = crate::accounts::load(&self.paths.accounts()).ok()?;
        crate::accounts::current(&list, r).cloned()
    }

    /// Where the CLIs record sessions now: the current accounts' folders, or the
    /// defaults (`sources`) for an account in the CLI's own folder.
    pub(super) fn live_sources(&self) -> Sources {
        let pick = |r: lila_core::Runtime, default: &PathBuf| self.account(r).and_then(|a| a.dir).unwrap_or_else(|| default.clone());
        Sources { claude: pick(lila_core::Runtime::Claude, &self.sources.claude), codex: pick(lila_core::Runtime::Codex, &self.sources.codex) }
    }

    /// Reads project.toml and the roles again (after the settings changed).
    /// Running members keep running; `status` marks those on old settings `stale`.
    pub(super) fn project_reload(&mut self, caller: &Value, now: i64) -> Result<Value> {
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
    pub(super) fn member_removed(&mut self, pid: &str, mid: &str, terminal: Option<String>, now: i64) {
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
    pub(super) fn stale_members(&self, pid: &str) -> Vec<String> {
        let proj = &self.projects[pid];
        proj.team.members.iter().filter(|m| proj.live.get(&m.id).is_some_and(|l| l.terminal.is_some() && l.launched.is_some_and(|k| k != m.launch_key()))).map(|m| m.id.clone()).collect()
    }

    /// Stops one member and starts it again with the current settings. Its
    /// open task goes back to the queue and is delivered again.
    pub(super) fn member_restart(&mut self, caller: &Value, params: &Value, now: i64) -> Result<Value> {
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

    pub(super) fn project_down(&mut self, caller: &Value, now: i64) -> Result<Value> {
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
    pub(super) fn project_remove(&mut self, caller: &Value, now: i64) -> Result<Value> {
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

    pub(super) fn status(&mut self, caller: &Value) -> Result<Value> {
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
                    "caps": conversation::caps(m.role.file.runtime),
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
        let (held_folder, held_worktree) = if self.uses_slots(&pid) {
            let proj = &self.projects[&pid];
            let started: BTreeSet<String> =
                plain.iter().filter(|t| t.to != proj.team.owner && t.state != TaskState::Queued).map(|t| route::ancestors(&plain, &t.id).last().map_or(t.id.clone(), |r| r.id.clone())).collect();
            let waiting = plain
                .iter()
                .filter(|t| t.state == TaskState::Queued && t.to != proj.team.owner)
                .map(|t| route::ancestors(&plain, &t.id).last().map_or(t.id.clone(), |r| r.id.clone()))
                .filter(|c| !started.contains(c) && self.slot_of(&pid, c).is_none())
                .count();
            (0, waiting)
        } else {
            (self.held_for_folder(&pid, &tasks).len(), 0)
        };
        let proj = &self.projects[&pid];
        Ok(json!({
            "project": pid, "cwd": proj.cwd, "paused": paused, "members": members, "open_tasks": open, "inbox": inbox, "runs": runs,
            "held_for_review": held, "unreviewed": self.unreviewed(&pid), "codex_paused_until": codex_until,
            "open_chains": self.open_chains(&pid, &plain), "held_for_folder": held_folder,
            "worktrees": self.slots_json(&pid), "held_for_worktree": held_worktree, "sources": self.sources_json(&pid),
        }))
    }

    /// Every request (chain) and whose move it is (`lila_core::request`): the
    /// window's list and `lila requests`. Finished ones: the latest `done_limit` (30).
    pub(super) fn request_list(&mut self, caller: &Value, params: &Value) -> Result<Value> {
        use lila_core::request::Group;
        let (pid, _) = self.who(caller)?;
        let limit = params.get("done_limit").and_then(Value::as_u64).unwrap_or(30) as usize;
        let mut finished = 0;
        let list: Vec<lila_core::request::Request> = self
            .requests_of(&pid)?
            .into_iter()
            .filter(|r| {
                let done = matches!(r.group, Group::Delivered | Group::Dropped);
                finished += usize::from(done);
                !done || finished <= limit
            })
            .collect();
        Ok(serde_json::to_value(list)?)
    }

    /// One request for the window's panel: the request, every task in its chain
    /// (oldest first), their history as one timeline, and the open inbox items about it.
    pub(super) fn request_show(&mut self, caller: &Value, params: &Value) -> Result<Value> {
        let (pid, _) = self.who(caller)?;
        let id = req(params, "id")?.to_ascii_uppercase();
        let list = self.requests_of(&pid)?;
        let r = list.into_iter().find(|r| r.id == id).ok_or_else(|| anyhow!("no request {id}"))?;
        let tasks: Vec<Task> = self.store.tasks(&pid)?.into_iter().map(|(t, _)| t).collect();
        let chain: Vec<&Task> = tasks.iter().filter(|t| t.id == id || route::ancestors(&tasks, &t.id).iter().any(|a| a.id == id)).collect();
        let mut events = Vec::new();
        for t in &chain {
            for e in self.store.events(&pid, Some(&t.id), 200)? {
                events.push(json!({"at_ms": e.at_ms, "task": e.task, "actor": e.actor, "kind": e.kind, "detail": e.detail}));
            }
        }
        events.sort_by_key(|e| e["at_ms"].as_i64().unwrap_or(0));
        let items: Vec<Value> = self.store.inbox(&pid, false)?.iter().filter(|i| r.inbox.contains(&i.id)).map(inbox_json).collect();
        let tasks: Vec<Value> = chain
            .iter()
            .map(|t| {
                let mut v = task_json(t);
                v["artifacts"] = json!(self.store.artifacts(&pid, &t.id).unwrap_or_default());
                v
            })
            .collect();
        Ok(json!({"request": r, "tasks": tasks, "events": events, "inbox": items}))
    }

    /// Every request of a project (`lila_core::request`).
    fn requests_of(&mut self, pid: &str) -> Result<Vec<lila_core::request::Request>> {
        use lila_core::request::{self, Facts, Hold, InboxFact, RunFact};
        let pid = pid.to_owned();
        let with_meta = self.store.tasks(&pid)?;
        let tasks: Vec<Task> = with_meta.iter().map(|(t, _)| t.clone()).collect();
        let slotted = self.uses_slots(&pid);
        let proj = &self.projects[&pid];
        let owner = proj.team.owner.clone();
        let active = |m: &str| tasks.iter().filter(|t| t.to == m && matches!(t.state, TaskState::Delivered | TaskState::InProgress | TaskState::Blocked)).max_by_key(|t| t.updated_ms).map(|t| t.id.clone());
        // An item about a member is about the task it holds.
        let inbox: Vec<InboxFact> = self
            .store
            .inbox(&pid, false)?
            .into_iter()
            .map(|i| {
                let task = i.task.clone().or_else(|| i.member.as_deref().and_then(active));
                InboxFact { id: i.id, kind: i.kind, task, created_ms: i.created_ms }
            })
            .collect();
        let runs: Vec<RunFact> = self.store.runs(&pid)?.into_iter().filter(|r| r.state.is_open()).map(|r| RunFact { anchor: r.anchor, checking: r.state == RunState::Checking, updated_ms: r.updated_ms }).collect();
        let committing: Vec<String> = self.committing.iter().filter(|(p, _)| *p == pid).map(|(_, c)| c.clone()).collect();
        let root = |id: &str| route::ancestors(&tasks, id).last().map(|t| t.id.clone()).unwrap_or_else(|| id.to_owned());
        let started: BTreeSet<String> = tasks.iter().filter(|t| t.to != owner && t.state != TaskState::Queued).map(|t| root(&t.id)).collect();
        let folder = if slotted { BTreeSet::new() } else { self.held_for_folder(&pid, &with_meta) };
        let codex_until = self.codex_paused.get(&pid).copied();
        let parallel = proj.team.settings.parallel as usize;
        let full = proj.slots.iter().take(parallel).all(|s| s.chain.is_some());
        let mut held = BTreeMap::new();
        for t in tasks.iter().filter(|t| t.state == TaskState::Queued) {
            let chain = root(&t.id);
            let why = if self.held_for_review(&pid, t) {
                Hold::Review
            } else if slotted && full && t.to != owner && self.slot_of(&pid, &chain).is_none() && !started.contains(&chain) {
                Hold::Worktree
            } else if folder.contains(&t.id) {
                Hold::Folder
            } else if codex_until.is_some() && proj.team.member(&t.to).is_some_and(|m| m.role.file.runtime == lila_core::Runtime::Codex) {
                Hold::Quota
            } else if proj.live.get(&t.to).is_none_or(|l| l.terminal.is_none()) {
                Hold::Stopped
            } else if proj.live.get(&t.to).is_some_and(|l| !l.reported) {
                Hold::Starting
            } else {
                Hold::Member
            };
            held.insert(t.id.clone(), why);
        }
        let slots: BTreeMap<String, usize> = proj.slots.iter().enumerate().filter_map(|(i, s)| s.chain.clone().map(|c| (c, i + 1))).collect();
        let external: BTreeMap<String, request::External> = self
            .store
            .externals(&pid)?
            .into_iter()
            .map(|e| (e.task.clone(), request::External { source: e.source, rf: e.rf, url: e.url, author: e.author, triage: e.triage, pr: e.pr, asked_ms: e.asked_ms }))
            .collect();
        let facts = Facts { tasks: &tasks, owner: &owner, inbox: &inbox, runs: &runs, committing: &committing, held: &held, slots: &slots, external: &external };
        Ok(request::requests(&facts))
    }

    pub(super) fn create_task(&mut self, pid: &str, new: NewTask, now: i64) -> Result<Task> {
        let task = self.store.create_task(pid, new, now)?;
        let cwd = self.projects[pid].cwd.clone();
        write_task_file(&cwd, &task)?;
        self.say(format!("{pid}: {} {} → {}: {}", task.id, task.from, task.to, task.title));
        Ok(task)
    }

    pub(super) fn task_new(&mut self, caller: &Value, params: &Value, now: i64) -> Result<Value> {
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
        } else if task.state == TaskState::Queued && self.uses_slots(&pid) && self.slot_of(&pid, &self.store.ancestors(&pid, &task.id)?.last().map(|t| t.id.clone()).unwrap_or_else(|| task.id.clone())).is_none() && task.to != self.projects[&pid].team.owner {
            let held: Vec<String> = self.projects[&pid].slots.iter().filter_map(|s| s.chain.clone()).collect();
            v["held"] = json!(format!("every worktree is taken (chains {}) until the person signs one off; this chain starts in the first one free", held.join(", ")));
        } else if task.state == TaskState::Queued && !self.uses_slots(&pid) && self.held_for_folder(&pid, &self.store.tasks(&pid)?).contains(&task.id) {
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
    pub(super) fn resolve_to(&self, pid: &str, actor: &Actor, to: &str, parent: Option<&str>) -> Result<String> {
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

    pub(super) fn task_move(&mut self, caller: &Value, params: &Value, t: Transition, now: i64) -> Result<Value> {
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
    pub(super) fn report_done(&mut self, pid: &str, task: &Task, files: &[String], now: i64) -> Result<()> {
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
    pub(super) fn report(&mut self, pid: &str, task: &Task, text: String, now: i64) -> Result<()> {
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
    pub(super) fn assigner(&self, pid: &str, task: &Task) -> Actor {
        if task.from.as_str() == LILAD
            && let Some((anchor, _)) = &task.run
            && let Ok(Some(a)) = self.store.task(pid, anchor)
        {
            return a.from;
        }
        task.from.clone()
    }

    pub(super) fn check_cmd(&self, pid: &str) -> Option<String> {
        let c = self.projects[pid].team.settings.check.trim();
        (!c.is_empty()).then(|| c.to_owned())
    }

    /// The open run `task` is the step under way of: the run, its flow and the step.
    pub(super) fn current_step(&self, pid: &str, task: &Task) -> Option<(Run, Flow, usize)> {
        let (anchor, step) = task.run.clone()?;
        let run = self.store.run(pid, &anchor).ok()??;
        if run.state != RunState::Running || run.step != step {
            return None;
        }
        let f = self.projects[pid].team.flows.get(&run.flow)?.clone();
        (step < f.steps.len()).then_some((run, f, step))
    }

    /// What a member doing a step is told about the rest of its flow.
    pub(super) fn step_hint(&self, pid: &str, task: &Task) -> Option<StepHint> {
        if let Some(asked) = self.asked_hint(pid, task) {
            return Some(asked);
        }
        let (anchor, step) = task.run.as_ref()?;
        let run = self.store.run(pid, anchor).ok()??;
        let f = self.projects[pid].team.flows.get(&run.flow)?;
        let st = f.steps.get(*step)?;
        Some(if st.wants_verdict() { StepHint::Verdict } else { StepHint::HandOn(flow::later(f, *step, self.check_cmd(pid).as_deref())) })
    }

    /// A request sent again while its reporter's answer is awaited: nothing was
    /// started under it since the comment went out.
    fn asked_hint(&self, pid: &str, task: &Task) -> Option<StepHint> {
        if task.parent.is_some() {
            return None;
        }
        let ext = self.store.external_for(pid, &task.id).ok()??;
        let at = ext.asked_ms?;
        let tasks = self.store.tasks(pid).ok()?;
        if tasks.iter().any(|(t, _)| t.parent.as_deref() == Some(task.id.as_str()) && t.created_ms >= at) {
            return None;
        }
        let comment = self.store.events(pid, Some(&task.id), 200).ok()?.into_iter().rfind(|e| e.kind == "commented").map(|e| e.detail).unwrap_or_default();
        Some(StepHint::Asked { who: ext.author, comment })
    }

    /// Changes a task you assigned. Queued: rewritten in place. Already delivered:
    /// what was typed can't change, so `append` goes to the member as an update.
    pub(super) fn task_edit(&mut self, caller: &Value, params: &Value, now: i64) -> Result<Value> {
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
        let chain = self.store.ancestors(&pid, &updated.id)?.last().map(|t| t.id.clone()).unwrap_or_else(|| updated.id.clone());
        let dir = self.chain_dir(&pid, &chain);
        if dir != self.projects[&pid].cwd {
            write_task_file(&dir, &updated)?;
        }
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

    pub(super) fn task_show(&self, caller: &Value, params: &Value) -> Result<Value> {
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

    pub(super) fn task_ls(&self, caller: &Value, params: &Value) -> Result<Value> {
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

    pub(super) fn ask(&mut self, caller: &Value, params: &Value, now: i64) -> Result<Value> {
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

    pub(super) fn inbox(&self, caller: &Value, params: &Value) -> Result<Value> {
        let (pid, _) = self.who(caller)?;
        let all = params.get("all").and_then(Value::as_bool).unwrap_or(false);
        Ok(json!(self.store.inbox(&pid, all)?.iter().map(inbox_json).collect::<Vec<_>>()))
    }

    pub(super) fn answer(&mut self, caller: &Value, params: &Value, now: i64) -> Result<Value> {
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
            "merge" => self.merge_answer(&pid, &item, text, now)?,
            "triage" => self.triage_answer(&pid, &item, text, now)?,
            "draft" => self.draft_answer(&pid, &item, text, now)?,
            "permission" => self.permission_answer(&pid, &item, text, now)?,
            "budget" => self.budget_answer(&pid, &item, text, now)?,
            _ => {}
        }
        self.store.resolve_inbox(&pid, &item.id, text, now)?;
        self.store.event(&pid, item.task.as_deref(), "human", "answered", &format!("{}: {}", item.id, text.unwrap_or("dismissed")), now)?;
        self.pump(&pid, now);
        Ok(json!({"id": item.id, "resolved": true}))
    }

    pub(super) fn events(&self, caller: &Value, params: &Value) -> Result<Value> {
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
}
