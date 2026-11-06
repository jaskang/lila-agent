//! What the window shows, fetched from lilad (or the database when it is down).

use lila::paths::Paths;
use serde_json::{Value, json};

#[derive(Clone, Default)]
pub struct Project {
    pub id: String,
    pub cwd: String,
    /// The checked-out git branch, if the folder is a repository.
    pub branch: Option<String>,
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

/// The branch `.git/HEAD` points at (a short commit id when detached).
fn git_branch(cwd: &str) -> Option<String> {
    let head = std::fs::read_to_string(std::path::Path::new(cwd).join(".git").join("HEAD")).ok()?;
    let head = head.trim();
    Some(match head.strip_prefix("ref: refs/heads/") {
        Some(b) => b.to_owned(),
        None => head.chars().take(7).collect(),
    })
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
    /// The project the rest was fetched for.
    pub project: Option<String>,
    pub projects: Vec<Project>,
    /// `status` of the selected project: members, open tasks, inbox count.
    pub status: Option<Value>,
    pub tasks: Vec<Value>,
    pub inbox: Vec<Value>,
    pub log: Vec<Value>,
    /// `task.show` of the open task: body, children, events, files.
    pub detail: Option<Value>,
    pub error: Option<String>,
}

/// Requests as the person, for one project.
pub fn caller(p: &Project) -> Value {
    json!({"cwd": p.cwd, "project": p.id, "member": null})
}

fn projects_from_db(paths: &Paths) -> Vec<Project> {
    let Ok(store) = lila_store::Store::open(&paths.db()) else { return Vec::new() };
    store.projects().unwrap_or_default().into_iter().map(|p| Project { branch: git_branch(&p.cwd), title: title(&p.cwd), missing: !std::path::Path::new(&p.cwd).join(".lila/project.toml").exists(), id: p.id, cwd: p.cwd }).collect()
}

pub fn fetch(paths: &Paths, selected: Option<&str>, task: Option<&str>) -> Snapshot {
    let mut snap = Snapshot { daemon: lila::client::is_running(paths), projects: projects_from_db(paths), ..Default::default() };
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
    snap.log = call("log", json!({"limit": 40})).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    if let Some(id) = task {
        snap.detail = call("task.show", json!({"id": id})).ok();
    }
    snap
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

/// The project tabs open in the window, kept across launches.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Tabs {
    pub open: Vec<String>,
    pub selected: Option<String>,
}

fn tabs_file(paths: &Paths) -> std::path::PathBuf {
    paths.state.join("app-tabs.json")
}

pub fn load_tabs(paths: &Paths) -> Option<Tabs> {
    serde_json::from_str(&std::fs::read_to_string(tabs_file(paths)).ok()?).ok()
}

pub fn save_tabs(paths: &Paths, tabs: &Tabs) {
    let _ = std::fs::create_dir_all(&paths.state);
    if let Ok(text) = serde_json::to_string(tabs) {
        let _ = std::fs::write(tabs_file(paths), text);
    }
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

/// Days from the local day of `now` to that of `ms`: 0 today, 1 tomorrow, -1 yesterday.
pub fn days_from_today(ms: i64, now: i64) -> i64 {
    (local(ms).date_naive() - local(now).date_naive()).num_days()
}

/// When something happens, for people: "今天 14:20", "明天 09:00", "11月12日 14:20".
pub fn local_when(ms: i64, now: i64) -> String {
    let prefix = match days_from_today(ms, now) {
        0 => "今天".to_owned(),
        1 => "明天".to_owned(),
        -1 => "昨天".to_owned(),
        _ => local(ms).format("%-m月%-d日").to_string(),
    };
    format!("{prefix} {}", clock(ms))
}

/// Wall clock with milliseconds, for debug logs.
pub fn clock_ms() -> String {
    chrono::Local::now().format("%H:%M:%S%.3f").to_string()
}
