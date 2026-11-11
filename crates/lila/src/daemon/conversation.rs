//! Threads' conversations for the window (architecture v3 §9): each runtime's
//! records translated into `lila_agent` items and kept in the store, read back
//! by seq. Claude Code's come from its transcript, followed as it grows.

use super::*;
use lila_agent::claude::Transcript;
use lila_agent::{Attachment, AttachmentKind, Command, Item};
use std::io::{Read, Seek, SeekFrom};

/// How far a thread's transcript has been read.
pub(super) struct Tail {
    session: String,
    file: PathBuf,
    offset: u64,
    /// The end of the file when it stopped mid-line.
    partial: String,
    parser: Transcript,
}

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
    // `@"a path with spaces"` and `@plain/path`.
    let mut words: Vec<&str> = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find('@') {
        rest = &rest[at..];
        match rest.strip_prefix("@\"").and_then(|r| r.split_once('"')) {
            Some((quoted, after)) => {
                words.push(quoted);
                rest = after;
            }
            None => {
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                words.push(&rest[1..end]);
                rest = &rest[end..];
            }
        }
    }
    for word in words {
        let rel = word.trim_end_matches([',', '.', ';', ':', ')', '?', '!']);
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

impl<T: Terminals> Daemon<T> {
    /// Reads what running Claude Code threads added to their transcripts since last time.
    pub(super) fn follow_transcripts(&mut self) {
        let src = self.live_sources();
        let mut wanted: Vec<(String, String, PathBuf)> = Vec::new();
        for t in self.store.live_threads().unwrap_or_default() {
            if t.engine != Runtime::Claude || !self.live.contains_key(&t.id) {
                continue;
            }
            let Some(session) = t.session.clone() else { continue };
            wanted.push((t.id.clone(), session.clone(), sessions::claude_file(&src, &t.cwd, &session)));
        }
        for (tid, session, file) in wanted {
            // A new session (or file): read it from the start.
            if self.tails.get(&tid).is_none_or(|t| t.session != session || t.file != file) {
                self.tails.insert(tid.clone(), Tail { session: session.clone(), file: file.clone(), offset: 0, partial: String::new(), parser: Transcript::new() });
            }
            let tail = self.tails.get_mut(&tid).unwrap();
            let items = read_more(tail);
            // What it really runs on, from the model that wrote its latest reply (also after a change made in its terminal).
            if let Some(model) = tail.parser.model().map(String::from)
                && let Some(live) = self.live.get_mut(&tid)
                && live.config.model.as_deref() != Some(model.as_str())
            {
                live.config.model = Some(model);
                self.mark(&tid);
            }
            if items.is_empty() {
                continue;
            }
            // The reply being written is in the transcript now.
            if items.iter().any(|i| matches!(i.kind, lila_agent::ItemKind::AgentMessage { .. })) {
                self.writing.remove(&tid);
            }
            let rows: Vec<(String, i64, String)> = items.iter().map(|i| (i.id.clone(), i.at_ms, serde_json::to_string(i).unwrap_or_default())).collect();
            let before = self.store.last_seq(&tid).unwrap_or(0);
            match self.store.put_items(&tid, &rows) {
                Ok(after) if after != before => {
                    let latest = items.iter().map(|i| i.at_ms).max().unwrap_or(0);
                    let _ = self.store.touch_thread(&tid, latest.min(now_ms()));
                    self.mark(&tid);
                }
                Ok(_) => {}
                Err(e) => self.say(format!("conversation of {tid}: {e:#}")),
            }
        }
    }

    /// `thread.read {thread, after?}`: the thread's items after a seq (the latest when there are
    /// many), the cards it waits on, the reply being written, and what its engine offers.
    pub(super) fn thread_read(&mut self, params: &Value, now: i64) -> Result<Value> {
        let t = self.thread_of(params)?;
        let after = params["after"].as_i64().unwrap_or(0);
        let limit = params["limit"].as_u64().map(|n| n as usize).unwrap_or(READ_LIMIT);
        let rows = self.store.items(&t.id, after, limit)?;
        let seq = rows.iter().map(|(n, _)| *n).max().unwrap_or(after);
        let items: Vec<Value> = rows.into_iter().filter_map(|(_, body)| serde_json::from_str(&body).ok()).collect();
        let cards: Vec<Value> = self.asking.iter().filter(|a| a.thread == t.id).map(super::permissions::card_json).collect();
        Ok(json!({
            "thread": self.thread_json(&t, now), "seq": seq, "items": items, "permissions": cards,
            "writing": self.writing_of(&t.id), "config": self.config_json(&t),
        }))
    }

    /// `thread.send {thread, text, attachments?}`: the person's words to a thread, with files (absolute
    /// paths) that go along. Words for a thread still starting wait and go when it takes them; a thread
    /// that waits on a card or a dialog takes none (they would answer it); an ended one must be resumed.
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

    /// Words from the person to `t`: now, or queued until its engine takes them.
    pub(super) fn send_to(&mut self, t: &Thread, text: &str, mut files: Vec<Attachment>, now: i64) -> Result<()> {
        let Some(live) = self.live.get(&t.id).filter(|_| t.terminal.is_some()) else {
            bail!("{} has ended; resume it first", t.id);
        };
        if self.merging.contains(&t.id) {
            bail!("{} is being merged", t.id);
        }
        match live.status {
            AgentStatus::Blocked => bail!("{} is waiting for an answer (its card, or its terminal): give it first", t.id),
            AgentStatus::Error => bail!("{} can't take a message now (error)", t.id),
            _ => {}
        }
        if self.asking.iter().any(|a| a.thread == t.id) && !t.engine.hosted() {
            bail!("{} is waiting for an answer to its card: give it first", t.id);
        }
        // A hosted engine has no `@` of its own to read a mention with: the files named in the words go along as files.
        if t.engine.hosted() {
            for f in mentioned(text, Path::new(&t.cwd)) {
                if !files.iter().any(|a| a.path == f.path) {
                    files.push(f);
                }
            }
            if t.engine == Runtime::Codex {
                files.extend(self.called_skills(t, text));
            }
        }
        let attachments = serde_json::to_string(&files)?;
        self.store.push_outbox(&t.id, text, &attachments, now)?;
        self.store.touch_thread(&t.id, now)?;
        self.mark(&t.id);
        self.pump(now);
        Ok(())
    }

    /// The skills a Codex thread's words call (`$name`), as the inputs Codex takes them by.
    fn called_skills(&self, t: &Thread, text: &str) -> Vec<Attachment> {
        let names: Vec<&str> = text.split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_' || c == '$' || c == ':')).filter_map(|w| w.strip_prefix('$')).collect();
        self.palette(t)
            .into_iter()
            .filter(|e| e.kind == crate::commands::Kind::Skill && names.contains(&e.name.as_str()))
            .map(|e| Attachment { path: e.path.to_string_lossy().to_string(), name: e.name, kind: AttachmentKind::Skill })
            .collect()
    }

    /// The skills and custom commands a thread's CLI finds on disk, for the folder it works in.
    fn palette(&self, t: &Thread) -> Vec<crate::commands::Entry> {
        let (src, cwd) = (self.live_sources(), PathBuf::from(&t.cwd));
        match t.engine {
            Runtime::Claude => crate::commands::claude(&src.claude, &cwd),
            Runtime::Codex => {
                let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
                crate::commands::codex(&src.codex, &home, &cwd)
            }
            // omp lists its own over ACP.
            Runtime::Omp => Vec::new(),
        }
    }

    /// `thread.commands {thread}`: what the `/` menu offers for this thread's engine (besides Lila's
    /// own actions): the built-in commands that finish without asking anything in a terminal, then its
    /// skills and custom commands. Each has `insert` (the words it puts in the box) or `action`
    /// (what `thread.run` does for it).
    pub(super) fn thread_commands(&mut self, params: &Value) -> Result<Value> {
        let t = self.thread_of(params)?;
        let entry = |name: &str, description: &str, kind: &str, hint: Option<&str>, insert: Option<String>, action: Option<&str>| {
            json!({"name": name, "description": description, "kind": kind, "hint": hint, "insert": insert, "action": action})
        };
        let mut out: Vec<Value> = Vec::new();
        match t.engine {
            // Claude Code's other built-ins ask in its terminal (/config, /mcp, /usage…), or change something
            // beyond this session (/model and /effort save the person's defaults, /clear starts a session Lila does not follow).
            Runtime::Claude => out.push(entry("compact", "压缩上下文", "builtin", Some("[要保留的重点]"), Some("/compact ".into()), None)),
            Runtime::Codex => out.push(entry("compact", "压缩上下文", "builtin", None, None, Some("compact"))),
            Runtime::Omp => {
                for c in self.live.get(&t.id).map(|l| l.commands.clone()).unwrap_or_default() {
                    let (kind, name) = match c.name.strip_prefix("skill:") {
                        Some(skill) => ("skill", skill.to_owned()),
                        None => ("builtin", c.name.clone()),
                    };
                    // ACP's generic "arguments" hint only says it takes some.
                    let hint = c.hint.as_deref().map(|h| if h == "arguments" { "[参数]" } else { h });
                    out.push(entry(&name, &c.description, kind, hint, Some(format!("/{} ", c.name)), None));
                }
            }
        }
        let sigil = if t.engine == Runtime::Codex { '$' } else { '/' };
        for e in self.palette(&t) {
            let kind = match e.kind {
                crate::commands::Kind::Skill => "skill",
                crate::commands::Kind::Command => "command",
            };
            out.push(entry(&e.name, &e.description, kind, e.hint.as_deref(), Some(format!("{sigil}{} ", e.name)), None));
        }
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
    /// on, and from now on when it is resumed. A running Codex or ACP thread changes at once (each turn
    /// carries them; ACP agents have `session/set_config_option`). Claude Code's `/model` and `/effort`
    /// save the person's defaults, so a running Claude thread is not changed: end it, change this, resume it.
    pub(super) fn thread_model(&mut self, params: &Value) -> Result<Value> {
        let t = self.thread_of(params)?;
        let (model, effort) = (s(params, "model"), s(params, "effort"));
        if model.is_none() && effort.is_none() {
            bail!("missing model or effort");
        }
        let running = self.live.contains_key(&t.id) && t.terminal.is_some();
        if running && t.engine == Runtime::Claude {
            bail!("Claude Code can't change its model for one session: its /model and /effort save the defaults for every session. End the thread, change this, and resume it");
        }
        if running {
            if let Some(m) = model {
                self.host_command(&t.id, &Command::SetModel { model: m.to_owned() })?;
            }
            if let Some(e) = effort {
                self.host_command(&t.id, &Command::SetEffort { effort: e.to_owned() })?;
            }
        }
        self.store.set_model(&t.id, model.unwrap_or(&t.model), effort.or(t.effort.as_deref()))?;
        self.mark(&t.id);
        Ok(json!({"set": true}))
    }

    /// `thread.run {thread, action}`: what an engine does by a request of its protocol instead of words.
    pub(super) fn thread_run(&mut self, params: &Value) -> Result<Value> {
        let t = self.thread_of(params)?;
        match (req(params, "action")?, t.engine) {
            ("compact", Runtime::Codex) => self.host_command(&t.id, &Command::Compact)?,
            (action, _) => bail!("{} has no {action} to run", t.id),
        }
        Ok(json!({"ran": true}))
    }

    /// The model and effort for the box under the conversation: what it runs on now (else the stored ones), the
    /// stored ones, whether the person can change them here, and the choices when the engine lists them.
    fn config_json(&self, t: &Thread) -> Value {
        let live = self.live.get(&t.id).filter(|_| t.terminal.is_some());
        let cfg = live.map(|l| l.config.clone()).unwrap_or_default();
        // omp changes only what it lists; Codex takes any of its models each turn; Claude Code can't (see `thread_model`).
        let switch = live.is_some()
            && match t.engine {
                Runtime::Claude => false,
                Runtime::Codex => true,
                Runtime::Omp => !cfg.models.is_empty(),
            };
        json!({
            "model": cfg.model.clone().unwrap_or_else(|| t.model.clone()),
            "effort": cfg.effort.clone().or_else(|| t.effort.clone()),
            "default_model": t.model, "default_effort": t.effort,
            "switch": switch, "models": cfg.models, "efforts": cfg.efforts,
        })
    }

    /// `thread.interrupt {thread}`: stops what the thread is doing this turn. A hosted
    /// engine (Codex, omp) is told to; Claude Code's TUI gets Esc, its own stop key. Only
    /// while it works: Esc at an idle prompt would clear what is typed there.
    pub(super) fn thread_interrupt(&mut self, params: &Value) -> Result<Value> {
        let t = self.thread_of(params)?;
        let terminal = t.terminal.clone().ok_or_else(|| anyhow!("{} is not running", t.id))?;
        let status = self.live.get(&t.id).map_or(AgentStatus::Gone, |l| l.status);
        if status != AgentStatus::Working {
            bail!("{} is not working ({})", t.id, status.as_str());
        }
        if t.engine.hosted() {
            self.host_command(&t.id, &Command::Interrupt)?;
        } else {
            self.term.keys(&terminal, &["esc"])?;
        }
        Ok(json!({"interrupted": true, "thread": t.id}))
    }
}

/// The items the lines appended since the last read give.
fn read_more(tail: &mut Tail) -> Vec<Item> {
    let Ok(mut f) = std::fs::File::open(&tail.file) else { return Vec::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len < tail.offset {
        // Rewritten from scratch: read it again.
        tail.offset = 0;
        tail.partial.clear();
        tail.parser = Transcript::new();
    }
    if len == tail.offset || f.seek(SeekFrom::Start(tail.offset)).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    if f.take(len - tail.offset).read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    tail.offset += bytes.len() as u64;
    tail.partial.push_str(&String::from_utf8_lossy(&bytes));
    let Some(cut) = tail.partial.rfind('\n') else { return Vec::new() };
    let complete: String = tail.partial.drain(..=cut).collect();
    let mut out: Vec<Item> = Vec::new();
    for line in complete.lines() {
        for item in tail.parser.feed(line) {
            // A tool call and its result in one read: keep the last version.
            out.retain(|i| i.id != item.id);
            out.push(item);
        }
    }
    out
}
