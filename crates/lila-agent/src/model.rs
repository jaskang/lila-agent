//! The conversation model every runtime is translated into (architecture v3 §5).

use serde::{Deserialize, Serialize};

/// Who a message came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "snake_case")]
pub enum Origin {
    /// Typed by the person (in the window or the terminal).
    Person,
    /// Delivered by Lila: a task (its id) or a note, an answer, a reminder.
    Lila { task: Option<String> },
    /// What Claude Code wrote down when it compacted the context: the CLI's own, not a message.
    Summary,
    /// What a slash command printed in the CLI itself (`/compact`'s "Not enough messages"): a note, not a message.
    Local,
}

/// What kind of file the person put on a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Image,
    #[default]
    File,
    /// A skill the message calls (Codex takes it as an input item, not as text).
    Skill,
}

/// A file (or image, or skill) that goes with a message: an absolute path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub path: String,
    /// What to call it (a skill's name; else the file's name).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default)]
    pub kind: AttachmentKind,
}

impl Attachment {
    /// A file or image at `path`, told apart by its extension.
    pub fn file(path: &str) -> Attachment {
        let kind = if image_mime(path).is_some() { AttachmentKind::Image } else { AttachmentKind::File };
        Attachment { path: path.to_owned(), name: file_name(path), kind }
    }

    /// What it is called: its name, else its file's.
    pub fn label(&self) -> String {
        if self.name.is_empty() { file_name(&self.path) } else { self.name.clone() }
    }
}

fn file_name(path: &str) -> String {
    path.rsplit('/').find(|s| !s.is_empty()).unwrap_or(path).to_owned()
}

/// The media type of an image file, by its extension (the ones every runtime takes).
pub fn image_mime(path: &str) -> Option<&'static str> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// What kind of tool ran: the window picks its icon from this (ACP's kinds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Fetch,
    Think,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemState {
    Running,
    Done,
    Failed,
}

/// One file's change, as unified-diff lines (`@@ …` headers, then ` `, `-`, `+` lines).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: String,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ItemKind {
    UserMessage {
        text: String,
        #[serde(flatten)]
        origin: Origin,
        /// The files that went with it.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<Attachment>,
    },
    /// Markdown.
    AgentMessage { text: String },
    Thought { text: String },
    ToolCall {
        tool: ToolKind,
        /// The tool's own name (`Bash`, `Edit`, …).
        name: String,
        /// One line: the command, the file, the pattern.
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        diffs: Vec<FileDiff>,
    },
}

/// One entry of a conversation. Items are keyed by `id`: a later version of
/// the same id (a tool call that finished) replaces the earlier one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    /// The turn it belongs to: the person's or Lila's message that started it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    pub at_ms: i64,
    pub state: ItemState,
    #[serde(flatten)]
    pub kind: ItemKind,
}

/// A runtime asks the person before doing something (running a command, changing files).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionRequest {
    /// The runtime's own id for the request; the answer names it.
    pub id: String,
    pub tool: ToolKind,
    /// What it wants to do: the command, the files.
    pub title: String,
    /// Why, or where: the reason it gave, the folder.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// The answers it takes, first the one to suggest.
    pub options: Vec<PermOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermOption {
    pub id: String,
    pub label: String,
}

/// A host (`lila host <runtime>`) → lilad. One JSON line each.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AgentEvent {
    /// The conversation's id, once the runtime has one (a new thread, or the one resumed).
    Session { id: String },
    Item { item: Item },
    Permission { request: PermissionRequest },
    /// Answered (by the person, or it went away with its turn).
    PermissionResolved { id: String },
    TurnStarted { turn: String },
    TurnEnded {
        turn: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failed: Option<String>,
    },
    /// Tokens the session has used so far.
    Usage { input: u64, output: u64, cached: u64 },
    /// The account's usage window.
    Limits {
        used_percent: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resets_ms: Option<i64>,
    },
    /// Something the person should hear (not signed in, the runtime stopped…).
    Notice { text: String },
    /// The model and effort the session runs on now, and the ones it offers when the runtime lists them (ACP).
    Config {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        models: Vec<Choice>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        efforts: Vec<Choice>,
    },
    /// The slash commands the agent declares (ACP's `available_commands_update`).
    Commands { commands: Vec<SlashCommand> },
}

/// What a session runs on, as `AgentEvent::Config` events add up: a later event changes only what it carries.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub models: Vec<Choice>,
    pub efforts: Vec<Choice>,
}

impl Config {
    /// Folds in an event (others are not about the config).
    pub fn apply(&mut self, ev: &AgentEvent) {
        let AgentEvent::Config { model, effort, models, efforts } = ev else { return };
        if model.is_some() {
            self.model = model.clone();
        }
        if effort.is_some() {
            self.effort = effort.clone();
        }
        if !models.is_empty() {
            self.models = models.clone();
        }
        if !efforts.is_empty() {
            self.efforts = efforts.clone();
        }
    }

    /// The event that gives this config again.
    pub fn event(&self) -> AgentEvent {
        AgentEvent::Config { model: self.model.clone(), effort: self.effort.clone(), models: self.models.clone(), efforts: self.efforts.clone() }
    }
}

/// One thing a runtime offers to pick: an id to send back, a name to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub id: String,
    pub name: String,
}

/// A slash command an agent declares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlashCommand {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// What follows the name, if anything (`[focus]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// lilad → a host. One JSON line each.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    /// Words for the member: a task, a note, the person's message. While it works,
    /// they join the running turn where the runtime can (`Caps::steer`), else wait.
    Prompt {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<Attachment>,
    },
    Interrupt,
    Answer { permission: String, option: String },
    /// Run on this model from now on, in this session only.
    SetModel { model: String },
    /// Think at this effort from now on, in this session only.
    SetEffort { effort: String },
    /// Compact the context (Codex: `thread/compact/start`; the others take `/compact` as words).
    Compact,
}

/// What a host reports to hc (OSC 7501), and so lilad's dispatch sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostStatus {
    Idle,
    Working,
    /// A finished turn nobody has looked at yet.
    Done,
    Blocked { why: String },
    Error { why: String },
}

impl HostStatus {
    /// The OSC 7501 record: `ESC ]7501;state=…:app=…[:kind=…][:msg=<base64>] ESC \`.
    pub fn osc(&self, app: &str) -> String {
        let (state, kind, msg) = match self {
            HostStatus::Idle => ("idle", None, None),
            HostStatus::Working => ("working", None, None),
            HostStatus::Done => ("done", None, None),
            HostStatus::Blocked { why } => ("blocked", Some("permission"), Some(why.as_str())),
            HostStatus::Error { why } => ("error", None, Some(why.as_str())),
        };
        let mut body = format!("state={state}");
        if let Some(k) = kind {
            body.push_str(&format!(":kind={k}"));
        }
        body.push_str(&format!(":app={app}"));
        if let Some(m) = msg.map(|m| m.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|m| !m.is_empty()) {
            body.push_str(&format!(":msg={}", base64(clip(&m, 2000).as_bytes())));
        }
        format!("\x1b]7501;{body}\x1b\\")
    }
}

/// Standard base64 with padding (OSC 7501's `msg`).
pub fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// What a runtime can do; the window and lilad decide by these, not by names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Caps {
    /// Its terminal can be shown instead (Claude Code: the TUI keeps running).
    pub tui: bool,
    /// Lila draws a conversation for it.
    pub gui: bool,
    /// A message sent while it works reaches the running turn.
    pub steer: bool,
}

/// Who wrote a message: Lila's deliveries open with `<lila task="T3" …>`, `<lila note …>`, `<lila answer …>`.
pub fn origin(text: &str) -> Origin {
    if !text.starts_with("<lila") {
        return Origin::Person;
    }
    let head = text.lines().next().unwrap_or_default();
    let task = head.split("task=\"").nth(1).and_then(|r| r.split('"').next()).map(String::from);
    Origin::Lila { task }
}

/// Longest tool output kept on an item; the rest is in the CLI's own record.
pub const MAX_OUTPUT: usize = 8000;

/// `text` cut to `max` bytes on a character boundary, marked when cut.
pub fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n…", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_serialize_flat_for_the_window() {
        let item = Item {
            id: "u1".into(),
            turn: Some("p1".into()),
            at_ms: 5,
            state: ItemState::Done,
            kind: ItemKind::UserMessage { text: "hi".into(), origin: Origin::Lila { task: Some("T3".into()) }, attachments: Vec::new() },
        };
        let v = serde_json::to_value(&item).unwrap();
        assert_eq!(v, serde_json::json!({"id": "u1", "turn": "p1", "at_ms": 5, "state": "done", "kind": "user_message", "text": "hi", "from": "lila", "task": "T3"}));
        assert_eq!(serde_json::from_value::<Item>(v).unwrap(), item);
    }

    #[test]
    fn host_status_is_an_osc_7501_record() {
        assert_eq!(HostStatus::Idle.osc("lila-codex"), "\x1b]7501;state=idle:app=lila-codex\x1b\\");
        assert_eq!(HostStatus::Blocked { why: "waiting on Q7".into() }.osc("x"), "\x1b]7501;state=blocked:kind=permission:app=x:msg=d2FpdGluZyBvbiBRNw==\x1b\\");
        assert_eq!(base64(b"ab"), "YWI=");
        let ev: AgentEvent = serde_json::from_str(r#"{"event":"session","id":"t1"}"#).unwrap();
        assert_eq!(ev, AgentEvent::Session { id: "t1".into() });
        assert_eq!(serde_json::to_string(&Command::Prompt { text: "hi".into(), attachments: Vec::new() }).unwrap(), r#"{"command":"prompt","text":"hi"}"#);
    }

    #[test]
    fn a_later_config_changes_only_what_it_carries() {
        let mut c = Config::default();
        let ids = |v: &[(&str, &str)]| v.iter().map(|(i, n)| Choice { id: (*i).into(), name: (*n).into() }).collect::<Vec<_>>();
        c.apply(&AgentEvent::Config { model: Some("a".into()), effort: Some("high".into()), models: ids(&[("a", "A"), ("b", "B")]), efforts: vec![] });
        c.apply(&AgentEvent::Config { model: Some("b".into()), effort: None, models: vec![], efforts: vec![] });
        c.apply(&AgentEvent::Notice { text: "x".into() });
        assert_eq!((c.model.as_deref(), c.effort.as_deref(), c.models.len()), (Some("b"), Some("high"), 2));
        let mut again = Config::default();
        again.apply(&c.event());
        assert_eq!(again, c);
    }

    #[test]
    fn clip_keeps_characters_whole() {
        assert_eq!(clip("héllo", 2), "h\n…");
        assert_eq!(clip("ok", 5), "ok");
    }
}
