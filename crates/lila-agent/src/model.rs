//! The conversation model omp is translated into (architecture v3 §5).

use serde::{Deserialize, Serialize};

/// Who a message came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "snake_case")]
pub enum Origin {
    /// Typed by the person.
    Person,
    /// Delivered by Lila: a task (its id) or a note, an answer, a reminder.
    Lila { task: Option<String> },
    /// Lila put the folder's files back to a checkpoint here (written by lilad, not by omp): a quiet mark, not a message.
    /// `undo` is the checkpoint of the state before, which a restore to it would undo this with.
    Restored {
        checkpoint: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        undo: Option<i64>,
    },
}

/// What kind of file the person put on a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Image,
    #[default]
    File,
}

/// A file (or image) that goes with a message: an absolute path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub path: String,
    /// What to call it (else the file's name).
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

    /// Whether the words already point at this file with `@` (a file lilad took from a mention, named as written in them): they show
    /// it, so it is not shown a second time as a card. A file the person attached is named by its own name, which a mention
    /// of the same file also is: its card stays only when the words do not say it.
    pub fn is_mentioned_in(&self, text: &str) -> bool {
        self.kind == AttachmentKind::File && !self.name.is_empty() && at_mentions(text).contains(&self.name.as_str())
    }
}

/// What the words point at with `@`: `@src/main.rs`, `@"a path with spaces.md"`, as written, without the punctuation that ends a sentence.
pub fn at_mentions(text: &str) -> Vec<&str> {
    let mut words: Vec<&str> = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find('@') {
        rest = &rest[at..];
        match rest.strip_prefix("@\"").and_then(|r| r.split_once('"')) {
            Some((quoted, after)) => {
                words.push(quoted);
                rest = after;
            }
            None => {
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                words.push(&rest[1..end]);
                rest = &rest[end..];
            }
        }
    }
    words.into_iter().map(|w| w.trim_end_matches([',', '.', ';', ':', ')', '?', '!'])).collect()
}

fn file_name(path: &str) -> String {
    path.rsplit('/').find(|s| !s.is_empty()).unwrap_or(path).to_owned()
}

/// The media type of an image file, by its extension.
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

/// What kind of tool ran: the window picks its icon from this, and the permission tiers read it (`Omp` sorts omp's tools by name).
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
        /// The tool's own name as omp gives it (`bash`, `edit`, `mcp__server__tool`).
        name: String,
        /// One line: the command, the file, the pattern.
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        diffs: Vec<FileDiff>,
    },
    /// Something went wrong that the person should see where it happened: a model request that failed, omp that stopped.
    Error { text: String },
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

/// omp asks the person before doing something (running a command, changing files).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionRequest {
    /// omp's own id for the request; the answer names it.
    pub id: String,
    pub tool: ToolKind,
    /// What it wants to do: the command, the files.
    pub title: String,
    /// Why, or where: the reason it gave, the folder.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// The answers it takes, first the one to suggest.
    pub options: Vec<PermOption>,
    /// The tool's own name as omp gives it (`bash`, `edit`, `mcp__server__tool`); empty for a request that names none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The shell command, when the tool is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// The files it changes or deletes, as the model named them (absolute, or relative to `cwd`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// The folder the command or tool runs in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

impl PermissionRequest {
    /// The option that lets it go ahead (`allow`) or refuses it, the one-time kind before the lasting one.
    pub fn pick(&self, allow: bool) -> Option<&PermOption> {
        let (once, always) = if allow { (PermKind::Allow, PermKind::AllowAlways) } else { (PermKind::Reject, PermKind::RejectAlways) };
        self.options.iter().find(|o| o.kind == once).or_else(|| self.options.iter().find(|o| o.kind == always))
    }

    /// A request with only what is always there; the rest is filled in with struct update syntax.
    pub fn new(id: impl Into<String>, tool: ToolKind, title: impl Into<String>, detail: impl Into<String>, options: Vec<PermOption>) -> Self {
        PermissionRequest { id: id.into(), tool, title: title.into(), detail: detail.into(), options, name: String::new(), command: None, paths: Vec::new(), cwd: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermOption {
    pub id: String,
    pub label: String,
    /// What choosing it means; lilad answers by the kind, not omp's ids.
    #[serde(default)]
    pub kind: PermKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermKind {
    /// Let this one go ahead.
    #[default]
    Allow,
    /// Let this and the like of it go ahead from now on.
    AllowAlways,
    Reject,
    RejectAlways,
}

impl PermKind {
    pub fn allows(self) -> bool {
        matches!(self, PermKind::Allow | PermKind::AllowAlways)
    }
}

/// What an agent asks the person (omp's `ask` tool, or anything that opens a dialog): not a permission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Question {
    /// omp's own id for the dialog; the reply names it.
    pub id: String,
    /// The question, which may run over several lines.
    pub title: String,
    /// More words under it (a confirmation's message).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    pub kind: QuestionKind,
    /// The choices of a `Choice`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// What a `Text` starts with (a placeholder or a draft).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prefill: String,
    /// A `Text` that is a secret: its words are not shown while typed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub secret: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    /// One of `options`.
    Choice,
    /// Yes or no.
    Confirm,
    /// Words of the person's own.
    Text,
}

/// The person's answer to a question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reply {
    Choice { value: String },
    Confirm { yes: bool },
    Text { value: String },
    /// No answer: the agent goes on without one.
    Cancel,
}

/// A host (`lila host omp`) → lilad. One JSON line each.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AgentEvent {
    /// The conversation's id, once omp has one (a new thread, or the one resumed).
    Session { id: String },
    Item { item: Item },
    Permission { request: PermissionRequest },
    /// Answered (by the person, or it went away with its turn).
    PermissionResolved { id: String },
    /// The agent asks the person something.
    Question { question: Question },
    /// Answered, or taken back by the agent (or it went away with its turn).
    QuestionResolved { id: String },
    TurnStarted { turn: String },
    TurnEnded {
        turn: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failed: Option<String>,
    },
    /// Tokens the session has used so far.
    Usage { input: u64, output: u64, cached: u64 },
    /// Something that happened outside the conversation (omp retries a request, a command it declares failed…).
    Notice { text: String },
    /// The model and effort the session runs on now, and the ones it offers.
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
    /// The slash commands omp declares.
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

/// One thing omp offers to pick: an id to send back, a name to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub id: String,
    pub name: String,
}

/// A slash command omp declares.
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
    /// Words for the agent: the person's message, with notes in front. omp takes them one turn at a time:
    /// words sent while it works wait for the turn to end.
    Prompt {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<Attachment>,
    },
    Interrupt,
    Answer { permission: String, option: String },
    /// The person's answer to a question.
    Reply { question: String, reply: Reply },
    /// Run on this model from now on, in this session only.
    SetModel { model: String },
    /// Think at this effort from now on, in this session only.
    SetEffort { effort: String },
}

/// What omp is doing, as the connection reports it to lilad.
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
    /// The state word (`idle`, `working`, `done`, `blocked`, `error`), what it is blocked on (`kind`), and the
    /// message (one line, cut).
    pub fn parts(&self) -> (&'static str, Option<&'static str>, Option<String>) {
        let (state, kind, msg) = match self {
            HostStatus::Idle => ("idle", None, None),
            HostStatus::Working => ("working", None, None),
            HostStatus::Done => ("done", None, None),
            HostStatus::Blocked { why } => ("blocked", Some("permission"), Some(why.as_str())),
            HostStatus::Error { why } => ("error", None, Some(why.as_str())),
        };
        let msg = msg.map(|m| m.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|m| !m.is_empty()).map(|m| clip(&m, 2000));
        (state, kind, msg)
    }
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

/// The person's words, when Lila's notes were queued in front of them and went out in the same message (`<lila note …>…</lila>`,
/// such as the one after a restore of the files): the notes are for the agent, the message is the person's. A text that is
/// only notes is left as it is.
pub fn without_notes(text: &str) -> &str {
    let mut rest = text.trim_start();
    while rest.starts_with("<lila note") {
        let Some(end) = rest.find("</lila>") else { return text };
        rest = rest[end + "</lila>".len()..].trim_start();
    }
    if rest.is_empty() { text } else { rest }
}

/// Longest tool output kept on an item.
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
    fn the_words_point_at_files_with_at() {
        assert_eq!(at_mentions("read @README.md, then @\"my notes.md\" and @src/a.rs."), ["README.md", "my notes.md", "src/a.rs"]);
        assert!(at_mentions("no mention here").is_empty());
        // A card is for a file the words do not already name.
        let mentioned = Attachment { name: "src/a.rs".into(), ..Attachment::file("/r/src/a.rs") };
        assert!(mentioned.is_mentioned_in("look at @src/a.rs") && !mentioned.is_mentioned_in("look at src/a.rs"));
        assert!(Attachment::file("/x/notes.md").is_mentioned_in("see @notes.md") && !Attachment::file("/x/notes.md").is_mentioned_in("see @other.md"));
        // A picture is never "in the words".
        assert!(!Attachment::file("/x/p.png").is_mentioned_in("@p.png"));
    }

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
    fn host_status_has_a_word_a_kind_and_a_one_line_message() {
        assert_eq!(HostStatus::Idle.parts(), ("idle", None, None));
        assert_eq!(HostStatus::Blocked { why: "waiting\non  Q7".into() }.parts(), ("blocked", Some("permission"), Some("waiting on Q7".into())));
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
    fn notes_in_front_of_the_persons_words_are_not_part_of_them() {
        let note = "<lila note about=\"restore\">\nFiles were put back.\n</lila>";
        assert_eq!(without_notes(&format!("{note}\n\nfix the test")), "fix the test");
        assert_eq!(origin(without_notes(&format!("{note}\n\nfix the test"))), Origin::Person);
        assert_eq!(without_notes(&format!("{note}\n\n{note}\n\nagain")), "again");
        // A note on its own stays Lila's, and so does anything else Lila delivers.
        assert_eq!(without_notes(note), note);
        assert_eq!(without_notes("<lila task=\"T1\">\ndo it\n</lila>"), "<lila task=\"T1\">\ndo it\n</lila>");
        assert_eq!(without_notes("<lila note\nbroken"), "<lila note\nbroken");
        assert_eq!(without_notes("plain"), "plain");
        let marker = Item { id: "restored-1".into(), turn: None, at_ms: 5, state: ItemState::Done, kind: ItemKind::UserMessage { text: "x".into(), origin: Origin::Restored { checkpoint: 3, undo: Some(7) }, attachments: Vec::new() } };
        assert_eq!(serde_json::to_value(&marker).unwrap(), serde_json::json!({"id": "restored-1", "at_ms": 5, "state": "done", "kind": "user_message", "text": "x", "from": "restored", "checkpoint": 3, "undo": 7}));
    }

    #[test]
    fn clip_keeps_characters_whole() {
        assert_eq!(clip("héllo", 2), "h\n…");
        assert_eq!(clip("ok", 5), "ok");
    }
}
