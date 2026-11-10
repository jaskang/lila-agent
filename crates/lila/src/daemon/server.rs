//! The server: the socket, herdr-core's events and the hosts, all feeding one `Daemon`.

use super::*;

// ---------------- the server ----------------

pub(super) enum Input {
    Request(String, Value, std::sync::mpsc::Sender<Value>),
    Hc(Value),
    /// A fresh `events.subscribe` snapshot and when it was taken.
    Snapshot(Value, i64),
    Tick,
    /// Read what Claude Code threads added to their transcripts (fast: the window follows along).
    Follow,
    /// A thread's host attached: the thread, its command channel.
    HostAttach(String, std::sync::mpsc::Sender<String>),
    /// An event from a thread's host.
    HostEvent(String, lila_agent::AgentEvent),
    /// A thread's host detached.
    HostGone(String),
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
                let mut lines = BufReader::new(conn).lines();
                while let Some(Ok(line)) = lines.next() {
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
                        // A thread's host: events in, commands out, until it goes.
                        Ok(req) if req["method"] == "host.attach" => {
                            let tid = req["params"]["thread"].as_str().unwrap_or_default().to_owned();
                            let (ctx, crx) = mpsc::channel::<String>();
                            if writeln!(writer, "{}", json!({"result": {"attached": true}})).is_err() || tx.send(Input::HostAttach(tid.clone(), ctx)).is_err() {
                                return;
                            }
                            std::thread::spawn(move || {
                                for cmd in crx {
                                    if writeln!(writer, "{cmd}").is_err() {
                                        return;
                                    }
                                }
                            });
                            for line in lines.by_ref().map_while(Result::ok) {
                                if let Ok(ev) = serde_json::from_str::<lila_agent::AgentEvent>(&line)
                                    && tx.send(Input::HostEvent(tid.clone(), ev)).is_err()
                                {
                                    return;
                                }
                            }
                            let _ = tx.send(Input::HostGone(tid));
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

    let tx_follow = tx.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(250));
            if tx_follow.send(Input::Follow).is_err() {
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
            Input::Follow => daemon.follow_transcripts(),
            Input::HostAttach(tid, ctx) => daemon.host_attach(&tid, ctx, now),
            Input::HostEvent(tid, ev) => daemon.host_event(&tid, ev, now),
            Input::HostGone(tid) => daemon.host_gone(&tid),
            Input::Stop => break,
        }
        let fp = daemon.fingerprint();
        if fp != seen {
            seen = fp;
            seq += 1;
            let (threads, projects) = daemon.take_dirty();
            let mut events = vec![json!({"event": "changed", "seq": seq})];
            events.extend(projects.iter().map(|p| json!({"event": "project", "project": p, "seq": seq})));
            events.extend(threads.iter().filter_map(|t| daemon.thread_event(t, seq)));
            let mut watchers = watchers.lock().unwrap();
            for event in events {
                let event = event.to_string();
                watchers.retain(|w| w.send(event.clone()).is_ok());
            }
        }
    }
    let _ = std::fs::remove_file(&sock);
    eprintln!("[lilad] stopped");
    Ok(())
}
