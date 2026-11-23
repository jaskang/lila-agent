//! Pure Rust desktop shell helpers: safe file opening, Finder reveal, local media validation,
//! macOS folder picker, and process RSS memory reporting.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::core_host::{data_dir, home_dir};

const MAX_MEDIA_BYTES: u64 = 20 * 1024 * 1024;

const BLOCKED_OPEN_EXTS: &[&str] = &[
    "workflow",
    "scpt",
    "applescript",
    "terminal",
    "inetloc",
    "webloc",
    "jar",
    "app",
    "command",
    "sh",
    "bash",
    "zsh",
    "exe",
    "bat",
    "cmd",
    "dmg",
    "pkg",
];

const MEDIA_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "avif", "bmp", "ico",
];

fn agent_dir() -> PathBuf {
    std::env::var("PI_CODING_AGENT_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".pi/agent"))
}

fn registered_project_paths() -> Vec<PathBuf> {
    let state_path = data_dir().join("state.json");
    let Ok(raw) = fs::read_to_string(state_path) else {
        return Vec::new();
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    parsed
        .get("projects")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("path").and_then(Value::as_str))
                .filter(|p| !p.is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn resolve_real_path(target: &Path) -> PathBuf {
    let abs = if target.is_absolute() {
        normalize_lexical(target)
    } else if let Ok(cwd) = std::env::current_dir() {
        normalize_lexical(&cwd.join(target))
    } else {
        normalize_lexical(target)
    };

    if abs.exists()
        && let Ok(canon) = fs::canonicalize(&abs)
    {
        return canon;
    }

    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = abs.as_path();
    while !cursor.exists() {
        let Some(name) = cursor.file_name() else {
            return abs;
        };
        missing.push(name.to_os_string());
        let Some(parent) = cursor.parent() else {
            return abs;
        };
        cursor = parent;
    }
    let mut resolved = fs::canonicalize(cursor).unwrap_or_else(|_| cursor.to_path_buf());
    for part in missing.into_iter().rev() {
        resolved.push(part);
    }
    resolved
}

fn is_within(root: &Path, target: &Path) -> bool {
    target.starts_with(root)
}

fn allowed_roots() -> Vec<PathBuf> {
    let mut roots = vec![agent_dir(), home_dir().join(".agents")];
    roots.extend(registered_project_paths());
    roots.into_iter().map(|r| resolve_real_path(&r)).collect()
}

fn allowed_log_files() -> Vec<PathBuf> {
    let dir = data_dir();
    vec![
        resolve_real_path(&dir.join("core.log")),
        resolve_real_path(&dir.join("core.log.1")),
    ]
}

fn is_allowed_path(real_target: &Path) -> bool {
    if allowed_roots()
        .iter()
        .any(|root| is_within(root, real_target))
    {
        return true;
    }
    allowed_log_files().iter().any(|log| log == real_target)
}

fn is_allowed_mcp_config(real_target: &Path) -> bool {
    if real_target == resolve_real_path(&agent_dir().join("mcp.json")) {
        return true;
    }
    registered_project_paths()
        .iter()
        .any(|p| real_target == resolve_real_path(&p.join(".pi/mcp.json")))
}

fn has_blocked_ext(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|ext| BLOCKED_OPEN_EXTS.contains(&ext.as_str()))
}

fn is_media_ext(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|ext| MEDIA_EXTS.contains(&ext.as_str()))
}

pub fn memory_mb() -> f64 {
    let pid = std::process::id().to_string();
    if let Ok(out) = Command::new("/bin/ps")
        .args(["-o", "rss=", "-p", &pid])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        && out.status.success()
        && let Ok(kb) = String::from_utf8_lossy(&out.stdout).trim().parse::<f64>()
    {
        return (kb / 1024.0 * 10.0).round() / 10.0;
    }
    0.0
}

pub fn open_whitelisted_path(path: &str) -> Result<(), String> {
    let raw = PathBuf::from(path);
    let real_target = resolve_real_path(&raw);
    if !is_allowed_path(&real_target) {
        return Err(format!("不允许打开该路径：{path}"));
    }
    if has_blocked_ext(&raw) || has_blocked_ext(&real_target) {
        return Err(format!("不允许直接打开可执行文件：{path}"));
    }
    if real_target.exists() {
        if let Ok(meta) = fs::metadata(&real_target)
            && meta.is_file()
            && (meta.permissions().mode() & 0o111) != 0
        {
            return Err(format!("不允许直接打开可执行文件：{path}"));
        }
    } else if is_allowed_mcp_config(&real_target) {
        if let Some(parent) = real_target.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(&real_target, "{\"mcpServers\":{}}\n")
            .map_err(|e| format!("初始化 mcp.json 失败: {e}"))?;
    }
    let status = Command::new("/usr/bin/open")
        .arg(&real_target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("打开文件失败: {e}"))?;
    if !status.success() {
        return Err(format!("系统未能打开路径：{path}"));
    }
    Ok(())
}

pub fn reveal_whitelisted_path(path: &str) -> Result<(), String> {
    let real_target = resolve_real_path(Path::new(path));
    if !is_allowed_path(&real_target) {
        return Err(format!("不允许定位该路径：{path}"));
    }
    let _ = Command::new("/usr/bin/open")
        .arg("-R")
        .arg(&real_target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    Ok(())
}

pub fn resolve_local_image(raw_path: &str, base_path: Option<&str>) -> Option<String> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with("data:image/") {
        return Some(trimmed.to_owned());
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        let local_http = trimmed.starts_with("http://localhost")
            || trimmed.starts_with("http://127.0.0.1")
            || trimmed.starts_with("https://localhost")
            || trimmed.starts_with("https://127.0.0.1");
        return local_http.then(|| trimmed.to_owned());
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    let raw = Path::new(trimmed);
    if raw.is_absolute() {
        candidates.push(raw.to_path_buf());
        if let Some(base) = base_path
            && Path::new(base).is_absolute()
        {
            let stripped = trimmed.trim_start_matches('/');
            if !stripped.is_empty() {
                candidates.push(Path::new(base).join(stripped));
                candidates.push(Path::new(base).join("public").join(stripped));
            }
        }
    } else if let Some(base) = base_path
        && Path::new(base).is_absolute()
    {
        candidates.push(Path::new(base).join(raw));
    }

    let mut roots = allowed_roots();
    let lila_tmp = std::env::temp_dir().join("lila");
    if lila_tmp.exists() {
        roots.push(resolve_real_path(&lila_tmp));
    }

    for candidate in candidates {
        if !is_media_ext(&candidate) || !candidate.is_file() {
            continue;
        }
        let Ok(meta) = fs::metadata(&candidate) else {
            continue;
        };
        if meta.len() > MAX_MEDIA_BYTES {
            continue;
        }
        let Ok(real) = fs::canonicalize(&candidate) else {
            continue;
        };
        if roots.iter().any(|root| is_within(root, &real)) {
            return Some(format!("file://{}", real.display()));
        }
    }
    None
}

pub async fn pick_directory() -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let picked = Command::new("/usr/bin/osascript")
            .args([
                "-e",
                "POSIX path of (choose folder with prompt \"选择项目目录\")",
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .and_then(|output| {
                if !output.status.success() {
                    return None;
                }
                let path = String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .trim_end_matches('/')
                    .to_owned();
                if path.is_empty() {
                    Some("/".to_owned())
                } else {
                    Some(path)
                }
            });
        let _ = tx.send(picked);
    });
    rx.await.ok().flatten()
}
