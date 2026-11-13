//! omp as a child process of lilad: one thread per running thread of the conversation reads omp's output, translates it
//! (`lila_agent::omp::Omp`, which does no IO) and hands what comes out to lilad as facts.

use super::agents::{Agents, Fact, Spawn, Started};
use anyhow::{Context, Result, anyhow};
use lila_agent::omp::{Omp, Options, Step};
use lila_agent::{Command, HostStatus};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long omp has to leave by itself once its input is closed.
const LEAVE_MS: u64 = 3000;

/// Where facts go: the thread, the run, the fact.
type Sink = Arc<dyn Fn(&str, u64, Fact) + Send + Sync>;

/// What a thread's loop waits on.
enum In {
    /// A line from omp.
    Line(String),
    /// omp's output is closed: it has stopped.
    Exited,
    Command(Command),
    /// lilad let go of the thread: omp is to end.
    Closed,
}

pub struct OmpAgents {
    /// omp's own log goes to `<logs>/<thread>.log`.
    logs: PathBuf,
    sink: Sink,
    next_run: u64,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl OmpAgents {
    pub fn new(logs: PathBuf, sink: impl Fn(&str, u64, Fact) + Send + Sync + 'static) -> OmpAgents {
        OmpAgents { logs, sink: Arc::new(sink), next_run: 0, workers: Vec::new() }
    }
}

impl Agents for OmpAgents {
    fn start(&mut self, spawn: Spawn) -> Result<Started> {
        std::fs::create_dir_all(&self.logs)?;
        let log = self.logs.join(format!("{}.log", spawn.thread));
        let mut child = spawn_omp(&spawn, &log)?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin for {}", spawn.program.display()))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout for {}", spawn.program.display()))?;
        self.next_run += 1;
        let run = self.next_run;

        let (tx, rx) = mpsc::channel::<In>();
        let out = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if out.send(In::Line(line)).is_err() {
                    return;
                }
            }
            let _ = out.send(In::Exited);
        });
        let (commands, crx) = mpsc::channel::<Command>();
        std::thread::spawn(move || {
            for c in crx {
                if tx.send(In::Command(c)).is_err() {
                    return;
                }
            }
            let _ = tx.send(In::Closed);
        });

        let omp = Omp::new(Options { cwd: spawn.cwd.to_string_lossy().to_string(), run: format!("{:x}", now_ms()) });
        let (sink, thread) = (self.sink.clone(), spawn.thread);
        self.workers.retain(|w| !w.is_finished());
        self.workers.push(std::thread::spawn(move || work(&thread, run, child, stdin, rx, omp, &log, &*sink)));
        Ok(Started { run, commands })
    }

    fn wait(&mut self) {
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

fn spawn_omp(spawn: &Spawn, log: &Path) -> Result<Child> {
    let stderr = std::fs::File::create(log).with_context(|| format!("make {}", log.display()))?;
    std::process::Command::new(&spawn.program)
        .args(&spawn.args)
        .current_dir(&spawn.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr))
        .spawn()
        .with_context(|| format!("start {}", spawn.program.display()))
}

/// Runs until omp exits or lilad lets go of the thread.
#[allow(clippy::too_many_arguments)]
fn work(thread: &str, run: u64, mut child: Child, mut stdin: ChildStdin, rx: mpsc::Receiver<In>, mut omp: Omp, log: &Path, sink: &dyn Fn(&str, u64, Fact)) {
    let emit = |fact: Fact| sink(thread, run, fact);
    let mut shown: Option<HostStatus> = None;
    let report = |omp: &Omp, shown: &mut Option<HostStatus>| {
        let status = omp.status();
        if shown.as_ref() != Some(&status) {
            emit(Fact::Status(status.clone()));
            *shown = Some(status);
        }
    };
    report(&omp, &mut shown);
    loop {
        let input = match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(i) => Some(i),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => Some(In::Closed),
        };
        let now = now_ms();
        let step = match input {
            // Now and then: a reply being written is shown again.
            None => omp.tick(now),
            Some(In::Line(line)) => omp.on_line(&line, now),
            Some(In::Command(c)) => omp.command(c, now),
            Some(In::Exited) => {
                let why = stopped(&mut child, log);
                let step = omp.exited(&why, now);
                send(&mut stdin, &step);
                for ev in step.events {
                    emit(Fact::Event(ev));
                }
                report(&omp, &mut shown);
                emit(Fact::Exited(why));
                return;
            }
            Some(In::Closed) => {
                leave(child, stdin);
                return;
            }
        };
        send(&mut stdin, &step);
        for ev in step.events {
            emit(Fact::Event(ev));
        }
        report(&omp, &mut shown);
    }
}

fn send(stdin: &mut ChildStdin, step: &Step) {
    for m in &step.send {
        let _ = writeln!(stdin, "{m}");
    }
    let _ = stdin.flush();
}

/// omp is to end: its input is closed, and it is killed if it is still there after `LEAVE_MS`.
fn leave(mut child: Child, stdin: ChildStdin) {
    drop(stdin);
    let deadline = Instant::now() + Duration::from_millis(LEAVE_MS);
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// omp's output is over: how it ended, and what it said on its way out.
fn stopped(child: &mut Child, log: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code().map_or_else(|| "killed by a signal".to_owned(), |c| format!("code {c}")),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
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

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lila_agent::AgentEvent;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;

    /// A stand-in omp: a shell script that says `ready` and then waits for input to close (or for a signal).
    fn script(dir: &Path, body: &str) -> PathBuf {
        let p = dir.join("omp");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn start(dir: &Path, body: &str) -> (OmpAgents, Started, Arc<Mutex<Vec<Fact>>>) {
        let facts = Arc::new(Mutex::new(Vec::new()));
        let sink = facts.clone();
        let mut agents = OmpAgents::new(dir.join("logs"), move |_, _, f| sink.lock().unwrap().push(f));
        let spawn = Spawn { thread: "t1".into(), program: script(dir, body), args: Vec::new(), cwd: dir.to_path_buf() };
        let started = agents.start(spawn).unwrap();
        (agents, started, facts)
    }

    fn until(facts: &Arc<Mutex<Vec<Fact>>>, what: impl Fn(&Fact) -> bool) -> Vec<Fact> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let seen = facts.lock().unwrap().clone();
            if seen.iter().any(&what) {
                return seen;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the fact never came; saw {:?}", facts.lock().unwrap());
    }

    #[test]
    fn an_omp_that_dies_is_reported_with_its_code_and_last_words() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut agents, _started, facts) = start(tmp.path(), "echo boom on stderr >&2; exit 7");
        let seen = until(&facts, |f| matches!(f, Fact::Exited(_)));
        let Some(Fact::Exited(why)) = seen.last() else { panic!("{seen:?}") };
        assert!(why.contains("code 7") && why.contains("boom on stderr"), "{why}");
        let error = seen.iter().find_map(|f| match f {
            Fact::Event(AgentEvent::Item { item }) => match &item.kind {
                lila_agent::ItemKind::Error { text } => Some(text.clone()),
                _ => None,
            },
            _ => None,
        });
        assert_eq!(error.as_deref(), Some(why.as_str()), "the conversation gets the error: {seen:?}");
        agents.wait();
    }

    #[test]
    fn dropping_the_commands_closes_omps_input_and_it_leaves() {
        let tmp = tempfile::tempdir().unwrap();
        // Leaves when its input closes, and says so.
        let (mut agents, started, _facts) = start(tmp.path(), "cat >/dev/null; touch left");
        drop(started);
        agents.wait();
        assert!(tmp.path().join("left").exists());
    }

    #[test]
    fn an_omp_that_stays_is_killed() {
        let tmp = tempfile::tempdir().unwrap();
        // Ignores its input closing and sleeps on.
        let (mut agents, started, _facts) = start(tmp.path(), "exec sleep 60");
        let began = Instant::now();
        drop(started);
        agents.wait();
        let took = began.elapsed();
        assert!(took >= Duration::from_millis(LEAVE_MS) && took < Duration::from_secs(10), "{took:?}");
    }

    #[test]
    fn a_program_that_cannot_start_is_an_error_at_once() {
        let tmp = tempfile::tempdir().unwrap();
        let mut agents = OmpAgents::new(tmp.path().join("logs"), |_, _, _| {});
        let err = agents.start(Spawn { thread: "t1".into(), program: tmp.path().join("nope"), args: Vec::new(), cwd: tmp.path().to_path_buf() });
        assert!(err.is_err());
    }
}
