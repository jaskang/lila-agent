//! Who hears about a finished task. Every note typed into a member's terminal
//! starts a model turn, so a done note goes only to the member whose own work
//! waits on the result, and only once the work it set off has finished.

use crate::task::{Actor, Task};

#[derive(Debug, Clone, PartialEq)]
pub enum DoneRoute {
    /// The person assigned it: it goes to the inbox.
    Inbox,
    /// Work it handed on is still open. Whoever waits hears when that work finishes.
    Hold,
    /// Type a note to `member`.
    Tell {
        member: String,
        /// The open task of `member`'s this work was for (None: nobody waits on it).
        waiting: Option<String>,
        /// Tasks finished between the done one and `waiting` (nearest first): nobody
        /// was told about them while the chain went on.
        path: Vec<String>,
    },
    /// Nobody to tell: the member that finished it is the one that would hear.
    Nobody,
}

/// `id` has just become done. `tasks` is every task of the project.
pub fn route_done(tasks: &[Task], id: &str, owner: &str) -> DoneRoute {
    let Some(task) = find(tasks, id) else { return DoneRoute::Nobody };
    if task.from == Actor::Human {
        return DoneRoute::Inbox;
    }
    if !open_under(tasks, id).is_empty() {
        return DoneRoute::Hold;
    }
    let mut path = Vec::new();
    for a in ancestors(tasks, id) {
        if a.state.is_open() && a.to != task.to {
            return DoneRoute::Tell { member: a.to.clone(), waiting: Some(a.id.clone()), path };
        }
        if a.state == crate::TaskState::Done {
            path.push(a.id.clone());
        }
    }
    // Nobody's open task waits on it: the PM hears that the chain ended.
    let member = if task.to != owner {
        owner.to_owned()
    } else {
        match &task.from {
            Actor::Member(m) => m.clone(),
            Actor::Human => return DoneRoute::Inbox,
        }
    };
    if member == task.to { DoneRoute::Nobody } else { DoneRoute::Tell { member, waiting: None, path } }
}

/// Open tasks anywhere below `id` (children, their children, ...), oldest first.
pub fn open_under<'a>(tasks: &'a [Task], id: &str) -> Vec<&'a Task> {
    let mut out: Vec<&Task> = Vec::new();
    let mut frontier = vec![id.to_owned()];
    while let Some(p) = frontier.pop() {
        for t in tasks.iter().filter(|t| t.parent.as_deref() == Some(p.as_str())) {
            if out.len() + frontier.len() > tasks.len() {
                return out; // a parent loop; the store refuses to build one
            }
            frontier.push(t.id.clone());
            if t.state.is_open() {
                out.push(t);
            }
        }
    }
    out.sort_by_key(|t| t.created_ms);
    out
}

/// Parent, grandparent, ... of `id` (nearest first).
pub fn ancestors<'a>(tasks: &'a [Task], id: &str) -> Vec<&'a Task> {
    let mut out: Vec<&Task> = Vec::new();
    let mut cur = find(tasks, id).and_then(|t| t.parent.as_deref());
    while let Some(p) = cur {
        let Some(t) = find(tasks, p) else { break };
        if out.iter().any(|o| o.id == t.id) {
            break;
        }
        out.push(t);
        cur = t.parent.as_deref();
    }
    out
}

pub fn find<'a>(tasks: &'a [Task], id: &str) -> Option<&'a Task> {
    tasks.iter().find(|t| t.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TaskState::{self, *};

    fn t(id: &str, from: &str, to: &str, parent: Option<&str>, state: TaskState) -> Task {
        let n: i64 = id[1..].parse().unwrap();
        Task {
            id: id.into(),
            project: "p".into(),
            title: format!("title {id}"),
            body: String::new(),
            state,
            from: Actor::parse(from),
            to: to.into(),
            parent: parent.map(Into::into),
            priority: 0,
            summary: None,
            block_reason: None,
            created_ms: n,
            updated_ms: n,
        }
    }

    fn tell(member: &str, waiting: Option<&str>, path: &[&str]) -> DoneRoute {
        DoneRoute::Tell { member: member.into(), waiting: waiting.map(Into::into), path: path.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn handed_on_work_holds_the_note_until_it_finishes() {
        // human → pm T1; pm → engineer T2; engineer → tester T3.
        let mut tasks = vec![
            t("T1", "human", "pm", None, InProgress),
            t("T2", "pm", "engineer", Some("T1"), Done),
            t("T3", "engineer", "tester", Some("T2"), Queued),
        ];
        assert_eq!(route_done(&tasks, "T2", "pm"), DoneRoute::Hold, "the PM can't act until the tester reports");
        tasks[2].state = Done;
        // The engineer's part is over: it is not woken. The PM hears, with the engineer's step.
        assert_eq!(route_done(&tasks, "T3", "pm"), tell("pm", Some("T1"), &["T2"]));
    }

    #[test]
    fn the_nearest_open_task_hears() {
        // pm → architect T2 (under T1); architect → engineer T3 under its own open T2.
        let tasks = vec![
            t("T1", "human", "pm", None, InProgress),
            t("T2", "pm", "architect", Some("T1"), InProgress),
            t("T3", "architect", "engineer", Some("T2"), Done),
        ];
        assert_eq!(route_done(&tasks, "T3", "pm"), tell("architect", Some("T2"), &[]));
    }

    #[test]
    fn a_failure_sent_back_as_a_task_wakes_nobody() {
        let tasks = vec![
            t("T1", "pm", "engineer", None, Done),
            t("T2", "engineer", "tester", Some("T1"), Done),
            t("T3", "tester", "engineer", Some("T2"), Queued),
        ];
        assert_eq!(route_done(&tasks, "T2", "pm"), DoneRoute::Hold, "the fix task itself tells the engineer");
    }

    #[test]
    fn without_a_waiting_task_the_pm_hears() {
        let tasks = vec![t("T1", "pm", "engineer", None, Done), t("T2", "engineer", "tester", None, Done), t("T3", "engineer", "pm", None, Done)];
        assert_eq!(route_done(&tasks, "T1", "pm"), tell("pm", None, &[]));
        assert_eq!(route_done(&tasks, "T2", "pm"), tell("pm", None, &[]));
        assert_eq!(route_done(&tasks, "T3", "pm"), tell("engineer", None, &[]), "work the PM did for a member goes back to it");
    }

    #[test]
    fn human_tasks_go_to_the_inbox() {
        let tasks = vec![t("T1", "human", "pm", None, Done)];
        assert_eq!(route_done(&tasks, "T1", "pm"), DoneRoute::Inbox);
    }

    #[test]
    fn open_under_walks_the_whole_subtree() {
        let tasks = vec![
            t("T1", "human", "pm", None, InProgress),
            t("T2", "pm", "engineer", Some("T1"), Done),
            t("T3", "engineer", "tester", Some("T2"), InProgress),
            t("T4", "pm", "designer", Some("T1"), Cancelled),
            t("T5", "pm", "designer", Some("T1"), Queued),
        ];
        let ids: Vec<&str> = open_under(&tasks, "T1").iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["T3", "T5"]);
        assert!(open_under(&tasks, "T3").is_empty());
        assert_eq!(ancestors(&tasks, "T3").iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["T2", "T1"]);
    }
}
