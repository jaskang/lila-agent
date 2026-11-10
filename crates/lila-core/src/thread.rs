//! A thread: one conversation of the person with an agent, in a project's folder.
//! The state it can be in, the permission tier it runs under, and what the
//! terminal's status words mean for it.

use crate::config::Runtime;
use serde::{Deserialize, Serialize};

/// How much the agent may do without asking (docs/product-v4.md §6). Per thread, switchable any time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// No changes to files, no commands with side effects.
    ReadOnly,
    /// Every change and every command is asked first.
    Confirm,
    /// Nothing is asked, except what leaves the workspace or the machine (`policy`).
    #[default]
    Full,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::ReadOnly => "read_only",
            Tier::Confirm => "confirm",
            Tier::Full => "full",
        }
    }

    pub fn parse(s: &str) -> Option<Tier> {
        match s.trim() {
            "read_only" | "readonly" | "read-only" => Some(Tier::ReadOnly),
            "confirm" => Some(Tier::Confirm),
            "full" | "auto" => Some(Tier::Full),
            _ => None,
        }
    }
}

/// What the window shows a thread as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadState {
    /// The engine was started and has not reported a status yet.
    Starting,
    /// Working on a turn.
    Running,
    /// Waits for the person: a permission card, or a dialog in the terminal.
    Waiting,
    /// At the prompt; takes a message.
    Idle,
    /// Its engine process is gone. Can be resumed.
    Ended,
}

impl ThreadState {
    pub fn as_str(self) -> &'static str {
        match self {
            ThreadState::Starting => "starting",
            ThreadState::Running => "running",
            ThreadState::Waiting => "waiting",
            ThreadState::Idle => "idle",
            ThreadState::Ended => "ended",
        }
    }
}

/// A thread's terminal status as its program reports it (OSC 7501), plus what Lila knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    /// Not started, or started but has reported nothing yet.
    Unknown,
    Idle,
    Working,
    /// Waiting for a person in its terminal (permission, question, login).
    Blocked,
    /// Finished a turn while nobody looked; as free as idle.
    Done,
    Error,
    /// Its terminal ended or was lost.
    Gone,
}

impl AgentStatus {
    pub fn from_word(word: Option<&str>) -> AgentStatus {
        match word {
            Some("idle") => AgentStatus::Idle,
            Some("working") => AgentStatus::Working,
            Some("blocked") => AgentStatus::Blocked,
            Some("done") => AgentStatus::Done,
            Some("error") => AgentStatus::Error,
            _ => AgentStatus::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AgentStatus::Unknown => "unknown",
            AgentStatus::Idle => "idle",
            AgentStatus::Working => "working",
            AgentStatus::Blocked => "blocked",
            AgentStatus::Done => "done",
            AgentStatus::Error => "error",
            AgentStatus::Gone => "gone",
        }
    }

    /// Can take typed input that starts a new turn right now.
    pub fn is_free(self) -> bool {
        matches!(self, AgentStatus::Idle | AgentStatus::Done)
    }

    /// Can receive a message: queued input is safe while working, but not into a dialog.
    pub fn accepts_input(self) -> bool {
        matches!(self, AgentStatus::Idle | AgentStatus::Done | AgentStatus::Working)
    }

    /// The thread's state for this status. `waiting_card`: a permission card is open for it.
    pub fn thread_state(self, waiting_card: bool) -> ThreadState {
        match self {
            AgentStatus::Gone => ThreadState::Ended,
            _ if waiting_card => ThreadState::Waiting,
            AgentStatus::Unknown => ThreadState::Starting,
            AgentStatus::Idle | AgentStatus::Done => ThreadState::Idle,
            AgentStatus::Working => ThreadState::Running,
            AgentStatus::Blocked | AgentStatus::Error => ThreadState::Waiting,
        }
    }
}

/// A thread as the database holds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub project: String,
    pub title: String,
    pub engine: Runtime,
    pub model: String,
    pub effort: Option<String>,
    pub tier: Tier,
    /// The folder the engine runs in (the project's folder, or a worktree later).
    pub cwd: String,
    /// The engine's own id for the conversation, once known: what resumes it.
    pub session: Option<String>,
    /// The hc terminal it runs in now; none once it ended.
    pub terminal: Option<String>,
    pub created_ms: i64,
    /// The latest message or reply.
    pub active_ms: i64,
    pub started_ms: Option<i64>,
    pub ended_ms: Option<i64>,
    pub archived: bool,
}

/// A thread's id: `t` and eight hex digits (terminal names, labels, file names).
pub fn new_thread_id(random: u32) -> String {
    format!("t{random:08x}")
}

pub fn valid_thread_id(id: &str) -> bool {
    id.len() == 9 && id.starts_with('t') && id[1..].bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_become_the_four_states_and_starting() {
        use AgentStatus::*;
        assert_eq!(Unknown.thread_state(false), ThreadState::Starting);
        assert_eq!(Working.thread_state(false), ThreadState::Running);
        assert_eq!(Done.thread_state(false), ThreadState::Idle);
        assert_eq!(Blocked.thread_state(false), ThreadState::Waiting);
        assert_eq!(Working.thread_state(true), ThreadState::Waiting, "a card is open: it waits for the person");
        assert_eq!(Gone.thread_state(true), ThreadState::Ended);
    }

    #[test]
    fn tiers_and_ids_round_trip() {
        for t in [Tier::ReadOnly, Tier::Confirm, Tier::Full] {
            assert_eq!(Tier::parse(t.as_str()), Some(t));
        }
        assert_eq!(Tier::default(), Tier::Full);
        assert!(valid_thread_id(&new_thread_id(0xdead_beef)) && !valid_thread_id("T1") && !valid_thread_id("tDEADBEEF"));
    }
}
