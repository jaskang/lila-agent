//! Talking to lilad over its socket, and starting it inside herdr-core when it is not running.

use crate::paths::Paths;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub fn hc_bin() -> String {
    std::env::var("LILA_HC").unwrap_or_else(|_| "hc".into())
}

/// Who is asking: a member (from the env lilad gave its terminal), else the person.
pub fn caller() -> Value {
    let cwd = std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    json!({"cwd": cwd, "project": var("LILA_PROJECT"), "member": var("LILA_MEMBER")})
}

fn try_request(paths: &Paths, method: &str, params: &Value) -> Result<Value, (bool, anyhow::Error)> {
    let sock = paths.socket().map_err(|e| (false, e))?;
    let mut conn = match UnixStream::connect(&sock) {
        Ok(c) => c,
        // A sandbox (codex workspace-write) refusing the socket, not a stopped daemon:
        // go through the project's spool folder, which the sandbox may write.
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return spool_request(method, params).map_err(|e| (false, e)),
        Err(e) => return Err((true, anyhow!("cannot reach lilad at {}: {e}", sock.display()))),
    };
    conn.set_read_timeout(Some(Duration::from_secs(120))).ok();
    let line = json!({"method": method, "params": params});
    writeln!(conn, "{line}").map_err(|e| (false, e.into()))?;
    let mut reply = String::new();
    BufReader::new(conn).read_line(&mut reply).map_err(|e| (false, e.into()))?;
    let v: Value = serde_json::from_str(&reply).map_err(|e| (false, anyhow!("bad reply from lilad: {e}")))?;
    if let Some(e) = v.get("error") {
        return Err((false, anyhow!("{}", e.as_str().unwrap_or("error"))));
    }
    Ok(v["result"].clone())
}

/// One request. With `autostart`, starts lilad first if it is not running.
pub fn request(paths: &Paths, method: &str, mut params: Value, autostart: bool) -> Result<Value> {
    if params.get("caller").is_none() {
        params["caller"] = caller();
    }
    match try_request(paths, method, &params) {
        Ok(v) => Ok(v),
        Err((true, e)) if autostart => {
            start_daemon(paths).context(e)?;
            try_request(paths, method, &params).map_err(|(_, e)| e)
        }
        Err((true, e)) => Err(e.context("lilad is not running; `lila up` starts it")),
        Err((false, e)) => Err(e),
    }
}

/// The project folder a member works in: `LILA_ROOT` (set by lilad), else `.lila/` above the cwd.
fn project_root() -> Option<PathBuf> {
    if let Some(r) = std::env::var_os("LILA_ROOT").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(r));
    }
    crate::paths::find_project_root(&std::env::current_dir().ok()?)
}

fn spool_request(method: &str, params: &Value) -> Result<Value> {
    let root = project_root().ok_or_else(|| anyhow!("the lilad socket is blocked (sandbox) and there is no .lila/ folder here to use instead"))?;
    let dir = crate::daemon::spool_dir(&root);
    let id = write_spool(&dir, method, params)?;
    let timeout = Duration::from_secs(std::env::var("LILA_SPOOL_TIMEOUT").ok().and_then(|v| v.parse().ok()).unwrap_or(60));
    await_spool(&dir, &id, timeout)
}

pub fn write_spool(dir: &Path, method: &str, params: &Value) -> Result<String> {
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let id = format!("{nanos}-{}", std::process::id());
    let tmp = dir.join(format!("{id}.req.tmp"));
    std::fs::write(&tmp, json!({"method": method, "params": params}).to_string()).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, dir.join(format!("{id}.req.json")))?;
    Ok(id)
}

pub fn await_spool(dir: &Path, id: &str, timeout: Duration) -> Result<Value> {
    let reply = dir.join(format!("{id}.reply.json"));
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(&reply) {
            let _ = std::fs::remove_file(&reply);
            let v: Value = serde_json::from_str(&text).map_err(|e| anyhow!("bad spool reply: {e}"))?;
            if let Some(e) = v.get("error") {
                bail!("{}", e.as_str().unwrap_or("error"));
            }
            return Ok(v["result"].clone());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = std::fs::remove_file(dir.join(format!("{id}.req.json")));
    bail!("lilad did not answer through {} within {}s; is it running? (`lila daemon status` from your own shell)", dir.display(), timeout.as_secs())
}

/// Hears from lilad whenever anything changes: one `{"event":"changed","seq":N}` per change.
/// The iterator ends when lilad goes away.
pub fn subscribe(paths: &Paths) -> Result<impl Iterator<Item = Value>> {
    let sock = paths.socket()?;
    let mut conn = UnixStream::connect(&sock).with_context(|| format!("cannot reach lilad at {}", sock.display()))?;
    writeln!(conn, "{}", json!({"method": "subscribe", "params": {}}))?;
    let mut lines = BufReader::new(conn).lines();
    let first = lines.next().ok_or_else(|| anyhow!("lilad closed the subscription"))??;
    let v: Value = serde_json::from_str(&first)?;
    if let Some(e) = v.get("error") {
        bail!("{}", e.as_str().unwrap_or("error"));
    }
    Ok(lines.map_while(Result::ok).filter_map(|l| serde_json::from_str(&l).ok()))
}

pub fn is_running(paths: &Paths) -> bool {
    try_request(paths, "ping", &json!({})).is_ok()
}

/// Running, stopped, or unreachable (and why).
pub fn daemon_state(paths: &Paths) -> Result<bool> {
    match try_request(paths, "ping", &json!({})) {
        Ok(_) => Ok(true),
        Err((true, _)) => Ok(false),
        Err((false, e)) => Err(e),
    }
}

/// The `lila` program: this one, a `lila` beside this program (lila-app in the
/// same folder), or `lila` on PATH. lilad and every member run it.
pub fn lila_bin() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    if exe.file_stem().is_some_and(|n| n == "lila") {
        return Ok(exe);
    }
    if let Some(sibling) = exe.parent().map(|d| d.join("lila"))
        && sibling.is_file()
    {
        return Ok(sibling);
    }
    crate::paths::which("lila", &std::env::var("PATH").unwrap_or_default())
        .ok_or_else(|| anyhow!("the `lila` command is not installed; run `cargo install --path crates/lila` in the lila-agent repo"))
}

/// Runs `lila daemon run` in its own herdr-core terminal, so it outlives this shell
/// and you can watch it with `hc read lilad`.
pub fn start_daemon(paths: &Paths) -> Result<()> {
    paths.ensure()?;
    let exe = lila_bin()?;
    let mut env = json!({"PATH": std::env::var("PATH").unwrap_or_default()});
    if let Some(h) = std::env::var_os("LILA_HOME") {
        env["LILA_HOME"] = json!(h.to_string_lossy());
    }
    if let Ok(hc) = std::env::var("LILA_HC") {
        env["LILA_HC"] = json!(hc);
    }
    let mut run = json!({
        "argv": [exe.to_string_lossy(), "daemon", "run"],
        "cwd": std::env::var("HOME").unwrap_or_else(|_| "/".into()),
        "env": env,
        "labels": {"lila.daemon": "1"},
        "no_parent": true,
        "no_notify": true,
    });
    if !paths.isolated {
        run["name"] = json!("lilad");
    }
    let mut hc = hc_client::Hc::connect(&hc_bin())?;
    let r = hc.call("terminal.run", run).map_err(|e| {
        if hc_client::error_kind(&e) == Some("conflict") || e.to_string().contains("name") {
            anyhow!("a terminal named lilad exists but does not answer; check `hc read lilad`, then `hc kill lilad` and retry ({e})")
        } else {
            e
        }
    })?;
    let addr = r["address"].as_str().unwrap_or("?").to_owned();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if is_running(paths) {
            eprintln!("lila: started lilad in terminal {addr}");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    bail!("lilad did not come up in terminal {addr}; see `hc read {addr}`")
}
