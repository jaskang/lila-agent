//! `lila host omp`: runs a thread's omp over its RPC mode inside the thread's hc terminal
//! (architecture v3 §4 D1). It starts omp, attaches to lilad over its socket
//! (events out, commands in; reattaching when lilad comes back), reports its
//! status to hc with OSC 7501 like any program, and prints a readable log: what
//! the terminal shows.

use crate::paths::Paths;
use anyhow::{Context, Result, anyhow};
use lila_agent::omp::Omp;
use lila_agent::{AgentEvent, Command, HostStatus, Item, PermissionRequest, Question};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// What the host's loop waits on.
enum In {
    /// A line from omp.
    Agent(String),
    /// omp's output is closed: it has stopped.
    AgentExited,
    /// lilad is attached; commands come from it, events go to this.
    Attached(UnixStream),
    Detached,
    Command(Command),
}

/// OSC 7501's `app`.
const APP: &str = "lila-omp";

/// What lilad gets again when it comes back: the session, the items so far, open questions.
#[derive(Default)]
struct Kept {
    session: Option<String>,
    items: BTreeMap<String, Item>,
    order: Vec<String>,
    asking: Vec<PermissionRequest>,
    questions: Vec<Question>,
    /// The latest model/effort and the commands the agent declared.
    config: lila_agent::Config,
    commands: Option<AgentEvent>,
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
            AgentEvent::Question { question } => self.questions.push(question.clone()),
            AgentEvent::QuestionResolved { id } => self.questions.retain(|q| q.id != *id),
            AgentEvent::Config { .. } => self.config.apply(ev),
            AgentEvent::Commands { .. } => self.commands = Some(ev.clone()),
            _ => {}
        }
    }

    fn replay(&self) -> Vec<AgentEvent> {
        let mut out: Vec<AgentEvent> = self.session.iter().map(|id| AgentEvent::Session { id: id.clone() }).collect();
        out.extend(self.order.iter().filter_map(|id| self.items.get(id)).map(|i| AgentEvent::Item { item: i.clone() }));
        if self.config != lila_agent::Config::default() {
            out.push(self.config.event());
        }
        out.extend(self.commands.clone());
        out.extend(self.asking.iter().map(|r| AgentEvent::Permission { request: r.clone() }));
        out.extend(self.questions.iter().map(|q| AgentEvent::Question { question: q.clone() }));
        out
    }
}

/// omp's program, its arguments, and the folder it runs in.
pub struct Agent {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: String,
}

/// What `omp` is started with: its RPC mode with the tool dialogs (`ask` needs them), every write and every command
/// asked about (Lila answers them: `policy`), the thread's own folder of sessions, and the last session in it when
/// there is one. `extra` goes last.
pub fn omp_args(model: Option<&str>, session_dir: &Path, extra: &[String]) -> Vec<String> {
    let mut args: Vec<String> = ["--mode", "rpc-ui", "--approval-mode", "always-ask"].map(String::from).into();
    if let Some(m) = model.filter(|m| !m.is_empty()) {
        args.extend(["--model".to_owned(), m.to_owned()]);
    }
    args.extend(["--session-dir".to_owned(), session_dir.to_string_lossy().to_string()]);
    if has_session(session_dir) {
        args.push("--continue".into());
    }
    args.extend(extra.iter().cloned());
    args
}

/// Does the thread's folder of sessions hold one (omp writes each session as a `.jsonl` file)?
pub fn has_session(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|d| d.flatten().any(|e| e.path().extension().is_some_and(|x| x == "jsonl")))
}

/// Runs omp until it exits, translating with `server`.
pub fn run(paths: &Paths, agent: Agent, mut server: Omp) -> Result<()> {
    let thread = std::env::var("LILA_THREAD").context("LILA_THREAD is not set: lilad starts hosts")?;
    let (tx, rx) = mpsc::channel::<In>();
    let log = paths.state.join("hosts").join(format!("{thread}.log"));
    let mut child = spawn_agent(&agent, &log, &tx)?;
    let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin for {}", agent.program.display()))?;
    attach_loop(paths.clone(), thread, tx.clone());

    let mut kept = Kept::default();
    let mut lilad: Option<UnixStream> = None;
    let mut shown = None;
    report(&server.status(), APP, &mut shown);
    loop {
        let input = match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(i) => Some(i),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let now = now_ms();
        let step = match input {
            // Now and then: a reply being written is shown again.
            None => server.tick(now),
            Some(input) => match input {
                In::Agent(line) => server.on_line(&line, now),
                In::AgentExited => {
                    let why = stopped(&mut child, &log);
                    let step = server.exited(&why, now);
                    for ev in &step.events {
                        log_event(ev);
                        kept.note(ev);
                        emit(&mut lilad, ev);
                    }
                    report(&server.status(), APP, &mut shown);
                    println!("{why}");
                    return Ok(());
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
                    server.command(c, now)
                }
            },
        };
        send_all(&mut stdin, &step.send);
        for ev in &step.events {
            log_event(ev);
            kept.note(ev);
            emit(&mut lilad, ev);
        }
        report(&server.status(), APP, &mut shown);
    }
    let _ = child.kill();
    Ok(())
}

fn spawn_agent(agent: &Agent, log: &Path, tx: &mpsc::Sender<In>) -> Result<Child> {
    // omp's own log goes to a file: the terminal shows what the agent does.
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let stderr = std::fs::File::create(log)?;
    let mut child = std::process::Command::new(&agent.program)
        .args(&agent.args)
        .current_dir(&agent.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr))
        .spawn()
        .with_context(|| format!("start {}", agent.program.display()))?;
    let out = child.stdout.take().ok_or_else(|| anyhow!("no stdout for {}", agent.program.display()))?;
    let tx = tx.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if tx.send(In::Agent(line)).is_err() {
                return;
            }
        }
        let _ = tx.send(In::AgentExited);
    });
    Ok(child)
}

/// omp's output is over: how it ended, and what it said on its way out.
fn stopped(child: &mut Child, log: &Path) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code().map_or_else(|| "killed by a signal".to_owned(), |c| format!("code {c}")),
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break "did not exit; killed".to_owned();
            }
        }
    };
    let tail = log_tail(log);
    if tail.is_empty() { format!("omp stopped ({status})") } else { format!("omp stopped ({status}): {tail}") }
}

/// The last words in omp's log: up to six lines, 800 bytes.
fn log_tail(log: &Path) -> String {
    let text = std::fs::read(log).map(|b| String::from_utf8_lossy(&b[b.len().saturating_sub(800)..]).into_owned()).unwrap_or_default();
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines[lines.len().saturating_sub(6)..].join("\n")
}

/// Keeps a connection to lilad: attaches, reads commands, and tries again when it drops.
fn attach_loop(paths: Paths, thread: String, tx: mpsc::Sender<In>) {
    std::thread::spawn(move || {
        loop {
            if let Ok(sock) = paths.socket()
                && let Ok(mut conn) = UnixStream::connect(&sock)
            {
                let hello = json!({"method": "host.attach", "params": {"thread": thread}});
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
fn report(status: &HostStatus, app: &str, shown: &mut Option<HostStatus>) {
    if shown.as_ref() != Some(status) {
        print!("{}", status.osc(app));
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
        Command::Prompt { text, attachments } if attachments.is_empty() => println!("» {}", one_line(text, 100)),
        Command::Prompt { text, attachments } => println!("» {} (+{} files)", one_line(text, 100), attachments.len()),
        Command::Interrupt => println!("» interrupt"),
        Command::SetModel { model } => println!("» model {model}"),
        Command::SetEffort { effort } => println!("» effort {effort}"),
        Command::Answer { permission, option } => println!("» {permission}: {option}"),
        Command::Reply { question, reply } => println!("» {question}: {}", serde_json::to_string(reply).unwrap_or_default()),
    }
}

fn log_event(ev: &AgentEvent) {
    use lila_agent::ItemKind::*;
    match ev {
        AgentEvent::Session { id } => println!("· session {id}"),
        AgentEvent::Item { item } if item.state != lila_agent::ItemState::Running => match &item.kind {
            AgentMessage { text } => println!("{}", one_line(text, 160)),
            ToolCall { title, .. } => println!("$ {}", one_line(title, 140)),
            Error { text } => println!("✗ {}", one_line(text, 160)),
            _ => {}
        },
        AgentEvent::Permission { request } => println!("? {}", one_line(&request.title, 140)),
        AgentEvent::Question { question } => println!("? {}", one_line(&question.title, 140)),
        AgentEvent::TurnEnded { failed: Some(why), .. } => println!("✗ {why}"),
        AgentEvent::Notice { text } => println!("! {text}"),
        _ => {}
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}
