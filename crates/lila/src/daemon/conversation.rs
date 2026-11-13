//! Threads' conversations for the window (architecture v3 §9): what omp does, translated by its host into
//! `lila_agent` items, kept in the store and read back by seq; and the words that go to it.

use super::*;
use lila_agent::{Attachment, Command};

/// Most items a read returns: the latest ones.
const READ_LIMIT: usize = 400;
/// Most files a message takes along.
const MAX_FILES: usize = 20;
/// Most files `@` lists.
const MAX_LISTED: usize = 20_000;

/// The files in `cwd` that git tracks or would track (what `.gitignore` leaves out stays out), relative to it.
/// None outside a git repository.
pub fn project_files(cwd: &Path) -> Vec<String> {
    let Ok(out) = std::process::Command::new("git").arg("-C").arg(cwd).args(["ls-files", "-z", "--cached", "--others", "--exclude-standard"]).output() else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    let mut files: Vec<String> = String::from_utf8_lossy(&out.stdout).split('\0').filter(|f| !f.is_empty()).map(String::from).collect();
    files.sort();
    files.dedup();
    files.retain(|f| cwd.join(f).is_file());
    files.truncate(MAX_LISTED);
    files
}

/// The files `@relative/path` points at in the words (those that exist in `cwd`), as attachments.
pub(super) fn mentioned(text: &str, cwd: &Path) -> Vec<Attachment> {
    let mut out: Vec<Attachment> = Vec::new();
    for rel in lila_agent::at_mentions(text) {
        let path = cwd.join(rel);
        if !rel.is_empty() && !rel.starts_with('/') && !rel.contains("..") && path.is_file() {
            let a = Attachment { name: rel.to_owned(), ..Attachment::file(&path.to_string_lossy()) };
            if !out.iter().any(|o| o.path == a.path) {
                out.push(a);
            }
        }
    }
    out
}

impl<A: Agents> Daemon<A> {
    /// `thread.read {thread, after?}`: the thread's items after a seq (the latest when there are
    /// many), the cards it waits on (`permissions`, `questions`), and what omp offers.
    pub(super) fn thread_read(&mut self, params: &Value) -> Result<Value> {
        let t = self.thread_of(params)?;
        let after = params["after"].as_i64().unwrap_or(0);
        let limit = params["limit"].as_u64().map(|n| n as usize).unwrap_or(READ_LIMIT);
        let rows = self.store.items(&t.id, after, limit)?;
        let seq = rows.iter().map(|(n, _)| *n).max().unwrap_or(after);
        let items: Vec<Value> = rows.into_iter().filter_map(|(_, body)| serde_json::from_str(&body).ok()).collect();
        let cards: Vec<Value> = self.asking.iter().filter(|a| a.thread == t.id).map(super::permissions::card_json).collect();
        let questions: Vec<Value> = self.questions.iter().filter(|a| a.thread == t.id).map(super::questions::card_json).collect();
        Ok(json!({
            "thread": self.thread_json(&t), "seq": seq, "items": items, "permissions": cards, "questions": questions,
            "config": self.config_json(&t), "checkpoints": self.checkpoint_rows(&t)?, "rules": self.rules_json(&t.id),
        }))
    }

    /// `thread.send {thread, text, attachments?}`: the person's words to a thread, with files (absolute
    /// paths) that go along. Words for a thread still starting wait and go when it takes them; a thread
    /// that waits on a card takes none; an ended one must be resumed.
    pub(super) fn thread_send(&mut self, params: &Value, now: i64) -> Result<Value> {
        let t = self.thread_of(params)?;
        let text = s(params, "text").unwrap_or_default().trim().to_owned();
        let files = self.attachments_of(params)?;
        if text.is_empty() && files.is_empty() {
            bail!("nothing to send");
        }
        self.send_to(&t, &text, files, now)?;
        Ok(json!({"sent": true, "thread": t.id}))
    }

    pub(super) fn attachments_of(&self, params: &Value) -> Result<Vec<Attachment>> {
        let mut files: Vec<Attachment> = Vec::new();
        for path in params["attachments"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            let p = Path::new(path);
            if !p.is_absolute() || !p.is_file() {
                bail!("{path} is not a file");
            }
            files.push(Attachment::file(path));
        }
        if files.len() > MAX_FILES {
            bail!("at most {MAX_FILES} files at once");
        }
        Ok(files)
    }

    /// Words from the person to `t`: now, or queued until omp takes them.
    pub(super) fn send_to(&mut self, t: &Thread, text: &str, mut files: Vec<Attachment>, now: i64) -> Result<()> {
        let Some(live) = self.live.get(&t.id) else {
            bail!("{} has ended; resume it first", t.id);
        };
        if self.merging.contains(&t.id) {
            bail!("{} is being merged", t.id);
        }
        if self.restoring.contains(&t.id) {
            bail!("{} is having its files put back", t.id);
        }
        if live.status == AgentStatus::Blocked || self.asking.iter().any(|a| a.thread == t.id) || self.questions.iter().any(|a| a.thread == t.id) {
            bail!("{} is waiting for an answer to its card: give it first", t.id);
        }
        if live.status == AgentStatus::Error {
            bail!("{} can't take a message now (error)", t.id);
        }
        // omp has no `@` of its own to read a mention with: the files named in the words go along as files.
        for f in mentioned(text, Path::new(&t.cwd)) {
            if !files.iter().any(|a| a.path == f.path) {
                files.push(f);
            }
        }
        let attachments = serde_json::to_string(&files)?;
        self.store.push_outbox(&t.id, text, &attachments, now)?;
        self.store.touch_thread(&t.id, now)?;
        self.mark(&t.id);
        self.pump(now);
        Ok(())
    }

    /// `thread.commands {thread}`: what the `/` menu offers (besides Lila's own actions): the commands and skills omp
    /// declared for the thread, each with `insert`, the words it puts in the box.
    pub(super) fn thread_commands(&mut self, params: &Value) -> Result<Value> {
        let t = self.thread_of(params)?;
        let out: Vec<Value> = self
            .live
            .get(&t.id)
            .map(|l| l.commands.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|c| {
                let (kind, name) = match c.name.strip_prefix("skill:") {
                    Some(skill) => ("skill", skill.to_owned()),
                    None => ("builtin", c.name.clone()),
                };
                // omp's generic "arguments" hint only says it takes some.
                let hint = c.hint.as_deref().map(|h| if h == "arguments" { "[参数]" } else { h });
                json!({"name": name, "description": c.description, "kind": kind, "hint": hint, "insert": format!("/{} ", c.name)})
            })
            .collect();
        Ok(json!({"commands": out}))
    }

    /// `thread.files {thread}`: the files the thread can be pointed at with `@`: those in the folder it
    /// works in that git tracks or would (what `.gitignore` leaves out stays out), relative to it.
    pub(super) fn thread_files(&mut self, params: &Value) -> Result<Value> {
        // A thread's folder; before the thread exists (the window's new-session box), the project's.
        let cwd = if s(params, "thread").is_some() { self.thread_of(params)?.cwd } else { self.projects[&self.resolve_project(params)?].cwd.to_string_lossy().to_string() };
        Ok(json!({"files": project_files(Path::new(&cwd)), "cwd": cwd}))
    }

    /// `thread.file {thread, path}`: a file of the thread's folder to look at: `{path, text}`, or `binary` or `too_large` instead, and its `size`.
    /// The path is relative to the folder and stays inside it (`changes::read_file`).
    pub(super) fn thread_file(&mut self, params: &Value) -> Result<Value> {
        let t = self.thread_of(params)?;
        crate::changes::read_file(Path::new(&t.cwd), req(params, "path")?)
    }

    /// `thread.model {thread, model?, effort?}`: the thread runs on another model or effort from now
    /// on, and from now on when it is resumed. A running thread changes at once (`set_model`, `set_thinking_level`).
    pub(super) fn thread_model(&mut self, params: &Value) -> Result<Value> {
        let t = self.thread_of(params)?;
        let (model, effort) = (s(params, "model"), s(params, "effort"));
        if model.is_none() && effort.is_none() {
            bail!("missing model or effort");
        }
        if self.live.contains_key(&t.id) {
            if let Some(m) = model {
                self.agent_command(&t.id, &Command::SetModel { model: m.to_owned() })?;
            }
            if let Some(e) = effort {
                self.agent_command(&t.id, &Command::SetEffort { effort: e.to_owned() })?;
            }
        }
        self.store.set_model(&t.id, model.unwrap_or(&t.model), effort.or(t.effort.as_deref()))?;
        self.mark(&t.id);
        Ok(json!({"set": true}))
    }

    /// The model and effort for the box under the conversation: what it runs on now (else the stored ones), the
    /// stored ones, whether the person can change them here (omp changes only what it lists), and the choices.
    fn config_json(&self, t: &Thread) -> Value {
        let live = self.live.get(&t.id);
        let cfg = live.map(|l| l.config.clone()).unwrap_or_default();
        // The thread's own omp names the choices once it has; until then, those omp was last asked for.
        let models = if cfg.models.is_empty() { self.models.as_ref().map(|(_, m)| m.clone()).unwrap_or_default() } else { cfg.models.clone() };
        json!({
            "model": cfg.model.clone().unwrap_or_else(|| t.model.clone()),
            "effort": cfg.effort.clone().or_else(|| t.effort.clone()),
            "default_model": t.model, "default_effort": t.effort,
            "switch": live.is_some() && !models.is_empty(), "models": models, "efforts": cfg.efforts,
        })
    }

    /// `thread.interrupt {thread}`: stops what the thread is doing this turn. Only while it works.
    pub(super) fn thread_interrupt(&mut self, params: &Value) -> Result<Value> {
        let t = self.thread_of(params)?;
        if !self.live.contains_key(&t.id) {
            bail!("{} is not running", t.id);
        }
        let status = self.live.get(&t.id).map_or(AgentStatus::Gone, |l| l.status);
        if status != AgentStatus::Working {
            bail!("{} is not working ({})", t.id, status.as_str());
        }
        self.agent_command(&t.id, &Command::Interrupt)?;
        Ok(json!({"interrupted": true, "thread": t.id}))
    }
}
