//! Core process discovery, Node >= 22.19 resolution, health checks, and detached launch.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub const PROTOCOL_VERSION: u32 = 7;
pub const DEFAULT_CORE_PORT: u16 = 7457;
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
const LOCK_STALE_SECS: u64 = 30;

#[derive(Debug, Clone)]
pub struct CoreConnectionInfo {
    pub ws_url: String,
    pub token: String,
}

#[derive(Debug, Deserialize)]
struct HealthResponse {
    ok: Option<bool>,
    app: Option<String>,
    #[serde(rename = "protocolVersion")]
    protocol_version: Option<u32>,
    pid: Option<u32>,
}

pub fn data_dir() -> PathBuf {
    if let Ok(custom) = std::env::var("LILA_DATA_DIR")
        && !custom.trim().is_empty()
    {
        return PathBuf::from(custom);
    }
    home_dir()
        .join("Library")
        .join("Application Support")
        .join("Lila")
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn core_host() -> String {
    std::env::var("LILA_CORE_HOST")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "127.0.0.1".to_owned())
}

pub fn core_port() -> u16 {
    std::env::var("LILA_CORE_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(DEFAULT_CORE_PORT)
}

fn url_host(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    }
}

fn parse_node_version(raw: &str) -> Option<(u32, u32, u32)> {
    let trimmed = raw.trim().trim_start_matches('v');
    let mut parts = trimmed.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    let patch = parts
        .next()
        .and_then(|s| s.split('-').next())
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    Some((major, minor, patch))
}

fn is_supported_node(version: (u32, u32, u32)) -> bool {
    version.0 > 22 || (version.0 == 22 && version.1 >= 19)
}

fn probe_node_binary(path: &Path) -> Option<(PathBuf, (u32, u32, u32))> {
    if !path.is_file() {
        return None;
    }
    let output = Command::new(path)
        .arg("-v")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = parse_node_version(&String::from_utf8_lossy(&output.stdout))?;
    if is_supported_node(version) {
        Some((path.to_path_buf(), version))
    } else {
        None
    }
}

/// Locates a local Node.js binary >= 22.19.
pub fn resolve_node_binary() -> Result<PathBuf, String> {
    if let Ok(explicit) = std::env::var("LILA_NODE_BIN")
        && !explicit.trim().is_empty()
    {
        let path = PathBuf::from(&explicit);
        if let Some((bin, _)) = probe_node_binary(&path) {
            return Ok(bin);
        }
        return Err(format!("LILA_NODE_BIN ({explicit}) 不可用或版本低于 Node 22.19"));
    }

    let home = home_dir();
    let mut candidates: Vec<PathBuf> = vec![
        PathBuf::from("/opt/homebrew/bin/node"),
        PathBuf::from("/usr/local/bin/node"),
        home.join(".local/share/mise/shims/node"),
        home.join(".volta/bin/node"),
        home.join(".asdf/shims/node"),
        home.join(".fnm/current/bin/node"),
    ];

    // Check PATH entries directly.
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            candidates.push(dir.join("node"));
        }
    }

    // Check nvm versions sorted newest-first.
    let nvm_versions = home.join(".nvm/versions/node");
    if let Ok(entries) = fs::read_dir(&nvm_versions) {
        let mut nvm_bins: Vec<((u32, u32, u32), PathBuf)> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name();
                let ver = parse_node_version(&name.to_string_lossy())?;
                Some((ver, entry.path().join("bin/node")))
            })
            .collect();
        nvm_bins.sort_by(|a, b| b.0.cmp(&a.0));
        candidates.extend(nvm_bins.into_iter().map(|(_, p)| p));
    }

    for candidate in candidates {
        if let Some((bin, _)) = probe_node_binary(&candidate) {
            return Ok(bin);
        }
    }

    // Last resort: ask user's login shell where `node` lives.
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_owned());
    if let Ok(output) = Command::new(shell)
        .args(["-lc", "command -v node"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        && output.status.success()
    {
        let resolved = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !resolved.is_empty()
            && let Some((bin, _)) = probe_node_binary(Path::new(&resolved))
        {
            return Ok(bin);
        }
    }

    Err("未找到 Node.js >= 22.19，请安装 Node 22.19+ 或设置环境变量 LILA_NODE_BIN".to_owned())
}

/// Locates `packages/core` directory (dev checkout or packaged Resources/core).
pub fn resolve_core_dir() -> Result<PathBuf, String> {
    if let Ok(explicit) = std::env::var("LILA_CORE_DIR")
        && !explicit.trim().is_empty()
    {
        let p = PathBuf::from(explicit);
        if p.join("src/main.ts").is_file() {
            return Ok(p);
        }
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    // Packaged macOS bundle: MyApp.app/Contents/MacOS/bin -> ../Resources/core
    if let Ok(exe) = std::env::current_exe()
        && let Some(macos_dir) = exe.parent()
        && let Some(contents_dir) = macos_dir.parent()
    {
        candidates.push(contents_dir.join("Resources/core"));
    }
    // Dev checkout: compile-time manifest dir (`packages/app-native` -> `packages/core`).
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(packages_dir) = manifest_dir.parent() {
        candidates.push(packages_dir.join("core"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("packages/core"));
        candidates.push(cwd.join("../core"));
    }

    for candidate in candidates {
        if candidate.join("src/main.ts").is_file() {
            return Ok(candidate);
        }
    }

    Err("未找到 packages/core/src/main.ts，请设置 LILA_CORE_DIR".to_owned())
}

fn decode_chunked_body(raw: &str) -> Option<String> {
    let mut out = String::new();
    let mut rest = raw;
    loop {
        let (size_line, after_size) = rest.split_once("\r\n")?;
        let hex = size_line.split(';').next()?.trim();
        let size = usize::from_str_radix(hex, 16).ok()?;
        if size == 0 {
            return Some(out);
        }
        if after_size.len() < size + 2 {
            return None;
        }
        out.push_str(&after_size[..size]);
        rest = &after_size[size + 2..];
    }
}

fn parse_health_response(raw_http: &str) -> Option<HealthResponse> {
    let (head, body) = raw_http.split_once("\r\n\r\n")?;
    let status_line = head.lines().next()?;
    if !status_line.contains(" 200 ") {
        return None;
    }
    let is_chunked = head.lines().skip(1).any(|line| {
        let lower = line.to_ascii_lowercase();
        lower.starts_with("transfer-encoding:") && lower.contains("chunked")
    });
    let decoded = if is_chunked {
        decode_chunked_body(body)?
    } else {
        body.to_owned()
    };
    serde_json::from_str::<HealthResponse>(decoded.trim()).ok()
}

async fn check_health(host: &str, port: u16) -> Option<HealthResponse> {
    let addr = format!("{}:{port}", url_host(host));
    let mut stream = tokio::time::timeout(Duration::from_secs(1), TcpStream::connect(&addr))
        .await
        .ok()?
        .ok()?;
    let request = format!(
        "GET /health HTTP/1.0\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    );
    tokio::time::timeout(Duration::from_secs(1), stream.write_all(request.as_bytes()))
        .await
        .ok()?
        .ok()?;
    let mut buf = Vec::with_capacity(512);
    tokio::time::timeout(Duration::from_secs(1), stream.read_to_end(&mut buf))
        .await
        .ok()?
        .ok()?;
    let text = String::from_utf8_lossy(&buf);
    parse_health_response(&text)
}

fn read_recorded_pid(dir: &Path) -> Option<u32> {
    let raw = fs::read_to_string(dir.join("core.pid")).ok()?;
    raw.trim().parse::<u32>().ok().filter(|&pid| pid > 0)
}

fn is_owned_core(health: &HealthResponse, dir: &Path) -> bool {
    if health.app.as_deref() == Some("lila-core") {
        return true;
    }
    matches!((health.pid, read_recorded_pid(dir)), (Some(a), Some(b)) if a == b)
}

fn send_signal(pid: u32, sig: i32) -> Result<bool, String> {
    // SAFETY: standard POSIX kill syscall with validated pid and signal.
    let rc = unsafe { libc::kill(pid as libc::pid_t, sig) };
    if rc == 0 {
        return Ok(false);
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::ESRCH) {
        return Ok(true);
    }
    Err(format!("向旧 core 进程 (pid {pid}) 发送信号失败: {err}"))
}

fn rotate_log_if_needed(log_path: &Path) {
    if let Ok(meta) = fs::metadata(log_path)
        && meta.len() >= MAX_LOG_BYTES
    {
        let rotated = PathBuf::from(format!("{}.1", log_path.display()));
        let _ = fs::rename(log_path, rotated);
    }
}

struct LockGuard(PathBuf);

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

async fn acquire_core_lock(dir: &Path) -> Result<LockGuard, String> {
    if !dir.is_dir() {
        fs::create_dir_all(dir).map_err(|e| format!("创建数据目录失败: {e}"))?;
    }
    let lock_path = dir.join("core.lock");
    let deadline = Instant::now() + Duration::from_secs(15);

    while Instant::now() < deadline {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&lock_path)
        {
            Ok(mut file) => {
                let _ = writeln!(file, "{}", std::process::id());
                return Ok(LockGuard(lock_path));
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if let Ok(meta) = fs::metadata(&lock_path)
                    && let Ok(modified) = meta.modified()
                    && SystemTime::now()
                        .duration_since(modified)
                        .unwrap_or_default()
                        > Duration::from_secs(LOCK_STALE_SECS)
                {
                    let _ = fs::remove_file(&lock_path);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(err) => return Err(format!("获取 core 启动锁失败: {err}")),
        }
    }

    Err(format!("获取 core 启动锁超时：{}", lock_path.display()))
}

async fn wait_for<F, Fut>(mut check: F, timeout: Duration) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if check().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// Ensures a compatible `lila-core` daemon is running and returns WebSocket connection info.
pub async fn ensure_core() -> Result<CoreConnectionInfo, String> {
    let host = core_host();
    let port = core_port();
    let dir = data_dir();
    let ws_url = format!("ws://{}:{port}/ws", url_host(&host));

    let healthy = |h: Option<HealthResponse>| {
        h.and_then(|res| {
            (res.ok.unwrap_or(false) && res.protocol_version == Some(PROTOCOL_VERSION))
                .then_some(())
        })
        .is_some()
    };

    if !healthy(check_health(&host, port).await) {
        let _lock = acquire_core_lock(&dir).await?;
        if !healthy(check_health(&host, port).await) {
            if let Some(current) = check_health(&host, port).await
                && let Some(pid) = current.pid
            {
                if !is_owned_core(&current, &dir) {
                    return Err(format!(
                        "端口 {port} 已被其他进程占用（pid {pid}），无法启动 Lila core"
                    ));
                }
                let mut exited = send_signal(pid, libc::SIGTERM)?;
                if !exited {
                    exited = wait_for(|| async { check_health(&host, port).await.is_none() }, Duration::from_secs(5)).await;
                }
                if !exited {
                    exited = send_signal(pid, libc::SIGKILL)?;
                    if !exited {
                        exited = wait_for(|| async { check_health(&host, port).await.is_none() }, Duration::from_secs(2)).await;
                    }
                }
                if !exited {
                    return Err(format!("旧的 Lila core（pid {pid}）未在规定时间内退出"));
                }
            }

            let node_bin = resolve_node_binary()?;
            let core_dir = resolve_core_dir()?;

            // Fast-fail in dev checkout when workspace deps or @lila/protocol build output are missing.
            if let Some(packages_dir) = core_dir.parent() {
                let protocol_pkg = packages_dir.join("protocol/package.json");
                let protocol_dist = packages_dir.join("protocol/dist/index.js");
                let has_deps = core_dir.join("node_modules").is_dir()
                    || packages_dir
                        .parent()
                        .is_some_and(|root| root.join("node_modules").is_dir());
                if protocol_pkg.is_file() && (!has_deps || !protocol_dist.is_file()) {
                    return Err(
                        "缺少 core 依赖或 @lila/protocol 构建产物，请在仓库根目录运行：bun install && bun run build:protocol"
                            .to_owned(),
                    );
                }
            }

            if !dir.is_dir() {
                fs::create_dir_all(&dir).map_err(|e| format!("创建数据目录失败: {e}"))?;
            }
            let log_path = dir.join("core.log");
            rotate_log_if_needed(&log_path);

            let log_out = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .map_err(|e| format!("打开 core 日志失败: {e}"))?;
            let log_err = log_out
                .try_clone()
                .map_err(|e| format!("复制日志句柄失败: {e}"))?;

            let mut cmd = Command::new(&node_bin);
            cmd.arg(core_dir.join("src/main.ts"))
                .current_dir(&core_dir)
                .env("LILA_CORE_HOST", &host)
                .env("LILA_CORE_PORT", port.to_string())
                .env("LILA_DATA_DIR", &dir)
                .stdin(Stdio::null())
                .stdout(Stdio::from(log_out))
                .stderr(Stdio::from(log_err));

            // Detach core into its own session so it survives app quit.
            // SAFETY: setsid is async-signal-safe and called immediately before exec.
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }

            let mut child = cmd
                .spawn()
                .map_err(|e| format!("启动 core 失败 ({}): {e}", node_bin.display()))?;

            let deadline = Instant::now() + Duration::from_secs(10);
            let mut ready = false;
            while Instant::now() < deadline {
                if check_health(&host, port)
                    .await
                    .and_then(|h| h.protocol_version)
                    == Some(PROTOCOL_VERSION)
                {
                    ready = true;
                    break;
                }
                if let Ok(Some(status)) = child.try_wait() {
                    return Err(format!(
                        "core 进程提前退出（{status}），日志见 {}",
                        log_path.display()
                    ));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            if !ready {
                return Err(format!(
                    "core 没有在 http://{}:{port} 启动，日志见 {}",
                    url_host(&host),
                    log_path.display()
                ));
            }
        }
    }

    let token_path = dir.join("core-token");
    let token = fs::read_to_string(&token_path)
        .map_err(|e| format!("读取 core-token 失败 ({}): {e}", token_path.display()))?
        .trim()
        .to_owned();
    if token.is_empty() {
        return Err("core-token 为空".to_owned());
    }

    Ok(CoreConnectionInfo { ws_url, token })
}

#[cfg(test)]
mod tests {
    use super::parse_health_response;

    #[test]
    fn parses_plain_and_chunked_health_responses() {
        let plain = "HTTP/1.0 200 OK\r\ncontent-type: application/json\r\n\r\n{\"ok\":true,\"app\":\"lila-core\",\"protocolVersion\":7,\"pid\":42}";
        let parsed = parse_health_response(plain).expect("plain response");
        assert_eq!(parsed.ok, Some(true));
        assert_eq!(parsed.app.as_deref(), Some("lila-core"));
        assert_eq!(parsed.protocol_version, Some(7));
        assert_eq!(parsed.pid, Some(42));

        let body = "{\"ok\":true,\"app\":\"lila-core\",\"protocolVersion\":7,\"pid\":99}";
        let chunked = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
            body.len(),
            body
        );
        let parsed = parse_health_response(&chunked).expect("chunked response");
        assert_eq!(parsed.protocol_version, Some(7));
        assert_eq!(parsed.pid, Some(99));
    }
}
