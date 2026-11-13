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
    pub model: String,
    pub permission: String,
    /// The folder is the top of a git repository: a session there can have a worktree of its own.
    pub repo: bool,
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
            model: d("model"),
            permission: Some(d("permission")).filter(|e| !e.is_empty()).unwrap_or_else(|| "full".into()),
            repo: v["repo"] == true,
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
    /// What the running lilad says of itself (`ping`): its version and protocol.
    pub daemon_info: Option<lila::client::DaemonInfo>,
    pub projects: Vec<Project>,
    /// `thread.list` of every project, archived ones too: each thread with its state.
    pub threads: Vec<Value>,
    /// `thread.read` of the shown thread, when it is shown as its conversation.
    pub conversation: Option<Value>,
    pub error: Option<String>,
}

impl Snapshot {
    /// A running lilad that speaks another protocol than this window (or none that it can say): older or newer than the `lila` this window was built with.
    pub fn protocol_differs(&self) -> bool {
        self.daemon_info.as_ref().is_some_and(|i| !i.is_current())
    }
}

/// What the 关于 page says of lilad.
pub fn daemon_line(snap: &Snapshot) -> String {
    let Some(i) = snap.daemon_info.as_ref().filter(|_| snap.daemon) else { return "没有运行".into() };
    let mut line = format!("运行中 · {}", i.version.as_deref().unwrap_or("版本未知"));
    match i.protocol {
        Some(p) => line.push_str(&format!(" · 协议 {p}")),
        None => line.push_str(" · 协议未知"),
    }
    if !i.is_current() {
        line.push_str(&format!("（窗口 {}）", lila::PROTOCOL));
    }
    line
}

/// Restarts lilad in the background (its sessions keep running), then says whether it still speaks another protocol than this window:
/// then the `lila` on disk is the old one.
pub fn restart_daemon(paths: &Paths) -> anyhow::Result<bool> {
    #[cfg(feature = "shots")]
    if let Some(still) = fake_restart() {
        return Ok(still);
    }
    lila::client::restart_daemon(paths)?;
    Ok(!matches!(lila::client::daemon_version(paths), lila::client::DaemonVersion::Current(_)))
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
    let mut snap = Snapshot::default();
    if let lila::client::DaemonVersion::Current(i) | lila::client::DaemonVersion::Different(i) = lila::client::daemon_version(paths) {
        snap.daemon = true;
        snap.daemon_info = Some(i);
    }
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
    #[cfg(feature = "shots")]
    if let Some(answer) = fake_act(method, &params) {
        return answer;
    }
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

fn sidebar_file(paths: &Paths) -> std::path::PathBuf {
    paths.state.join("app-sidebar.json")
}

/// Was the sidebar folded away last time?
pub fn load_sidebar_hidden(paths: &Paths) -> bool {
    std::fs::read_to_string(sidebar_file(paths)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).is_some_and(|v| v["hidden"] == true)
}

pub fn save_sidebar_hidden(paths: &Paths, hidden: bool) {
    let _ = std::fs::create_dir_all(&paths.state);
    let _ = std::fs::write(sidebar_file(paths), json!({ "hidden": hidden }).to_string());
}

fn panel_file(paths: &Paths) -> std::path::PathBuf {
    paths.state.join("app-panel.json")
}

/// The right panel as it was left: open or not, which view (`files` or `changes`), how wide.
pub fn load_panel(paths: &Paths) -> (bool, Option<String>, Option<f32>) {
    let v: Option<Value> = std::fs::read_to_string(panel_file(paths)).ok().and_then(|t| serde_json::from_str(&t).ok());
    match v {
        Some(v) => (v["open"] == true, v["view"].as_str().map(String::from), v["width"].as_f64().map(|w| w as f32)),
        None => (false, None, None),
    }
}

pub fn save_panel(paths: &Paths, open: bool, view: &str, width: f32) {
    let _ = std::fs::create_dir_all(&paths.state);
    let _ = std::fs::write(panel_file(paths), json!({ "open": open, "view": view, "width": width.round() }).to_string());
}

pub fn save_last(paths: &Paths, thread: Option<&str>, project: Option<&str>) {
    let _ = std::fs::create_dir_all(&paths.state);
    let _ = std::fs::write(last_file(paths), json!({ "thread": thread, "project": project }).to_string());
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// How long ago, for a sidebar row: 刚刚, 5 分钟, 19 小时, 3 天.
pub fn age(ms: i64, now: i64) -> String {
    let mins = (now - ms).max(0) / 60_000;
    match mins {
        0 => "刚刚".to_owned(),
        1..=59 => format!("{mins} 分钟"),
        60..=1439 => format!("{} 小时", mins / 60),
        _ => format!("{} 天", mins / 1440),
    }
}

/// Wall clock with milliseconds, for debug logs.
pub fn clock_ms() -> String {
    chrono::Local::now().format("%H:%M:%S%.3f").to_string()
}

/// `--features shots`, with `LILA_APP_FAKE`: what the menus and the right panel list, and how a send goes
/// (`LILA_APP_FAKE_SEND_MS` takes that long, `LILA_APP_FAKE_READ_MS` for the panel's reads, `LILA_APP_FAKE_SEND_ERR=<words>` fails with them). In the file:
/// `files` and `commands` (the menus; `files` is also the file tree), `models` (`[{id, name}]`, the models omp offers), `changes` (a thread's id to what `thread.changes`
/// says), `diffs` (a thread's id to its path to the `thread.diff` item), `file` (a path to what `thread.file` says),
/// `commit` and `merge` (what those answer, or `{"error": "…"}`, after `LILA_APP_FAKE_SEND_MS`), `restore` (`{"dry_run": …, "restore": …}`,
/// the replies of `thread.restore` with and without `dry_run`, or `{"error": "…"}`; under a checkpoint's number, `{"7": {"dry_run": …}}`, for that
/// checkpoint alone); every other request is left to fail.
#[cfg(feature = "shots")]
fn fake_act(method: &str, params: &Value) -> Option<anyhow::Result<Value>> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(std::env::var("LILA_APP_FAKE").ok()?).ok()?).ok()?;
    let wait = |var: &str| std::thread::sleep(std::time::Duration::from_millis(std::env::var(var).ok().and_then(|m| m.parse().ok()).unwrap_or(0)));
    // What is read (`LILA_APP_FAKE_READ_MS`) and what is done (`LILA_APP_FAKE_SEND_MS`) take their own time.
    let (read, slow) = (|| wait("LILA_APP_FAKE_READ_MS"), || wait("LILA_APP_FAKE_SEND_MS"));
    let thread = params["thread"].as_str().unwrap_or_default();
    // What the file says as an answer: a message in `error` is a failure.
    let answer = |a: &Value| match a["error"].as_str() {
        Some(e) => Err(anyhow::anyhow!(e.to_owned())),
        None => Ok(a.clone()),
    };
    match method {
        "thread.files" => Some(Ok(json!({"files": v["files"]}))),
        "thread.commands" => Some(Ok(json!({"commands": v["commands"]}))),
        "models.list" if v["models"].is_array() => Some(Ok(json!({"models": v["models"]}))),
        "thread.changes" => {
            read();
            Some(match &v["changes"][thread] {
                Value::Null => Err(anyhow::anyhow!("no fake changes for {thread}")),
                a => answer(a),
            })
        }
        "thread.diff" => {
            read();
            let files = &v["diffs"][thread];
            let wanted = params["path"].as_str();
            let mut diffs: Vec<Value> = v["changes"][thread]["files"].as_array().into_iter().flatten().filter_map(|f| files.get(f["path"].as_str()?).cloned()).collect();
            diffs.retain(|d| wanted.is_none_or(|w| d["path"] == w || d["old_path"] == w));
            Some(Ok(json!({"diffs": diffs, "truncated": false})))
        }
        "thread.file" => {
            read();
            let path = params["path"].as_str().unwrap_or_default();
            Some(match &v["file"][path] {
                Value::Null => Err(anyhow::anyhow!("no such file")),
                a => answer(a),
            })
        }
        "thread.commit" | "thread.merge" => {
            slow();
            Some(answer(&v[method.trim_start_matches("thread.")]))
        }
        "thread.restore" => {
            // What a dry run and a restore answer (`dry_run`, `restore`), for any checkpoint or, under its number, for that one;
            // the dry run takes `LILA_APP_FAKE_READ_MS`, the restore `LILA_APP_FAKE_SEND_MS`.
            let key = if params["dry_run"] == true { "dry_run" } else { "restore" };
            if key == "dry_run" { read() } else { slow() }
            let by_number = &v["restore"][params["checkpoint"].as_i64().unwrap_or_default().to_string()][key];
            Some(match if by_number.is_null() { &v["restore"][key] } else { by_number } {
                Value::Null => Err(anyhow::anyhow!("no fake {key} for {thread}")),
                a => answer(a),
            })
        }
        "thread.log" => Some(Ok(v["log"][thread].clone())),
        "thread.new" if v["new"].is_object() => {
            slow();
            Some(match std::env::var("LILA_APP_FAKE_SEND_ERR") {
                Ok(e) => Err(anyhow::anyhow!(e)),
                Err(_) => Ok(v["new"].clone()),
            })
        }
        "thread.send" | "thread.resume" => {
            slow();
            Some(match std::env::var("LILA_APP_FAKE_SEND_ERR") {
                Ok(e) => Err(anyhow::anyhow!(e)),
                Err(_) => Ok(json!({"sent": true})),
            })
        }
        _ => None,
    }
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
    // `daemon`: what `ping` says (the version and protocol; without it, a current lilad). `restart`: `"fixed"` makes the window's restart button
    // work (the next look finds a current lilad); else the restart leaves the old one, as when the `lila` on disk is itself old.
    let daemon_info = match v.get("daemon") {
        Some(d) if !(FAKE_RESTARTED.load(std::sync::atomic::Ordering::Relaxed) && v["restart"] == "fixed") => lila::client::DaemonInfo::from_ping(d),
        _ => lila::client::DaemonInfo { pid: Some(1), version: Some(env!("CARGO_PKG_VERSION").into()), protocol: Some(lila::PROTOCOL) },
    };
    Some(Snapshot { daemon: true, daemon_info: Some(daemon_info), projects: list("projects").iter().map(Project::of).collect(), threads, conversation, error: None })
}

#[cfg(feature = "shots")]
static FAKE_RESTARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `--features shots` with `LILA_APP_FAKE`: the restart button takes `LILA_APP_FAKE_SEND_MS` (at least a moment) and touches no lilad;
/// the answer is whether lilad still differs after it (the file's `restart`, see [`fake`]).
#[cfg(feature = "shots")]
fn fake_restart() -> Option<bool> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(std::env::var("LILA_APP_FAKE").ok()?).ok()?).ok()?;
    std::thread::sleep(std::time::Duration::from_millis(std::env::var("LILA_APP_FAKE_SEND_MS").ok().and_then(|m| m.parse().ok()).unwrap_or(800)));
    let fixed = v["restart"] == "fixed";
    FAKE_RESTARTED.store(fixed, std::sync::atomic::Ordering::Relaxed);
    Some(!fixed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_projects_defaults_come_from_the_list_with_lilads_own_fallbacks() {
        let p = Project::of(&json!({"id": "shop", "cwd": "/x", "label": "商店", "title": "商店", "defaults": {"model": "openai/gpt-6", "permission": "confirm"}}));
        assert_eq!((p.model.as_str(), p.permission.as_str()), ("openai/gpt-6", "confirm"));
        let bare = Project::of(&json!({"id": "a", "cwd": "/a", "label": "a"}));
        assert_eq!((bare.model.as_str(), bare.permission.as_str()), ("", "full"));
        assert_eq!(bare.label(), "a");
        // Whether the folder can have worktrees is lilad's word; not said, no.
        assert!(!bare.repo);
        assert!(Project::of(&json!({"id": "a", "cwd": "/a", "repo": true})).repo);
    }

    #[test]
    fn the_about_page_says_which_lilad_runs() {
        let snap = |info: Option<lila::client::DaemonInfo>| Snapshot { daemon: info.is_some(), daemon_info: info, ..Default::default() };
        let info = |protocol| lila::client::DaemonInfo { pid: Some(1), version: Some("0.1.0".into()), protocol };
        assert_eq!(daemon_line(&snap(None)), "没有运行");
        let current = snap(Some(info(Some(lila::PROTOCOL))));
        assert_eq!((daemon_line(&current), current.protocol_differs()), (format!("运行中 · 0.1.0 · 协议 {}", lila::PROTOCOL), false));
        let other = snap(Some(info(Some(lila::PROTOCOL + 1))));
        assert_eq!((daemon_line(&other), other.protocol_differs()), (format!("运行中 · 0.1.0 · 协议 {}（窗口 {}）", lila::PROTOCOL + 1, lila::PROTOCOL), true));
        // A lilad from before the handshake says nothing, which counts as different.
        let old = snap(Some(lila::client::DaemonInfo { pid: Some(1), ..Default::default() }));
        assert_eq!((daemon_line(&old), old.protocol_differs()), (format!("运行中 · 版本未知 · 协议未知（窗口 {}）", lila::PROTOCOL), true));
        // No lilad is not "a different one".
        assert!(!snap(None).protocol_differs());
    }

    #[test]
    fn the_panel_is_kept_open_with_its_view_and_width() {
        let tmp = std::env::temp_dir().join(format!("lila-app-panel-{}", std::process::id()));
        let paths = Paths { state: tmp.clone(), ..Paths::from_env() };
        assert_eq!(load_panel(&paths), (false, None, None));
        save_panel(&paths, true, "files", 412.4);
        assert_eq!(load_panel(&paths), (true, Some("files".to_owned()), Some(412.)));
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn ages_are_short() {
        let now = 10_000_000_000;
        assert_eq!(age(now - 20_000, now), "刚刚");
        assert_eq!(age(now - 5 * 60_000, now), "5 分钟");
        assert_eq!(age(now - 19 * 3_600_000, now), "19 小时");
        assert_eq!(age(now - 50 * 3_600_000, now), "2 天");
    }
}
