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

/// Rounds two members have bounced a chain between them, counting a new task
/// `from → to` under `ancestors` (nearest parent first). Engineer → tester,
/// tester → engineer is one round; any other hand-off in between ends the streak.
pub fn rounds(from: &str, to: &str, ancestors: &[(String, String)]) -> u32 {
    let pair = |a: &str, b: &str| (a == from && b == to) || (a == to && b == from);
    let streak = 1 + ancestors.iter().take_while(|(a, b)| pair(a, b)).count() as u32;
    streak / 2
}

/// What the person asked for and what counts as done, set when work starts.
/// Every step of the chain below sees it as written, so a tester checks the
/// request itself and not the engineer's account of it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Brief {
    /// The person's own words, unchanged.
    pub quote: Option<String>,
    /// Checkable outcomes, agreed before the work.
    pub accept: Vec<String>,
    /// What not to do: the kind of rule a summary loses first.
    pub out_of_scope: Vec<String>,
}

impl Brief {
    pub fn is_empty(&self) -> bool {
        self.quote.as_deref().is_none_or(|q| q.trim().is_empty()) && self.accept.is_empty() && self.out_of_scope.is_empty()
    }
}

/// What a member reports with `done` besides its summary: the calls it made that
/// nobody asked it to make, and what it did not check.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    pub decisions: Vec<String>,
    pub unverified: Vec<String>,
    /// A flow's verdict step: passed or failed.
    pub passed: Option<bool>,
}

impl Outcome {
    pub fn is_empty(&self) -> bool {
        self.decisions.is_empty() && self.unverified.is_empty() && self.passed.is_none()
    }
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
    pub brief: Brief,
    pub outcome: Outcome,
    /// The flow run this task is a step of: its anchor (the run's first task) and step index.
    pub run: Option<(String, usize)>,
}

impl Task {
    /// The brief its chain works to, when this task starts a chain: its own, or
    /// for a task from the person, the task itself, since those are their words.
    pub fn chain_brief(&self) -> Brief {
        let mut b = self.brief.clone();
        if self.from == Actor::Human && b.quote.is_none() {
            // A brief's first line is its title and stays in the body too.
            let (title, body) = (self.title.trim(), self.body.trim());
            let words = if body.starts_with(title) { body.to_owned() } else { format!("{title}\n\n{body}") };
            b.quote = Some(words.trim().to_owned());
        }
        b
    }
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
    fn rounds_count_back_and_forth_between_the_same_pair() {
        let chain = |v: &[(&str, &str)]| v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect::<Vec<_>>();
        // owner → engineer, then engineer hands to the tester: no round yet.
        assert_eq!(rounds("engineer", "tester", &chain(&[("owner", "engineer")])), 0);
        // tester sends a fix back: one round.
        assert_eq!(rounds("tester", "engineer", &chain(&[("engineer", "tester"), ("owner", "engineer")])), 1);
        let four = chain(&[("engineer", "tester"), ("tester", "engineer"), ("engineer", "tester"), ("owner", "engineer")]);
        assert_eq!(rounds("tester", "engineer", &four), 2);
        // A different hand-off breaks the streak.
        assert_eq!(rounds("tester", "engineer", &chain(&[("engineer", "tester"), ("architect", "engineer"), ("tester", "engineer")])), 1);
    }

    #[test]
    fn a_task_from_the_person_is_their_words() {
        let mut t = Task {
            id: "T1".into(),
            project: "p".into(),
            title: "Add login".into(),
            body: "Add login\nwith email".into(),
            state: TaskState::Queued,
            from: Actor::Human,
            to: "pm".into(),
            parent: None,
            priority: 0,
            summary: None,
            block_reason: None,
            created_ms: 0,
            updated_ms: 0,
            brief: Brief::default(),
            outcome: Outcome::default(),
            run: None,
        };
        assert_eq!(t.chain_brief().quote.as_deref(), Some("Add login\nwith email"));
        t.body = "with email".into();
        assert_eq!(t.chain_brief().quote.as_deref(), Some("Add login\n\nwith email"));
        t.body.clear();
        assert_eq!(t.chain_brief().quote.as_deref(), Some("Add login"));
        t.from = Actor::Member("pm".into());
        assert!(t.chain_brief().is_empty(), "a member's task is not the person's words");
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
