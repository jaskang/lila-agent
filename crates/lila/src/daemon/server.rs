//! The server: the socket, herdr-core's events, the check and commit threads, all feeding one `Daemon`.

use super::*;

// ---------------- the server ----------------

pub(super) enum Input {
    Request(String, Value, std::sync::mpsc::Sender<Value>),
    Hc(Value),
    /// A fresh `events.subscribe` snapshot and when it was taken.
    Snapshot(Value, i64),
    Tick,
    /// Look for spool requests (fast: sandboxed members wait on them).
    Spool,
    /// A flow's check finished: project, run anchor, result.
    CheckDone(String, String, CheckResult),
    /// An accepted chain's commit finished: project, chain, result.
    CommitDone(String, String, CommitResult),
    Stop,
}

/// Runs until `lila daemon stop` or the socket is removed.
pub fn serve(paths: Paths, hc_bin: String) -> Result<()> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::{Arc, Mutex, mpsc};

    paths.ensure()?;
    let sock = paths.socket()?;
    if std::os::unix::net::UnixStream::connect(&sock).is_ok() {
        bail!("lilad is already running on {}", sock.display());
    }
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).with_context(|| format!("bind {}", sock.display()))?;

    let store = Store::open(&paths.db())?;
    let hc = hc_client::Hc::connect(&hc_bin)?;
    let path_env = std::env::var("PATH").unwrap_or_default();
    let lila_bin = std::env::current_exe()?;
    let mut daemon = Daemon::new(store, HcTerminals(hc), paths.clone(), path_env.clone(), lila_bin);
    daemon.magpie = crate::magpie::find(&path_env);
    eprintln!("[lilad] listening on {} (pid {})", sock.display(), std::process::id());

    let (tx, rx) = mpsc::channel::<Input>();
    // Watchers (lila-app) that asked to hear when anything changes.
    let watchers: Arc<Mutex<Vec<mpsc::Sender<String>>>> = Arc::default();

    // Socket: one JSON request per line, one reply line per request.
    // `subscribe` turns the connection into a stream of `{"event":"changed"}` lines.
    let tx_sock = tx.clone();
    let watchers_sock = watchers.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(conn) = conn else { continue };
            let tx = tx_sock.clone();
            let watchers = watchers_sock.clone();
            std::thread::spawn(move || {
                let mut writer = match conn.try_clone() {
                    Ok(w) => w,
                    Err(_) => return,
                };
                for line in BufReader::new(conn).lines().map_while(Result::ok) {
                    let reply = match serde_json::from_str::<Value>(&line) {
                        Ok(req) if req["method"] == "subscribe" => {
                            let (wtx, wrx) = mpsc::channel::<String>();
                            watchers.lock().unwrap().push(wtx);
                            if writeln!(writer, "{}", json!({"result": {"subscribed": true}})).is_err() {
                                return;
                            }
                            for event in wrx {
                                if writeln!(writer, "{event}").is_err() {
                                    return;
                                }
                            }
                            return;
                        }
                        Ok(req) => {
                            let (rtx, rrx) = mpsc::channel();
                            let method = req["method"].as_str().unwrap_or_default().to_owned();
                            if tx.send(Input::Request(method, req["params"].clone(), rtx)).is_err() {
                                break;
                            }
                            rrx.recv().unwrap_or_else(|_| json!({"error": "daemon stopped"}))
                        }
                        Err(e) => json!({"error": format!("bad request: {e}")}),
                    };
                    if writeln!(writer, "{reply}").is_err() {
                        break;
                    }
                }
            });
        }
    });

    // herdr-core events, resubscribing when the core restarts.
    let tx_ev = tx.clone();
    std::thread::spawn(move || {
        loop {
            match hc_client::Events::subscribe(&hc_bin, json!({})) {
                Ok(mut ev) => {
                    if tx_ev.send(Input::Snapshot(ev.snapshot.clone(), now_ms())).is_err() {
                        return;
                    }
                    while let Ok(line) = ev.next_line() {
                        if tx_ev.send(Input::Hc(line)).is_err() {
                            return;
                        }
                    }
                    eprintln!("[lilad] lost herdr-core events; resubscribing");
                }
                Err(e) => eprintln!("[lilad] cannot subscribe to herdr-core events: {e:#}"),
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });

    let tx_spool = tx.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(250));
            if tx_spool.send(Input::Spool).is_err() {
                return;
            }
        }
    });

    let tx_tick = tx.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            if tx_tick.send(Input::Tick).is_err() {
                return;
            }
        }
    });

    spawn_checks(&mut daemon, &tx);
    let mut seen = daemon.fingerprint();
    let mut seq: u64 = 0;
    for input in rx {
        let now = now_ms();
        match input {
            Input::Request(method, params, reply) => {
                if method == "stop" {
                    let _ = reply.send(json!({"result": {"stopped": true}}));
                    let _ = tx.send(Input::Stop);
                    continue;
                }
                let out = match daemon.handle(&method, &params, now) {
                    Ok(v) => json!({"result": v}),
                    Err(e) => json!({"error": format!("{e:#}")}),
                };
                let _ = reply.send(out);
            }
            Input::Snapshot(snap, taken_ms) => {
                for rec in snap["terminals"].as_array().cloned().unwrap_or_default() {
                    daemon.on_terminal(&rec, now);
                }
                // Members whose terminal is not in the snapshot are gone.
                let known: Vec<String> = snap["terminals"].as_array().into_iter().flatten().filter(|t| t["state"] == "live").filter_map(|t| t["address"].as_str().map(String::from)).collect();
                daemon.reconcile(&known, taken_ms, now);
            }
            Input::Hc(line) => daemon.on_event(&line, now),
            Input::Tick => daemon.tick(now),
            Input::Spool => daemon.process_spool(now),
            Input::CheckDone(pid, anchor, r) => daemon.on_check_done(&pid, &anchor, &r, now),
            Input::CommitDone(pid, chain, r) => daemon.on_commit_done(&pid, &chain, &r, now),
            Input::Stop => break,
        }
        spawn_checks(&mut daemon, &tx);
        let fp = daemon.fingerprint();
        if fp != seen {
            seen = fp;
            seq += 1;
            let event = json!({"event": "changed", "seq": seq}).to_string();
            watchers.lock().unwrap().retain(|w| w.send(event.clone()).is_ok());
        }
    }
    let _ = std::fs::remove_file(&sock);
    eprintln!("[lilad] stopped");
    Ok(())
}

/// Runs each check and commit lilad asked for on its own thread.
pub(super) fn spawn_checks<T: Terminals>(daemon: &mut Daemon<T>, tx: &std::sync::mpsc::Sender<Input>) {
    for job in daemon.take_checks() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let r = crate::check::run(&job);
            let _ = tx.send(Input::CheckDone(job.project, job.anchor, r));
        });
    }
    for job in daemon.take_commits() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let r = crate::commit::run(&job);
            let _ = tx.send(Input::CommitDone(job.project, job.chain, r));
        });
    }
}
