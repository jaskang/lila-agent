//! Members' conversations for the window (architecture v3 §9): each runtime's
//! records translated into `lila_agent` items and kept in the store, read back
//! by seq. Claude Code's come from its transcript, followed as it grows.

use super::*;
use lila_agent::claude::Transcript;
use lila_agent::{Caps, Item};
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
    }
}

/// Most items a read returns: the latest ones.
const READ_LIMIT: usize = 400;

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
        Ok(json!({"member": mid, "session": session, "seq": seq, "items": items, "caps": caps, "status": live.status.as_str(), "writing": writing}))
    }

    /// `conversation.send {member, text}`: the person's words to a member, as if typed in
    /// its terminal. Not while it waits on a dialog there (the words would answer it).
    pub(super) fn conversation_send(&mut self, caller: &Value, params: &Value) -> Result<Value> {
        let (pid, _) = self.who(caller)?;
        let mid = req(params, "member")?;
        let text = req(params, "text")?.trim().to_owned();
        if text.is_empty() {
            bail!("nothing to send");
        }
        let live = self.projects[&pid].live.get(mid).ok_or_else(|| anyhow!("no member {mid} in {pid}"))?;
        let terminal = live.terminal.clone().ok_or_else(|| anyhow!("{mid} is not running"))?;
        match live.status {
            AgentStatus::Idle | AgentStatus::Done | AgentStatus::Working => {}
            AgentStatus::Blocked => bail!("{mid} is waiting for an answer (its card, or its terminal): give it first"),
            other => bail!("{mid} can't take a message now ({})", other.as_str()),
        }
        self.tell(&pid, mid, &terminal, &text, None)?;
        Ok(json!({"sent": true}))
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
