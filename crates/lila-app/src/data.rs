//! What the window shows, fetched from lilad (or the database when it is down).

use lila::paths::Paths;
use serde_json::{Value, json};

#[derive(Clone, Default)]
pub struct Project {
    pub id: String,
    pub cwd: String,
    /// `title` in project.toml: what the window calls it.
    pub title: Option<String>,
    /// Its `.lila/project.toml` is gone: registered, but lilad can't load it.
    pub missing: bool,
}

impl Project {
    pub fn label(&self) -> String {
        self.title.clone().unwrap_or_else(|| self.id.clone())
    }
}

fn title(cwd: &str) -> Option<String> {
    let text = std::fs::read_to_string(std::path::Path::new(cwd).join(".lila").join("project.toml")).ok()?;
    lila_core::ProjectConfig::parse(&text).ok()?.title.filter(|t| !t.trim().is_empty())
}

/// `/Users/me/x` as `~/x`.
/// A long path as its last `n` characters, so the folder's own name shows.
pub fn tail(path: &str, n: usize) -> String {
    let count = path.chars().count();
    if count <= n { path.to_owned() } else { format!("…{}", path.chars().skip(count - n + 1).collect::<String>()) }
}

pub fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_owned(),
    }
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub daemon: bool,
    /// The project the rest was fetched for.
    pub project: Option<String>,
    pub projects: Vec<Project>,
    /// The database couldn't be read: `projects` is empty, not known.
    pub projects_unknown: bool,
    /// `status` of the selected project: members, open tasks, inbox count.
    pub status: Option<Value>,
    pub tasks: Vec<Value>,
    pub inbox: Vec<Value>,
    /// `task.show` of the open task: body, children, events, files.
    pub detail: Option<Value>,
    /// `request.list`: every request and whose move it is, in the window's order.
    pub requests: Vec<Value>,
    /// `request.show` of the selected request: its tasks, history, inbox items.
    pub request: Option<Value>,
    /// `conversation.read` of the shown member, when it is shown as its conversation.
    pub conversation: Option<Value>,
    pub error: Option<String>,
}

/// Requests as the person, for one project.
pub fn caller(p: &Project) -> Value {
    json!({"cwd": p.cwd, "project": p.id, "member": null})
}

fn projects_from_db(paths: &Paths) -> anyhow::Result<Vec<Project>> {
    let store = lila_store::Store::open(&paths.db())?;
    Ok(store.projects()?.into_iter().map(|p| Project { title: title(&p.cwd), missing: !std::path::Path::new(&p.cwd).join(".lila/project.toml").exists(), id: p.id, cwd: p.cwd }).collect())
}

pub fn fetch(paths: &Paths, selected: Option<&str>, task: Option<&str>, request: Option<&str>, conv: Option<&crate::conversation::ConvWant>) -> Snapshot {
    let mut snap = Snapshot { daemon: lila::client::is_running(paths), ..Default::default() };
    match projects_from_db(paths) {
        Ok(p) => snap.projects = p,
        Err(e) => {
            snap.projects_unknown = true;
            snap.error = Some(format!("{e:#}"));
            return snap;
        }
    }
    let Some(p) = selected.and_then(|id| snap.projects.iter().find(|p| p.id == id)).cloned() else { return snap };
    snap.project = Some(p.id.clone());
    if !snap.daemon {
        return snap;
    }
    let call = |method: &str, params: Value| {
        let mut params = params;
        params["caller"] = caller(&p);
        lila::client::request(paths, method, params, false)
    };
    match call("status", json!({})) {
        Ok(v) => snap.status = Some(v),
        Err(e) => {
            snap.error = Some(format!("{e:#}"));
            return snap;
        }
    }
    snap.tasks = call("task.ls", json!({"all": true})).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    snap.inbox = call("inbox", json!({})).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    snap.requests = call("request.list", json!({})).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    // An inbox item's id names no request: it is shown from `inbox`.
    if let Some(id) = request.filter(|id| snap.requests.iter().any(|r| r["id"] == *id)) {
        snap.request = call("request.show", json!({"id": id})).ok();
    }
    if let Some(id) = task {
        snap.detail = call("task.show", json!({"id": id})).ok();
    }
    if let Some(c) = conv {
        snap.conversation = call("conversation.read", json!({"member": c.member, "session": c.session, "after": c.after})).ok();
    }
    snap
}

/// A macOS notification, through the system's own `osascript`.
pub fn notify(title: &str, body: &str) {
    let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let script = format!("display notification {} with title \"Lila\" subtitle {}", quote(body), quote(title));
    std::thread::spawn(move || {
        let _ = std::process::Command::new("osascript").arg("-e").arg(script).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    });
}

/// Opens a file (relative paths from the project folder) with the system's default app.
pub fn open_path(project_cwd: &str, path: &str) {
    let p = std::path::Path::new(path);
    let full = if p.is_absolute() { p.to_path_buf() } else { std::path::Path::new(project_cwd).join(p) };
    let _ = std::process::Command::new("open").arg(full).spawn();
}

/// One action as the person. `autostart`: start lilad first if needed (`project.up`).
pub fn act(paths: &Paths, p: &Project, method: &str, mut params: Value, autostart: bool) -> anyhow::Result<Value> {
    params["caller"] = caller(p);
    lila::client::request(paths, method, params, autostart)
}

/// Removes a project from Lila: lilad stops its team and forgets it; with
/// lilad down, its rows go from the database directly. Its folder stays.
pub fn remove_project(paths: &Paths, p: &Project) -> anyhow::Result<()> {
    if lila::client::is_running(paths) {
        act(paths, p, "project.remove", json!({}), false)?;
    } else {
        lila_store::Store::open(&paths.db())?.remove_project(&p.id)?;
    }
    Ok(())
}

/// Writes a missing `.lila/project.toml` again with the default team, under the
/// id the project is registered with, and has a running lilad load it (no
/// member starts). Returns what the window should say, if anything.
pub fn restore_config(paths: &Paths, p: &Project, daemon: bool) -> anyhow::Result<()> {
    let dir = std::path::Path::new(&p.cwd);
    if !dir.is_dir() {
        anyhow::bail!("项目文件夹 {} 不在了", tilde(&p.cwd));
    }
    if !dir.join(".lila/project.toml").exists() {
        lila::init::init_project(paths, dir, Some(p.id.clone()))?;
    }
    if daemon {
        act(paths, p, "project.reload", json!({}), false)?;
    }
    Ok(())
}

/// The project shown last time, kept across launches.
fn last_file(paths: &Paths) -> std::path::PathBuf {
    paths.state.join("app-tabs.json")
}

pub fn load_last(paths: &Paths) -> Option<String> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(last_file(paths)).ok()?).ok()?;
    v["selected"].as_str().map(String::from)
}

fn views_file(paths: &Paths) -> std::path::PathBuf {
    paths.state.join("app-views.json")
}

/// Members shown as their terminal rather than their conversation ("project/member").
pub fn load_views(paths: &Paths) -> std::collections::HashSet<String> {
    std::fs::read_to_string(views_file(paths)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v["tui"].as_array().cloned()).into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect()
}

pub fn save_views(paths: &Paths, tui: &std::collections::HashSet<String>) {
    let mut list: Vec<&String> = tui.iter().collect();
    list.sort();
    let _ = std::fs::create_dir_all(&paths.state);
    let _ = std::fs::write(views_file(paths), json!({ "tui": list }).to_string());
}

pub fn save_last(paths: &Paths, selected: Option<&str>) {
    let _ = std::fs::create_dir_all(&paths.state);
    let _ = std::fs::write(last_file(paths), json!({ "selected": selected }).to_string());
}

/// `ms` (Unix milliseconds) in the local time zone.
pub fn local(ms: i64) -> chrono::DateTime<chrono::Local> {
    use chrono::TimeZone;
    chrono::Local.timestamp_millis_opt(ms).single().unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::UNIX_EPOCH.with_timezone(&chrono::Local))
}

/// HH:MM in local time.
pub fn clock(ms: i64) -> String {
    local(ms).format("%H:%M").to_string()
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Wall clock with milliseconds, for debug logs.
pub fn clock_ms() -> String {
    chrono::Local::now().format("%H:%M:%S%.3f").to_string()
}
