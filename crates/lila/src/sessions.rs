//! The CLI sessions Lila starts: their ids, where the CLIs record them, and how
//! big their context is now (the input of their latest turn).
//!
//! Claude Code takes the id Lila gives it (`--session-id`) and writes
//! `<claude>/projects/<folder>/<id>.jsonl`. Codex and ACP agents report theirs to the host.

use crate::usage::{Sources, claude_folder};
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
}
