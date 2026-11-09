//! Token use per member, read from the session records the CLIs keep on this
//! machine: Claude Code's `~/.claude/projects/<folder>/<session>.jsonl` and
//! Codex's `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.
//!
//! Members share folders (the project's and its worktrees), so the folder
//! doesn't say whose a session is.
//! Its system prompt does: both CLIs record it, and Lila's says
//! "Project: <id>, working directory: <cwd>" then "You are `<member>`"
//! (see `lila_core::prompt::render`). Sessions without it are the person's own.

use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// The marker of a Lila member's system prompt.
const MARKER: &str = "## Your team (Lila)";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tokens {
    /// Input not read from the cache.
    pub input: u64,
    pub cache_write: u64,
    pub cache_read: u64,
    /// Output, reasoning included.
    pub output: u64,
}

impl Tokens {
    pub fn total(&self) -> u64 {
        self.input + self.cache_write + self.cache_read + self.output
    }

    fn add(&mut self, o: &Tokens) {
        self.input += o.input;
        self.cache_write += o.cache_write;
        self.cache_read += o.cache_read;
        self.output += o.output;
    }
}

/// One model reply.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub ms: i64,
    pub model: String,
    pub tokens: Tokens,
}

/// How much of a Codex usage window was used, as last recorded.
#[derive(Debug, Clone, PartialEq)]
pub struct Limit {
    pub used_percent: f64,
    pub window_minutes: u64,
    pub resets_at_ms: i64,
    /// When this was recorded.
    pub seen_ms: i64,
    pub plan: Option<String>,
}

/// What one session file says.
#[derive(Debug, Clone, Default)]
struct FileUsage {
    /// `(project, member)`, when Lila started it.
    who: Option<(String, String)>,
    runtime: &'static str,
    entries: Vec<Entry>,
    limits: Vec<Limit>,
}

/// One member's use over a period.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MemberUsage {
    pub member: String,
    /// "claude" or "codex".
    pub runtime: String,
    /// Tokens per model.
    pub models: BTreeMap<String, Tokens>,
    pub tokens: Tokens,
    pub sessions: usize,
    pub last_ms: i64,
}

#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub members: Vec<MemberUsage>,
    pub total: Tokens,
    /// The newest Codex window record (primary and secondary windows).
    pub codex_limits: Vec<Limit>,
}

/// Where the CLIs keep their records.
#[derive(Debug, Clone)]
pub struct Sources {
    pub claude: PathBuf,
    pub codex: PathBuf,
}

impl Sources {
    /// `$CLAUDE_CONFIG_DIR` or `~/.claude`, and `$CODEX_HOME` or `~/.codex`.
    pub fn from_env() -> Sources {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let dir = |var: &str, default: &str| std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(default));
        Sources { claude: dir("CLAUDE_CONFIG_DIR", ".claude"), codex: dir("CODEX_HOME", ".codex") }
    }
}

/// Claude Code's folder name for a working directory: every character but
/// ASCII letters and digits becomes `-`.
pub fn claude_folder(cwd: &str) -> String {
    cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// `(project, member)` from a Lila system prompt.
pub fn identity(prompt: &str) -> Option<(String, String)> {
    let at = prompt.find(MARKER)?;
    let rest = &prompt[at..];
    let p = rest.find("Project: ")? + "Project: ".len();
    let project = rest[p..].split(", working directory").next()?.trim().to_owned();
    let y = rest.find("You are `")? + "You are `".len();
    let member = rest[y..].split('`').next()?.to_owned();
    (!project.is_empty() && !member.is_empty()).then_some((project, member))
}

/// The first string in `v` (searched depth first) that holds a Lila prompt.
fn find_prompt(v: &Value) -> Option<&str> {
    match v {
        Value::String(s) if s.contains(MARKER) => Some(s),
        Value::Array(a) => a.iter().find_map(find_prompt),
        Value::Object(o) => o.values().find_map(find_prompt),
        _ => None,
    }
}

/// `2026-11-05T07:30:04.123Z` (or with an offset) as Unix milliseconds.
pub fn parse_time(s: &str) -> Option<i64> {
    use chrono::{DateTime, NaiveDateTime};
    let s = s.trim();
    DateTime::parse_from_rfc3339(s)
        .or_else(|_| DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f%z"))
        .map(|t| t.timestamp_millis())
        .ok()
        // No offset: UTC, as the CLIs write it.
        .or_else(|| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f").ok().map(|t| t.and_utc().timestamp_millis()))
}

fn num(v: &Value, key: &str) -> u64 {
    v[key].as_u64().unwrap_or(0)
}

/// A Claude Code session file. A reply is written once per content block, so
/// replies are counted once by message id.
fn read_claude(text: &str) -> FileUsage {
    let mut f = FileUsage { runtime: "claude", ..Default::default() };
    let mut order: Vec<String> = Vec::new();
    let mut by_id: HashMap<String, Entry> = HashMap::new();
    for line in text.lines() {
        let prompt = f.who.is_none() && line.contains(MARKER);
        if !prompt && !line.contains("\"usage\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if prompt {
            f.who = find_prompt(&v).and_then(identity);
        }
        if v["type"] != "assistant" {
            continue;
        }
        let msg = &v["message"];
        let u = &msg["usage"];
        if !u.is_object() {
            continue;
        }
        let model = msg["model"].as_str().unwrap_or_default().to_owned();
        if model == "<synthetic>" {
            continue;
        }
        let tokens = Tokens { input: num(u, "input_tokens"), cache_write: num(u, "cache_creation_input_tokens"), cache_read: num(u, "cache_read_input_tokens"), output: num(u, "output_tokens") };
        let id = msg["id"].as_str().or(v["uuid"].as_str()).unwrap_or_default().to_owned();
        let e = Entry { ms: v["timestamp"].as_str().and_then(parse_time).unwrap_or(0), model, tokens };
        if !by_id.contains_key(&id) {
            order.push(id.clone());
        }
        by_id.insert(id, e);
    }
    f.entries = order.into_iter().filter_map(|id| by_id.remove(&id)).collect();
    f
}

/// A Codex rollout: one `token_usage_record` per reply, the model from the
/// latest `turn_context`, the usage windows from `token_count` events.
fn read_codex(text: &str) -> FileUsage {
    let mut f = FileUsage { runtime: "codex", ..Default::default() };
    let mut model = String::new();
    for line in text.lines() {
        let prompt = f.who.is_none() && line.contains(MARKER);
        let wanted = prompt || line.contains("\"token_usage_record\"") || line.contains("\"turn_context\"") || line.contains("\"rate_limits\"");
        if !wanted {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if prompt {
            f.who = find_prompt(&v).and_then(identity);
        }
        let ms = v["timestamp"].as_str().and_then(parse_time).unwrap_or(0);
        let p = &v["payload"];
        match v["type"].as_str() {
            Some("turn_context") => {
                if let Some(m) = p["model"].as_str() {
                    model = m.to_owned();
                }
            }
            Some("token_usage_record") => {
                let u = &p["usage"];
                let cached = num(u, "cached_input_tokens");
                let tokens = Tokens { input: num(u, "input_tokens").saturating_sub(cached), cache_write: num(u, "cache_write_input_tokens"), cache_read: cached, output: num(u, "output_tokens") };
                f.entries.push(Entry { ms, model: model.clone(), tokens });
            }
            Some("event_msg") if p["type"] == "token_count" => {
                let r = &p["rate_limits"];
                for w in ["primary", "secondary"] {
                    let l = &r[w];
                    if let Some(used) = l["used_percent"].as_f64() {
                        f.limits.push(Limit { used_percent: used, window_minutes: num(l, "window_minutes"), resets_at_ms: l["resets_at"].as_i64().unwrap_or(0) * 1000, seen_ms: ms, plan: r["plan_type"].as_str().map(String::from) });
                    }
                }
            }
            _ => {}
        }
    }
    f
}

/// The `cwd` in a rollout's first line (`session_meta`).
fn codex_cwd(first_line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(first_line).ok()?;
    v["payload"]["cwd"].as_str().map(String::from)
}

/// The `.jsonl` files in `dir` and up to `depth` folders below it.
fn jsonl_under(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    let files = walkdir::WalkDir::new(dir).min_depth(1).max_depth(depth + 1).into_iter().filter_map(Result::ok);
    out.extend(files.filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|x| x == "jsonl")).map(|e| e.into_path()));
}

/// Reads session files once and again only when they change.
#[derive(Default)]
pub struct Scanner {
    files: HashMap<PathBuf, (u64, std::time::SystemTime, FileUsage)>,
    /// The folder each Codex rollout was started in, by path (from its first line).
    elsewhere: HashMap<PathBuf, ((u64, std::time::SystemTime), Option<String>)>,
}

impl Scanner {
    fn stamp(p: &Path) -> Option<(u64, std::time::SystemTime)> {
        let m = std::fs::metadata(p).ok()?;
        Some((m.len(), m.modified().ok()?))
    }

    fn read(&mut self, p: &Path, stamp: (u64, std::time::SystemTime), read: fn(&str) -> FileUsage) -> &FileUsage {
        let fresh = self.files.get(p).is_some_and(|(l, t, _)| (*l, *t) == stamp);
        if !fresh {
            let text = std::fs::read_to_string(p).unwrap_or_default();
            self.files.insert(p.to_owned(), (stamp.0, stamp.1, read(&text)));
        }
        &self.files[p].2
    }

    /// Every session of `project` Lila started, in any of its folders (the
    /// project folder and its worktree slots).
    fn sessions(&mut self, src: &Sources, project: &str, cwds: &[String]) -> Vec<FileUsage> {
        let mut out = Vec::new();
        for cwd in cwds {
            self.claude_sessions(src, cwd, &mut out);
        }
        self.codex_sessions(src, cwds, &mut out);
        out.retain(|f| f.who.as_ref().is_some_and(|(p, _)| p == project));
        out
    }

    /// Claude Code: the folder's own directory; a session's subagents sit in `<session>/subagents/`.
    fn claude_sessions(&mut self, src: &Sources, cwd: &str, out: &mut Vec<FileUsage>) {
        let dir = src.claude.join("projects").join(claude_folder(cwd));
        let mut files = Vec::new();
        jsonl_under(&dir, &mut files, 2);
        files.sort();
        let mut who_of: HashMap<String, Option<(String, String)>> = HashMap::new();
        let mut subagents = Vec::new();
        for p in files {
            let Some(stamp) = Self::stamp(&p) else { continue };
            if p.parent() != Some(dir.as_path()) {
                subagents.push((p, stamp));
                continue;
            }
            let f = self.read(&p, stamp, read_claude).clone();
            who_of.insert(p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(), f.who.clone());
            out.push(f);
        }
        for (p, stamp) in subagents {
            // <dir>/<session>/subagents/<agent>.jsonl: the session's member.
            let session = p.strip_prefix(&dir).ok().and_then(|r| r.components().next()).map(|c| c.as_os_str().to_string_lossy().to_string()).unwrap_or_default();
            let Some(who) = who_of.get(&session).cloned().flatten() else { continue };
            let mut f = self.read(&p, stamp, read_claude).clone();
            f.who = Some(who);
            out.push(f);
        }
    }

    /// Codex: every rollout; only the first line says the folder.
    fn codex_sessions(&mut self, src: &Sources, cwds: &[String], out: &mut Vec<FileUsage>) {
        let mut files = Vec::new();
        jsonl_under(&src.codex.join("sessions"), &mut files, 4);
        for p in files {
            let Some(stamp) = Self::stamp(&p) else { continue };
            // The folder a rollout is in, read once per version of the file.
            let cwd = match self.elsewhere.get(&p) {
                Some((at, cwd)) if *at == stamp => cwd.clone(),
                _ => {
                    use std::io::BufRead;
                    let first = std::fs::File::open(&p).ok().and_then(|f| std::io::BufReader::new(f).lines().next()).and_then(|l| l.ok()).unwrap_or_default();
                    let cwd = codex_cwd(&first);
                    self.elsewhere.insert(p.clone(), (stamp, cwd.clone()));
                    cwd
                }
            };
            if !cwd.is_some_and(|c| cwds.contains(&c)) {
                continue;
            }
            out.push(self.read(&p, stamp, read_codex).clone());
        }
    }

    /// Use per member since `since_ms` (`None`: all of it), most first.
    /// Tokens (all kinds) `member` used between `from_ms` and `to_ms`.
    pub fn tokens_between(&mut self, src: &Sources, project: &str, cwds: &[String], member: &str, from_ms: i64, to_ms: i64) -> u64 {
        self.sessions(src, project, cwds)
            .iter()
            .filter(|f| f.who.as_ref().is_some_and(|(p, m)| p == project && m == member))
            .flat_map(|f| f.entries.iter())
            .filter(|e| e.ms >= from_ms && e.ms <= to_ms)
            .map(|e| e.tokens.total())
            .sum()
    }

    pub fn usage(&mut self, src: &Sources, project: &str, cwds: &[String], since_ms: Option<i64>) -> Usage {
        let sessions = self.sessions(src, project, cwds);
        let mut members: BTreeMap<String, MemberUsage> = BTreeMap::new();
        let mut total = Tokens::default();
        let mut limits: Vec<Limit> = Vec::new();
        for f in &sessions {
            let (_, member) = f.who.clone().expect("kept only with an owner");
            if f.runtime == "codex"
                && let Some(newest) = f.limits.iter().map(|l| l.seen_ms).max()
                && limits.first().is_none_or(|l| l.seen_ms < newest)
            {
                limits = f.limits.iter().filter(|l| l.seen_ms == newest).cloned().collect();
            }
            let entries: Vec<&Entry> = f.entries.iter().filter(|e| since_ms.is_none_or(|s| e.ms >= s)).collect();
            if entries.is_empty() {
                continue;
            }
            let m = members.entry(member.clone()).or_insert_with(|| MemberUsage { member, runtime: f.runtime.to_owned(), ..Default::default() });
            m.sessions += 1;
            for e in entries {
                m.tokens.add(&e.tokens);
                m.models.entry(e.model.clone()).or_default().add(&e.tokens);
                m.last_ms = m.last_ms.max(e.ms);
                total.add(&e.tokens);
            }
        }
        let mut members: Vec<MemberUsage> = members.into_values().collect();
        members.sort_by(|a, b| b.tokens.total().cmp(&a.tokens.total()).then(a.member.cmp(&b.member)));
        Usage { members, total, codex_limits: limits }
    }
}

/// 1234 → "1.2k", 3_400_000 → "3.4M".
pub fn short(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => trim(format!("{:.1}", n as f64 / 1e3)) + "k",
        1_000_000..=999_999_999 => trim(format!("{:.1}", n as f64 / 1e6)) + "M",
        _ => trim(format!("{:.2}", n as f64 / 1e9)) + "B",
    }
}

fn trim(s: String) -> String {
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn prompt(project: &str, cwd: &str, member: &str) -> String {
        format!("Be good.\n\n---\n\n## Your team (Lila)\n\nProject: {project}, working directory: {cwd}\nYou are `{member}` (Engineer).\n\nTeam:\n- `pm`: PM\n")
    }

    fn lines(v: &[Value]) -> String {
        v.iter().map(|v| v.to_string() + "\n").collect()
    }

    fn claude_reply(id: &str, at: &str, out: u64) -> Value {
        json!({"type": "assistant", "timestamp": at, "message": {"id": id, "model": "claude-sonnet-5-5", "usage": {"input_tokens": 2, "cache_creation_input_tokens": 100, "cache_read_input_tokens": 1000, "output_tokens": out}}})
    }

    #[test]
    fn identity_and_folder() {
        assert_eq!(identity(&prompt("shop", "/Users/me/shop", "engineer-2")), Some(("shop".into(), "engineer-2".into())));
        assert_eq!(identity("You are `x`"), None);
        assert_eq!(claude_folder("/Users/jarax/codes/lila-agent"), "-Users-jarax-codes-lila-agent");
        assert_eq!(claude_folder("/private/tmp/claude-501/-Users-x"), "-private-tmp-claude-501--Users-x");
    }

    #[test]
    fn times() {
        assert_eq!(parse_time("1970-01-01T00:00:01.500Z"), Some(1500));
        assert_eq!(parse_time("2026-11-05T07:30:04.000Z"), Some(1_793_863_804_000));
        assert_eq!(parse_time("2026-11-05T15:30:04+08:00"), Some(1_793_863_804_000));
        assert_eq!(short(950), "950");
        assert_eq!(short(1_250), "1.2k");
        assert_eq!(short(3_000_000), "3M");
    }

    #[test]
    fn claude_counts_each_reply_once() {
        let text = lines(&[
            json!({"type": "attachment", "attachment": {"type": "prompt_snapshot", "content": prompt("shop", "/r", "engineer")}}),
            claude_reply("m1", "2026-11-05T01:00:00Z", 10),
            claude_reply("m1", "2026-11-05T01:00:00Z", 10),
            claude_reply("m2", "2026-11-05T02:00:00Z", 20),
            json!({"type": "assistant", "message": {"id": "m3", "model": "<synthetic>", "usage": {"output_tokens": 0}}}),
        ]);
        let f = read_claude(&text);
        assert_eq!(f.who, Some(("shop".into(), "engineer".into())));
        assert_eq!(f.entries.len(), 2);
        assert_eq!(f.entries[1].tokens, Tokens { input: 2, cache_write: 100, cache_read: 1000, output: 20 });
    }

    #[test]
    fn codex_records_models_and_windows() {
        let text = lines(&[
            json!({"type": "session_meta", "timestamp": "2026-11-05T01:00:00Z", "payload": {"cwd": "/r"}}),
            json!({"type": "response_item", "payload": {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": prompt("shop", "/r", "tester")}]}}),
            json!({"type": "turn_context", "payload": {"model": "gpt-6.1-sol"}}),
            json!({"type": "token_usage_record", "timestamp": "2026-11-05T01:01:00Z", "payload": {"usage": {"input_tokens": 1000, "cached_input_tokens": 800, "cache_write_input_tokens": 0, "output_tokens": 50, "total_tokens": 1050}}}),
            json!({"type": "event_msg", "timestamp": "2026-11-05T01:01:01Z", "payload": {"type": "token_count", "rate_limits": {"primary": {"used_percent": 41.0, "window_minutes": 10080, "resets_at": 1_794_453_095}, "secondary": null, "plan_type": "pro"}}}),
        ]);
        let f = read_codex(&text);
        assert_eq!(f.who, Some(("shop".into(), "tester".into())));
        assert_eq!(f.entries, [Entry { ms: parse_time("2026-11-05T01:01:00Z").unwrap(), model: "gpt-6.1-sol".into(), tokens: Tokens { input: 200, cache_write: 0, cache_read: 800, output: 50 } }]);
        assert_eq!(f.entries[0].tokens.total(), 1050);
        assert_eq!(f.limits.len(), 1);
        assert_eq!(f.limits[0].resets_at_ms, 1_794_453_095_000);
        assert_eq!(codex_cwd(text.lines().next().unwrap()).as_deref(), Some("/r"));
    }

    #[test]
    fn sums_per_member_from_both_clis() {
        let home = tempfile::tempdir().unwrap();
        let src = Sources { claude: home.path().join("claude"), codex: home.path().join("codex") };
        let cwd = "/Users/me/shop";
        let dir = src.claude.join("projects").join(claude_folder(cwd));
        std::fs::create_dir_all(dir.join("s1/subagents")).unwrap();
        let session = |member: &str, replies: &[Value]| {
            let mut v = vec![json!({"type": "attachment", "attachment": {"content": prompt("shop", cwd, member)}})];
            v.extend_from_slice(replies);
            lines(&v)
        };
        std::fs::write(dir.join("s1.jsonl"), session("engineer", &[claude_reply("a", "2026-11-04T10:00:00Z", 10), claude_reply("b", "2026-11-05T10:00:00Z", 20)])).unwrap();
        std::fs::write(dir.join("s1/subagents/agent-1.jsonl"), lines(&[claude_reply("c", "2026-11-05T11:00:00Z", 5)])).unwrap();
        std::fs::write(dir.join("s2.jsonl"), session("pm", &[claude_reply("d", "2026-11-05T10:00:00Z", 1)])).unwrap();
        // The person's own session in the same folder, and another project's member.
        std::fs::write(dir.join("mine.jsonl"), lines(&[claude_reply("e", "2026-11-05T10:00:00Z", 999)])).unwrap();
        std::fs::write(dir.join("other.jsonl"), session("x", &[]).replace("Project: shop", "Project: blog")).unwrap();
        let roll = src.codex.join("sessions/2026/11/05");
        std::fs::create_dir_all(&roll).unwrap();
        let codex = |cwd: &str| {
            lines(&[
                json!({"type": "session_meta", "payload": {"cwd": cwd}}),
                json!({"type": "response_item", "payload": {"content": [{"text": prompt("shop", cwd, "tester")}]}}),
                json!({"type": "token_usage_record", "timestamp": "2026-11-05T12:00:00Z", "payload": {"usage": {"input_tokens": 100, "cached_input_tokens": 0, "output_tokens": 7}}}),
            ])
        };
        std::fs::write(roll.join("rollout-1.jsonl"), codex(cwd)).unwrap();
        std::fs::write(roll.join("rollout-2.jsonl"), codex("/elsewhere")).unwrap();

        let mut s = Scanner::default();
        let cwds = [cwd.to_owned()];
        let all = s.usage(&src, "shop", &cwds, None);
        let by = |u: &Usage, m: &str| u.members.iter().find(|x| x.member == m).map(|x| x.tokens.output);
        assert_eq!(all.members.len(), 3, "{:?}", all.members);
        assert_eq!(by(&all, "engineer"), Some(35));
        assert_eq!(by(&all, "pm"), Some(1));
        assert_eq!(by(&all, "tester"), Some(7));
        assert_eq!(all.members[0].member, "engineer", "most first");
        assert_eq!(all.members[0].sessions, 2, "the subagent file counts as its own");

        let today = s.usage(&src, "shop", &cwds, parse_time("2026-11-05T00:00:00Z"));
        assert_eq!(by(&today, "engineer"), Some(25));

        // A file that grows is read again.
        std::fs::write(dir.join("s2.jsonl"), session("pm", &[claude_reply("d", "2026-11-05T10:00:00Z", 1), claude_reply("f", "2026-11-05T13:00:00Z", 4)])).unwrap();
        assert_eq!(by(&s.usage(&src, "shop", &cwds, None), "pm"), Some(5));
    }
}
