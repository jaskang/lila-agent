//! What the window shows, fetched from lilad over its socket.

use lila::paths::Paths;
use serde_json::{Value, json};

#[derive(Clone, Default, Debug)]
pub struct Project {
    pub id: String,
    pub cwd: String,
    /// What the window calls it: `title` in project.toml, else the id.
    pub label: String,
    /// `title` in project.toml, if it has one.
    pub title: String,
    /// Why lilad could not read its `.lila/project.toml`.
    pub error: Option<String>,
    /// What a new session starts with (`project.list`'s `defaults`).
    pub engine: String,
    pub model: String,
    pub effort: String,
    pub permission: String,
}

impl Project {
    pub fn label(&self) -> String {
        if self.label.is_empty() { self.id.clone() } else { self.label.clone() }
    }

    pub fn of(v: &Value) -> Project {
        let s = |k: &str| v[k].as_str().unwrap_or_default().to_owned();
        let d = |k: &str| v["defaults"][k].as_str().unwrap_or_default().to_owned();
        Project {
            id: s("id"),
            cwd: s("cwd"),
            label: s("label"),
            title: s("title"),
            error: v["error"].as_str().map(String::from),
            engine: Some(d("engine")).filter(|e| !e.is_empty()).unwrap_or_else(|| "claude".into()),
            model: d("model"),
            effort: d("effort"),
            permission: Some(d("permission")).filter(|e| !e.is_empty()).unwrap_or_else(|| "full".into()),
        }
    }
}

/// A long path as its last `n` characters, so the folder's own name shows.
pub fn tail(path: &str, n: usize) -> String {
    let count = path.chars().count();
    if count <= n { path.to_owned() } else { format!("…{}", path.chars().skip(count - n + 1).collect::<String>()) }
}

/// `/Users/me/x` as `~/x`.
pub fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_owned(),
    }
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub daemon: bool,
    pub projects: Vec<Project>,
    /// `thread.list` of every project, archived ones too: each thread with its state.
    pub threads: Vec<Value>,
    /// `thread.read` of the shown thread, when it is shown as its conversation.
    pub conversation: Option<Value>,
    pub error: Option<String>,
}

/// What the next fetch reads of the shown thread's conversation.
pub struct Want {
    pub thread: String,
    pub after: i64,
}

pub fn fetch(paths: &Paths, want: Option<&Want>) -> Snapshot {
    #[cfg(feature = "shots")]
    if let Some(snap) = fake(want) {
        return snap;
    }
    let mut snap = Snapshot { daemon: lila::client::is_running(paths), ..Default::default() };
    if !snap.daemon {
        return snap;
    }
    let call = |method: &str, params: Value| lila::client::request(paths, method, params, false);
    match call("project.list", json!({})) {
        Ok(v) => snap.projects = v.as_array().into_iter().flatten().map(Project::of).collect(),
        Err(e) => {
            snap.error = Some(format!("{e:#}"));
            return snap;
        }
    }
    snap.threads = call("thread.list", json!({"archived": true})).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    if let Some(w) = want {
        snap.conversation = call("thread.read", json!({"thread": w.thread, "after": w.after})).ok();
    }
    snap
}

/// A macOS notification, through the system's own `osascript`.
pub fn notify(title: &str, body: &str) {
    // `--features shots`: `LILA_APP_NOTIFY_LOG=<file>` records what would be shown, instead of showing it.
    #[cfg(feature = "shots")]
    if let Ok(log) = std::env::var("LILA_APP_NOTIFY_LOG") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
            let _ = writeln!(f, "{title} | {body}");
        }
        return;
    }
    let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let script = format!("display notification {} with title \"Lila\" subtitle {}", quote(body), quote(title));
    std::thread::spawn(move || {
        let _ = std::process::Command::new("osascript").arg("-e").arg(script).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    });
}

/// One request to lilad. `autostart`: start lilad first if needed.
pub fn act(paths: &Paths, method: &str, params: Value, autostart: bool) -> anyhow::Result<Value> {
    lila::client::request(paths, method, params, autostart)
}

/// The thread shown last time, kept across launches.
fn last_file(paths: &Paths) -> std::path::PathBuf {
    paths.state.join("app-tabs.json")
}

/// The thread and project shown last time.
pub fn load_last(paths: &Paths) -> (Option<String>, Option<String>) {
    let v: Option<Value> = std::fs::read_to_string(last_file(paths)).ok().and_then(|t| serde_json::from_str(&t).ok());
    let get = |k: &str| v.as_ref().and_then(|v| v[k].as_str().map(String::from));
    (get("thread"), get("project"))
}

fn views_file(paths: &Paths) -> std::path::PathBuf {
    paths.state.join("app-views.json")
}

/// Threads shown as their terminal rather than their conversation (thread ids).
pub fn load_views(paths: &Paths) -> std::collections::HashSet<String> {
    std::fs::read_to_string(views_file(paths)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v["tui"].as_array().cloned()).into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect()
}

pub fn save_views(paths: &Paths, tui: &std::collections::HashSet<String>) {
    let mut list: Vec<&String> = tui.iter().collect();
    list.sort();
    let _ = std::fs::create_dir_all(&paths.state);
    let _ = std::fs::write(views_file(paths), json!({ "tui": list }).to_string());
}

pub fn save_last(paths: &Paths, thread: Option<&str>, project: Option<&str>) {
    let _ = std::fs::create_dir_all(&paths.state);
    let _ = std::fs::write(last_file(paths), json!({ "thread": thread, "project": project }).to_string());
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// How long ago, in a few characters for a sidebar row: 刚刚, 5分, 3时, 2天.
pub fn age(ms: i64, now: i64) -> String {
    let mins = (now - ms).max(0) / 60_000;
    match mins {
        0 => "刚刚".to_owned(),
        1..=59 => format!("{mins}分"),
        60..=1439 => format!("{}时", mins / 60),
        _ => format!("{}天", mins / 1440),
    }
}

/// Wall clock with milliseconds, for debug logs.
pub fn clock_ms() -> String {
    chrono::Local::now().format("%H:%M:%S%.3f").to_string()
}

/// `--features shots`: `LILA_APP_FAKE=<file>` stands in for lilad, so a screenshot can show any state without
/// running an engine. The file is JSON: `projects` (as `project.list`), `threads` (as `thread.list`) and
/// `conversations`, each thread's id to what `thread.read` would say (`items`, `permissions`, `writing`, `config`).
#[cfg(feature = "shots")]
fn fake(want: Option<&Want>) -> Option<Snapshot> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(std::env::var("LILA_APP_FAKE").ok()?).ok()?).ok()?;
    let list = |k: &str| v[k].as_array().cloned().unwrap_or_default();
    let threads = list("threads");
    let conversation = want.and_then(|w| {
        let mut read = v["conversations"][&w.thread].clone();
        if read.is_null() {
            read = json!({"items": [], "seq": 0});
        }
        read["thread"] = threads.iter().find(|t| t["id"] == w.thread.as_str())?.clone();
        Some(read)
    });
    Some(Snapshot { daemon: true, projects: list("projects").iter().map(Project::of).collect(), threads, conversation, error: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_projects_defaults_come_from_the_list_with_lilads_own_fallbacks() {
        let p = Project::of(&json!({"id": "shop", "cwd": "/x", "label": "商店", "title": "商店", "defaults": {"engine": "codex", "model": "gpt-6-luna", "effort": "low", "permission": "confirm"}}));
        assert_eq!((p.engine.as_str(), p.model.as_str(), p.effort.as_str(), p.permission.as_str()), ("codex", "gpt-6-luna", "low", "confirm"));
        let bare = Project::of(&json!({"id": "a", "cwd": "/a", "label": "a"}));
        assert_eq!((bare.engine.as_str(), bare.permission.as_str()), ("claude", "full"));
        assert_eq!(bare.label(), "a");
    }

    #[test]
    fn ages_are_short() {
        let now = 10_000_000_000;
        assert_eq!(age(now - 20_000, now), "刚刚");
        assert_eq!(age(now - 5 * 60_000, now), "5分");
        assert_eq!(age(now - 3 * 3_600_000, now), "3时");
        assert_eq!(age(now - 50 * 3_600_000, now), "2天");
    }
}
