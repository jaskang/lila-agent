//! Talking to lilad over its socket, and starting it inside herdr-core when it is not running.

use crate::paths::Paths;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
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
    let mut conn = UnixStream::connect(&sock).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            // A sandbox (codex workspace-write) refusing the socket, not a stopped daemon.
            return (false, anyhow!(
                "a sandbox blocks the lilad socket {}: allow this agent network access (codex: -c sandbox_workspace_write.network_access=true) or approve lila commands ({e})",
                sock.display()
            ));
        }
        (true, anyhow!("cannot reach lilad at {}: {e}", sock.display()))
    })?;
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

/// Runs `lila daemon run` in its own herdr-core terminal, so it outlives this shell
/// and you can watch it with `hc read lilad`.
pub fn start_daemon(paths: &Paths) -> Result<()> {
    paths.ensure()?;
    let exe = std::env::current_exe()?;
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
