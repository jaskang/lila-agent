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
    // A merge runs the person's commit hooks, a worktree is a checkout, and a restore reads and writes the whole folder: these may take a while.
    let wait = if matches!(method, "thread.merge" | "thread.new" | "thread.restore") { 900 } else { 120 };
    conn.set_read_timeout(Some(Duration::from_secs(wait))).ok();
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

/// What a running lilad says about itself (`ping`). A lilad from before the version handshake says nothing: `protocol` is `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DaemonInfo {
    pub pid: Option<u64>,
    pub version: Option<String>,
    pub protocol: Option<u32>,
}

impl DaemonInfo {
    /// Whether it speaks this build's protocol (`lila::PROTOCOL`).
    pub fn is_current(&self) -> bool {
        self.protocol == Some(crate::PROTOCOL)
    }

    pub fn from_ping(v: &Value) -> DaemonInfo {
        DaemonInfo { pid: v["pid"].as_u64(), version: v["version"].as_str().map(String::from), protocol: v["protocol"].as_u64().and_then(|p| u32::try_from(p).ok()) }
    }
}

/// Whether lilad is there, and whether it speaks this build's protocol (`lila::PROTOCOL`).
#[derive(Debug)]
pub enum DaemonVersion {
    /// Running, same protocol.
    Current(DaemonInfo),
    /// Running, another protocol (or too old to report one).
    Different(DaemonInfo),
    NotRunning,
    /// Something answers, or fails, in a way that is not "not running" (the reason).
    Unreachable(String),
}

pub fn daemon_version(paths: &Paths) -> DaemonVersion {
    match try_request(paths, "ping", &json!({})) {
        Ok(v) => match DaemonInfo::from_ping(&v) {
            info if info.is_current() => DaemonVersion::Current(info),
            info => DaemonVersion::Different(info),
        },
        Err((true, _)) => DaemonVersion::NotRunning,
        Err((false, e)) => DaemonVersion::Unreachable(format!("{e:#}")),
    }
}

/// Stops lilad if it runs (its threads keep running: each is in a terminal of its own), waits until the socket is gone, and starts it again from
/// the `lila` on disk. Blocks for a few seconds: not for a UI thread.
pub fn restart_daemon(paths: &Paths) -> Result<()> {
    if daemon_state(paths)? {
        request(paths, "stop", json!({}), false)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while is_running(paths) || paths.socket().is_ok_and(|s| s.exists()) {
            if Instant::now() > deadline {
                bail!("lilad did not stop within 10 s");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    start_daemon(paths)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// A stand-in lilad that answers every request with `reply`.
    fn stand_in(paths: &Paths, reply: Value) {
        let listener = UnixListener::bind(paths.socket().unwrap()).unwrap();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let mut line = String::new();
                BufReader::new(&conn).read_line(&mut line).unwrap();
                writeln!(&conn, "{reply}").unwrap();
            }
        });
    }

    fn paths_in(root: &std::path::Path) -> Paths {
        let paths = Paths { config: root.join("config"), data: root.join("data"), state: root.join("state"), isolated: true };
        paths.ensure().unwrap();
        paths
    }

    #[test]
    fn the_protocol_of_a_running_lilad_is_compared_with_ours() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_in(tmp.path());
        assert!(matches!(daemon_version(&paths), DaemonVersion::NotRunning));
        stand_in(&paths, json!({"result": {"pong": true, "pid": 9, "version": "1.2.3", "protocol": crate::PROTOCOL}}));
        match daemon_version(&paths) {
            DaemonVersion::Current(i) => assert_eq!(i, DaemonInfo { pid: Some(9), version: Some("1.2.3".into()), protocol: Some(crate::PROTOCOL) }),
            other => panic!("{other:?}"),
        }
        assert!(is_running(&paths) && daemon_state(&paths).unwrap());
    }

    #[test]
    fn another_protocol_and_a_lilad_too_old_to_say_both_differ() {
        for (reply, protocol) in [(json!({"pong": true, "pid": 9}), None), (json!({"pong": true, "pid": 9, "version": "0.0.1", "protocol": crate::PROTOCOL + 1}), Some(crate::PROTOCOL + 1))] {
            let tmp = tempfile::tempdir().unwrap();
            let paths = paths_in(tmp.path());
            stand_in(&paths, json!({"result": reply}));
            match daemon_version(&paths) {
                DaemonVersion::Different(i) => assert_eq!(i.protocol, protocol),
                other => panic!("{other:?}"),
            }
            // The callers that only ask "is it there" are not bothered.
            assert!(is_running(&paths));
        }
    }

    #[test]
    fn a_garbled_answer_is_unreachable_not_stopped() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_in(tmp.path());
        let listener = UnixListener::bind(paths.socket().unwrap()).unwrap();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let mut line = String::new();
                BufReader::new(&conn).read_line(&mut line).unwrap();
                writeln!(&conn, "not json").unwrap();
            }
        });
        assert!(matches!(daemon_version(&paths), DaemonVersion::Unreachable(_)));
    }
}
