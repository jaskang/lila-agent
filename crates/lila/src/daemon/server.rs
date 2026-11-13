//! The server: the socket, herdr-core's events and the hosts, all feeding one `Daemon`.

use super::*;

// ---------------- the server ----------------

pub(super) enum Input {
    Request(String, Value, std::sync::mpsc::Sender<Value>),
    Hc(Value),
    /// A fresh `events.subscribe` snapshot and when it was taken.
    Snapshot(Value, i64),
    Tick,
    /// A thread's host attached: the thread, its command channel.
    HostAttach(String, std::sync::mpsc::Sender<String>),
    /// An event from a thread's host.
    HostEvent(String, lila_agent::AgentEvent),
    /// A thread's host detached.
    HostGone(String),
    /// A job off the loop is done: the job, what it gave, and who is waiting for the answer (none: lilad asked for it).
    JobDone(Job, std::result::Result<Output, String>, Option<std::sync::mpsc::Sender<Value>>),
    Stop,
}

type Watchers = std::sync::Arc<std::sync::Mutex<Vec<std::sync::mpsc::Sender<String>>>>;

/// The `id` a request carries, if any: a string or a number, echoed unchanged in its reply (`Err`: it is something else).
fn request_id(req: &Value) -> std::result::Result<Option<Value>, Value> {
    match req.get("id") {
        None | Some(Value::Null) => Ok(None),
        Some(id) if id.is_string() || id.is_number() => Ok(Some(id.clone())),
        Some(_) => Err(json!({"error": "bad request: id must be a string or a number"})),
    }
}

/// A reply with the request's id (when it had one) beside the result or the error.
fn with_id(mut reply: Value, id: &Option<Value>) -> Value {
    if let (Some(id), Some(obj)) = (id, reply.as_object_mut()) {
        obj.insert("id".into(), id.clone());
    }
    reply
}

/// One socket connection: one JSON request per line, one reply line per request, in order.
/// `subscribe` turns it into a stream of `{"event":"changed"}` lines, `host.attach` into a host's channel.
fn connection(conn: std::os::unix::net::UnixStream, tx: std::sync::mpsc::Sender<Input>, watchers: Watchers) {
    use std::io::{BufRead, BufReader, Write};
    use std::sync::mpsc;

    let mut writer = match conn.try_clone() {
        Ok(w) => w,
        Err(_) => return,
    };
    let mut lines = BufReader::new(conn).lines();
    while let Some(Ok(line)) = lines.next() {
        let parsed = serde_json::from_str::<Value>(&line).map_err(|e| json!({"error": format!("bad request: {e}")})).and_then(|req| request_id(&req).map(|id| (req, id)));
        let reply = match parsed {
            Ok((req, id)) if req["method"] == "subscribe" => {
                let (wtx, wrx) = mpsc::channel::<String>();
                watchers.lock().unwrap().push(wtx);
                if writeln!(writer, "{}", with_id(json!({"result": {"subscribed": true}}), &id)).is_err() {
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
            Ok((req, id)) if req["method"] == "host.attach" => {
                let tid = req["params"]["thread"].as_str().unwrap_or_default().to_owned();
                let (ctx, crx) = mpsc::channel::<String>();
                if writeln!(writer, "{}", with_id(json!({"result": {"attached": true}}), &id)).is_err() || tx.send(Input::HostAttach(tid.clone(), ctx)).is_err() {
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
            Ok((req, id)) => {
                let (rtx, rrx) = mpsc::channel();
                let method = req["method"].as_str().unwrap_or_default().to_owned();
                if tx.send(Input::Request(method, req["params"].clone(), rtx)).is_err() {
                    break;
                }
                with_id(rrx.recv().unwrap_or_else(|_| json!({"error": "daemon stopped"})), &id)
            }
            Err(reply) => reply,
        };
        if writeln!(writer, "{reply}").is_err() {
            break;
        }
    }
}

/// Runs until `lila daemon stop` or the socket is removed.
pub fn serve(paths: Paths, hc_bin: String) -> Result<()> {
    use std::os::unix::net::UnixListener;
    use std::sync::{Arc, mpsc};

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
    eprintln!("[lilad] listening on {} (pid {})", sock.display(), std::process::id());

    let (tx, rx) = mpsc::channel::<Input>();
    // Watchers (lila-app) that asked to hear when anything changes.
    let watchers: Watchers = Arc::default();

    // Socket: one JSON request per line, one reply line per request.
    // `subscribe` turns the connection into a stream of `{"event":"changed"}` lines.
    let tx_sock = tx.clone();
    let watchers_sock = watchers.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(conn) = conn else { continue };
            let tx = tx_sock.clone();
            let watchers = watchers_sock.clone();
            std::thread::spawn(move || connection(conn, tx, watchers));
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

    let tx_tick = tx.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            if tx_tick.send(Input::Tick).is_err() {
                return;
            }
        }
    });

    // Jobs run on threads of their own, so the loop goes on while git works; the summaries of changes one at a time on one.
    let tx_job = tx.clone();
    let (serial_tx, serial_rx) = mpsc::channel::<Job>();
    let tx_serial = tx.clone();
    std::thread::spawn(move || {
        for job in serial_rx {
            let result = job.run();
            if tx_serial.send(Input::JobDone(job, result, None)).is_err() {
                return;
            }
        }
    });
    let launch = move |job: Job, reply: Option<mpsc::Sender<Value>>| {
        if job.serial() {
            let _ = serial_tx.send(job);
            return;
        }
        let tx = tx_job.clone();
        std::thread::spawn(move || {
            let result = job.run();
            let _ = tx.send(Input::JobDone(job, result, reply));
        });
    };

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
                match daemon.begin(&method, &params, now) {
                    Ok(Begin::Done(v)) => {
                        let _ = reply.send(json!({"result": v}));
                    }
                    Ok(Begin::Job(job)) => launch(job, Some(reply)),
                    Err(e) => {
                        let _ = reply.send(json!({"error": format!("{e:#}")}));
                    }
                }
            }
            Input::JobDone(job, result, reply) => match (daemon.finish(job, result, now), reply) {
                (Ok(v), Some(reply)) => {
                    let _ = reply.send(json!({"result": v}));
                }
                (Err(e), Some(reply)) => {
                    let _ = reply.send(json!({"error": format!("{e:#}")}));
                }
                (Err(e), None) => daemon.say(format!("job: {e:#}")),
                (Ok(_), None) => {}
            },
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
            Input::HostAttach(tid, ctx) => daemon.host_attach(&tid, ctx, now),
            Input::HostEvent(tid, ev) => daemon.host_event(&tid, ev, now),
            Input::HostGone(tid) => daemon.host_gone(&tid),
            Input::Stop => break,
        }
        for job in daemon.take_jobs() {
            launch(job, None);
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

#[cfg(test)]
mod tests {
    use super::{Input, Watchers, connection};
    use serde_json::{Value, json};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    /// A connection served by `connection`, with a stand-in for the loop that answers requests by echoing the method.
    fn socket() -> (UnixStream, Watchers) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<Input>();
        let watchers = Watchers::default();
        let w = watchers.clone();
        std::thread::spawn(move || connection(theirs, tx, w));
        std::thread::spawn(move || {
            for input in rx {
                if let Input::Request(method, _, reply) = input {
                    let _ = reply.send(if method == "boom" { json!({"error": "nope"}) } else { json!({"result": {"method": method}}) });
                }
            }
        });
        (ours, watchers)
    }

    fn ask(conn: &mut UnixStream, reader: &mut BufReader<UnixStream>, line: &str) -> Value {
        writeln!(conn, "{line}").unwrap();
        let mut reply = String::new();
        reader.read_line(&mut reply).unwrap();
        serde_json::from_str(&reply).unwrap()
    }

    #[test]
    fn a_requests_id_comes_back_in_its_reply() {
        let (mut conn, _w) = socket();
        let mut reader = BufReader::new(conn.try_clone().unwrap());
        // Without an id the reply is exactly what it was.
        assert_eq!(ask(&mut conn, &mut reader, r#"{"method":"ping","params":{}}"#), json!({"result": {"method": "ping"}}));
        // A string, a number, and the same id on an error.
        assert_eq!(ask(&mut conn, &mut reader, r#"{"id":"a-1","method":"ping"}"#), json!({"id": "a-1", "result": {"method": "ping"}}));
        assert_eq!(ask(&mut conn, &mut reader, r#"{"id":7,"method":"ping"}"#), json!({"id": 7, "result": {"method": "ping"}}));
        assert_eq!(ask(&mut conn, &mut reader, r#"{"id":"e","method":"boom"}"#), json!({"id": "e", "error": "nope"}));
        // null counts as no id; anything else is a bad request, which has no id to echo.
        assert_eq!(ask(&mut conn, &mut reader, r#"{"id":null,"method":"ping"}"#), json!({"result": {"method": "ping"}}));
        let bad = ask(&mut conn, &mut reader, r#"{"id":{"x":1},"method":"ping"}"#);
        assert!(bad["error"].as_str().unwrap().contains("id must be"), "{bad}");
        assert!(bad.get("id").is_none());
        // The connection goes on after a bad line, which can't be parsed and so has no id either.
        let junk = ask(&mut conn, &mut reader, "not json");
        assert!(junk["error"].as_str().unwrap().starts_with("bad request"), "{junk}");
        assert_eq!(ask(&mut conn, &mut reader, r#"{"id":1,"method":"ping"}"#)["id"], 1);
    }

    #[test]
    fn subscribe_echoes_the_id_in_its_first_reply_only() {
        let (mut conn, watchers) = socket();
        let mut reader = BufReader::new(conn.try_clone().unwrap());
        assert_eq!(ask(&mut conn, &mut reader, r#"{"id":"s","method":"subscribe","params":{}}"#), json!({"id": "s", "result": {"subscribed": true}}));
        for w in watchers.lock().unwrap().iter() {
            w.send(json!({"event": "changed", "seq": 1}).to_string()).unwrap();
        }
        let mut event = String::new();
        reader.read_line(&mut event).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&event).unwrap(), json!({"event": "changed", "seq": 1}));
    }

    #[test]
    fn host_attach_echoes_the_id() {
        let (mut conn, _w) = socket();
        let mut reader = BufReader::new(conn.try_clone().unwrap());
        assert_eq!(ask(&mut conn, &mut reader, r#"{"id":3,"method":"host.attach","params":{"thread":"t1"}}"#), json!({"id": 3, "result": {"attached": true}}));
    }
}
