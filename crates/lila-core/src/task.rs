//! Tasks and their state machine.
//!
//! ```text
//! queued ─▶ delivered ─▶ in_progress ─▶ done
//!              │  ▲          │  ▲
//!              │  └─ requeue │  │ start
//!              │             ▼  │
//!              └──────────▶ blocked
//! any open state ─▶ cancelled
//! ```

use std::fmt;

/// Who did something: the person, or a member by id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Actor {
    Human,
    Member(String),
}

impl Actor {
    pub fn parse(s: &str) -> Actor {
        if s == "human" { Actor::Human } else { Actor::Member(s.to_owned()) }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Actor::Human => "human",
            Actor::Member(m) => m,
        }
    }
}

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskState {
    /// Waiting for its member to be free.
    Queued,
    /// Typed into the member's terminal; it has not said `lila task start` yet.
    Delivered,
    InProgress,
    Blocked,
    Done,
    Cancelled,
}

impl TaskState {
    pub const ALL: [TaskState; 6] =
        [TaskState::Queued, TaskState::Delivered, TaskState::InProgress, TaskState::Blocked, TaskState::Done, TaskState::Cancelled];

    pub fn as_str(self) -> &'static str {
        match self {
            TaskState::Queued => "queued",
            TaskState::Delivered => "delivered",
            TaskState::InProgress => "in_progress",
            TaskState::Blocked => "blocked",
            TaskState::Done => "done",
            TaskState::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<TaskState> {
        TaskState::ALL.into_iter().find(|t| t.as_str() == s)
    }

    pub fn is_open(self) -> bool {
        !matches!(self, TaskState::Done | TaskState::Cancelled)
    }

    /// Holds its member: no other task is delivered to it meanwhile.
    pub fn is_active(self) -> bool {
        matches!(self, TaskState::Delivered | TaskState::InProgress | TaskState::Blocked)
    }
}

impl fmt::Display for TaskState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    Deliver,
    /// Delivery could not be confirmed, or its member was lost: back in the queue.
    Requeue,
    Start,
    Block,
    Done,
    Cancel,
}

impl Transition {
    pub fn as_str(self) -> &'static str {
        match self {
            Transition::Deliver => "delivered",
            Transition::Requeue => "requeued",
            Transition::Start => "started",
            Transition::Block => "blocked",
            Transition::Done => "done",
            Transition::Cancel => "cancelled",
        }
    }
}

/// The state after `t`, or why it is not allowed.
pub fn apply(state: TaskState, t: Transition) -> Result<TaskState, String> {
    use TaskState::*;
    use Transition as T;
    let next = match (state, t) {
        (Queued, T::Deliver) => Delivered,
        (Delivered | InProgress, T::Requeue) => Queued,
        (Delivered | InProgress | Blocked, T::Start) => InProgress,
        (Delivered | InProgress, T::Block) => Blocked,
        (Delivered | InProgress | Blocked, T::Done) => Done,
        (Queued | Delivered | InProgress | Blocked, T::Cancel) => Cancelled,
        (s, t) => {
            let hint = match (s, t) {
                (Queued, T::Start | T::Done | T::Block) => " (it has not been delivered to its member yet)",
                (Done | Cancelled, _) => " (it is closed)",
                (Blocked, T::Block) => " (it is already blocked)",
                _ => "",
            };
            return Err(format!("cannot mark a {s} task {}{hint}", t.as_str()));
        }
    };
    Ok(next)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    /// `T12`, numbered per project.
    pub id: String,
    pub project: String,
    pub title: String,
    pub body: String,
    pub state: TaskState,
    pub from: Actor,
    pub to: String,
    pub parent: Option<String>,
    /// Lower runs first.
    pub priority: i64,
    pub summary: Option<String>,
    pub block_reason: Option<String>,
    pub created_ms: i64,
    pub updated_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use TaskState::*;
    use Transition as T;

    #[test]
    fn happy_path() {
        let s = apply(Queued, T::Deliver).unwrap();
        let s = apply(s, T::Start).unwrap();
        let s = apply(s, T::Block).unwrap();
        let s = apply(s, T::Start).unwrap();
        assert_eq!(apply(s, T::Done).unwrap(), Done);
    }

    #[test]
    fn done_without_start_is_fine() {
        assert_eq!(apply(Delivered, T::Done).unwrap(), Done);
        assert_eq!(apply(Blocked, T::Done).unwrap(), Done);
    }

    #[test]
    fn refuses_with_reasons() {
        assert!(apply(Queued, T::Start).unwrap_err().contains("not been delivered"));
        assert!(apply(Done, T::Start).unwrap_err().contains("closed"));
        assert!(apply(Blocked, T::Block).unwrap_err().contains("already blocked"));
        assert!(apply(Cancelled, T::Cancel).is_err());
        assert!(apply(Blocked, T::Requeue).is_err());
    }

    #[test]
    fn names_round_trip() {
        for s in TaskState::ALL {
            assert_eq!(TaskState::parse(s.as_str()), Some(s));
        }
        assert_eq!(Actor::parse("human"), Actor::Human);
        assert_eq!(Actor::parse("tester").to_string(), "tester");
    }
}
