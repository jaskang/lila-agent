//! Projects and threads: adding and removing projects, their defaults, and a thread's life
//! (start, send, stop, resume, archive, tier).

use super::*;
use lila_core::runtime;
use super::jobs::{Begin, Plan};
use lila_store::NewThread;

impl<T: Terminals> Daemon<T> {
    // ---------------- who is asked about ----------------

    /// The registered project a request is about: `project`, else the one whose folder holds `cwd`.
    pub(super) fn resolve_project(&self, params: &Value) -> Result<String> {
        if let Some(p) = s(params, "project") {
            return if self.projects.contains_key(p) { Ok(p.to_owned()) } else { Err(anyhow!("unknown project {p}; add it with `lila project add`")) };
        }
        let cwd = s(params, "cwd").ok_or_else(|| anyhow!("give project (or cwd)"))?;
        let mut dir = Some(Path::new(cwd));
        while let Some(d) = dir {
            if let Some(p) = self.store.project_by_cwd(&d.to_string_lossy())? {
                return Ok(p.id);
            }
            dir = d.parent();
        }
        bail!("{cwd} is not in a Lila project; add it with `lila project add`")
    }

    pub(super) fn thread_of(&self, params: &Value) -> Result<Thread> {
        let id = req(params, "thread")?;
        self.store.thread(id)?.ok_or_else(|| anyhow!("no thread {id}"))
    }

    // ---------------- projects ----------------

    pub(super) fn project_json(&self, id: &str) -> Result<Value> {
        let p = self.projects.get(id).ok_or_else(|| anyhow!("unknown project {id}"))?;
        let threads = self.store.threads(id, false)?;
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for t in &threads {
            *counts.entry(self.state_of(t).0.as_str()).or_default() += 1;
        }
        let d = &p.config.defaults;
        Ok(json!({
            "id": id, "cwd": p.cwd, "title": p.config.title, "label": p.config.title.clone().unwrap_or_else(|| id.to_owned()), "error": p.error, "repo": p.repo,
            "defaults": {"model": d.model, "permission": d.permission.as_str()},
            "threads": threads.len(), "states": counts,
            "waiting": self.asking.iter().filter(|a| threads.iter().any(|t| t.id == a.thread)).count() + self.questions.iter().filter(|a| threads.iter().any(|t| t.id == a.thread)).count(),
        }))
    }

    /// `project.add {cwd, name?, title?}`: registers a folder as a project (and writes its `.lila/project.toml` if it has none).
    pub(super) fn project_add(&mut self, params: &Value, now: i64) -> Result<Value> {
        let cwd = req(params, "cwd")?;
        let root = std::fs::canonicalize(cwd).with_context(|| format!("{cwd} is not a folder"))?;
        let (cfg, made) = crate::project::ensure(&root, s(params, "name"), s(params, "title"))?;
        let root_s = root.to_string_lossy().to_string();
        self.store.upsert_project(&cfg.name, &root_s, now)?;
        self.load_project(&cfg.name, &root);
        self.say(format!("project {}: added ({})", cfg.name, root.display()));
        let mut v = self.project_json(&cfg.name)?;
        v["created_config"] = json!(made);
        Ok(v)
    }

    /// `project.remove {project}`: stops its threads and forgets everything recorded for it. The folder stays.
    pub(super) fn project_remove(&mut self, params: &Value, now: i64) -> Result<Value> {
        let pid = self.resolve_project(params)?;
        let mut stopped = Vec::new();
        for t in self.store.threads(&pid, true)? {
            if self.stop_thread(&t, "project removed", now) {
                stopped.push(t.id.clone());
            }
            self.failed_sends.remove(&t.id);
        }
        // Their checkpoints' refs are in the project's repository (a worktree's too: the refs are shared).
        let ids: Vec<String> = self.store.threads(&pid, true)?.into_iter().map(|t| t.id).collect();
        let dir = self.projects[&pid].cwd.clone();
        self.drop_checkpoints_later(&ids, dir)?;
        self.store.remove_project(&pid)?;
        self.projects.remove(&pid);
        self.dirty_projects.insert(pid.clone());
        self.live_version += 1;
        self.say(format!("project {pid}: removed"));
        Ok(json!({"project": pid, "removed": true, "stopped": stopped}))
    }

    /// `project.reload {project}`: reads `.lila/project.toml` again (the person edited it).
    pub(super) fn project_reload(&mut self, params: &Value) -> Result<Value> {
        let pid = self.resolve_project(params)?;
        let cwd = self.projects[&pid].cwd.clone();
        self.load_project(&pid, &cwd);
        self.project_json(&pid)
    }

    /// `project.set {project, title?, defaults?: {model?, permission?}}`: writes the project's
    /// defaults (an empty `model` clears it) and returns the project.
    pub(super) fn project_set(&mut self, params: &Value) -> Result<Value> {
        let pid = self.resolve_project(params)?;
        let p = &self.projects[&pid];
        let mut cfg = p.config.clone();
        let cwd = p.cwd.clone();
        if let Some(t) = params.get("title").and_then(Value::as_str) {
            cfg.title = Some(t.trim().to_owned()).filter(|t| !t.is_empty());
        }
        let d = &params["defaults"];
        if let Some(m) = d.get("model").and_then(Value::as_str) {
            cfg.defaults.model = m.trim().to_owned();
        }
        if let Some(t) = d.get("permission").and_then(Value::as_str) {
            cfg.defaults.permission = Tier::parse(t).ok_or_else(|| anyhow!("unknown permission {t:?} (read_only, confirm or full)"))?;
        }
        crate::project::save(&cwd, &cfg)?;
        self.load_project(&pid, &cwd);
        self.project_json(&pid)
    }

    // ---------------- threads ----------------

    /// `thread.new {project|cwd, text?, attachments?, title?, model?, permission?, worktree?}`: a new thread
    /// in a project, on what the project's defaults say unless told otherwise, and omp started. The
    /// first message waits for omp to take it. With `worktree`, the thread gets a worktree of its own
    /// first (`worktree.rs`), made off the loop: the answer comes when the thread has started.
    pub(super) fn thread_new(&mut self, params: &Value, now: i64) -> Result<Begin> {
        let pid = self.resolve_project(params)?;
        let proj = &self.projects[&pid];
        let defaults = proj.config.defaults.clone();
        let model = s(params, "model").map(String::from).unwrap_or(defaults.model);
        let tier = match s(params, "permission") {
            Some(t) => Tier::parse(t).ok_or_else(|| anyhow!("unknown permission {t:?} (read_only, confirm or full)"))?,
            None => defaults.permission,
        };
        let cwd = proj.cwd.to_string_lossy().to_string();
        if !proj.cwd.is_dir() {
            bail!("the project folder {cwd} is gone");
        }
        let main = proj.cwd.clone();
        // The program first, so a missing one doesn't leave half a thread behind.
        self.omp_env()?;
        let text = s(params, "text").unwrap_or_default().trim().to_owned();
        let files = self.attachments_of(params)?;
        let title = s(params, "title").map(String::from).unwrap_or_else(|| title_of(&text));
        let id = lila_core::new_thread_id(uuid::Uuid::new_v4().as_u128() as u32);
        let plan = Plan { new: NewThread { id, project: pid, title, model, effort: None, tier, cwd, worktree: None }, text, files };
        if params["worktree"].as_bool().unwrap_or(false) {
            return Ok(Begin::Job(self.worktree_job(plan, main)));
        }
        Ok(Begin::Done(self.start_new(plan, now)?))
    }

    /// Makes the thread of a plan and starts omp (in the worktree, when it has one).
    pub(super) fn start_new(&mut self, plan: Plan, now: i64) -> Result<Value> {
        let Plan { new, text, files } = plan;
        let id = new.id.clone();
        let t = self.store.create_thread(new, now)?;
        if let Err(e) = self.start_thread(&t, now) {
            let _ = self.store.remove_thread(&id);
            return Err(e.context("start omp"));
        }
        if !text.is_empty() || !files.is_empty() {
            let mut files = files;
            files.extend(mentioned_files_of(&text, Path::new(&t.cwd), &files));
            self.store.push_outbox(&id, &text, &serde_json::to_string(&files)?, now)?;
        }
        self.pump(now);
        self.refresh_changes(&id);
        let t = self.store.thread(&id)?.expect("just made");
        Ok(self.thread_json(&t))
    }

    /// Starts a thread's omp in a terminal of its own.
    pub(super) fn start_thread(&mut self, t: &Thread, now: i64) -> Result<String> {
        let mut launch = runtime::launch(&t.model, &self.paths.omp_sessions(&t.id));
        let (program, mut env) = self.omp_env()?;
        // Driven over omp's RPC mode by `lila host omp`, which starts omp.
        launch.argv[0] = self.lila_bin.to_string_lossy().to_string();
        launch.argv.splice(3..3, ["--program".to_owned(), program.to_string_lossy().to_string()]);
        env.insert("LILA_PROJECT".into(), t.project.clone());
        env.insert("LILA_THREAD".into(), t.id.clone());
        env.insert("LILA_ROOT".into(), t.cwd.clone());
        if let Some(h) = std::env::var_os("LILA_HOME") {
            env.insert("LILA_HOME".into(), h.to_string_lossy().to_string());
        }
        let addr = self.term.run(json!({
            "argv": launch.argv,
            "cwd": t.cwd,
            "env": env,
            "labels": {"lila.project": t.project, "lila.thread": t.id},
            "name": terminal_name(&t.id),
            "no_parent": true,
            "no_notify": true,
        }))?;
        self.store.set_running(&t.id, &addr, now)?;
        self.live.insert(t.id.clone(), Live::new(AgentStatus::Unknown, now));
        let shown: Vec<String> = launch.argv.iter().map(|a| if a.chars().count() > 80 { format!("{}…", a.chars().take(40).collect::<String>()) } else { a.clone() }).collect();
        self.store.event(&t.id, "lilad", "started", &format!("in {addr}: {}", shown.join(" ")), now)?;
        self.say(format!("{}: started thread {} in {addr}", t.project, t.id));
        self.mark(&t.id);
        Ok(addr)
    }

    /// Kills a thread's terminal and marks it ended. Returns whether there was a host running.
    pub(super) fn stop_thread(&mut self, t: &Thread, why: &str, now: i64) -> bool {
        let Some(terminal) = t.terminal.clone() else { return false };
        if let Err(e) = self.term.kill(&terminal) {
            self.say(format!("{}: kill {} ({terminal}): {e:#}", t.project, t.id));
        }
        self.thread_gone(&t.id, why, now);
        true
    }

    /// `thread.stop {thread}`: ends omp. The conversation stays and `thread.resume` goes on with it.
    pub(super) fn thread_stop(&mut self, params: &Value, now: i64) -> Result<Value> {
        let t = self.thread_of(params)?;
        if !self.stop_thread(&t, "stopped by the person", now) {
            bail!("{} is not running", t.id);
        }
        let t = self.thread_of(params)?;
        Ok(self.thread_json(&t))
    }

    /// `thread.resume {thread, text?, model?, permission?}`: starts an ended thread's omp on the session
    /// it had (the last in the thread's folder of omp sessions); a thread from before that folder, or one that never got
    /// as far as a session, starts a new omp session and keeps its conversation.
    pub(super) fn thread_resume(&mut self, params: &Value, now: i64) -> Result<Value> {
        let mut t = self.thread_of(params)?;
        if let Some(old) = &t.legacy {
            bail!("{} ran on {old}, which Lila no longer runs; its conversation stays, start a new thread to go on", t.id);
        }
        if t.terminal.is_some() {
            bail!("{} is running", t.id);
        }
        if t.merged_ms.is_some() {
            bail!("{} was merged and its worktree removed; start a new thread", t.id);
        }
        if self.merging.contains(&t.id) {
            bail!("{} is being merged", t.id);
        }
        if self.restoring.contains(&t.id) {
            bail!("{} is having its files put back", t.id);
        }
        if let Some(p) = self.projects.get(&t.project)
            && !p.cwd.is_dir()
        {
            bail!("the project folder {} is gone", p.cwd.display());
        }
        if t.worktree && !Path::new(&t.cwd).is_dir() {
            bail!("the worktree {} is gone", t.cwd);
        }
        self.omp_env()?;
        if let Some(m) = s(params, "model") {
            self.store.set_model(&t.id, m, t.effort.as_deref())?;
        }
        if let Some(p) = s(params, "permission") {
            self.store.set_tier(&t.id, Tier::parse(p).ok_or_else(|| anyhow!("unknown permission {p:?}"))?)?;
        }
        t = self.thread_of(params)?;
        let dir = self.paths.omp_sessions(&t.id);
        let detail = if crate::host::has_session(&dir) { format!("goes on with the omp session in {}", dir.display()) } else { format!("no omp session in {}: a new one", dir.display()) };
        self.start_thread(&t, now)?;
        self.store.event(&t.id, "human", "resumed", &detail, now)?;
        let text = s(params, "text").unwrap_or_default().trim().to_owned();
        let files = self.attachments_of(params)?;
        if !text.is_empty() || !files.is_empty() {
            self.store.push_outbox(&t.id, &text, &serde_json::to_string(&files)?, now)?;
        }
        self.pump(now);
        let t = self.thread_of(params)?;
        Ok(self.thread_json(&t))
    }

    /// `thread.archive {thread}` / `thread.unarchive`: an archived thread is left out of lists (and its
    /// omp ended).
    pub(super) fn thread_archive(&mut self, params: &Value, archive: bool, now: i64) -> Result<Value> {
        let t = self.thread_of(params)?;
        if archive {
            self.stop_thread(&t, "archived", now);
        }
        self.store.set_archived(&t.id, archive)?;
        self.mark(&t.id);
        if !archive {
            self.refresh_changes(&t.id);
        }
        let t = self.thread_of(params)?;
        Ok(self.thread_json(&t))
    }

    /// `thread.rename {thread, title}`.
    pub(super) fn thread_rename(&mut self, params: &Value) -> Result<Value> {
        let t = self.thread_of(params)?;
        let title = req(params, "title")?.trim();
        self.store.set_title(&t.id, title)?;
        self.mark(&t.id);
        let t = self.thread_of(params)?;
        Ok(self.thread_json(&t))
    }

    /// `thread.permission {thread, permission}`: another tier from now on. Takes effect at once: lilad answers
    /// by it. Requests already asked stay as they are.
    pub(super) fn thread_permission(&mut self, params: &Value, now: i64) -> Result<Value> {
        let t = self.thread_of(params)?;
        let p = req(params, "permission")?;
        let tier = Tier::parse(p).ok_or_else(|| anyhow!("unknown permission {p:?} (read_only, confirm or full)"))?;
        self.store.set_tier(&t.id, tier)?;
        self.store.event(&t.id, "human", "permission", &format!("{} → {}", t.tier.as_str(), tier.as_str()), now)?;
        // Read-only is the tier where nothing is let through: what was allowed "always" is forgotten, not kept for later.
        if tier == Tier::ReadOnly {
            let dropped = self.store.clear_rules(&t.id)?;
            if !dropped.is_empty() {
                self.store.event(&t.id, "lilad", "rules_dropped", &dropped.iter().map(|r| r.key.clone()).collect::<Vec<_>>().join(", "), now)?;
            }
        }
        self.mark(&t.id);
        let t = self.thread_of(params)?;
        Ok(self.thread_json(&t))
    }

    /// `thread.list {project|cwd?, archived?}`: a project's threads, the most recently active first (every project's when none is named).
    pub(super) fn thread_list(&self, params: &Value) -> Result<Value> {
        let archived = params["archived"].as_bool().unwrap_or(false);
        let projects: Vec<String> = if s(params, "project").is_some() || s(params, "cwd").is_some() { vec![self.resolve_project(params)?] } else { self.projects.keys().cloned().collect() };
        let mut out: Vec<Thread> = Vec::new();
        for p in projects {
            out.extend(self.store.threads(&p, archived)?);
        }
        out.sort_by(|a, b| b.active_ms.cmp(&a.active_ms).then(a.id.cmp(&b.id)));
        Ok(Value::Array(out.iter().map(|t| self.thread_json(t)).collect()))
    }

    /// `thread.log {thread, limit?}`: what lilad did on its own for the thread: starts and ends, answers it gave
    /// by the tier, cards it opened.
    pub(super) fn thread_log(&self, params: &Value) -> Result<Value> {
        let t = self.thread_of(params)?;
        let limit = params["limit"].as_u64().unwrap_or(50) as usize;
        Ok(json!(self.store.events(&t.id, limit)?.iter().map(|e| json!({"at_ms": e.at_ms, "actor": e.actor, "kind": e.kind, "detail": e.detail})).collect::<Vec<_>>()))
    }

    // ---------------- delivery ----------------

    /// Everything that can move now: words waiting for threads that take them.
    pub fn pump(&mut self, now: i64) {
        for tid in self.store.outbox_threads().unwrap_or_default() {
            self.deliver(&tid, now);
        }
    }

    /// The words waiting for a thread, as one message, once omp takes input.
    fn deliver(&mut self, tid: &str, now: i64) {
        self.deliver_with(tid, now, false);
    }

    /// `checkpointed`: the checkpoint for the turn these words begin has just been taken (`checkpoints`).
    pub(super) fn deliver_with(&mut self, tid: &str, now: i64, checkpointed: bool) {
        let Ok(Some(t)) = self.store.thread(tid) else {
            for m in self.store.outbox(tid).unwrap_or_default() {
                let _ = self.store.pop_outbox(m.id);
            }
            return;
        };
        let (Some(_), Some(live)) = (t.terminal.as_ref(), self.live.get(tid)) else { return };
        let status = live.status;
        // omp takes words one turn at a time: they go when it is free, which is when their turn begins.
        if self.merging.contains(tid) || self.restoring.contains(tid) || self.snapshotting.contains(tid) || !status.is_free() || !self.hosts.contains_key(tid) {
            return;
        }
        let msgs = self.store.outbox(tid).unwrap_or_default();
        // Notes that wait for a message go in front of it; they never go alone.
        let (notes, words): (Vec<_>, Vec<_>) = msgs.iter().partition(|m| m.hold);
        if words.is_empty() {
            return;
        }
        let said = words.iter().map(|m| m.text.as_str()).filter(|x| !x.is_empty()).collect::<Vec<_>>().join("\n\n");
        // These words begin a turn: the folder is saved first, and they go when that is done.
        if !checkpointed && let Some(job) = self.checkpoint_job(&t, said.clone(), now) {
            self.snapshotting.insert(tid.to_owned());
            self.jobs.push(job);
            return;
        }
        let text = notes.iter().map(|m| m.text.as_str()).chain(std::iter::once(said.as_str())).filter(|x| !x.is_empty()).collect::<Vec<_>>().join("\n\n");
        let files: Vec<lila_agent::Attachment> = words.iter().flat_map(|m| serde_json::from_str::<Vec<lila_agent::Attachment>>(&m.attachments).unwrap_or_default()).collect();
        match self.tell_with(tid, &text, &files) {
            Ok(()) => {
                for m in &msgs {
                    let _ = self.store.pop_outbox(m.id);
                }
                self.failed_sends.remove(tid);
                if let Some(l) = self.live.get_mut(tid) {
                    l.free_since = now;
                }
                let _ = self.store.touch_thread(tid, now);
                self.mark(tid);
            }
            Err(e) => {
                let e = format!("{e:#}");
                if self.failed_sends.get(tid) != Some(&e) {
                    self.say(format!("{tid}: not delivered yet: {e}"));
                    self.failed_sends.insert(tid.to_owned(), e);
                }
            }
        }
    }
}

/// A thread's title from its first message: its first line, cut.
fn title_of(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    if line.is_empty() {
        return "新会话".into();
    }
    let cut: String = line.chars().take(60).collect();
    if cut.chars().count() < line.chars().count() { format!("{cut}…") } else { cut }
}

/// The files `@relative/path` names in the words, that are not among `have` already.
fn mentioned_files_of(text: &str, cwd: &Path, have: &[lila_agent::Attachment]) -> Vec<lila_agent::Attachment> {
    conversation::mentioned(text, cwd).into_iter().filter(|f| !have.iter().any(|a| a.path == f.path)).collect()
}

#[cfg(test)]
mod title_tests {
    use super::title_of;

    #[test]
    fn a_title_is_the_first_line_cut() {
        assert_eq!(title_of("\n  Fix the login\nmore"), "Fix the login");
        assert_eq!(title_of(""), "新会话");
        assert_eq!(title_of(&"字".repeat(80)).chars().count(), 61);
    }
}
