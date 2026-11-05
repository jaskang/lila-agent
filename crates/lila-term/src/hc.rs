// Ported from herdr-app (https://github.com/herdrdev/herdr-app, Apache-2.0), trimmed for Lila.
//! One framed connection to herdr-core through `hc connect`: JSON-RPC on
//! channel 0, terminal byte streams on their own channels.

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const HEADER: usize = 9;
pub const DATA: u8 = 0;
pub const CREDIT: u8 = 1;
pub const CLOSE: u8 = 2;
pub const RESIZE: u8 = 3;
pub const FOCUS: u8 = 4;
pub const FACTS: u8 = 5;
pub const AUTOMATIC_INPUT: u8 = 6;

pub enum Stream {
    Data(Vec<u8>),
    /// The core ended the stream; the payload says why (e.g. the program exited).
    Closed(Value),
    /// The connection to the core dropped. The program is most likely still
    /// running (cores restart and their programs survive), so reattach later.
    Lost,
}

/// Event streams are lines; DATA frame boundaries can fall anywhere in a line.
#[derive(Default)]
pub struct EventLines {
    pending: Vec<u8>,
    /// The last event applied: older ones were in the snapshot.
    seq: u64,
    /// Every terminal the stream describes, in its order (schema/README.md, Events).
    terminals: Vec<Value>,
    /// Each terminal's highest `herdr.seen` among the label changes since the
    /// last `take_seen`: `terminals` keeps only the last, which may hide a
    /// write of ours under a later, lower one (docs/labels.md).
    seen: HashMap<String, u64>,
    /// A fresh snapshot replaced the terminals since the last `take_seen`.
    resynced: bool,
}

/// What a push changed.
#[derive(Default)]
pub struct Pushed {
    /// A layout document changed, or events were missed: read them again.
    pub docs: bool,
    pub terminals: bool,
}

impl EventLines {
    /// From `events.subscribe`'s reply: its `seq` and `terminals`.
    pub fn new(snapshot: &Value) -> Self {
        let terminals = snapshot["terminals"].as_array().cloned().unwrap_or_default();
        EventLines { seq: snapshot["seq"].as_u64().unwrap_or(0), terminals, resynced: true, ..Default::default() }
    }

    /// The `herdr.seen` values that went by, and whether a snapshot replaced everything.
    pub fn take_seen(&mut self) -> (HashMap<String, u64>, bool) {
        (std::mem::take(&mut self.seen), std::mem::take(&mut self.resynced))
    }

    fn note_seen(&mut self, address: &Value, labels: &Value) {
        let (Some(address), Some(seen)) = (address.as_str(), labels["herdr.seen"].as_str().and_then(|s| s.parse::<u64>().ok())) else { return };
        let high = self.seen.entry(address.to_owned()).or_default();
        *high = (*high).max(seen);
    }

    /// The live terminals, as `terminal.list` puts them.
    pub fn live(&self) -> Value {
        let live: Vec<&Value> = self.terminals.iter().filter(|t| t["state"] == "live").collect();
        json!({ "terminals": live })
    }

    pub fn push(&mut self, bytes: &[u8]) -> Pushed {
        self.pending.extend_from_slice(bytes);
        let mut consumed = 0;
        let mut pushed = Pushed::default();
        let lines: Vec<Value> = self
            .pending
            .iter()
            .enumerate()
            .filter(|(_, b)| **b == b'\n')
            .map(|(i, _)| {
                let line = serde_json::from_slice::<Value>(&self.pending[consumed..i]).unwrap_or_default();
                consumed = i + 1;
                line
            })
            .collect();
        self.pending.drain(..consumed);
        for line in lines {
            let p = &line["params"];
            match line["method"].as_str() {
                Some("missed") => {
                    self.terminals = p["terminals"].as_array().cloned().unwrap_or_default();
                    self.seq = p["seq"].as_u64().unwrap_or(self.seq);
                    self.resynced = true;
                    pushed.docs = true;
                    pushed.terminals = true;
                }
                Some("event") if p["seq"].as_u64().is_none_or(|s| s > self.seq) => {
                    self.seq = p["seq"].as_u64().unwrap_or(self.seq);
                    if p["type"] == "doc" {
                        pushed.docs = true;
                    } else {
                        pushed.terminals |= self.apply(p);
                    }
                }
                _ => {}
            }
        }
        pushed
    }

    /// One event's change to the terminal it names (schema/README.md, Events).
    fn apply(&mut self, p: &Value) -> bool {
        let (kind, address, data) = (p["type"].as_str().unwrap_or_default(), &p["terminal"], &p["data"]);
        if matches!(kind, "created" | "restarted" | "labels") {
            self.note_seen(address, &data["labels"]);
        }
        let at = self.terminals.iter().position(|t| &t["address"] == address);
        let whole = match kind {
            "created" | "restarted" => Some(data),
            "labels" => data.get("record"),
            _ => None,
        };
        if let Some(record) = whole {
            match at {
                Some(i) => self.terminals[i] = record.clone(),
                None => self.terminals.push(record.clone()),
            }
            return true;
        }
        let Some(i) = at else { return false };
        if kind == "removed" {
            self.terminals.remove(i);
            return true;
        }
        let t = &mut self.terminals[i];
        let mut set = |keys: &[&str]| keys.iter().for_each(|k| t[*k] = data[*k].clone());
        match kind {
            "finished" | "lost" => {
                set(&["reason", "exit_code", "signal", "finished_ms"]);
                t["state"] = if kind == "lost" { "live" } else { "finished" }.into();
                if kind == "lost" {
                    t["pid"] = Value::Null;
                }
                t.as_object_mut().map(|r| r.remove("foreground"));
            }
            "status" => set(&["status", "records", "reported"]),
            "turn" => set(&["turn"]),
            "title" => set(&["title"]),
            "labels" => set(&["labels"]),
            "keep" => set(&["keep"]),
            "size" => set(&["cols", "rows"]),
            "foreground" => t["foreground"] = data.clone(),
            _ => return false,
        }
        true
    }
}

#[derive(Default)]
struct Routes {
    closed: bool,
    automatic_input: HashSet<u32>,
    replies: HashMap<u64, mpsc::Sender<Result<Value, Value>>>,
    streams: HashMap<u32, async_channel::Sender<Stream>>,
    /// Frames for a channel whose attach reply hasn't been handled yet.
    early: HashMap<u32, Vec<Stream>>,
}

#[derive(Debug)]
pub struct RpcError(pub Value);

impl RpcError {
    pub fn kind(&self) -> Option<&str> { self.0.pointer("/data/kind").and_then(Value::as_str) }
}
impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.get("message").and_then(Value::as_str).unwrap_or("core call failed"))?;
        if let Some(hint) = self.0.pointer("/data/hint").and_then(Value::as_str) { write!(f, " ({hint})")?; }
        Ok(())
    }
}
impl std::error::Error for RpcError {}

pub fn error_kind(error: &anyhow::Error) -> Option<&str> {
    error.downcast_ref::<RpcError>().and_then(RpcError::kind)
}

pub struct Core {
    /// What the core said when we connected (generation, version, methods).
    pub hello: Value,
    /// Frames for the writer thread: a core that stops reading never blocks the caller (the UI).
    writer: mpsc::Sender<Vec<u8>>,
    routes: Arc<Mutex<Routes>>,
    next_id: AtomicU64,
    child: Mutex<Child>,
    /// Closes when the connection to the core drops.
    lost: async_channel::Receiver<()>,
}

pub struct Attached {
    pub channel: u32,
    pub stream: async_channel::Receiver<Stream>,
    pub terminal: Value,
}

fn encode(channel: u32, kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + payload.len());
    out.extend_from_slice(&channel.to_le_bytes());
    out.push(kind);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

fn read_frame(r: &mut impl Read) -> Option<(u32, u8, Vec<u8>)> {
    let mut h = [0u8; HEADER];
    r.read_exact(&mut h).ok()?;
    let channel = u32::from_le_bytes(h[0..4].try_into().unwrap());
    let len = u32::from_le_bytes(h[5..9].try_into().unwrap()) as usize;
    let mut payload = vec![0; len];
    r.read_exact(&mut payload).ok()?;
    Some((channel, h[4], payload))
}

impl Core {
    /// In-memory outbound frames for view tests; no herdr daemon or model CLI.
    #[cfg(test)]
    pub(crate) fn for_test() -> (Arc<Self>, mpsc::Receiver<Vec<u8>>) {
        let (writer, frames) = mpsc::channel();
        let (_, lost) = async_channel::bounded(1);
        let mut child = std::process::Command::new("true").spawn().unwrap();
        child.wait().unwrap();
        (Arc::new(Self { hello: Value::Null, writer, routes: Arc::new(Mutex::new(Routes::default())), next_id: AtomicU64::new(1), child: Mutex::new(child), lost }), frames)
    }

    /// Connects to the user's core (starting it if needed) and switches to framed mode.
    pub fn connect() -> Result<Arc<Core>> {
        let mut child = std::process::Command::new(hc_bin())
            .arg("connect")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("cannot run `hc connect`: is herdr-core installed?")?;
        let mut stdin = child.stdin.take().unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"hello\",\"params\":{\"framed\":true}}\n")?;
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let hello: Value = serde_json::from_str(&line).context("bad hello reply from core")?;
        if let Some(err) = hello.get("error") {
            bail!("core refused hello: {err}");
        }

        let (writer, frames) = mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new().name("hc-writer".into()).spawn(move || {
            while let Ok(frame) = frames.recv() {
                if stdin.write_all(&frame).is_err() {
                    break;
                }
            }
        })?;

        let routes = Arc::new(Mutex::new(Routes::default()));
        let (lost_tx, lost) = async_channel::bounded::<()>(1);
        let core = Arc::new(Core {
            hello: hello.get("result").cloned().unwrap_or(Value::Null),
            writer,
            routes: routes.clone(),
            next_id: AtomicU64::new(1),
            child: Mutex::new(child),
            lost,
        });
        std::thread::Builder::new().name("hc-reader".into()).spawn(move || {
            let _lost_tx = lost_tx;
            while let Some((channel, kind, payload)) = read_frame(&mut reader) {
                let mut r = routes.lock().unwrap();
                if channel == 0 {
                    let Ok(msg) = serde_json::from_slice::<Value>(&payload) else { continue };
                    let Some(id) = msg.get("id").and_then(Value::as_u64) else { continue };
                    if let Some(tx) = r.replies.remove(&id) {
                        let _ = tx.send(match msg.get("error") {
                            Some(e) => Err(e.clone()),
                            None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                        });
                    }
                    continue;
                }
                let item = match kind {
                    DATA => Stream::Data(payload),
                    CLOSE => Stream::Closed(serde_json::from_slice(&payload).unwrap_or(Value::Null)),
                    _ => continue,
                };
                let closing = matches!(item, Stream::Closed(_));
                match r.streams.get(&channel) {
                    Some(tx) => {
                        let _ = tx.try_send(item);
                    }
                    None => r.early.entry(channel).or_default().push(item),
                }
                if closing {
                    r.automatic_input.remove(&channel);
                    r.streams.remove(&channel);
                }
            }
            // The core went away: tell every pane, and fail calls still waiting.
            let mut r = routes.lock().unwrap();
            r.closed = true;
            r.automatic_input.clear();
            for (_, tx) in r.streams.drain() {
                let _ = tx.try_send(Stream::Lost);
            }
            for (_, tx) in r.replies.drain() {
                let _ = tx.send(Err(json!({"message": "lost the core"})));
            }
        })?;
        Ok(core)
    }

    /// Resolves once the connection to the core is gone.
    pub async fn closed(&self) {
        let _ = self.lost.recv().await;
    }

    pub fn send(&self, channel: u32, kind: u8, payload: &[u8]) {
        let _ = self.writer.send(encode(channel, kind, payload));
    }

    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.call_timeout(method, params, Duration::from_secs(10))
    }

    pub fn call_timeout(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        {
            let mut routes = self.routes.lock().unwrap();
            if routes.closed { bail!("{method}: lost the core"); }
            routes.replies.insert(id, tx);
        }
        let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.send(0, DATA, req.to_string().as_bytes());
        match rx.recv_timeout(timeout) {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(anyhow::Error::new(RpcError(e)).context(method.to_owned())),
            Err(_) => {
                self.routes.lock().unwrap().replies.remove(&id);
                Err(anyhow!("{method}: no reply from core"))
            },
        }
    }

    /// Starts a program in a new core terminal and returns its address (`tN`).
    /// Starts a terminal, here or on another machine (`on`: its name in `hc machine ls`).
    pub fn run(&self, argv: Vec<String>, cwd: Option<String>, labels: Value, on: Option<&str>, size: Option<(u16, u16)>, display: Value) -> Result<String> {
        let env = json!({"TERM": "xterm-256color", "COLORTERM": "truecolor", "TERM_PROGRAM": "herdr"});
        let mut params = json!({"argv": argv, "cwd": cwd, "env": env, "labels": labels, "on": on, "keep": true, "display": display});
        // Released cores ignore unknown parameters, including when forwarding to another machine.
        if let Some((cols, rows)) = size {
            params["size"] = json!({"cols": cols, "rows": rows});
        }
        let r = self.call("terminal.run", params)?;
        r.get("address")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("terminal.run returned no address: {r}"))
    }

    /// One subscription includes local and forwarded remote events.
    /// The event stream, and its snapshot (`seq`, `terminals`) to apply it to.
    pub fn subscribe(&self) -> Result<(u32, async_channel::Receiver<Stream>, Value)> {
        let result = self.call("events.subscribe", json!({}))?;
        let channel = result["channel"].as_u64().ok_or_else(|| anyhow!("subscription returned no channel"))? as u32;
        let (tx, rx) = async_channel::unbounded();
        let mut r = self.routes.lock().unwrap();
        for item in r.early.remove(&channel).unwrap_or_default() { let _ = tx.try_send(item); }
        r.streams.insert(channel, tx);
        drop(r);
        self.credit(channel, 1024 * 1024);
        Ok((channel, rx, result))
    }

    /// `observe`: watch only; the core never takes typing or a size from this viewer.
    pub fn attach(&self, terminal: &str, cols: u16, rows: u16, display: Value, observe: bool) -> Result<Attached> {
        let result = self.call(
            "terminal.attach",
            json!({"terminal": terminal, "cols": cols, "rows": rows, "history": 2000, "display": display, "observe": observe}),
        )?;
        let channel = result["channel"].as_u64().ok_or_else(|| anyhow!("attach returned no channel"))? as u32;
        let (tx, rx) = async_channel::unbounded();
        let mut r = self.routes.lock().unwrap();
        for item in r.early.remove(&channel).unwrap_or_default() {
            let _ = tx.try_send(item);
        }
        r.streams.insert(channel, tx);
        if self.hello["features"].as_array().is_some_and(|features| features.iter().any(|f| f == "automatic_input")) && result["automatic_input"] == true {
            r.automatic_input.insert(channel);
        }
        Ok(Attached { channel, stream: rx, terminal: result["terminal"].clone() })
    }

    pub fn input(&self, channel: u32, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.send(channel, DATA, bytes);
        }
    }

    /// Terminal-engine replies are not typing. Older paths require legacy DATA.
    pub fn automatic_input(&self, channel: u32, bytes: &[u8]) {
        if !bytes.is_empty() {
            let kind = if self.routes.lock().unwrap().automatic_input.contains(&channel) { AUTOMATIC_INPUT } else { DATA };
            self.send(channel, kind, bytes);
        }
    }

    pub fn credit(&self, channel: u32, n: usize) {
        self.send(channel, CREDIT, &(n as u32).to_le_bytes());
    }

    pub fn resize(&self, channel: u32, cols: u16, rows: u16) {
        let mut p = cols.to_le_bytes().to_vec();
        p.extend(rows.to_le_bytes());
        self.send(channel, RESIZE, &p);
    }

    /// This viewer's display facts changed (colors, cell size): the core
    /// answers the program's queries with them.
    pub fn facts(&self, channel: u32, display: &Value) {
        self.send(channel, FACTS, display.to_string().as_bytes());
    }

    pub fn focus(&self, channel: u32, focused: bool) {
        self.send(channel, FOCUS, &[focused as u8]);
    }

    pub fn detach(&self, channel: u32) {
        let mut routes = self.routes.lock().unwrap();
        routes.streams.remove(&channel);
        routes.automatic_input.remove(&channel);
        drop(routes);
        self.send(channel, CLOSE, &[]);
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The `hc` program: `LILA_HC` overrides it.
pub fn hc_bin() -> String {
    std::env::var("LILA_HC").unwrap_or_else(|_| "hc".into())
}
