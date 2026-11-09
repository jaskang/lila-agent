//! `lila host <runtime>`: runs a member's agent through its protocol inside the
//! member's hc terminal (architecture v3 §4 D1). It starts the agent, attaches
//! to lilad over its socket (events out, commands in; reattaching when lilad
//! comes back), reports its status to hc with OSC 7501 like any program, and
//! prints a readable log: what the terminal shows.

use crate::paths::Paths;
use anyhow::{Context, Result, anyhow};
use lila_agent::codex::{AppServer, Options};
use lila_agent::{AgentEvent, Command, HostStatus, Item, PermissionRequest};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// What the host's loop waits on.
enum In {
    /// A line from the agent.
    Agent(String),
    AgentExited(String),
    /// lilad is attached; commands come from it, events go to this.
    Attached(UnixStream),
    Detached,
    Command(Command),
}

/// What lilad gets again when it comes back: the session, the items so far, open questions.
#[derive(Default)]
struct Kept {
    session: Option<String>,
    items: BTreeMap<String, Item>,
    order: Vec<String>,
    asking: Vec<PermissionRequest>,
}

impl Kept {
    fn note(&mut self, ev: &AgentEvent) {
        match ev {
            AgentEvent::Session { id } => self.session = Some(id.clone()),
            AgentEvent::Item { item } => {
                if self.items.insert(item.id.clone(), item.clone()).is_none() {
                    self.order.push(item.id.clone());
                }
            }
            AgentEvent::Permission { request } => self.asking.push(request.clone()),
            AgentEvent::PermissionResolved { id } => self.asking.retain(|r| r.id != *id),
            _ => {}
        }
    }

    fn replay(&self) -> Vec<AgentEvent> {
        let mut out: Vec<AgentEvent> = self.session.iter().map(|id| AgentEvent::Session { id: id.clone() }).collect();
        out.extend(self.order.iter().filter_map(|id| self.items.get(id)).map(|i| AgentEvent::Item { item: i.clone() }));
        out.extend(self.asking.iter().map(|r| AgentEvent::Permission { request: r.clone() }));
        out
    }
}

pub struct CodexArgs {
    pub program: PathBuf,
    pub opts: Options,
    /// More arguments for `codex app-server` (the role's `extra_args`).
    pub extra: Vec<String>,
}

/// Runs a Codex member until its app-server exits.
pub fn codex(paths: &Paths, args: CodexArgs) -> Result<()> {
    let project = std::env::var("LILA_PROJECT").context("LILA_PROJECT is not set: lilad starts hosts")?;
    let member = std::env::var("LILA_MEMBER").context("LILA_MEMBER is not set: lilad starts hosts")?;
    let (tx, rx) = mpsc::channel::<In>();
    let mut child = spawn_codex(paths, &args, &project, &member, &tx)?;
    let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin for codex app-server"))?;
    attach_loop(paths.clone(), project, member, tx.clone());

    let mut server = AppServer::new(args.opts);
    let mut kept = Kept::default();
    let mut lilad: Option<UnixStream> = None;
    let mut shown = None;
    let step = server.start();
    send_all(&mut stdin, &step.send);
    report(&server.status(), &mut shown);
    loop {
        let input = match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(i) => Some(i),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let now = now_ms();
        let step = match input {
            // Now and then: a stuck turn gets a word.
            None => server.tick(now),
            Some(input) => match input {
                In::Agent(line) => match serde_json::from_str::<Value>(&line) {
                    Ok(m) => server.on_message(&m, now),
                    Err(_) => continue,
                },
                In::AgentExited(why) => {
                    report(&HostStatus::Error { why: why.clone() }, &mut shown);
                    println!("codex app-server ended: {why}");
                    break;
                }
                In::Attached(conn) => {
                    println!("· lilad attached");
                    lilad = Some(conn);
                    for ev in kept.replay() {
                        emit(&mut lilad, &ev);
                    }
                    continue;
                }
                In::Detached => {
                    println!("· lilad went away; waiting for it");
                    lilad = None;
                    continue;
                }
                In::Command(c) => {
                    log_command(&c);
                    server.command(c)
                }
            },
        };
        send_all(&mut stdin, &step.send);
        for ev in &step.events {
            log_event(ev);
            kept.note(ev);
            emit(&mut lilad, ev);
        }
        report(&server.status(), &mut shown);
    }
    let _ = child.kill();
    Ok(())
}

fn spawn_codex(paths: &Paths, args: &CodexArgs, project: &str, member: &str, tx: &mpsc::Sender<In>) -> Result<Child> {
    // The server's own log goes to a file: the terminal shows what the member does.
    let logs = paths.state.join("hosts");
    std::fs::create_dir_all(&logs)?;
    let log = std::fs::File::create(logs.join(format!("{project}.{member}.log")))?;
    let mut child = std::process::Command::new(&args.program)
        .arg("app-server")
        .args(&args.extra)
        .current_dir(&args.opts.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(log))
        .spawn()
        .with_context(|| format!("start {} app-server", args.program.display()))?;
    let out = child.stdout.take().ok_or_else(|| anyhow!("no stdout for codex app-server"))?;
    let tx = tx.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if tx.send(In::Agent(line)).is_err() {
                return;
            }
        }
        let _ = tx.send(In::AgentExited("its output closed".into()));
    });
    Ok(child)
}

/// Keeps a connection to lilad: attaches, reads commands, and tries again when it drops.
fn attach_loop(paths: Paths, project: String, member: String, tx: mpsc::Sender<In>) {
    std::thread::spawn(move || {
        loop {
            if let Ok(sock) = paths.socket()
                && let Ok(mut conn) = UnixStream::connect(&sock)
            {
                let hello = json!({"method": "host.attach", "params": {"project": project, "member": member}});
                if writeln!(conn, "{hello}").is_ok()
                    && let Ok(read) = conn.try_clone()
                {
                    let mut lines = BufReader::new(read).lines();
                    let ok = lines.next().and_then(|l| l.ok()).and_then(|l| serde_json::from_str::<Value>(&l).ok()).is_some_and(|v| v["result"]["attached"] == true);
                    if ok && tx.send(In::Attached(conn)).is_ok() {
                        for line in lines.map_while(Result::ok) {
                            if let Ok(c) = serde_json::from_str::<Command>(&line)
                                && tx.send(In::Command(c)).is_err()
                            {
                                return;
                            }
                        }
                        if tx.send(In::Detached).is_err() {
                            return;
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

fn send_all(stdin: &mut ChildStdin, msgs: &[Value]) {
    for m in msgs {
        let _ = writeln!(stdin, "{m}");
    }
    let _ = stdin.flush();
}

fn emit(lilad: &mut Option<UnixStream>, ev: &AgentEvent) {
    if let Some(conn) = lilad
        && writeln!(conn, "{}", serde_json::to_string(ev).unwrap_or_default()).is_err()
    {
        *lilad = None;
    }
}

/// Tells hc (OSC 7501) when the status changes.
fn report(status: &HostStatus, shown: &mut Option<HostStatus>) {
    if shown.as_ref() != Some(status) {
        print!("{}", status.osc("lila-codex"));
        let _ = std::io::stdout().flush();
        *shown = Some(status.clone());
    }
}

fn one_line(s: &str, n: usize) -> String {
    let line = s.lines().next().unwrap_or_default();
    if line.chars().count() > n { format!("{}…", line.chars().take(n).collect::<String>()) } else { line.to_owned() }
}

fn log_command(c: &Command) {
    match c {
        Command::Prompt { text } => println!("» {}", one_line(text, 100)),
        Command::Interrupt => println!("» interrupt"),
        Command::Answer { permission, option } => println!("» {permission}: {option}"),
    }
}

fn log_event(ev: &AgentEvent) {
    use lila_agent::ItemKind::*;
    match ev {
        AgentEvent::Session { id } => println!("· thread {id}"),
        AgentEvent::Item { item } if item.state != lila_agent::ItemState::Running => match &item.kind {
            AgentMessage { text } => println!("{}", one_line(text, 160)),
            ToolCall { title, .. } => println!("$ {}", one_line(title, 140)),
            _ => {}
        },
        AgentEvent::Permission { request } => println!("? {}", one_line(&request.title, 140)),
        AgentEvent::TurnEnded { failed: Some(why), .. } => println!("✗ {why}"),
        AgentEvent::Notice { text } => println!("! {text}"),
        _ => {}
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}
