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
            kind: ItemKind::UserMessage { text: "hi".into(), origin: Origin::Lila { task: Some("T3".into()) } },
        };
        let v = serde_json::to_value(&item).unwrap();
        assert_eq!(v, serde_json::json!({"id": "u1", "turn": "p1", "at_ms": 5, "state": "done", "kind": "user_message", "text": "hi", "from": "lila", "task": "T3"}));
        assert_eq!(serde_json::from_value::<Item>(v).unwrap(), item);
    }

    #[test]
    fn clip_keeps_characters_whole() {
        assert_eq!(clip("héllo", 2), "h\n…");
        assert_eq!(clip("ok", 5), "ok");
    }
}
