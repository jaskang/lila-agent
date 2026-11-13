//! `lila host omp` as a process: a stand-in lilad on its socket, and a stand-in omp that speaks the RPC protocol
//! (this binary again, when `FAKE_OMP` is set). Checks what a unit test of the translation cannot: the arguments omp is
//! started with, `--continue` on a second run, a frame in pieces, the replies as lilad gets them, and what is said when omp dies.

use lila_agent::{AgentEvent, Command, ItemKind, Reply};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

fn main() {
    if std::env::var_os("FAKE_OMP").is_some() {
        return fake_omp();
    }
    let tmp = tempfile::tempdir().unwrap();
    let sessions = tmp.path().join("omp/t1");
    // First run: no session yet, a turn, a question, and then omp dies.
    let (mut host, mut lilad) = start(tmp.path(), &sessions);
    let first = lilad.until(|e| matches!(e, AgentEvent::Config { models, .. } if !models.is_empty()));
    assert!(first.iter().any(|e| matches!(e, AgentEvent::Session { id } if id == "s1")), "{first:?}");
    let args = std::fs::read_to_string(sessions.join("args.txt")).unwrap();
    assert!(args.starts_with("--mode rpc-ui --approval-mode always-ask --session-dir"), "{args}");
    assert!(!args.contains("--continue"), "a first run starts a session: {args}");
    lilad.send(&Command::Prompt { text: "hello".into(), attachments: vec![] });
    let turn = lilad.until(|e| matches!(e, AgentEvent::TurnEnded { .. }));
    assert!(turn.iter().any(|e| matches!(e, AgentEvent::Item { item } if matches!(&item.kind, ItemKind::AgentMessage { text } if text == "You said: hello"))), "{turn:?}");
    lilad.send(&Command::Prompt { text: "ask".into(), attachments: vec![] });
    let asked = lilad.until(|e| matches!(e, AgentEvent::Question { .. }));
    let AgentEvent::Question { question } = asked.last().unwrap() else { unreachable!() };
    lilad.send(&Command::Reply { question: question.id.clone(), reply: Reply::Choice { value: "SQLite".into() } });
    let done = lilad.until(|e| matches!(e, AgentEvent::TurnEnded { .. }));
    assert!(done.iter().any(|e| matches!(e, AgentEvent::Item { item } if matches!(&item.kind, ItemKind::AgentMessage { text } if text == "Chose SQLite"))), "{done:?}");
    lilad.send(&Command::Prompt { text: "crash".into(), attachments: vec![] });
    let dead = lilad.until(|e| matches!(e, AgentEvent::Notice { .. }));
    let why = dead.iter().find_map(|e| if let AgentEvent::Notice { text } = e { Some(text.clone()) } else { None }).unwrap();
    assert!(why.contains("code 7") && why.contains("boom on stderr"), "{why}");
    assert!(dead.iter().any(|e| matches!(e, AgentEvent::Item { item } if matches!(&item.kind, ItemKind::Error { text } if *text == why))), "{dead:?}");
    assert!(host.wait().unwrap().success(), "the host ends once it has said so");

    // Second run on the same folder: omp is told to go on with its session.
    let (mut host, mut lilad) = start(tmp.path(), &sessions);
    lilad.until(|e| matches!(e, AgentEvent::Config { models, .. } if !models.is_empty()));
    let args = std::fs::read_to_string(sessions.join("args.txt")).unwrap();
    assert!(args.contains("--continue"), "{args}");
    lilad.send(&Command::Prompt { text: "crash".into(), attachments: vec![] });
    lilad.until(|e| matches!(e, AgentEvent::Notice { .. }));
    let _ = host.wait();
    println!("host tests ok");
}

/// What lilad is to the host: a socket it attaches to, events in, commands out.
struct Lilad {
    conn: UnixStream,
    lines: std::io::Lines<BufReader<UnixStream>>,
}

impl Lilad {
    fn send(&mut self, c: &Command) {
        writeln!(self.conn, "{}", serde_json::to_string(c).unwrap()).unwrap();
    }

    /// The events up to and including the first that `stop` says yes to.
    fn until(&mut self, stop: impl Fn(&AgentEvent) -> bool) -> Vec<AgentEvent> {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut seen = Vec::new();
        while Instant::now() < deadline {
            let Some(Ok(line)) = self.lines.next() else { break };
            let Ok(ev) = serde_json::from_str::<AgentEvent>(&line) else { continue };
            let done = stop(&ev);
            seen.push(ev);
            if done {
                return seen;
            }
        }
        panic!("the event never came; saw {seen:?}");
    }
}

fn start(home: &std::path::Path, sessions: &std::path::Path) -> (Child, Lilad) {
    let sock = home.join("state/lilad.sock");
    std::fs::create_dir_all(sock.parent().unwrap()).unwrap();
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).unwrap();
    let host = std::process::Command::new(env!("CARGO_BIN_EXE_lila"))
        .args(["host", "omp", "--program", std::env::current_exe().unwrap().to_str().unwrap(), "--session-dir", sessions.to_str().unwrap()])
        .env("LILA_HOME", home)
        .env("LILA_THREAD", "t1")
        .env("FAKE_OMP", "1")
        .current_dir(home)
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let (mut conn, _) = listener.accept().unwrap();
    let mut lines = BufReader::new(conn.try_clone().unwrap()).lines();
    let hello: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    assert_eq!((hello["method"].as_str(), hello["params"]["thread"].as_str()), (Some("host.attach"), Some("t1")));
    writeln!(conn, "{}", json!({"result": {"attached": true}})).unwrap();
    (host, Lilad { conn, lines })
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
