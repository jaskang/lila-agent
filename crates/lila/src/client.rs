//! Talking to lilad over its socket, and starting it inside herdr-core when it is not running.

use crate::paths::Paths;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub fn hc_bin() -> String {
    std::env::var("LILA_HC").unwrap_or_else(|_| "hc".into())
}

fn try_request(paths: &Paths, method: &str, params: &Value) -> Result<Value, (bool, anyhow::Error)> {
    let sock = paths.socket().map_err(|e| (false, e))?;
    let mut conn = UnixStream::connect(&sock).map_err(|e| (true, anyhow!("cannot reach lilad at {}: {e}", sock.display())))?;
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
pub fn request(paths: &Paths, method: &str, params: Value, autostart: bool) -> Result<Value> {
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

/// How long `lila hook PermissionRequest` waits for the person before leaving the
/// question to the terminal (under the hook's own 600 s, `daemon::hooks`).
pub const HOOK_ANSWER_WAIT: std::time::Duration = std::time::Duration::from_secs(570);

/// `lila hook <event>`: a Claude Code hook in a thread's session. Hands the event
/// (its JSON on stdin) to lilad; for a permission request, waits for the person's
/// answer and prints the decision. Never fails the session: with lilad away, or no
/// answer in time, it prints nothing and Claude Code asks in its terminal.
pub fn hook(paths: &Paths, event: &str) -> Result<()> {
    let mut input = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)?;
    let payload: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let Some(thread) = std::env::var("LILA_THREAD").ok().filter(|t| !t.is_empty()) else { return Ok(()) };
    let Ok(r) = request(paths, "hook", json!({"thread": thread, "event": event, "payload": payload}), false) else { return Ok(()) };
    if event != "PermissionRequest" {
        return Ok(());
    }
    let mut decision = r["decision"].as_str().map(String::from);
    let mut message = r["message"].as_str().map(String::from);
    if let Some(id) = r["wait"].as_str() {
        let until = std::time::Instant::now() + HOOK_ANSWER_WAIT;
        while std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let Ok(a) = request(paths, "hook.answer", json!({"thread": thread, "id": id}), false) else { continue };
            if a["pending"] == true {
                continue;
            }
            decision = a["decision"].as_str().map(String::from);
            message = a["message"].as_str().map(String::from);
            break;
        }
    }
    if let Some(d) = decision {
        let behavior = if d == "allow" { "allow" } else { "deny" };
        let mut out = json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": {"behavior": behavior}}});
        if behavior == "deny" {
            out["hookSpecificOutput"]["decision"]["message"] = json!(message.unwrap_or_else(|| "The person said no in Lila.".into()));
        }
        println!("{out}");
    }
    Ok(())
}
