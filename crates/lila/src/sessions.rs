//! The CLI sessions Lila starts: their ids, where the CLIs record them, and how
//! big their context is now (the input of their latest turn).
//!
//! Claude Code takes the id Lila gives it (`--session-id`) and writes
//! `<claude>/projects/<folder>/<id>.jsonl`. Codex picks its own; Lila finds the
//! rollout by folder and by the identity line of its system prompt.

use crate::usage::{Sources, claude_folder, identity};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A random (version 4) UUID, as Claude Code's `--session-id` wants.
pub fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Where Claude Code records session `id` started in `cwd`.
pub fn claude_file(src: &Sources, cwd: &str, id: &str) -> PathBuf {
    src.claude.join("projects").join(claude_folder(cwd)).join(format!("{id}.jsonl"))
}

/// Whether a session's record (`file`) was made in `cwd`: a session goes on
/// only in the folder it started in (Claude Code looks for it by folder).
pub fn recorded_in(file: &str, runtime: &str, cwd: &str) -> bool {
    let path = Path::new(file);
    if runtime == "codex" {
        let head = read_head(path, 64 * 1024);
        return head.lines().next().and_then(|l| serde_json::from_str::<Value>(l).ok()).is_some_and(|v| v["payload"]["cwd"].as_str() == Some(cwd));
    }
    path.parent().and_then(|d| d.file_name()).is_some_and(|d| d.to_string_lossy() == crate::usage::claude_folder(cwd))
}

/// The Codex rollout of `member`'s session in `cwd` started at or after `after_ms`:
/// its id and file. The newest such one wins.
pub fn find_codex(src: &Sources, project: &str, member: &str, cwd: &str, after_ms: i64) -> Option<(String, PathBuf)> {
    let after = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(after_ms.saturating_sub(5_000).max(0) as u64);
    let mut found: Vec<(SystemTime, String, PathBuf)> = Vec::new();
    // Rollouts sit in sessions/YYYY/MM/DD/: only folders touched since the start can hold it.
    let modified = |e: &walkdir::DirEntry| e.metadata().ok().and_then(|m| m.modified().ok()).unwrap_or(SystemTime::UNIX_EPOCH);
    let walk = walkdir::WalkDir::new(src.codex.join("sessions")).max_depth(4).into_iter().filter_entry(|e| e.depth() == 0 || modified(e) >= after);
    for e in walk.filter_map(Result::ok).filter(|e| e.file_type().is_file()) {
        let (p, modified) = (e.path().to_path_buf(), modified(&e));
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if !(name.starts_with("rollout-") && name.ends_with(".jsonl")) {
            continue;
        }
        let head = read_head(&p, 256 * 1024);
        let mut lines = head.lines();
        let Some(meta_line) = lines.next().and_then(|l| serde_json::from_str::<Value>(l).ok()) else { continue };
        let payload = &meta_line["payload"];
        if payload["cwd"].as_str() != Some(cwd) {
            continue;
        }
        let Some(id) = payload["id"].as_str().or(payload["session_id"].as_str()) else { continue };
        let who = head.find("## Your team (Lila)").and_then(|at| identity(&unescape(&head[at..])));
        if who.is_some_and(|(p, m)| p == project && m == member) {
            found.push((modified, id.to_owned(), p));
        }
    }
    found.sort();
    found.pop().map(|(_, id, p)| (id, p))
}

/// JSON string escapes undone, enough to read an identity line out of raw JSONL.
fn unescape(s: &str) -> String {
    s.replace("\\n", "\n").replace("\\`", "`").replace("\\\"", "\"")
}

fn read_head(p: &Path, max: usize) -> String {
    let mut buf = Vec::new();
    if let Ok(f) = std::fs::File::open(p) {
        let _ = f.take(max as u64).read_to_end(&mut buf);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// The last `max` bytes of a JSONL file, from its first whole line.
pub(crate) fn read_tail(p: &Path, max: u64) -> String {
    let Ok(mut f) = std::fs::File::open(p) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(max);
    let _ = f.seek(SeekFrom::Start(start));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf).into_owned();
    // A cut first line is not JSON; drop it.
    if start > 0 { text.split_once('\n').map(|(_, rest)| rest.to_owned()).unwrap_or_default() } else { text }
}

/// The context of a session's latest turn, in tokens: what the model read
/// (Claude: input + cache write + cache read; Codex: `last_token_usage.input_tokens`).
pub fn context_tokens(text: &str) -> Option<u64> {
    let n = |v: &Value, k: &str| v[k].as_u64().unwrap_or(0);
    for line in text.lines().rev() {
        let claude = line.contains("\"assistant\"") && line.contains("\"usage\"");
        let codex = line.contains("\"token_count\"") && line.contains("\"last_token_usage\"");
        if !claude && !codex {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let tokens = if claude {
            let u = &v["message"]["usage"];
            n(u, "input_tokens") + n(u, "cache_creation_input_tokens") + n(u, "cache_read_input_tokens")
        } else {
            n(&v["payload"]["info"]["last_token_usage"], "input_tokens")
        };
        // Claude's synthetic replies (errors, interrupts) carry no input.
        if tokens > 0 {
            return Some(tokens);
        }
    }
    None
}

/// Context sizes by session file, read again only when the file changes.
#[derive(Default)]
pub struct Contexts {
    seen: HashMap<PathBuf, (u64, SystemTime, Option<u64>)>,
}

impl Contexts {
    pub fn of(&mut self, file: &Path) -> Option<u64> {
        let meta = std::fs::metadata(file).ok()?;
        let stamp = (meta.len(), meta.modified().ok()?);
        if let Some((l, t, tokens)) = self.seen.get(file)
            && (*l, *t) == stamp
        {
            return *tokens;
        }
        let tokens = context_tokens(&read_tail(file, 512 * 1024));
        self.seen.insert(file.to_owned(), (stamp.0, stamp.1, tokens));
        tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn uuids_look_like_uuids() {
        let a = new_uuid();
        assert_eq!(a.len(), 36);
        assert_eq!(a.as_bytes()[14], b'4');
        assert!(matches!(a.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
        assert_ne!(a, new_uuid());
    }

    #[test]
    fn context_is_the_latest_turns_input() {
        let claude = [
            json!({"type": "assistant", "message": {"usage": {"input_tokens": 5, "cache_creation_input_tokens": 100, "cache_read_input_tokens": 1000, "output_tokens": 9}}}),
            json!({"type": "assistant", "message": {"usage": {"input_tokens": 2, "cache_creation_input_tokens": 300, "cache_read_input_tokens": 41000, "output_tokens": 50}}}),
            json!({"type": "assistant", "message": {"model": "<synthetic>", "usage": {"input_tokens": 0, "output_tokens": 0}}}),
            json!({"type": "user", "message": {"content": "hi"}}),
        ]
        .map(|v| v.to_string())
        .join("\n");
        assert_eq!(context_tokens(&claude), Some(41_302));
        let codex = [
            json!({"type": "event_msg", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 29464, "cached_input_tokens": 13184}}}}),
            json!({"type": "event_msg", "payload": {"type": "token_count", "info": null, "rate_limits": {}}}),
        ]
        .map(|v| v.to_string())
        .join("\n");
        assert_eq!(context_tokens(&codex), Some(29_464));
        assert_eq!(context_tokens("{\"type\":\"user\"}"), None);
    }

    #[test]
    fn a_long_file_is_read_from_its_end() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let filler = json!({"type": "user", "message": {"content": "x".repeat(1000)}}).to_string();
        let last = json!({"type": "assistant", "message": {"usage": {"input_tokens": 7, "cache_read_input_tokens": 70}}}).to_string();
        let text = std::iter::repeat_n(filler, 700).chain([last]).collect::<Vec<_>>().join("\n");
        std::fs::write(&p, text).unwrap();
        let mut c = Contexts::default();
        assert_eq!(c.of(&p), Some(77));
        assert_eq!(c.of(&p), Some(77), "cached");
        assert_eq!(c.of(&dir.path().join("none.jsonl")), None);
    }

    #[test]
    fn codex_sessions_are_found_by_folder_and_member() {
        let dir = tempfile::tempdir().unwrap();
        let src = Sources { claude: dir.path().join("claude"), codex: dir.path().join("codex") };
        let day = src.codex.join("sessions/2026/11/06");
        std::fs::create_dir_all(&day).unwrap();
        let rollout = |id: &str, cwd: &str, member: &str| {
            let prompt = format!("Test.\n\n---\n\n## Your team (Lila)\n\nProject: shop, working directory: {cwd}\nYou are `{member}` (Tester).\n");
            [
                json!({"type": "session_meta", "payload": {"id": id, "cwd": cwd}}),
                json!({"type": "response_item", "payload": {"role": "developer", "content": [{"type": "input_text", "text": prompt}]}}),
            ]
            .map(|v| v.to_string())
            .join("\n")
        };
        std::fs::write(day.join("rollout-a-id1.jsonl"), rollout("id1", "/r", "tester")).unwrap();
        std::fs::write(day.join("rollout-b-id2.jsonl"), rollout("id2", "/r", "engineer")).unwrap();
        std::fs::write(day.join("rollout-c-id3.jsonl"), rollout("id3", "/other", "tester")).unwrap();
        let (id, file) = find_codex(&src, "shop", "tester", "/r", 0).unwrap();
        assert_eq!((id.as_str(), file.file_name().unwrap().to_str().unwrap()), ("id1", "rollout-a-id1.jsonl"));
        assert!(find_codex(&src, "shop", "designer", "/r", 0).is_none());
        let later = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as i64 + 60_000;
        assert!(find_codex(&src, "shop", "tester", "/r", later).is_none(), "older than the member's start");
    }
}
