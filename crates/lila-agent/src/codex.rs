//! Codex through `codex app-server` (JSON-RPC, one message per line), for
//! `lila host codex` (architecture v3 §7.2, P0 S2). Pure translation: messages
//! from the server and commands from lilad in, requests for the server and
//! events for lilad out, and the status to report. The host owns the process.

use crate::model::*;
use serde_json::{Value, json};
use std::collections::HashMap;

/// How the member's thread is started.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub cwd: String,
    pub model: String,
    pub effort: Option<String>,
    /// `read-only`, `workspace-write` or `danger-full-access`.
    pub sandbox: Option<String>,
    /// Lila's system prompt for the member (`developerInstructions`).
    pub instructions: String,
    /// A thread to go on with.
    pub thread: Option<String>,
}

/// What one input led to.
#[derive(Debug, Default, PartialEq)]
pub struct Step {
    /// For the server, in order.
    pub send: Vec<Value>,
    /// For lilad.
    pub events: Vec<AgentEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Asked {
    Initialize,
    Account,
    Thread { resume: bool },
    TurnStart,
    Other,
}

pub struct AppServer {
    opts: Options,
    next: u64,
    asked: HashMap<u64, Asked>,
    thread: Option<String>,
    turn: Option<String>,
    /// A turn/start is out and its turn hasn't started yet.
    starting: bool,
    /// Words that wait: for the thread, or for a turn to steer into, and the files that go with them.
    queue: Vec<String>,
    queue_files: Vec<Attachment>,
    /// Approvals the person hasn't answered: (permission id, request id).
    approvals: Vec<(String, Value, String)>,
    status: HostStatus,
    /// Items started and not completed (a command running, a reply being written).
    running: std::collections::HashSet<String>,
    /// When the server last said anything.
    heard_ms: i64,
    /// The turn already nudged (`tick`), once each.
    nudged: Option<String>,
}

/// A turn quiet this long with nothing running is stuck (P2: Codex sometimes stops
/// after a tool's result until new input arrives); a word moves it on.
pub const STUCK_MS: i64 = 120_000;
pub const NUDGE: &str = "(Lila: no progress for a while. Continue the task.)";

/// The answers a command or a change takes, the suggested first.
fn approval_options() -> Vec<PermOption> {
    [("accept", "允许"), ("acceptForSession", "本会话都允许"), ("decline", "拒绝")].into_iter().map(|(id, label)| PermOption { id: id.into(), label: label.into() }).collect()
}

impl AppServer {
    pub fn new(opts: Options) -> Self {
        AppServer {
            opts,
            next: 0,
            asked: HashMap::new(),
            thread: None,
            turn: None,
            starting: false,
            queue: Vec::new(),
            queue_files: Vec::new(),
            approvals: Vec::new(),
            status: HostStatus::Working,
            running: Default::default(),
            heard_ms: 0,
            nudged: None,
        }
    }

    pub fn status(&self) -> HostStatus {
        self.status.clone()
    }

    fn request(&mut self, step: &mut Step, asked: Asked, method: &str, params: Value) {
        self.next += 1;
        self.asked.insert(self.next, asked);
        step.send.push(json!({"id": self.next, "method": method, "params": params}));
    }

    /// The first message: the handshake.
    pub fn start(&mut self) -> Step {
        let mut step = Step::default();
        self.request(&mut step, Asked::Initialize, "initialize", json!({"clientInfo": {"name": "lila", "title": "Lila", "version": env!("CARGO_PKG_VERSION")}}));
        step
    }

    fn thread_params(&self) -> Value {
        let mut p = json!({"cwd": self.opts.cwd, "model": self.opts.model, "approvalPolicy": "on-request", "developerInstructions": self.opts.instructions});
        if let Some(s) = &self.opts.sandbox {
            p["sandbox"] = json!(s);
        }
        p
    }

    fn open_thread(&mut self, step: &mut Step) {
        let mut p = self.thread_params();
        match self.opts.thread.clone() {
            Some(id) => {
                p["threadId"] = json!(id);
                self.request(step, Asked::Thread { resume: true }, "thread/resume", p);
            }
            None => self.request(step, Asked::Thread { resume: false }, "thread/start", p),
        }
    }

    /// A turn's input: the words, then each file as the item Codex has for it (an image by its path,
    /// a file as a mention, a skill by name and path).
    fn input(text: &str, files: &[Attachment]) -> Value {
        let mut items = vec![json!({"type": "text", "text": text})];
        for f in files {
            items.push(match f.kind {
                AttachmentKind::Image => json!({"type": "localImage", "path": f.path}),
                AttachmentKind::File => json!({"type": "mention", "name": f.label(), "path": f.path}),
                AttachmentKind::Skill => json!({"type": "skill", "name": f.label(), "path": f.path}),
            });
        }
        Value::Array(items)
    }

    /// The model and effort the next turns run on.
    fn config(&self) -> AgentEvent {
        AgentEvent::Config { model: Some(self.opts.model.clone()).filter(|m| !m.is_empty()), effort: self.opts.effort.clone(), models: Vec::new(), efforts: Vec::new() }
    }

    /// Sends what waits: a new turn when none runs, else into the running one.
    fn flush(&mut self, step: &mut Step) {
        let Some(thread) = self.thread.clone() else { return };
        if self.queue.is_empty() || self.starting {
            return;
        }
        let text = std::mem::take(&mut self.queue).join("\n\n");
        let files = std::mem::take(&mut self.queue_files);
        match self.turn.clone() {
            Some(turn) => self.request(step, Asked::Other, "turn/steer", json!({"threadId": thread, "expectedTurnId": turn, "input": Self::input(&text, &files)})),
            None => {
                // The model and effort go with every turn: a change made in between is in effect from here on.
                let mut p = json!({"threadId": thread, "input": Self::input(&text, &files)});
                if !self.opts.model.is_empty() {
                    p["model"] = json!(self.opts.model);
                }
                if let Some(e) = &self.opts.effort {
                    p["effort"] = json!(e);
                }
                self.starting = true;
                self.status = HostStatus::Working;
                self.request(step, Asked::TurnStart, "turn/start", p);
            }
        }
    }

    /// lilad's command.
    pub fn command(&mut self, c: Command) -> Step {
        let mut step = Step::default();
        match c {
            Command::Prompt { text, attachments } => {
                self.queue.push(text);
                self.queue_files.extend(attachments);
                self.flush(&mut step);
            }
            Command::SetModel { model } => {
                self.opts.model = model;
                step.events.push(self.config());
            }
            Command::SetEffort { effort } => {
                self.opts.effort = Some(effort);
                step.events.push(self.config());
            }
            Command::Compact => {
                if let Some(thread) = self.thread.clone() {
                    self.request(&mut step, Asked::Other, "thread/compact/start", json!({"threadId": thread}));
                }
            }
            Command::Interrupt => {
                if let (Some(thread), Some(turn)) = (self.thread.clone(), self.turn.clone()) {
                    self.request(&mut step, Asked::Other, "turn/interrupt", json!({"threadId": thread, "turnId": turn}));
                }
            }
            Command::Answer { permission, option } => {
                if let Some(i) = self.approvals.iter().position(|(p, _, _)| *p == permission) {
                    let (p, rpc, _) = self.approvals.remove(i);
                    step.send.push(json!({"id": rpc, "result": {"decision": option}}));
                    step.events.push(AgentEvent::PermissionResolved { id: p });
                    self.unblock();
                }
            }
        }
        step
    }

    fn unblock(&mut self) {
        if self.approvals.is_empty() && matches!(self.status, HostStatus::Blocked { .. }) {
            self.status = HostStatus::Working;
        }
    }

    /// Called now and then: a turn that went quiet with nothing running gets a word, once.
    pub fn tick(&mut self, now_ms: i64) -> Step {
        let mut step = Step::default();
        let quiet = self.heard_ms > 0 && now_ms - self.heard_ms >= STUCK_MS;
        if let (Some(turn), true) = (self.turn.clone(), quiet && self.running.is_empty() && self.approvals.is_empty())
            && self.nudged.as_deref() != Some(turn.as_str())
        {
            self.nudged = Some(turn);
            step.events.push(AgentEvent::Notice { text: format!("no progress for {} s; nudged", STUCK_MS / 1000) });
            self.queue.push(NUDGE.into());
            self.flush(&mut step);
        }
        step
    }

    /// One message from the server; `now_ms` dates items it doesn't date itself.
    pub fn on_message(&mut self, m: &Value, now_ms: i64) -> Step {
        self.heard_ms = now_ms;
        let mut step = Step::default();
        match (m.get("id"), m.get("method")) {
            (Some(id), Some(method)) => self.on_request(&mut step, id.clone(), method.as_str().unwrap_or_default(), &m["params"]),
            (Some(id), None) => self.on_response(&mut step, id.as_u64().unwrap_or(0), m),
            (None, Some(method)) => self.on_notification(&mut step, method.as_str().unwrap_or_default(), &m["params"], now_ms),
            _ => {}
        }
        step
    }

    fn on_response(&mut self, step: &mut Step, id: u64, m: &Value) {
        let Some(asked) = self.asked.remove(&id) else { return };
        let r = &m["result"];
        let err = m["error"]["message"].as_str().map(String::from);
        match asked {
            Asked::Initialize => {
                step.send.push(json!({"method": "initialized"}));
                self.request(step, Asked::Account, "account/read", json!({}));
            }
            Asked::Account => {
                if r["account"].is_null() && r["requiresOpenaiAuth"] == true {
                    let why = "Codex 没有登录".to_owned();
                    step.events.push(AgentEvent::Notice { text: why.clone() });
                    self.status = HostStatus::Error { why };
                } else {
                    self.open_thread(step);
                }
            }
            Asked::Thread { resume } => match r["thread"]["id"].as_str() {
                Some(t) => {
                    self.thread = Some(t.to_owned());
                    step.events.push(AgentEvent::Session { id: t.to_owned() });
                    step.events.push(self.config());
                    self.status = HostStatus::Idle;
                    self.flush(step);
                }
                // The thread to go on with is gone: a new one.
                None if resume => {
                    self.opts.thread = None;
                    self.open_thread(step);
                }
                None => {
                    let why = err.unwrap_or_else(|| "Codex didn't start a thread".into());
                    step.events.push(AgentEvent::Notice { text: why.clone() });
                    self.status = HostStatus::Error { why };
                }
            },
            Asked::TurnStart => {
                if let Some(e) = err {
                    self.starting = false;
                    self.status = HostStatus::Idle;
                    step.events.push(AgentEvent::Notice { text: e });
                } else if let Some(t) = r["turn"]["id"].as_str() {
                    self.turn.get_or_insert_with(|| t.to_owned());
                    self.starting = false;
                    self.flush(step);
                }
            }
            Asked::Other => {
                if let Some(e) = err {
                    step.events.push(AgentEvent::Notice { text: e });
                }
            }
        }
    }

    fn on_request(&mut self, step: &mut Step, id: Value, method: &str, p: &Value) {
        let request = match method {
            "item/commandExecution/requestApproval" => Some(PermissionRequest {
                id: format!("codex-{id}"),
                tool: ToolKind::Execute,
                title: unwrap_shell(p["command"].as_str().unwrap_or_default()),
                detail: p["reason"].as_str().or(p["cwd"].as_str()).unwrap_or_default().to_owned(),
                options: approval_options(),
            }),
            "item/fileChange/requestApproval" => Some(PermissionRequest {
                id: format!("codex-{id}"),
                tool: ToolKind::Edit,
                title: p["grantRoot"].as_str().map(|r| format!("写入 {r}")).unwrap_or_else(|| "修改文件".into()),
                detail: p["reason"].as_str().unwrap_or_default().to_owned(),
                options: approval_options(),
            }),
            _ => None,
        };
        match request {
            Some(r) => {
                self.status = HostStatus::Blocked { why: r.title.clone() };
                self.approvals.push((r.id.clone(), id, method.to_owned()));
                step.events.push(AgentEvent::Permission { request: r });
            }
            // Questions Lila can't put to the person yet: declined, so the turn goes on.
            None => step.send.push(json!({"id": id, "error": {"code": -32601, "message": format!("Lila does not answer {method}")}})),
        }
    }

    fn on_notification(&mut self, step: &mut Step, method: &str, p: &Value, now_ms: i64) {
        match method {
            "turn/started" => {
                if let Some(t) = p["turn"]["id"].as_str() {
                    self.turn = Some(t.to_owned());
                    self.starting = false;
                    self.status = HostStatus::Working;
                    step.events.push(AgentEvent::TurnStarted { turn: t.to_owned() });
                    self.flush(step);
                }
            }
            "turn/completed" => {
                let turn = p["turn"]["id"].as_str().unwrap_or_default().to_owned();
                let failed = (p["turn"]["status"] == "failed").then(|| p["turn"]["error"]["message"].as_str().unwrap_or("failed").to_owned());
                self.turn = None;
                self.starting = false;
                self.running.clear();
                for (id, _, _) in self.approvals.drain(..) {
                    step.events.push(AgentEvent::PermissionResolved { id });
                }
                self.status = HostStatus::Done;
                step.events.push(AgentEvent::TurnEnded { turn, failed });
                self.flush(step);
            }
            "item/started" | "item/completed" => {
                if let Some(id) = p["item"]["id"].as_str() {
                    if method == "item/started" {
                        self.running.insert(id.to_owned());
                    } else {
                        self.running.remove(id);
                    }
                }
                if let Some(item) = item_of(&p["item"], self.turn.clone(), now_ms) {
                    step.events.push(AgentEvent::Item { item });
                }
            }
            "serverRequest/resolved" => {
                let id = p["requestId"].clone();
                if let Some(i) = self.approvals.iter().position(|(_, rpc, _)| *rpc == id) {
                    let (perm, _, _) = self.approvals.remove(i);
                    step.events.push(AgentEvent::PermissionResolved { id: perm });
                    self.unblock();
                }
            }
            "thread/tokenUsage/updated" => {
                let t = &p["tokenUsage"]["total"];
                let n = |k: &str| t[k].as_u64().unwrap_or(0);
                step.events.push(AgentEvent::Usage { input: n("inputTokens"), output: n("outputTokens"), cached: n("cachedInputTokens") });
            }
            "account/rateLimits/updated" => {
                let w = &p["rateLimits"]["primary"];
                if let Some(used) = w["usedPercent"].as_f64() {
                    step.events.push(AgentEvent::Limits { used_percent: used, resets_ms: w["resetsAt"].as_i64().map(|s| s * 1000) });
                }
            }
            "error" => {
                if let Some(text) = p["error"]["message"].as_str().or(p["message"].as_str()) {
                    step.events.push(AgentEvent::Notice { text: text.to_owned() });
                }
            }
            _ => {}
        }
    }
}

/// `/bin/zsh -lc 'cmd'` → `cmd`: Codex runs commands through a login shell.
fn unwrap_shell(command: &str) -> String {
    let Some((shell, rest)) = command.split_once(" -lc ") else { return command.to_owned() };
    if !shell.ends_with("sh") {
        return command.to_owned();
    }
    let rest = rest.trim();
    rest.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')).or_else(|| rest.strip_prefix('"').and_then(|r| r.strip_suffix('"'))).unwrap_or(rest).to_owned()
}

fn state_of(status: Option<&str>, exit: Option<i64>) -> ItemState {
    match status {
        Some("inProgress") => ItemState::Running,
        Some("failed" | "declined") => ItemState::Failed,
        _ if exit.is_some_and(|c| c != 0) => ItemState::Failed,
        _ => ItemState::Done,
    }
}

/// One of the thread's items, translated; `None` for those not shown.
fn item_of(i: &Value, turn: Option<String>, now_ms: i64) -> Option<Item> {
    let id = i["id"].as_str()?.to_owned();
    let at_ms = i["startedAtMs"].as_i64().or(i["completedAtMs"].as_i64()).unwrap_or(now_ms);
    let strings = |v: &Value| v.as_array().into_iter().flatten().filter_map(|x| x.as_str().or(x["text"].as_str())).collect::<Vec<_>>().join("\n");
    let tool = |tool: ToolKind, name: &str, title: String, output: Option<String>, diffs: Vec<FileDiff>| ItemKind::ToolCall { tool, name: name.into(), title, output: output.filter(|o| !o.trim().is_empty()).map(|o| clip(o.trim(), MAX_OUTPUT)), diffs };
    let (kind, state) = match i["type"].as_str()? {
        "userMessage" => {
            let text = strings(&i["content"]);
            let text = text.trim();
            // What went with the words: images by path, files as mentions (a skill is in the words as `$name`).
            let attachments: Vec<Attachment> = i["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| {
                    let path = c["path"].as_str()?;
                    match c["type"].as_str()? {
                        "localImage" => Some(Attachment::file(path)),
                        "mention" => Some(Attachment { path: path.to_owned(), name: c["name"].as_str().unwrap_or_default().to_owned(), kind: AttachmentKind::File }),
                        _ => None,
                    }
                })
                .collect();
            if text.is_empty() && attachments.is_empty() {
                return None;
            }
            (ItemKind::UserMessage { text: text.to_owned(), origin: origin(text), attachments }, ItemState::Done)
        }
        "agentMessage" => {
            let text = i["text"].as_str().unwrap_or_default().trim().to_owned();
            if text.is_empty() {
                return None;
            }
            (ItemKind::AgentMessage { text }, ItemState::Done)
        }
        "plan" => (ItemKind::AgentMessage { text: i["text"].as_str().unwrap_or_default().to_owned() }, ItemState::Done),
        "reasoning" => {
            let text = strings(&i["summary"]);
            if text.trim().is_empty() {
                return None;
            }
            (ItemKind::Thought { text: text.trim().to_owned() }, ItemState::Done)
        }
        "commandExecution" => (
            tool(ToolKind::Execute, "shell", unwrap_shell(i["command"].as_str().unwrap_or_default()), i["aggregatedOutput"].as_str().map(String::from), Vec::new()),
            state_of(i["status"].as_str(), i["exitCode"].as_i64()),
        ),
        "fileChange" => {
            let changes = i["changes"].as_array().cloned().unwrap_or_default();
            let paths: Vec<&str> = changes.iter().filter_map(|c| c["path"].as_str()).collect();
            let diffs = changes.iter().map(|c| FileDiff { path: c["path"].as_str().unwrap_or_default().to_owned(), lines: c["diff"].as_str().unwrap_or_default().lines().map(String::from).collect() }).collect();
            (tool(ToolKind::Edit, "apply_patch", paths.join(", "), None, diffs), state_of(i["status"].as_str(), None))
        }
        "mcpToolCall" => (
            tool(ToolKind::Other, "mcp", format!("{} · {}", i["server"].as_str().unwrap_or_default(), i["tool"].as_str().unwrap_or_default()), None, Vec::new()),
            state_of(i["status"].as_str(), None),
        ),
        "webSearch" => (tool(ToolKind::Fetch, "web_search", i["query"].as_str().unwrap_or_default().to_owned(), None, Vec::new()), ItemState::Done),
        _ => return None,
    };
    Some(Item { id, turn, at_ms, state, kind })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> AppServer {
        AppServer::new(Options { cwd: "/r".into(), model: "gpt".into(), effort: Some("low".into()), sandbox: Some("workspace-write".into()), instructions: "You are tester.".into(), thread: None })
    }

    /// Answers the request it sent last with `result`.
    fn reply(s: &mut AppServer, step: &Step, result: Value) -> Step {
        let id = step.send.iter().rev().find_map(|m| m["id"].as_u64()).expect("a request");
        s.on_message(&json!({"id": id, "result": result}), 0)
    }

    fn methods(step: &Step) -> Vec<&str> {
        step.send.iter().filter_map(|m| m["method"].as_str()).collect()
    }

    #[test]
    fn handshake_then_a_thread_then_turns_and_steering() {
        let mut s = server();
        let init = s.start();
        assert_eq!(methods(&init), ["initialize"]);
        let acc = reply(&mut s, &init, json!({"userAgent": "x"}));
        assert_eq!(methods(&acc), ["initialized", "account/read"]);
        // Words before the thread is open wait for it.
        assert!(s.command(Command::Prompt { text: "<lila task=\"T1\">\nVerify\n</lila>".into(), attachments: vec![] }).send.is_empty());
        let start = reply(&mut s, &acc, json!({"account": {"type": "chatgpt"}}));
        assert_eq!(methods(&start), ["thread/start"]);
        assert_eq!(start.send[0]["params"]["developerInstructions"], "You are tester.");
        assert_eq!(start.send[0]["params"]["approvalPolicy"], "on-request");
        let open = reply(&mut s, &start, json!({"thread": {"id": "th1"}}));
        assert_eq!(open.events, [AgentEvent::Session { id: "th1".into() }, AgentEvent::Config { model: Some("gpt".into()), effort: Some("low".into()), models: vec![], efforts: vec![] }]);
        assert_eq!(methods(&open), ["turn/start"], "the waiting words start a turn");
        assert_eq!((open.send[0]["params"]["threadId"].as_str(), open.send[0]["params"]["effort"].as_str()), (Some("th1"), Some("low")));
        assert_eq!(s.status(), HostStatus::Working);
        // A note while the turn is being opened waits, then joins it.
        assert!(s.command(Command::Prompt { text: "also check docs".into(), attachments: vec![] }).send.is_empty());
        let started = s.on_message(&json!({"method": "turn/started", "params": {"turn": {"id": "tu1"}}}), 0);
        assert_eq!(methods(&started), ["turn/steer"]);
        assert_eq!(started.send[0]["params"]["expectedTurnId"], "tu1");
        let done = s.on_message(&json!({"method": "turn/completed", "params": {"turn": {"id": "tu1", "status": "completed"}}}), 0);
        assert_eq!(done.events, [AgentEvent::TurnEnded { turn: "tu1".into(), failed: None }]);
        assert_eq!(s.status(), HostStatus::Done);
        // Idle again: the next words open a new turn.
        assert_eq!(methods(&s.command(Command::Prompt { text: "next".into(), attachments: vec![] })), ["turn/start"]);
    }

    #[test]
    fn a_resumed_thread_that_is_gone_starts_a_new_one_and_a_missing_login_is_an_error() {
        let mut s = AppServer::new(Options { thread: Some("old".into()), ..Options::default() });
        let init = s.start();
        let acc = reply(&mut s, &init, json!({}));
        let resume = reply(&mut s, &acc, json!({"account": {"type": "chatgpt"}}));
        assert_eq!(methods(&resume), ["thread/resume"]);
        assert_eq!(resume.send[0]["params"]["threadId"], "old");
        let id = resume.send[0]["id"].as_u64().unwrap();
        let fresh = s.on_message(&json!({"id": id, "error": {"code": -1, "message": "no rollout"}}), 0);
        assert_eq!(methods(&fresh), ["thread/start"]);

        let mut s = server();
        let init = s.start();
        let acc = reply(&mut s, &init, json!({}));
        let out = reply(&mut s, &acc, json!({"account": null, "requiresOpenaiAuth": true}));
        assert!(matches!(s.status(), HostStatus::Error { .. }));
        assert!(matches!(&out.events[0], AgentEvent::Notice { .. }));
    }

    #[test]
    fn approvals_block_until_answered() {
        let mut s = server();
        let req = s.on_message(&json!({"id": 9, "method": "item/commandExecution/requestApproval", "params": {"command": "/bin/zsh -lc 'npm test'", "cwd": "/r", "itemId": "e1"}}), 0);
        let AgentEvent::Permission { request } = &req.events[0] else { panic!("{req:?}") };
        assert_eq!((request.id.as_str(), request.title.as_str(), request.tool), ("codex-9", "npm test", ToolKind::Execute));
        assert_eq!(request.options.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(), ["accept", "acceptForSession", "decline"]);
        assert!(matches!(s.status(), HostStatus::Blocked { why } if why == "npm test"));
        let answered = s.command(Command::Answer { permission: "codex-9".into(), option: "accept".into() });
        assert_eq!(answered.send, [json!({"id": 9, "result": {"decision": "accept"}})]);
        assert_eq!(answered.events, [AgentEvent::PermissionResolved { id: "codex-9".into() }]);
        assert_eq!(s.status(), HostStatus::Working);
        // What Lila can't ask the person about is declined at once.
        let other = s.on_message(&json!({"id": 10, "method": "item/tool/requestUserInput", "params": {}}), 0);
        assert_eq!(other.send[0]["error"]["code"], -32601);
    }

    #[test]
    fn items_translate() {
        let mut s = server();
        let mut item = |v: Value| match s.on_message(&json!({"method": "item/completed", "params": {"item": v}}), 7).events.pop() {
            Some(AgentEvent::Item { item }) => Some(item),
            _ => None,
        };
        let cmd = item(json!({"type": "commandExecution", "id": "c1", "command": "/bin/zsh -lc 'cargo test'", "aggregatedOutput": "ok\n", "exitCode": 1, "status": "completed"})).unwrap();
        assert_eq!(cmd.state, ItemState::Failed);
        assert!(matches!(&cmd.kind, ItemKind::ToolCall { title, output: Some(o), .. } if title == "cargo test" && o == "ok"));
        let edit = item(json!({"type": "fileChange", "id": "f1", "status": "completed", "changes": [{"path": "a.rs", "kind": {"type": "update"}, "diff": "@@ -1 +1 @@\n-a\n+b"}]})).unwrap();
        assert!(matches!(&edit.kind, ItemKind::ToolCall { tool: ToolKind::Edit, diffs, .. } if diffs[0].lines == ["@@ -1 +1 @@", "-a", "+b"]));
        let said = item(json!({"type": "userMessage", "id": "u1", "content": [{"type": "text", "text": "<lila note about=\"T1\">ok</lila>"}]})).unwrap();
        assert!(matches!(said.kind, ItemKind::UserMessage { origin: Origin::Lila { task: None }, .. }));
        assert_eq!(said.at_ms, 7);
        assert!(item(json!({"type": "reasoning", "id": "r1", "summary": [], "content": []})).is_none(), "an empty thought is not shown");
        assert!(item(json!({"type": "contextCompaction", "id": "x"})).is_none());
    }

    /// A server with a thread open and no turn running.
    fn idle() -> AppServer {
        let mut s = server();
        s.thread = Some("th".into());
        s
    }

    #[test]
    fn files_go_with_the_words_as_the_items_codex_has_for_them() {
        let mut s = idle();
        let files = vec![Attachment::file("/r/shot.png"), Attachment::file("/r/notes.md"), Attachment { path: "/home/.agents/skills/tdd/SKILL.md".into(), name: "tdd".into(), kind: AttachmentKind::Skill }];
        let start = s.command(Command::Prompt { text: "$tdd look at these".into(), attachments: files.clone() });
        assert_eq!(
            start.send[0]["params"]["input"],
            json!([
                {"type": "text", "text": "$tdd look at these"},
                {"type": "localImage", "path": "/r/shot.png"},
                {"type": "mention", "name": "notes.md", "path": "/r/notes.md"},
                {"type": "skill", "name": "tdd", "path": "/home/.agents/skills/tdd/SKILL.md"},
            ])
        );
        // Into a running turn they go the same way.
        s.on_message(&json!({"method": "turn/started", "params": {"turn": {"id": "tu1"}}}), 0);
        let steer = s.command(Command::Prompt { text: "and this".into(), attachments: files[..1].to_vec() });
        assert_eq!(methods(&steer), ["turn/steer"]);
        assert_eq!(steer.send[0]["params"]["input"][1], json!({"type": "localImage", "path": "/r/shot.png"}));
        // The server echoes them back in the user's message: shown as attachments.
        let mut s2 = idle();
        let said = s2.on_message(&json!({"method": "item/completed", "params": {"item": {"type": "userMessage", "id": "u9", "content": [{"type": "text", "text": "look"}, {"type": "localImage", "path": "/r/shot.png"}, {"type": "mention", "name": "notes.md", "path": "/r/notes.md"}, {"type": "skill", "name": "tdd", "path": "/s"}]}}}), 5);
        let AgentEvent::Item { item } = &said.events[0] else { panic!("{said:?}") };
        assert!(matches!(&item.kind, ItemKind::UserMessage { text, attachments, .. } if text == "look" && attachments.len() == 2 && attachments[0].kind == AttachmentKind::Image && attachments[1].name == "notes.md"));
    }

    #[test]
    fn the_model_and_effort_can_change_for_the_rest_of_the_session() {
        let mut s = idle();
        let set = s.command(Command::SetModel { model: "gpt-6-luna".into() });
        assert_eq!(set.events, [AgentEvent::Config { model: Some("gpt-6-luna".into()), effort: Some("low".into()), models: vec![], efforts: vec![] }]);
        assert!(set.send.is_empty(), "nothing is sent until the next turn");
        s.command(Command::SetEffort { effort: "xhigh".into() });
        let turn = s.command(Command::Prompt { text: "go".into(), attachments: vec![] });
        assert_eq!((turn.send[0]["params"]["model"].as_str(), turn.send[0]["params"]["effort"].as_str()), (Some("gpt-6-luna"), Some("xhigh")));
        // The context can be compacted: a request of the protocol, not words.
        let compact = s.command(Command::Compact);
        assert_eq!((methods(&compact), &compact.send[0]["params"]["threadId"]), (vec!["thread/compact/start"], &json!("th")));
    }

    #[test]
    fn a_quiet_turn_with_nothing_running_is_nudged_once() {
        let mut s = server();
        s.thread = Some("th".into());
        s.on_message(&json!({"method": "turn/started", "params": {"turn": {"id": "tu"}}}), 1_000);
        s.on_message(&json!({"method": "item/started", "params": {"item": {"type": "commandExecution", "id": "c1", "command": "cargo build", "status": "inProgress"}}}), 2_000);
        assert!(s.tick(2_000 + STUCK_MS).send.is_empty(), "a command still runs");
        s.on_message(&json!({"method": "item/completed", "params": {"item": {"type": "commandExecution", "id": "c1", "command": "cargo build", "status": "completed", "exitCode": 0}}}), 3_000);
        assert!(s.tick(3_000 + STUCK_MS - 1).send.is_empty(), "not quiet long enough");
        let nudge = s.tick(3_000 + STUCK_MS);
        assert_eq!(methods(&nudge), ["turn/steer"]);
        assert_eq!(nudge.send[0]["params"]["input"][0]["text"], NUDGE);
        assert!(s.tick(9_000 + 2 * STUCK_MS).send.is_empty(), "once per turn");
    }

    #[test]
    fn shells_unwrap() {
        assert_eq!(unwrap_shell("/bin/zsh -lc 'lila task done T1'"), "lila task done T1");
        assert_eq!(unwrap_shell("ls -la"), "ls -la");
    }
}
