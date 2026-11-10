//! An agent over ACP (the Agent Client Protocol: JSON-RPC 2.0, one message per
//! line), for `lila host omp` (architecture v3 §7.3, P0 S3). Pure translation,
//! like `codex`: what the agent sends is read with ACP's own types
//! (`agent-client-protocol-schema`); the few requests Lila makes are plain JSON.

use crate::codex::Step;
use crate::model::*;
use agent_client_protocol_schema::v1 as acp;
use serde_json::{Value, json};
use std::collections::HashMap;

/// How the member's session is opened.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub cwd: String,
    /// A session to go on with.
    pub session: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Asked {
    Initialize,
    Session { load: bool },
    Prompt,
    /// `session/set_config_option`: the model or the effort.
    SetConfig,
}

/// Text being streamed (a reply, a thought, the person's words on a load), until it ends.
struct Stream {
    id: String,
    kind: StreamKind,
    text: String,
    at_ms: i64,
    shown_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum StreamKind {
    Reply,
    Thought,
    User,
}

pub struct Acp {
    opts: Options,
    next: u64,
    asked: HashMap<u64, Asked>,
    can_load: bool,
    /// The agent takes images in a prompt (`promptCapabilities.image`).
    can_image: bool,
    /// The ids of the session's model and thought-level options, once it lists them.
    model_option: Option<String>,
    effort_option: Option<String>,
    session: Option<String>,
    /// The prompt under way (its request id), if any.
    turn: Option<String>,
    queue: Vec<String>,
    queue_files: Vec<Attachment>,
    approvals: Vec<(String, Value)>,
    status: HostStatus,
    stream: Option<Stream>,
    tools: HashMap<String, Item>,
    streams: u64,
    /// The time of the last input, for items made on a command.
    clock: i64,
}

/// Largest image sent inline; a bigger one goes as a link.
const MAX_IMAGE: usize = 8 << 20;

/// How often a reply being streamed is shown again.
const SHOW_EVERY_MS: i64 = 800;

impl Acp {
    pub fn new(opts: Options) -> Self {
        Acp {
            opts,
            next: 0,
            asked: HashMap::new(),
            can_load: false,
            can_image: false,
            model_option: None,
            effort_option: None,
            session: None,
            turn: None,
            queue: Vec::new(),
            queue_files: Vec::new(),
            approvals: Vec::new(),
            status: HostStatus::Working,
            stream: None,
            tools: HashMap::new(),
            streams: 0,
            clock: 0,
        }
    }

    pub fn status(&self) -> HostStatus {
        self.status.clone()
    }

    fn request(&mut self, step: &mut Step, asked: Asked, method: &str, params: Value) -> u64 {
        self.next += 1;
        self.asked.insert(self.next, asked);
        step.send.push(json!({"jsonrpc": "2.0", "id": self.next, "method": method, "params": params}));
        self.next
    }

    pub fn start(&mut self) -> Step {
        let mut step = Step::default();
        let caps = json!({"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false});
        self.request(&mut step, Asked::Initialize, "initialize", json!({"protocolVersion": 1, "clientCapabilities": caps, "clientInfo": {"name": "lila", "version": env!("CARGO_PKG_VERSION")}}));
        step
    }

    fn open_session(&mut self, step: &mut Step) {
        let cwd = self.opts.cwd.clone();
        match self.opts.session.clone().filter(|_| self.can_load) {
            Some(id) => {
                self.session = Some(id.clone());
                self.request(step, Asked::Session { load: true }, "session/load", json!({"sessionId": id, "cwd": cwd, "mcpServers": []}));
            }
            None => {
                self.request(step, Asked::Session { load: false }, "session/new", json!({"cwd": cwd, "mcpServers": []}));
            }
        }
    }

    /// Sends what waits, when no prompt is under way (ACP has no steering).
    fn flush(&mut self, step: &mut Step) {
        let Some(session) = self.session.clone() else { return };
        if self.queue.is_empty() || self.turn.is_some() || self.asked.values().any(|a| matches!(a, Asked::Session { .. })) {
            return;
        }
        let text = std::mem::take(&mut self.queue).join("\n\n");
        let files = std::mem::take(&mut self.queue_files);
        let mut blocks = vec![json!({"type": "text", "text": text})];
        blocks.extend(files.iter().filter_map(|f| self.block(f)));
        let id = self.request(step, Asked::Prompt, "session/prompt", json!({"sessionId": session, "prompt": blocks}));
        let turn = format!("prompt-{id}");
        self.turn = Some(turn.clone());
        self.status = HostStatus::Working;
        step.events.push(AgentEvent::TurnStarted { turn: turn.clone() });
        // ACP doesn't echo the prompt back: it is the turn's first item.
        let said = text.trim().to_owned();
        let item = Item { id: format!("{turn}-said"), turn: Some(turn), at_ms: self.clock, state: ItemState::Done, kind: ItemKind::UserMessage { origin: origin(&said), text: said, attachments: files } };
        step.events.push(AgentEvent::Item { item });
    }

    /// A file as a prompt block: an image the agent takes, inline; any other file as a link it reads itself.
    fn block(&self, f: &Attachment) -> Option<Value> {
        let uri = format!("file://{}", f.path);
        if f.kind == AttachmentKind::Skill {
            return None;
        }
        if f.kind == AttachmentKind::Image
            && self.can_image
            && let Some(mime) = image_mime(&f.path)
            && let Ok(bytes) = std::fs::read(&f.path)
            && bytes.len() <= MAX_IMAGE
        {
            return Some(json!({"type": "image", "data": base64(&bytes), "mimeType": mime, "uri": uri}));
        }
        Some(json!({"type": "resource_link", "uri": uri, "name": f.label()}))
    }

    pub fn command(&mut self, c: Command, now_ms: i64) -> Step {
        self.clock = now_ms;
        let mut step = Step::default();
        match c {
            Command::Prompt { text, attachments } => {
                self.queue.push(text);
                self.queue_files.extend(attachments);
                self.flush(&mut step);
            }
            Command::SetModel { model } => self.set_config(&mut step, self.model_option.clone(), model),
            Command::SetEffort { effort } => self.set_config(&mut step, self.effort_option.clone(), effort),
            // The agent has `/compact` among its commands, sent as words.
            Command::Compact => {}
            Command::Interrupt => {
                if let (Some(session), Some(_)) = (self.session.clone(), self.turn.as_ref()) {
                    step.send.push(json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": session}}));
                    // Questions still open go away with the prompt.
                    for (id, rpc) in self.approvals.drain(..) {
                        step.send.push(json!({"jsonrpc": "2.0", "id": rpc, "result": {"outcome": {"outcome": "cancelled"}}}));
                        step.events.push(AgentEvent::PermissionResolved { id });
                    }
                }
            }
            Command::Answer { permission, option } => {
                if let Some(i) = self.approvals.iter().position(|(p, _)| *p == permission) {
                    let (p, rpc) = self.approvals.remove(i);
                    step.send.push(json!({"jsonrpc": "2.0", "id": rpc, "result": {"outcome": {"outcome": "selected", "optionId": option}}}));
                    step.events.push(AgentEvent::PermissionResolved { id: p });
                    if self.approvals.is_empty() && self.turn.is_some() {
                        self.status = HostStatus::Working;
                    }
                }
            }
        }
        step
    }

    fn set_config(&mut self, step: &mut Step, option: Option<String>, value: String) {
        if let (Some(session), Some(option)) = (self.session.clone(), option) {
            self.request(step, Asked::SetConfig, "session/set_config_option", json!({"sessionId": session, "configId": option, "value": value}));
        }
    }

    /// The model and effort the agent lists in `configOptions`, as an event (and the ids to set them by).
    fn config_event(&mut self, options: &[acp::SessionConfigOption]) -> Option<AgentEvent> {
        let mut ev = AgentEvent::Config { model: None, effort: None, models: Vec::new(), efforts: Vec::new() };
        let AgentEvent::Config { model, effort, models, efforts } = &mut ev else { unreachable!() };
        for o in options {
            let acp::SessionConfigKind::Select(sel) = &o.kind else { continue };
            let acp::SessionConfigSelectOptions::Ungrouped(list) = &sel.options else { continue };
            let choices: Vec<Choice> = list.iter().map(|c| Choice { id: c.value.0.to_string(), name: c.name.clone() }).collect();
            let current = Some(sel.current_value.0.to_string());
            match o.category {
                Some(acp::SessionConfigOptionCategory::Model) => {
                    self.model_option = Some(o.id.0.to_string());
                    (*model, *models) = (current, choices);
                }
                Some(acp::SessionConfigOptionCategory::ThoughtLevel) => {
                    self.effort_option = Some(o.id.0.to_string());
                    (*effort, *efforts) = (current, choices);
                }
                _ => {}
            }
        }
        (model.is_some() || effort.is_some()).then_some(ev)
    }

    /// Called now and then: a reply streamed in small pieces is shown as it grows.
    pub fn tick(&mut self, now_ms: i64) -> Step {
        let mut step = Step::default();
        if let Some(s) = &mut self.stream
            && s.kind == StreamKind::Reply
            && now_ms - s.shown_ms >= SHOW_EVERY_MS
        {
            s.shown_ms = now_ms;
            step.events.push(AgentEvent::Item { item: stream_item(s, self.turn.clone(), ItemState::Running) });
        }
        step
    }

    pub fn on_message(&mut self, m: &Value, now_ms: i64) -> Step {
        self.clock = now_ms;
        let mut step = Step::default();
        match (m.get("id"), m.get("method")) {
            (Some(id), Some(method)) => self.on_request(&mut step, id.clone(), method.as_str().unwrap_or_default(), &m["params"]),
            (Some(id), None) => self.on_response(&mut step, id.as_u64().unwrap_or(0), m, now_ms),
            (None, Some(method)) if method == "session/update" => self.on_update(&mut step, &m["params"], now_ms),
            _ => {}
        }
        step
    }

    fn on_response(&mut self, step: &mut Step, id: u64, m: &Value, now_ms: i64) {
        let Some(asked) = self.asked.remove(&id) else { return };
        let r = &m["result"];
        let err = m["error"]["message"].as_str().map(String::from);
        match asked {
            Asked::Initialize => {
                if let Some(e) = err {
                    self.fail(step, e);
                    return;
                }
                self.can_load = r["agentCapabilities"]["loadSession"] == true;
                self.can_image = r["agentCapabilities"]["promptCapabilities"]["image"] == true;
                self.open_session(step);
            }
            Asked::Session { load } => {
                if let Some(e) = err {
                    if load {
                        // The session to go on with is gone: a new one.
                        self.opts.session = None;
                        self.session = None;
                        self.open_session(step);
                    } else {
                        self.fail(step, e);
                    }
                    return;
                }
                self.end_stream(step, now_ms);
                if let Some(id) = r["sessionId"].as_str() {
                    self.session = Some(id.to_owned());
                }
                if let Some(id) = self.session.clone() {
                    step.events.push(AgentEvent::Session { id });
                }
                if let Ok(options) = serde_json::from_value::<Vec<acp::SessionConfigOption>>(r["configOptions"].clone())
                    && let Some(ev) = self.config_event(&options)
                {
                    step.events.push(ev);
                }
                self.status = HostStatus::Idle;
                self.flush(step);
            }
            Asked::SetConfig => {
                if let Some(e) = err {
                    step.events.push(AgentEvent::Notice { text: e });
                } else if let Ok(options) = serde_json::from_value::<Vec<acp::SessionConfigOption>>(r["configOptions"].clone())
                    && let Some(ev) = self.config_event(&options)
                {
                    step.events.push(ev);
                }
            }
            Asked::Prompt => {
                self.end_stream(step, now_ms);
                let turn = self.turn.take().unwrap_or_default();
                for (id, _) in self.approvals.drain(..) {
                    step.events.push(AgentEvent::PermissionResolved { id });
                }
                let failed = err.or_else(|| (r["stopReason"] == "refusal").then(|| "refused".to_owned()));
                if let Some(u) = r.get("usage").filter(|u| u.is_object()) {
                    let n = |k: &str| u[k].as_u64().unwrap_or(0);
                    step.events.push(AgentEvent::Usage { input: n("inputTokens"), output: n("outputTokens"), cached: n("cachedReadTokens") });
                }
                step.events.push(AgentEvent::TurnEnded { turn, failed });
                self.status = HostStatus::Done;
                self.flush(step);
            }
        }
    }

    fn fail(&mut self, step: &mut Step, why: String) {
        step.events.push(AgentEvent::Notice { text: why.clone() });
        self.status = HostStatus::Error { why };
    }

    fn on_request(&mut self, step: &mut Step, id: Value, method: &str, p: &Value) {
        if method != "session/request_permission" {
            // Lila offers the agent no files or terminals of its own.
            step.send.push(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("Lila does not answer {method}")}}));
            return;
        }
        let Ok(req) = serde_json::from_value::<acp::RequestPermissionRequest>(p.clone()) else {
            step.send.push(json!({"jsonrpc": "2.0", "id": id, "result": {"outcome": {"outcome": "cancelled"}}}));
            return;
        };
        let f = &req.tool_call.fields;
        let title = f.raw_input.as_ref().and_then(|i| i["command"].as_str().map(String::from)).or_else(|| f.title.clone()).unwrap_or_else(|| "tool".into());
        let mut options: Vec<PermOption> = req
            .options
            .iter()
            .map(|o| {
                let label = match o.kind {
                    acp::PermissionOptionKind::AllowOnce => "允许",
                    acp::PermissionOptionKind::AllowAlways => "总是允许",
                    acp::PermissionOptionKind::RejectOnce => "拒绝",
                    acp::PermissionOptionKind::RejectAlways => "总是拒绝",
                    _ => o.name.as_str(),
                };
                PermOption { id: o.option_id.0.to_string(), label: label.to_owned() }
            })
            .collect();
        // The one to suggest first.
        options.sort_by_key(|o| if o.label == "允许" { 0 } else { 1 });
        let request = PermissionRequest { id: format!("acp-{id}"), tool: f.kind.map(kind_of).unwrap_or(ToolKind::Other), title: unwrap_dollar(&title), detail: String::new(), options };
        self.status = HostStatus::Blocked { why: request.title.clone() };
        self.approvals.push((request.id.clone(), id));
        step.events.push(AgentEvent::Permission { request });
    }

    /// Ends the text being streamed: its final item.
    fn end_stream(&mut self, step: &mut Step, _now_ms: i64) {
        if let Some(s) = self.stream.take()
            && !s.text.trim().is_empty()
        {
            step.events.push(AgentEvent::Item { item: stream_item(&s, self.turn.clone(), ItemState::Done) });
        }
    }

    fn chunk(&mut self, step: &mut Step, kind: StreamKind, c: &acp::ContentChunk, now_ms: i64) {
        let acp::ContentBlock::Text(t) = &c.content else { return };
        let id = c.message_id.as_ref().map(|m| m.0.to_string());
        let same = self.stream.as_ref().is_some_and(|s| s.kind == kind && id.as_ref().is_none_or(|i| *i == s.id));
        if !same {
            self.end_stream(step, now_ms);
            self.streams += 1;
            let id = id.unwrap_or_else(|| format!("{kind:?}-{}", self.streams).to_lowercase());
            self.stream = Some(Stream { id, kind, text: String::new(), at_ms: now_ms, shown_ms: now_ms });
        }
        if let Some(s) = &mut self.stream {
            s.text.push_str(&t.text);
        }
    }

    fn on_update(&mut self, step: &mut Step, p: &Value, now_ms: i64) {
        let Ok(n) = serde_json::from_value::<acp::SessionNotification>(p.clone()) else { return };
        match n.update {
            acp::SessionUpdate::AgentMessageChunk(c) => self.chunk(step, StreamKind::Reply, &c, now_ms),
            acp::SessionUpdate::AgentThoughtChunk(c) => self.chunk(step, StreamKind::Thought, &c, now_ms),
            acp::SessionUpdate::UserMessageChunk(c) => self.chunk(step, StreamKind::User, &c, now_ms),
            acp::SessionUpdate::ToolCall(t) => {
                self.end_stream(step, now_ms);
                let id = t.tool_call_id.0.to_string();
                let input_title = t.raw_input.as_ref().and_then(|i| i["command"].as_str().map(String::from));
                let item = Item {
                    id: id.clone(),
                    turn: self.turn.clone(),
                    at_ms: now_ms,
                    state: state_of(t.status),
                    kind: ItemKind::ToolCall {
                        tool: kind_of(t.kind),
                        name: t.name.clone().unwrap_or_else(|| "tool".into()),
                        title: unwrap_dollar(&input_title.unwrap_or(t.title.clone())),
                        output: output_of(&t.content, t.raw_output.as_ref()),
                        diffs: diffs_of(&t.content),
                    },
                };
                self.tools.insert(id, item.clone());
                step.events.push(AgentEvent::Item { item });
            }
            acp::SessionUpdate::ToolCallUpdate(u) => {
                let id = u.tool_call_id.0.to_string();
                let Some(item) = self.tools.get_mut(&id) else { return };
                let f = &u.fields;
                if let Some(s) = f.status {
                    item.state = state_of(s);
                }
                if let ItemKind::ToolCall { tool, title, output, diffs, .. } = &mut item.kind {
                    if let Some(k) = f.kind {
                        *tool = kind_of(k);
                    }
                    if let Some(t) = f.raw_input.as_ref().and_then(|i| i["command"].as_str()).map(String::from).or_else(|| f.title.clone()) {
                        *title = unwrap_dollar(&t);
                    }
                    let content = f.content.clone().unwrap_or_default();
                    if let Some(o) = output_of(&content, f.raw_output.as_ref()) {
                        *output = Some(o);
                    }
                    let d = diffs_of(&content);
                    if !d.is_empty() {
                        *diffs = d;
                    }
                }
                let item = item.clone();
                if item.state != ItemState::Running {
                    self.tools.remove(&id);
                }
                step.events.push(AgentEvent::Item { item });
            }
            acp::SessionUpdate::AvailableCommandsUpdate(u) => {
                let commands = u
                    .available_commands
                    .into_iter()
                    .map(|c| SlashCommand { name: c.name, description: c.description, hint: c.input.and_then(|i| match i { acp::AvailableCommandInput::Unstructured(i) => Some(i.hint), _ => None }) })
                    .collect();
                step.events.push(AgentEvent::Commands { commands });
            }
            acp::SessionUpdate::ConfigOptionUpdate(u) => {
                if let Some(ev) = self.config_event(&u.config_options) {
                    step.events.push(ev);
                }
            }
            acp::SessionUpdate::Plan(plan) => {
                self.end_stream(step, now_ms);
                let text = plan
                    .entries
                    .iter()
                    .map(|e| format!("- [{}] {}", if matches!(e.status, acp::PlanEntryStatus::Completed) { "x" } else { " " }, e.content))
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    let item = Item { id: format!("plan-{}", self.turn.clone().unwrap_or_default()), turn: self.turn.clone(), at_ms: now_ms, state: ItemState::Done, kind: ItemKind::AgentMessage { text } };
                    step.events.push(AgentEvent::Item { item });
                }
            }
            _ => {}
        }
    }
}

fn stream_item(s: &Stream, turn: Option<String>, state: ItemState) -> Item {
    let text = s.text.trim().to_owned();
    let kind = match s.kind {
        StreamKind::Reply => ItemKind::AgentMessage { text },
        StreamKind::Thought => ItemKind::Thought { text },
        StreamKind::User => ItemKind::UserMessage { origin: origin(&text), text, attachments: Vec::new() },
    };
    Item { id: s.id.clone(), turn, at_ms: s.at_ms, state, kind }
}

fn kind_of(k: acp::ToolKind) -> ToolKind {
    match k {
        acp::ToolKind::Read => ToolKind::Read,
        acp::ToolKind::Edit => ToolKind::Edit,
        acp::ToolKind::Delete => ToolKind::Delete,
        acp::ToolKind::Move => ToolKind::Move,
        acp::ToolKind::Search => ToolKind::Search,
        acp::ToolKind::Execute => ToolKind::Execute,
        acp::ToolKind::Think => ToolKind::Think,
        acp::ToolKind::Fetch => ToolKind::Fetch,
        _ => ToolKind::Other,
    }
}

fn state_of(s: acp::ToolCallStatus) -> ItemState {
    match s {
        acp::ToolCallStatus::Completed => ItemState::Done,
        acp::ToolCallStatus::Failed => ItemState::Failed,
        _ => ItemState::Running,
    }
}

/// omp titles commands `$ cmd`.
fn unwrap_dollar(t: &str) -> String {
    t.strip_prefix("$ ").unwrap_or(t).to_owned()
}

/// A tool's output: its text content, else the text in its raw output.
fn output_of(content: &[acp::ToolCallContent], raw: Option<&Value>) -> Option<String> {
    let mut text: Vec<String> = content
        .iter()
        .filter_map(|c| match c {
            acp::ToolCallContent::Content(c) => match &c.content {
                acp::ContentBlock::Text(t) => Some(t.text.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    if text.is_empty()
        && let Some(r) = raw
    {
        text = r["content"].as_array().into_iter().flatten().filter_map(|c| c["text"].as_str().map(String::from)).collect();
    }
    let joined = text.join("\n");
    (!joined.trim().is_empty()).then(|| clip(joined.trim(), MAX_OUTPUT))
}

/// The before/after text of each edited file, as unified-diff lines.
fn diffs_of(content: &[acp::ToolCallContent]) -> Vec<FileDiff> {
    content
        .iter()
        .filter_map(|c| match c {
            acp::ToolCallContent::Diff(d) => {
                let old = d.old_text.clone().unwrap_or_default();
                let diff = similar::TextDiff::from_lines(&old, &d.new_text);
                let lines: Vec<String> = diff.unified_diff().context_radius(2).to_string().lines().map(String::from).collect();
                Some(FileDiff { path: d.path.to_string_lossy().to_string(), lines })
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(a: &mut Acp, step: &Step, result: Value) -> Step {
        let id = step.send.iter().rev().find_map(|m| m["id"].as_u64()).expect("a request");
        a.on_message(&json!({"jsonrpc": "2.0", "id": id, "result": result}), 0)
    }

    fn methods(step: &Step) -> Vec<&str> {
        step.send.iter().filter_map(|m| m["method"].as_str()).collect()
    }

    fn update(a: &mut Acp, u: Value, now: i64) -> Step {
        a.on_message(&json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "s1", "update": u}}), now)
    }

    #[test]
    fn a_session_then_prompts_one_at_a_time() {
        let mut a = Acp::new(Options { cwd: "/r".into(), session: None });
        let init = a.start();
        assert_eq!(methods(&init), ["initialize"]);
        assert!(a.command(Command::Prompt { text: "<lila task=\"T1\">\nGo\n</lila>".into(), attachments: vec![] }, 0).send.is_empty(), "waits for the session");
        let new = reply(&mut a, &init, json!({"protocolVersion": 1, "agentCapabilities": {"loadSession": true}}));
        assert_eq!(methods(&new), ["session/new"]);
        let open = reply(&mut a, &new, json!({"sessionId": "s1"}));
        assert_eq!(open.events[0], AgentEvent::Session { id: "s1".into() });
        assert_eq!(methods(&open), ["session/prompt"]);
        assert!(
            open.events.iter().any(|e| matches!(e, AgentEvent::Item { item } if matches!(&item.kind, ItemKind::UserMessage { origin: Origin::Lila { task: Some(t) }, .. } if t == "T1"))),
            "the prompt is the turn's first item: {:?}",
            open.events
        );
        assert!(a.command(Command::Prompt { text: "and docs".into(), attachments: vec![] }, 0).send.is_empty(), "one prompt at a time");
        // The reply streams in pieces: shown as it grows, then whole.
        update(&mut a, json!({"sessionUpdate": "agent_message_chunk", "messageId": "m1", "content": {"type": "text", "text": "Read"}}), 100);
        update(&mut a, json!({"sessionUpdate": "agent_message_chunk", "messageId": "m1", "content": {"type": "text", "text": "ing it."}}), 200);
        let shown = a.tick(100 + SHOW_EVERY_MS);
        assert!(matches!(&shown.events[0], AgentEvent::Item { item } if item.state == ItemState::Running));
        let call = update(&mut a, json!({"sessionUpdate": "tool_call", "toolCallId": "c1", "title": "$ cat README.md", "kind": "execute", "status": "pending", "rawInput": {"command": "cat README.md"}}), 300);
        assert!(matches!(&call.events[0], AgentEvent::Item { item } if item.id == "m1" && item.state == ItemState::Done && item.kind == ItemKind::AgentMessage { text: "Reading it.".into() }));
        assert!(matches!(&call.events[1], AgentEvent::Item { item } if matches!(&item.kind, ItemKind::ToolCall { tool: ToolKind::Execute, title, .. } if title == "cat README.md")));
        let done = update(&mut a, json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed", "rawOutput": {"content": [{"type": "text", "text": "# hi"}]}}), 400);
        assert!(matches!(&done.events[0], AgentEvent::Item { item } if item.state == ItemState::Done && matches!(&item.kind, ItemKind::ToolCall { output: Some(o), .. } if o == "# hi")));
        let id = open.send[0]["id"].as_u64().unwrap();
        let end = a.on_message(&json!({"jsonrpc": "2.0", "id": id, "result": {"stopReason": "end_turn", "usage": {"inputTokens": 10, "outputTokens": 2, "cachedReadTokens": 5}}}), 500);
        assert!(end.events.contains(&AgentEvent::Usage { input: 10, output: 2, cached: 5 }));
        assert_eq!(methods(&end), ["session/prompt"], "then what waited");
        assert_eq!(end.send[0]["params"]["prompt"][0]["text"], "and docs");
    }

    /// What omp answers to `initialize` and `session/new` (checked on omp 18.18.3), shortened.
    fn open_session(image: bool) -> (Acp, Step) {
        let mut a = Acp::new(Options { cwd: "/r".into(), session: None });
        let init = a.start();
        let new = reply(&mut a, &init, json!({"agentCapabilities": {"promptCapabilities": {"image": image, "embeddedContext": true}}}));
        let config = json!({"configOptions": [
            {"id": "mode", "name": "Mode", "category": "mode", "type": "select", "currentValue": "default", "options": [{"value": "default", "name": "Default"}]},
            {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "magpie/codex/gpt-6.1-sol",
             "options": [{"value": "magpie/codex/gpt-6.1-sol", "name": "GPT-6.1-Sol"}, {"value": "magpie/codex/gpt-6-luna", "name": "GPT-6-Luna"}]},
            {"id": "thinking", "name": "Thinking", "category": "thought_level", "type": "select", "currentValue": "high",
             "options": [{"value": "off", "name": "Off"}, {"value": "low", "name": "low"}, {"value": "high", "name": "high"}]}],
            "sessionId": "s1"});
        let open = reply(&mut a, &new, config);
        (a, open)
    }

    #[test]
    fn the_model_and_effort_are_what_the_agent_lists_and_are_set_by_its_option_ids() {
        let (mut a, open) = open_session(true);
        let Some(AgentEvent::Config { model, effort, models, efforts }) = open.events.iter().find(|e| matches!(e, AgentEvent::Config { .. })) else { panic!("{:?}", open.events) };
        assert_eq!((model.as_deref(), effort.as_deref()), (Some("magpie/codex/gpt-6.1-sol"), Some("high")));
        assert_eq!((models.len(), efforts.iter().map(|c| c.id.as_str()).collect::<Vec<_>>()), (2, vec!["off", "low", "high"]));
        let set = a.command(Command::SetModel { model: "magpie/codex/gpt-6-luna".into() }, 0);
        assert_eq!(set.send[0]["method"], "session/set_config_option");
        assert_eq!(set.send[0]["params"], json!({"sessionId": "s1", "configId": "model", "value": "magpie/codex/gpt-6-luna"}));
        assert_eq!(a.command(Command::SetEffort { effort: "low".into() }, 0).send[0]["params"]["configId"], "thinking");
        // Its answer holds the options as they stand now.
        let id = set.send[0]["id"].as_u64().unwrap();
        let done = a.on_message(&json!({"jsonrpc": "2.0", "id": id, "result": {"configOptions": [{"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "magpie/codex/gpt-6-luna", "options": [{"value": "magpie/codex/gpt-6-luna", "name": "GPT-6-Luna"}]}]}}), 0);
        assert!(matches!(&done.events[0], AgentEvent::Config { model: Some(m), .. } if m == "magpie/codex/gpt-6-luna"));
        // A refusal is a notice, not a change.
        let id = a.command(Command::SetModel { model: "nope".into() }, 0).send[0]["id"].as_u64().unwrap();
        let bad = a.on_message(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": "unknown model"}}), 0);
        assert_eq!(bad.events, [AgentEvent::Notice { text: "unknown model".into() }]);
    }

    #[test]
    fn declared_commands_are_passed_on_and_files_go_as_images_or_links() {
        let (mut a, _) = open_session(true);
        let cmds = update(&mut a, json!({"sessionUpdate": "available_commands_update", "availableCommands": [
            {"name": "compact", "description": "Compact the conversation", "input": {"hint": "[focus]"}}, {"name": "tools", "description": "Show available tools"}]}), 0);
        assert_eq!(
            cmds.events,
            [AgentEvent::Commands { commands: vec![
                SlashCommand { name: "compact".into(), description: "Compact the conversation".into(), hint: Some("[focus]".into()) },
                SlashCommand { name: "tools".into(), description: "Show available tools".into(), hint: None }] }]
        );
        let dir = std::env::temp_dir().join(format!("lila-acp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (png, md) = (dir.join("a.png"), dir.join("n.md"));
        std::fs::write(&png, b"\x89PNG").unwrap();
        std::fs::write(&md, "hi").unwrap();
        let files = vec![Attachment::file(png.to_str().unwrap()), Attachment::file(md.to_str().unwrap())];
        let sent = a.command(Command::Prompt { text: "look".into(), attachments: files.clone() }, 5);
        let prompt = &sent.send[0]["params"]["prompt"];
        assert_eq!(prompt[0], json!({"type": "text", "text": "look"}));
        assert_eq!((prompt[1]["type"].as_str(), prompt[1]["mimeType"].as_str(), prompt[1]["data"].as_str()), (Some("image"), Some("image/png"), Some("iVBORw==")));
        assert_eq!(prompt[2], json!({"type": "resource_link", "uri": format!("file://{}", md.display()), "name": "n.md"}));
        let said = sent.events.iter().find_map(|e| match e { AgentEvent::Item { item } => Some(item), _ => None }).unwrap();
        assert!(matches!(&said.kind, ItemKind::UserMessage { attachments, .. } if *attachments == files));
        // An agent without image input gets the image as a link.
        let (mut plain, _) = open_session(false);
        let sent = plain.command(Command::Prompt { text: "look".into(), attachments: files[..1].to_vec() }, 5);
        assert_eq!(sent.send[0]["params"]["prompt"][1]["type"], "resource_link");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn permissions_and_a_lost_session() {
        let mut a = Acp::new(Options { cwd: "/r".into(), session: Some("old".into()) });
        let init = a.start();
        let load = reply(&mut a, &init, json!({"agentCapabilities": {"loadSession": true}}));
        assert_eq!(methods(&load), ["session/load"]);
        let id = load.send[0]["id"].as_u64().unwrap();
        let fresh = a.on_message(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32002, "message": "not found"}}), 0);
        assert_eq!(methods(&fresh), ["session/new"]);
        let ask = a.on_message(
            &json!({"jsonrpc": "2.0", "id": 7, "method": "session/request_permission", "params": {"sessionId": "s1",
                "toolCall": {"toolCallId": "c1", "title": "echo hi > out.txt", "kind": "execute", "status": "pending", "rawInput": {"command": "echo hi > out.txt"}},
                "options": [{"optionId": "reject_once", "name": "Reject", "kind": "reject_once"}, {"optionId": "allow_once", "name": "Allow once", "kind": "allow_once"}]}}),
            0,
        );
        let AgentEvent::Permission { request } = &ask.events[0] else { panic!("{ask:?}") };
        assert_eq!((request.id.as_str(), request.title.as_str(), request.options[0].label.as_str()), ("acp-7", "echo hi > out.txt", "允许"));
        assert!(matches!(a.status(), HostStatus::Blocked { .. }));
        let ans = a.command(Command::Answer { permission: "acp-7".into(), option: "allow_once".into() }, 0);
        assert_eq!(ans.send[0], json!({"jsonrpc": "2.0", "id": 7, "result": {"outcome": {"outcome": "selected", "optionId": "allow_once"}}}));
        assert_eq!(a.on_message(&json!({"jsonrpc": "2.0", "id": 8, "method": "fs/read_text_file", "params": {}}), 0).send[0]["error"]["code"], -32601);
    }

    #[test]
    fn edits_become_unified_diffs() {
        let d: Vec<acp::ToolCallContent> = serde_json::from_value(json!([{"type": "diff", "path": "/r/a.py", "oldText": "a\nb\n", "newText": "a\nc\n"}])).unwrap();
        let diffs = diffs_of(&d);
        assert_eq!(diffs[0].path, "/r/a.py");
        assert!(diffs[0].lines.iter().any(|l| l == "-b") && diffs[0].lines.iter().any(|l| l == "+c"), "{:?}", diffs[0].lines);
    }
}
