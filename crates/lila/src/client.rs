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

/// How often `lila hook PermissionRequest` asks lilad whether the person has answered.
const HOOK_ASK_EVERY: std::time::Duration = std::time::Duration::from_millis(500);

/// What a `PreToolUse` hook prints for lilad's reply (`{verdict, reason?}`): Claude Code's
/// `permissionDecision` of allow, deny or ask.
fn pre_tool_use_output(reply: &Value) -> Option<Value> {
    let verdict = reply["verdict"].as_str()?;
    let mut out = json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": verdict}});
    if let Some(reason) = reply["reason"].as_str() {
        out["hookSpecificOutput"]["permissionDecisionReason"] = json!(reason);
    }
    Some(out)
}

/// Waits for the person's answer to the request `id` of `thread`: `(decision, message)` once given, or `(None, None)` when the
/// card was dismissed (Claude Code asks in its terminal). Asks lilad every `every`; while lilad is away, keeps asking. A lilad that
/// was restarted does not know the request: it says `unknown`, and the request is sent again with its `payload`, so the card is back.
pub fn wait_for_answer(paths: &Paths, thread: &str, id: &str, payload: &Value, every: std::time::Duration) -> (Option<String>, Option<String>) {
    let mut id = id.to_owned();
    let mut remind = false;
    loop {
        std::thread::sleep(every);
        let mut params = json!({"thread": thread, "id": id});
        if remind {
            params["payload"] = payload.clone();
        }
        let Ok(a) = request(paths, "hook.answer", params, false) else {
            // lilad is away or busy; it may come back.
            continue;
        };
        if a["pending"] == true {
            remind = false;
            continue;
        }
        if a["unknown"] == true {
            // Said once: the next ask carries the request. If that is not understood either, give up to the terminal.
            if remind {
                return (None, None);
            }
            remind = true;
            continue;
        }
        if let Some(new) = a["wait"].as_str() {
            id = new.to_owned();
            remind = false;
            continue;
        }
        return (a["decision"].as_str().map(String::from), a["message"].as_str().map(String::from));
    }
}

/// `lila hook <event>`: a Claude Code hook in a thread's session. Hands the event
/// (its JSON on stdin) to lilad; for a permission request, waits for the person's
/// answer and prints the decision; for a tool call about to run (`PreToolUse`), prints lilad's
/// verdict. Never fails the session: with lilad away, or no answer in time, it prints nothing and
/// Claude Code decides alone (asks in its terminal).
pub fn hook(paths: &Paths, event: &str) -> Result<()> {
    let mut input = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)?;
    let payload: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let Some(thread) = std::env::var("LILA_THREAD").ok().filter(|t| !t.is_empty()) else { return Ok(()) };
    let Ok(r) = request(paths, "hook", json!({"thread": thread, "event": event, "payload": payload}), false) else { return Ok(()) };
    if event == "PreToolUse" {
        if let Some(out) = pre_tool_use_output(&r) {
            println!("{out}");
        }
        return Ok(());
    }
    if event != "PermissionRequest" {
        return Ok(());
    }
    let mut decision = r["decision"].as_str().map(String::from);
    let mut message = r["message"].as_str().map(String::from);
    if let Some(id) = r["wait"].as_str() {
        // The person may be away for as long as the thread lives (Claude Code lets this hook wait that long); the card
        // is there for as long as that, and when lilad is restarted meanwhile it comes back (`wait_for_answer`).
        (decision, message) = wait_for_answer(paths, &thread, id, &payload, HOOK_ASK_EVERY);
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

    /// A stand-in lilad that gives one reply per request, in turn (the last for ever), and keeps what it was asked.
    fn scripted(paths: &Paths, replies: Vec<Value>) -> std::sync::Arc<std::sync::Mutex<Vec<Value>>> {
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = asked.clone();
        let listener = UnixListener::bind(paths.socket().unwrap()).unwrap();
        std::thread::spawn(move || {
            for (n, conn) in listener.incoming().flatten().enumerate() {
                let mut line = String::new();
                BufReader::new(&conn).read_line(&mut line).unwrap();
                log.lock().unwrap().push(serde_json::from_str::<Value>(&line).unwrap()["params"].clone());
                writeln!(&conn, "{}", json!({"result": replies[n.min(replies.len() - 1)]})).unwrap();
            }
        });
        asked
    }

    #[test]
    fn a_hook_waits_through_a_restart_of_lilad_and_sends_its_request_again() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_in(tmp.path());
        let payload = json!({"tool_name": "Bash", "tool_input": {"command": "touch x"}});
        // Pending, then lilad was restarted (it does not know the request), then it knows it again, then the person answers.
        let asked = scripted(
            &paths,
            vec![json!({"pending": true}), json!({"decision": null, "unknown": true}), json!({"wait": "hook-1"}), json!({"pending": true}), json!({"decision": "allow", "message": null})],
        );
        let (decision, message) = wait_for_answer(&paths, "t1", "hook-1", &payload, std::time::Duration::from_millis(1));
        assert_eq!((decision.as_deref(), message), (Some("allow"), None));
        let asked = asked.lock().unwrap();
        assert_eq!(asked.len(), 5);
        // The request travels only when lilad said it does not know it.
        let carried: Vec<bool> = asked.iter().map(|p| p.get("payload").is_some()).collect();
        assert_eq!(carried, [false, false, true, false, false]);
    }

    #[test]
    fn a_hook_dismissed_or_not_understood_leaves_the_question_to_the_terminal() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_in(tmp.path());
        scripted(&paths, vec![json!({"decision": null})]);
        assert_eq!(wait_for_answer(&paths, "t1", "hook-1", &json!({}), std::time::Duration::from_millis(1)), (None, None));
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_in(tmp.path());
        scripted(&paths, vec![json!({"decision": null, "unknown": true})]);
        assert_eq!(wait_for_answer(&paths, "t1", "hook-1", &json!({}), std::time::Duration::from_millis(1)), (None, None), "told twice that nobody knows it");
    }

    #[test]
    fn a_hook_keeps_asking_while_lilad_is_away() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_in(tmp.path());
        let p2 = Paths { config: paths.config.clone(), data: paths.data.clone(), state: paths.state.clone(), isolated: true };
        // lilad comes up only after the hook has been asking for a while.
        let late = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(60));
            scripted(&p2, vec![json!({"decision": "deny", "message": "no"})]);
            std::thread::sleep(std::time::Duration::from_millis(500));
        });
        let (decision, message) = wait_for_answer(&paths, "t1", "hook-1", &json!({}), std::time::Duration::from_millis(5));
        assert_eq!((decision.as_deref(), message.as_deref()), (Some("deny"), Some("no")));
        late.join().unwrap();
    }

    #[test]
    fn a_pre_tool_use_hook_prints_the_verdict_in_claude_codes_words() {
        assert_eq!(
            pre_tool_use_output(&json!({"verdict": "deny", "reason": "read-only"})),
            Some(json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": "read-only"}}))
        );
        assert_eq!(pre_tool_use_output(&json!({"verdict": "allow"})), Some(json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "allow"}})));
        assert_eq!(pre_tool_use_output(&json!({})), None, "a reply without a verdict leaves Claude Code to decide");
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
