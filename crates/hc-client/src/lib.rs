//! A small herdr-core client: JSON-RPC 2.0 in plain mode (one message per line)
//! over `hc connect`'s stdin/stdout. See herdr-core's schema/README.md.

use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::fmt;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// A JSON-RPC error from the core, with its stable `kind` and `hint`.
#[derive(Debug, Clone)]
pub struct HcError {
    pub kind: String,
    pub message: String,
    pub hint: Option<String>,
}

impl fmt::Display for HcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.kind)?;
        if let Some(h) = &self.hint {
            write!(f, ": {h}")?;
        }
        Ok(())
    }
}

impl std::error::Error for HcError {}

pub struct Hc {
    _child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: u64,
}

fn spawn(hc_bin: &str) -> Result<(Child, ChildStdin, BufReader<ChildStdout>)> {
    let mut child = Command::new(hc_bin)
        .arg("connect")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("cannot run `{hc_bin} connect`: is herdr-core installed?"))?;
    let stdin = child.stdin.take().expect("piped");
    let reader = BufReader::new(child.stdout.take().expect("piped"));
    Ok((child, stdin, reader))
}

fn read_json(reader: &mut BufReader<ChildStdout>) -> Result<Value> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        anyhow::bail!("herdr-core closed the connection");
    }
    serde_json::from_str(&line).with_context(|| format!("bad line from herdr-core: {line:?}"))
}

fn rpc_error(e: &Value) -> HcError {
    HcError {
        kind: e["data"]["kind"].as_str().unwrap_or("error").to_owned(),
        message: e["message"].as_str().unwrap_or("unknown error").to_owned(),
        hint: e["data"]["hint"].as_str().map(String::from),
    }
}

impl Hc {
    pub fn connect(hc_bin: &str) -> Result<Hc> {
        let (child, stdin, reader) = spawn(hc_bin)?;
        Ok(Hc { _child: child, stdin, reader, next_id: 1 })
    }

    /// One request, waiting for its reply. A JSON-RPC error comes back as an [`HcError`].
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{req}")?;
        self.stdin.flush()?;
        loop {
            let msg = read_json(&mut self.reader)?;
            if msg["id"] != id {
                continue; // a notification; this connection does not subscribe
            }
            if let Some(e) = msg.get("error") {
                return Err(rpc_error(e).into());
            }
            return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

/// An `events.subscribe` stream on its own connection: the snapshot, then notifications.
pub struct Events {
    _child: Child,
    _stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    pub snapshot: Value,
}

impl Events {
    pub fn subscribe(hc_bin: &str, params: Value) -> Result<Events> {
        let (child, mut stdin, mut reader) = spawn(hc_bin)?;
        writeln!(stdin, "{}", json!({"jsonrpc": "2.0", "id": 1, "method": "events.subscribe", "params": params}))?;
        stdin.flush()?;
        let reply = read_json(&mut reader)?;
        if let Some(e) = reply.get("error") {
            return Err(rpc_error(e).into());
        }
        Ok(Events { _child: child, _stdin: stdin, reader, snapshot: reply["result"].clone() })
    }

    /// The next notification (`event`, `heartbeat` or `missed`); an error when the core goes away.
    pub fn next_line(&mut self) -> Result<Value> {
        read_json(&mut self.reader)
    }
}

/// The `kind` of an hc error inside an anyhow chain, if it is one.
pub fn error_kind(e: &anyhow::Error) -> Option<&str> {
    e.downcast_ref::<HcError>().map(|h| h.kind.as_str())
}
