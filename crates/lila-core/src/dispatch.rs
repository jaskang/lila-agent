//! Pure decisions the daemon makes on every tick: which queued task goes to
//! which free member, and who has gone quiet on an open task.

/// A member's terminal status as its program reports it (OSC 7501), plus what Lila knows.
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

    /// Can receive a note: queued input is safe while working (M0), but not into a dialog.
    pub fn accepts_notes(self) -> bool {
        matches!(self, AgentStatus::Idle | AgentStatus::Done | AgentStatus::Working)
    }
}

#[derive(Debug, Clone)]
pub struct MemberView {
    pub id: String,
    pub is_owner: bool,
    pub status: AgentStatus,
    /// Has a delivered, in-progress or blocked task.
    pub holds_task: bool,
    /// Has a delivered or in-progress task (counts toward `max_working`).
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub struct QueuedTask {
    pub id: String,
    pub to: String,
    pub priority: i64,
    pub seq: i64,
}

/// (task, member) pairs to deliver now, at most one per member, highest priority
/// first, without exceeding `max_working` busy non-owner members (0: no limit). The owner is
/// never limited: it is the one planning.
pub fn pick_deliveries(members: &[MemberView], queued: &[QueuedTask], max_working: u32) -> Vec<(String, String)> {
    let mut busy = members.iter().filter(|m| !m.is_owner && m.busy).count() as u32;
    let mut order: Vec<&QueuedTask> = queued.iter().collect();
    order.sort_by_key(|t| (t.priority, t.seq));
    let mut taken: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for t in order {
        let Some(m) = members.iter().find(|m| m.id == t.to) else { continue };
        if taken.contains(&m.id.as_str()) || m.holds_task || !m.status.is_free() {
            continue;
        }
        if !m.is_owner {
            if max_working > 0 && busy >= max_working {
                continue;
            }
            busy += 1;
        }
        taken.push(&m.id);
        out.push((t.id.clone(), m.id.clone()));
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StallAction {
    None,
    /// Remind the member once that the task needs `done` or `block`.
    Nudge,
    /// It stayed quiet after the reminder: tell the owner.
    Report,
}

/// For a member that holds a delivered or in-progress task and is free (idle/done)
/// since `free_since_ms`.
pub fn stall_check(now_ms: i64, free_since_ms: i64, nudged_ms: Option<i64>, reported: bool, stall_ms: i64) -> StallAction {
    match nudged_ms {
        None if now_ms - free_since_ms >= stall_ms => StallAction::Nudge,
        Some(n) if !reported && now_ms - free_since_ms.max(n) >= stall_ms => StallAction::Report,
        _ => StallAction::None,
    }
}

/// Which member of a pool (`Team::pool`) a new task goes to. `affinity`: the
/// member that last worked in this chain, whose session may still hold the work
/// (a fix the tester sends back goes to whoever wrote the code). Otherwise the
/// one with the fewest open tasks, a free one first, in pool order.
/// `load(member)` gives (open tasks, free right now).
pub fn pick_slot<'a>(slots: &'a [String], affinity: Option<&str>, load: impl Fn(&str) -> (usize, bool)) -> &'a str {
    if let Some(a) = affinity.and_then(|a| slots.iter().find(|s| *s == a)) {
        return a;
    }
    slots.iter().min_by_key(|s| {
        let (open, free) = load(s);
        (open, !free)
    })
    .map(String::as_str)
    .expect("a pool has members")
}

#[cfg(test)]
mod tests {
    use super::*;
    use AgentStatus::*;

    #[test]
    fn a_pool_task_goes_back_to_the_chain_or_to_the_least_loaded() {
        let slots = vec!["engineer".to_string(), "engineer-2".into(), "engineer-3".into()];
        let load = |m: &str| match m {
            "engineer" => (1, false),
            "engineer-2" => (0, false),
            _ => (0, true),
        };
        assert_eq!(pick_slot(&slots, Some("engineer"), load), "engineer", "the chain's member, though busy");
        assert_eq!(pick_slot(&slots, None, load), "engineer-3", "nothing open and free");
        assert_eq!(pick_slot(&slots, Some("tester"), load), "engineer-3", "affinity outside the pool is ignored");
        assert_eq!(pick_slot(&slots, None, |_| (0, false)), "engineer", "ties go in pool order");
    }

    fn m(id: &str, owner: bool, status: AgentStatus, holds: bool, busy: bool) -> MemberView {
        MemberView { id: id.into(), is_owner: owner, status, holds_task: holds, busy }
    }

    fn q(id: &str, to: &str, priority: i64, seq: i64) -> QueuedTask {
        QueuedTask { id: id.into(), to: to.into(), priority, seq }
    }

    #[test]
    fn delivers_to_free_members_only() {
        let members = [m("eng", false, Idle, false, false), m("test", false, Working, false, false), m("arch", false, Unknown, false, false)];
        let picks = pick_deliveries(&members, &[q("T1", "eng", 0, 1), q("T2", "test", 0, 2), q("T3", "arch", 0, 3)], 9);
        assert_eq!(picks, [("T1".to_string(), "eng".to_string())]);
    }

    #[test]
    fn one_task_per_member_by_priority_then_age() {
        let members = [m("eng", false, Done, false, false)];
        let picks = pick_deliveries(&members, &[q("T1", "eng", 5, 1), q("T2", "eng", 0, 3), q("T3", "eng", 0, 2)], 9);
        assert_eq!(picks, [("T3".to_string(), "eng".to_string())]);
    }

    #[test]
    fn holding_a_task_blocks_new_ones() {
        let members = [m("eng", false, Idle, true, false)];
        assert!(pick_deliveries(&members, &[q("T1", "eng", 0, 1)], 9).is_empty());
    }

    #[test]
    fn working_limit_spares_the_owner() {
        let members = [
            m("owner", true, Idle, false, false),
            m("eng", false, Idle, false, false),
            m("test", false, Idle, false, false),
            m("arch", false, Working, true, true),
        ];
        let queued = [q("T1", "eng", 0, 1), q("T2", "test", 0, 2), q("T3", "owner", 0, 3)];
        assert_eq!(pick_deliveries(&members, &queued, 1), [("T3".to_string(), "owner".to_string())]);
        let picks = pick_deliveries(&members, &queued, 2);
        assert_eq!(picks, [("T1".to_string(), "eng".to_string()), ("T3".to_string(), "owner".to_string())]);
    }

    #[test]
    fn stall_nudges_then_reports_once() {
        let min = 60_000;
        assert_eq!(stall_check(4 * min, 0, None, false, 5 * min), StallAction::None);
        assert_eq!(stall_check(5 * min, 0, None, false, 5 * min), StallAction::Nudge);
        // Nudged at 5 min; it worked and went quiet again at 6 min.
        assert_eq!(stall_check(10 * min, 6 * min, Some(5 * min), false, 5 * min), StallAction::None);
        assert_eq!(stall_check(11 * min, 6 * min, Some(5 * min), false, 5 * min), StallAction::Report);
        assert_eq!(stall_check(30 * min, 6 * min, Some(5 * min), true, 5 * min), StallAction::None);
    }

    #[test]
    fn status_words() {
        assert_eq!(AgentStatus::from_word(Some("blocked")), Blocked);
        assert_eq!(AgentStatus::from_word(None), Unknown);
        assert!(Done.is_free() && !Working.is_free());
        assert!(Working.accepts_notes() && !Blocked.accepts_notes());
    }
}
