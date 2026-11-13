//! omp as a child process, through `OmpAgents`: a stand-in omp that speaks the RPC protocol (this binary again, when it
//! is started with omp's arguments). Checks what a unit test of the translation cannot: the arguments omp is started with,
//! `--continue` on a second run, a frame in pieces, the facts as lilad gets them, and what is said when omp dies.

use lila::daemon::{Agents, Fact, OmpAgents, Spawn, Started, omp_args};
use lila_agent::{AgentEvent, Command, ItemKind, Reply};
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn main() {
    if std::env::args().any(|a| a == "--mode") {
        return fake_omp();
    }
    let tmp = tempfile::tempdir().unwrap();
    let sessions = tmp.path().join("omp/t1");
    // First run: no session yet, a turn, a question, and then omp dies.
    let (mut agents, started, mut lilad) = start(tmp.path(), &sessions);
    let first = lilad.until(|e| matches!(e, AgentEvent::Config { models, .. } if !models.is_empty()));
    assert!(first.iter().any(|e| matches!(e, AgentEvent::Session { id } if id == "s1")), "{first:?}");
    let args = std::fs::read_to_string(sessions.join("args.txt")).unwrap();
    assert!(args.starts_with("--mode rpc-ui --approval-mode always-ask --session-dir"), "{args}");
    assert!(!args.contains("--continue"), "a first run starts a session: {args}");
    started.send(Command::Prompt { text: "hello".into(), attachments: vec![] });
    let turn = lilad.until(|e| matches!(e, AgentEvent::TurnEnded { .. }));
    assert!(turn.iter().any(|e| matches!(e, AgentEvent::Item { item } if matches!(&item.kind, ItemKind::AgentMessage { text } if text == "You said: hello"))), "{turn:?}");
    started.send(Command::Prompt { text: "ask".into(), attachments: vec![] });
    let asked = lilad.until(|e| matches!(e, AgentEvent::Question { .. }));
    let AgentEvent::Question { question } = asked.last().unwrap() else { unreachable!() };
    started.send(Command::Reply { question: question.id.clone(), reply: Reply::Choice { value: "SQLite".into() } });
    let done = lilad.until(|e| matches!(e, AgentEvent::TurnEnded { .. }));
    assert!(done.iter().any(|e| matches!(e, AgentEvent::Item { item } if matches!(&item.kind, ItemKind::AgentMessage { text } if text == "Chose SQLite"))), "{done:?}");
    started.send(Command::Prompt { text: "crash".into(), attachments: vec![] });
    let dead = lilad.until(|e| matches!(e, AgentEvent::Notice { .. }));
    let why = dead.iter().find_map(|e| if let AgentEvent::Notice { text } = e { Some(text.clone()) } else { None }).unwrap();
    assert!(why.contains("code 7") && why.contains("boom on stderr"), "{why}");
    assert!(dead.iter().any(|e| matches!(e, AgentEvent::Item { item } if matches!(&item.kind, ItemKind::Error { text } if *text == why))), "{dead:?}");
    assert_eq!(lilad.exited(), why, "the end is reported after the words about it");
    agents.wait();

    // Second run on the same folder: omp is told to go on with its session.
    let (mut agents, started, mut lilad) = start(tmp.path(), &sessions);
    lilad.until(|e| matches!(e, AgentEvent::Config { models, .. } if !models.is_empty()));
    let args = std::fs::read_to_string(sessions.join("args.txt")).unwrap();
    assert!(args.contains("--continue"), "{args}");

    // Letting go of the thread closes omp's input: it leaves (the stand-in exits when its input ends).
    let began = Instant::now();
    drop(started);
    agents.wait();
    assert!(began.elapsed() < Duration::from_secs(3), "it left by itself, not killed");
    println!("omp tests ok");
}

/// What lilad is to a thread's omp: the facts that come back.
struct Lilad {
    facts: mpsc::Receiver<Fact>,
}

trait Send_ {
    fn send(&self, c: Command);
}

impl Send_ for Started {
    fn send(&self, c: Command) {
        self.commands.send(c).unwrap();
    }
}

impl Lilad {
    /// The events up to and including the first that `stop` says yes to.
    fn until(&mut self, stop: impl Fn(&AgentEvent) -> bool) -> Vec<AgentEvent> {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut seen = Vec::new();
        while Instant::now() < deadline {
            let Ok(fact) = self.facts.recv_timeout(Duration::from_millis(200)) else { continue };
            let Fact::Event(ev) = fact else { continue };
            let done = stop(&ev);
            seen.push(ev);
            if done {
                return seen;
            }
        }
        panic!("the event never came; saw {seen:?}");
    }

    /// Why omp ended, once it has.
    fn exited(&mut self) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Ok(Fact::Exited(why)) = self.facts.recv_timeout(Duration::from_millis(200)) {
                return why;
            }
        }
        panic!("omp never ended");
    }
}

fn start(home: &std::path::Path, sessions: &std::path::Path) -> (OmpAgents, Started, Lilad) {
    std::fs::create_dir_all(sessions).unwrap();
    let (tx, facts) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let mut agents = OmpAgents::new(home.join("logs"), move |_, _, f| {
        let _ = tx.lock().unwrap().send(f);
    });
    let spawn = Spawn { thread: "t1".into(), program: std::env::current_exe().unwrap(), args: omp_args(None, sessions, &[]), cwd: home.to_path_buf() };
    let started = agents.start(spawn).unwrap();
    (agents, started, Lilad { facts })
}

// ---------------- the stand-in omp ----------------

fn fake_omp() {
    let out = |v: Value| {
        println!("{v}");
        std::io::stdout().flush().unwrap();
    };
    // What omp was started with, and the session file `--continue` looks for.
    let dir = std::env::args().skip_while(|a| a != "--session-dir").nth(1).unwrap();
    std::fs::write(std::path::Path::new(&dir).join("args.txt"), std::env::args().skip(1).collect::<Vec<_>>().join(" ")).unwrap();
    std::fs::write(std::path::Path::new(&dir).join("2026_s1.jsonl"), "{}\n").unwrap();
    out(json!({"type": "ready", "protocolVersion": 1, "supportedProtocolVersions": [1, 2]}));
    let mut waiting: Option<String> = None;
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        let f: Value = serde_json::from_str(&line).unwrap();
        let ok = |data: Value| json!({"type": "response", "id": f["id"], "command": f["type"], "success": true, "data": data});
        match f["type"].as_str().unwrap() {
            "negotiate_protocol" => out(ok(json!({"protocolVersion": 2}))),
            "get_state" => out(ok(json!({"model": {"provider": "p", "id": "m", "input": ["text"]}, "thinkingLevel": "high", "sessionId": "s1"}))),
            "get_available_thinking_levels" => out(ok(json!({"levels": ["off", "high"]}))),
            "get_available_models" => {
                // In two pieces, as omp does with a frame over 1 MiB.
                let body = ok(json!({"models": [{"provider": "p", "id": "m", "name": "M"}]})).to_string();
                let half = body.len() / 2;
                for (i, part) in [&body.as_bytes()[..half], &body.as_bytes()[half..]].iter().enumerate() {
                    use base64::Engine as _;
                    out(json!({"type": "rpc_chunk", "chunkId": "c1", "index": i, "count": 2, "byteLength": body.len(), "data": base64::engine::general_purpose::STANDARD.encode(part)}));
                }
            }
            "prompt" => {
                let text = f["message"].as_str().unwrap().to_owned();
                out(ok(json!({})));
                if text == "crash" {
                    eprintln!("boom on stderr");
                    std::process::exit(7);
                }
                out(json!({"type": "agent_start"}));
                if text == "ask" {
                    waiting = f["id"].as_str().map(String::from);
                    out(json!({"type": "extension_ui_request", "id": "ui1", "method": "select", "title": "Which database?", "options": ["Postgres", "SQLite"]}));
                    continue;
                }
                reply(&out, &format!("You said: {text}"), f["id"].as_str().unwrap());
            }
            "extension_ui_response" => {
                let chose = f["value"].as_str().unwrap_or("nothing").to_owned();
                if let Some(prompt) = waiting.take() {
                    reply(&out, &format!("Chose {chose}"), &prompt);
                }
            }
            _ => {}
        }
    }
}

fn reply(out: &dyn Fn(Value), text: &str, prompt: &str) {
    out(json!({"type": "message_start", "message": {"role": "assistant", "content": []}}));
    out(json!({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": text}}));
    out(json!({"type": "message_end", "message": {"role": "assistant", "content": [{"type": "text", "text": text}], "stopReason": "stop"}}));
    out(json!({"type": "agent_end"}));
    out(json!({"type": "prompt_result", "id": prompt, "agentInvoked": true, "status": "completed"}));
}
