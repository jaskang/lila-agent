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
    store.projects().unwrap_or_default().into_iter().map(|p| Project { branch: git_branch(&p.cwd), title: title(&p.cwd), id: p.id, cwd: p.cwd }).collect()
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

/// Makes `dir` a Lila project if it is not one yet, then starts its team
/// (and lilad). Returns the project id.
pub fn create_and_start(paths: &Paths, dir: &std::path::Path) -> anyhow::Result<String> {
    if lila::paths::find_project_root(dir).is_none() {
        lila::init::init_project(paths, dir, None)?;
    } else {
        lila::roles::install_defaults(&paths.roles())?;
    }
    let p = Project { id: String::new(), cwd: dir.to_string_lossy().to_string(), branch: None, title: None };
    let v = act(paths, &p, "project.up", json!({}), true)?;
    Ok(v["project"].as_str().unwrap_or_default().to_owned())
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

/// HH:MM in local time.
pub fn clock(ms: i64) -> String {
    use std::sync::OnceLock;
    static OFFSET: OnceLock<i64> = OnceLock::new();
    let offset = *OFFSET.get_or_init(|| {
        std::process::Command::new("date")
            .arg("+%z")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|z| {
                let z = z.trim();
                let sign = if z.starts_with('-') { -1 } else { 1 };
                let d = z.trim_start_matches(['+', '-']);
                if d.len() != 4 {
                    return None;
                }
                Some(sign * (d[..2].parse::<i64>().ok()? * 3600 + d[2..].parse::<i64>().ok()? * 60))
            })
            .unwrap_or(0)
    });
    let t = (ms / 1000 + offset).rem_euclid(86_400);
    format!("{:02}:{:02}", t / 3600, t / 60 % 60)
}

/// Wall clock with milliseconds, for debug logs.
pub fn clock_ms() -> String {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    format!("{}:{:02}.{:03}", clock(ms), ms / 1000 % 60, ms % 1000)
}
