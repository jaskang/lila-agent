//! Members' conversations for the window (architecture v3 §9): each runtime's
//! records translated into `lila_agent` items and kept in the store, read back
//! by seq. Claude Code's come from its transcript, followed as it grows.

use super::*;
use lila_agent::claude::Transcript;
use lila_agent::{Attachment, AttachmentKind, Caps, Command, Item};
use std::io::{Read, Seek, SeekFrom};

/// How far a member's transcript has been read.
pub(super) struct Tail {
    session: String,
    file: PathBuf,
    offset: u64,
    /// The end of the file when it stopped mid-line.
    partial: String,
    parser: Transcript,
}

/// What a runtime offers the window (P1: Claude Code's conversation; Codex keeps its terminal).
pub fn caps(runtime: lila_core::Runtime) -> Caps {
    match runtime {
        lila_core::Runtime::Claude => Caps { tui: true, gui: true, steer: true },
        // Through `codex app-server` (`lila host codex`): no TUI to switch to.
        lila_core::Runtime::Codex => Caps { tui: false, gui: true, steer: true },
        // Over ACP (`lila host omp`): words wait for the prompt under way.
        lila_core::Runtime::Omp => Caps { tui: false, gui: true, steer: false },
    }
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
fn mentioned_files(text: &str, cwd: &Path) -> Vec<Attachment> {
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
    /// Reads what running Claude Code members added to their transcripts since last time.
    pub(super) fn follow_transcripts(&mut self) {
        let src = self.live_sources();
        let mut wanted: Vec<(String, String, String, PathBuf)> = Vec::new();
        for (pid, proj) in &self.projects {
            for m in &proj.team.members {
                let live = &proj.live[&m.id];
                if m.role.file.runtime != lila_core::Runtime::Claude || live.terminal.is_none() {
                    continue;
                }
                let Some(session) = live.session.clone() else { continue };
                let cwd = live.cwd.as_ref().unwrap_or(&proj.cwd).to_string_lossy().to_string();
                wanted.push((pid.clone(), m.id.clone(), session.clone(), sessions::claude_file(&src, &cwd, &session)));
            }
        }
        for (pid, mid, session, file) in wanted {
            let key = (pid.clone(), mid.clone());
            // A new session (or file): read it from the start.
            if self.tails.get(&key).is_none_or(|t| t.session != session || t.file != file) {
                self.tails.insert(key.clone(), Tail { session: session.clone(), file: file.clone(), offset: 0, partial: String::new(), parser: Transcript::new() });
            }
            let tail = self.tails.get_mut(&key).unwrap();
            let items = read_more(tail);
            // What it really runs on, from the model that wrote its latest reply (also after a change made in its terminal).
            if let Some(model) = tail.parser.model().map(String::from)
                && let Some(live) = self.projects.get_mut(&pid).and_then(|p| p.live.get_mut(&mid))
                && live.config.model.as_deref() != Some(model.as_str())
            {
                live.config.model = Some(model);
                self.live_version += 1;
            }
            if items.is_empty() {
                continue;
            }
            // The reply being written is in the transcript now.
            if items.iter().any(|i| matches!(i.kind, lila_agent::ItemKind::AgentMessage { .. })) && self.writing.remove(&key).is_some() {
                self.live_version += 1;
            }
            let rows: Vec<(String, i64, String)> = items.iter().map(|i| (i.id.clone(), i.at_ms, serde_json::to_string(i).unwrap_or_default())).collect();
            if let Err(e) = self.store.put_items(&pid, &mid, &session, &rows) {
                self.say(format!("{pid}: conversation of {mid}: {e:#}"));
            }
        }
    }

    /// `conversation.read {member, after?, session?}`: the member's items after a seq
    /// (the latest when there are many), with what its runtime offers.
    pub(super) fn conversation_read(&mut self, caller: &Value, params: &Value) -> Result<Value> {
        let (pid, _) = self.who(caller)?;
        let mid = req(params, "member")?;
        let member = self.projects[&pid].team.member(mid).ok_or_else(|| anyhow!("no member {mid} in {pid}"))?;
        let caps = caps(member.role.file.runtime);
        let live = &self.projects[&pid].live[mid];
        let session = match s(params, "session") {
            Some(s) => Some(s.to_owned()),
            None => live.session.clone().or(self.store.last_conversation(&pid, mid)?),
        };
        let after = params["after"].as_i64().unwrap_or(0);
        let rows = match &session {
            Some(s) => self.store.items(&pid, mid, s, after, READ_LIMIT)?,
            None => Vec::new(),
        };
        let seq = rows.iter().map(|(n, _)| *n).max().unwrap_or(after);
        let items: Vec<Value> = rows.into_iter().filter_map(|(_, body)| serde_json::from_str(&body).ok()).collect();
        let writing = self.writing_of(&pid, mid);
        let config = self.config_json(&pid, mid);
        Ok(json!({"member": mid, "session": session, "seq": seq, "items": items, "caps": caps, "status": live.status.as_str(), "writing": writing, "config": config}))
    }

    /// `conversation.send {member, text, attachments?}`: the person's words to a member, as if typed in
    /// its terminal, with files (absolute paths) that go along. Not while it waits on a dialog there
    /// (the words would answer it).
    pub(super) fn conversation_send(&mut self, caller: &Value, params: &Value) -> Result<Value> {
        let (pid, _) = self.who(caller)?;
        let mid = req(params, "member")?;
        let text = s(params, "text").unwrap_or_default().trim().to_owned();
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
        if text.is_empty() && files.is_empty() {
            bail!("nothing to send");
        }
        let live = self.projects[&pid].live.get(mid).ok_or_else(|| anyhow!("no member {mid} in {pid}"))?;
        let terminal = live.terminal.clone().ok_or_else(|| anyhow!("{mid} is not running"))?;
        match live.status {
            AgentStatus::Idle | AgentStatus::Done | AgentStatus::Working => {}
            AgentStatus::Blocked => bail!("{mid} is waiting for an answer (its card, or its terminal): give it first"),
            other => bail!("{mid} can't take a message now ({})", other.as_str()),
        }
        // A hosted runtime has no `@` of its own to read a mention with: the files named in the words go along as files.
        if self.hosted(&pid, mid) {
            let cwd = self.member_cwd(&pid, mid);
            for f in mentioned_files(&text, &cwd) {
                if !files.iter().any(|a| a.path == f.path) {
                    files.push(f);
                }
            }
            if self.projects[&pid].team.member(mid).is_some_and(|m| m.role.file.runtime == lila_core::Runtime::Codex) {
                files.extend(self.called_skills(&pid, mid, &text));
            }
        }
        self.tell_with(&pid, mid, &terminal, &text, &files, None)?;
        Ok(json!({"sent": true}))
    }

    /// The folder a member works in now.
    pub(super) fn member_cwd(&self, pid: &str, mid: &str) -> PathBuf {
        let proj = &self.projects[pid];
        proj.live.get(mid).and_then(|l| l.cwd.clone()).unwrap_or_else(|| proj.cwd.clone())
    }

    /// The skills a Codex member's words call (`$name`), as the inputs Codex takes them by.
    fn called_skills(&self, pid: &str, mid: &str, text: &str) -> Vec<Attachment> {
        let skills = self.palette(pid, mid);
        let names: Vec<&str> = text.split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_' || c == '$' || c == ':')).filter_map(|w| w.strip_prefix('$')).collect();
        skills
            .into_iter()
            .filter(|e| e.kind == crate::commands::Kind::Skill && names.contains(&e.name.as_str()))
            .map(|e| Attachment { path: e.path.to_string_lossy().to_string(), name: e.name, kind: AttachmentKind::Skill })
            .collect()
    }

    /// The skills and custom commands a member's CLI finds on disk, for the folder it works in.
    fn palette(&self, pid: &str, mid: &str) -> Vec<crate::commands::Entry> {
        let Some(member) = self.projects[pid].team.member(mid) else { return Vec::new() };
        let (src, cwd) = (self.live_sources(), self.member_cwd(pid, mid));
        match member.role.file.runtime {
            lila_core::Runtime::Claude => crate::commands::claude(&src.claude, &cwd),
            lila_core::Runtime::Codex => {
                let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
                crate::commands::codex(&src.codex, &home, &cwd)
            }
            // omp lists its own over ACP.
            lila_core::Runtime::Omp => Vec::new(),
        }
    }

    /// `conversation.commands {member}`: what the `/` menu offers for this member's runtime (besides Lila's
    /// own actions): the built-in commands that finish without asking anything in a terminal, then its
    /// skills and custom commands. Each has `insert` (the words it puts in the box) or `action`
    /// (what `conversation.run` does for it).
    pub(super) fn conversation_commands(&mut self, caller: &Value, params: &Value) -> Result<Value> {
        let (pid, _) = self.who(caller)?;
        let mid = req(params, "member")?;
        let member = self.projects[&pid].team.member(mid).ok_or_else(|| anyhow!("no member {mid} in {pid}"))?;
        let runtime = member.role.file.runtime;
        let entry = |name: &str, description: &str, kind: &str, hint: Option<&str>, insert: Option<String>, action: Option<&str>| {
            json!({"name": name, "description": description, "kind": kind, "hint": hint, "insert": insert, "action": action})
        };
        let mut out: Vec<Value> = Vec::new();
        match runtime {
            // Claude Code's other built-ins ask in its terminal (/config, /mcp, /usage…), or change something
            // beyond this session (/model and /effort save the person's defaults, /clear starts a session Lila does not follow).
            lila_core::Runtime::Claude => out.push(entry("compact", "压缩上下文", "builtin", Some("[要保留的重点]"), Some("/compact ".into()), None)),
            lila_core::Runtime::Codex => out.push(entry("compact", "压缩上下文", "builtin", None, None, Some("compact"))),
            lila_core::Runtime::Omp => {
                for c in &self.projects[&pid].live[mid].commands {
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
        let sigil = if runtime == lila_core::Runtime::Codex { '$' } else { '/' };
        for e in self.palette(&pid, mid) {
            let kind = match e.kind {
                crate::commands::Kind::Skill => "skill",
                crate::commands::Kind::Command => "command",
            };
            out.push(entry(&e.name, &e.description, kind, e.hint.as_deref(), Some(format!("{sigil}{} ", e.name)), None));
        }
        Ok(json!({"commands": out}))
    }

    /// `conversation.files {member}`: the files the member can be pointed at with `@`: those in the folder it
    /// works in that git tracks or would (what `.gitignore` leaves out stays out), relative to it.
    pub(super) fn conversation_files(&mut self, caller: &Value, params: &Value) -> Result<Value> {
        let (pid, _) = self.who(caller)?;
        let mid = req(params, "member")?;
        if self.projects[&pid].team.member(mid).is_none() {
            bail!("no member {mid} in {pid}");
        }
        let cwd = self.member_cwd(&pid, mid);
        Ok(json!({"files": project_files(&cwd), "cwd": cwd}))
    }

    /// `conversation.model {member, model?, effort?}`: runs the member on another model or effort from now
    /// on, in this session only (a restart goes back to the role's). Where the runtime can do it without
    /// changing anything beyond the session: Codex (each turn carries them) and ACP agents (their
    /// `session/set_config_option`). Claude Code's `/model` and `/effort` save the person's defaults.
    pub(super) fn conversation_model(&mut self, caller: &Value, params: &Value) -> Result<Value> {
        let (pid, _) = self.who(caller)?;
        let mid = req(params, "member")?;
        let member = self.projects[&pid].team.member(mid).ok_or_else(|| anyhow!("no member {mid} in {pid}"))?;
        if member.role.file.runtime == lila_core::Runtime::Claude {
            bail!("Claude Code can't change its model for one session: its /model and /effort save the defaults for every session");
        }
        let (model, effort) = (s(params, "model"), s(params, "effort"));
        if model.is_none() && effort.is_none() {
            bail!("missing model or effort");
        }
        if let Some(m) = model {
            self.host_command(&pid, mid, &Command::SetModel { model: m.to_owned() })?;
        }
        if let Some(e) = effort {
            self.host_command(&pid, mid, &Command::SetEffort { effort: e.to_owned() })?;
        }
        Ok(json!({"set": true}))
    }

    /// `conversation.run {member, action}`: what a runtime does by a request of its protocol instead of words.
    pub(super) fn conversation_run(&mut self, caller: &Value, params: &Value) -> Result<Value> {
        let (pid, _) = self.who(caller)?;
        let mid = req(params, "member")?;
        let member = self.projects[&pid].team.member(mid).ok_or_else(|| anyhow!("no member {mid} in {pid}"))?;
        match (req(params, "action")?, member.role.file.runtime) {
            ("compact", lila_core::Runtime::Codex) => self.host_command(&pid, mid, &Command::Compact)?,
            (action, _) => bail!("{mid} has no {action} to run"),
        }
        Ok(json!({"ran": true}))
    }

    /// The model and effort for the box under the conversation: what it runs on now (else the role's), the
    /// role's, whether the person can change them here, and the choices when the runtime lists them.
    fn config_json(&self, pid: &str, mid: &str) -> Value {
        let proj = &self.projects[pid];
        let Some(member) = proj.team.member(mid) else { return Value::Null };
        let live = &proj.live[mid];
        let f = &member.role.file;
        let hosted = self.hosted(pid, mid);
        // omp changes only what it lists; Codex takes any of its models each turn.
        let switch = match f.runtime {
            lila_core::Runtime::Claude => false,
            lila_core::Runtime::Codex => hosted,
            lila_core::Runtime::Omp => !live.config.models.is_empty(),
        };
        json!({
            "model": live.config.model.clone().unwrap_or_else(|| f.model.clone()),
            "effort": live.config.effort.clone().or_else(|| f.effort.clone()),
            "default_model": f.model, "default_effort": f.effort,
            "switch": switch, "models": live.config.models, "efforts": live.config.efforts,
        })
    }

    /// `conversation.interrupt {member}`: stops what the member is doing this turn. A hosted
    /// runtime (Codex, omp) is told to; Claude Code's TUI gets Esc, its own stop key. Only
    /// while it works: Esc at an idle prompt would clear what is typed there.
    pub(super) fn conversation_interrupt(&mut self, caller: &Value, params: &Value) -> Result<Value> {
        let (pid, _) = self.who(caller)?;
        let mid = req(params, "member")?;
        let live = self.projects[&pid].live.get(mid).ok_or_else(|| anyhow!("no member {mid} in {pid}"))?;
        let terminal = live.terminal.clone().ok_or_else(|| anyhow!("{mid} is not running"))?;
        if live.status != AgentStatus::Working {
            bail!("{mid} is not working ({})", live.status.as_str());
        }
        if self.hosted(&pid, mid) {
            self.host_command(&pid, mid, &lila_agent::Command::Interrupt)?;
        } else {
            self.term.keys(&terminal, &["esc"])?;
        }
        Ok(json!({"interrupted": true}))
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
