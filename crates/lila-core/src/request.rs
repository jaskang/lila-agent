//! Requests: what the person follows. A request is a chain, named by its root
//! task: the person's ask (or the PM's) and everything done under it. Its group
//! says whose move it is next, from facts lilad keeps (tasks, flow runs, the
//! inbox, commits under way), never from what a member says about itself.

use crate::task::{Task, TaskState};
use serde::Serialize;
use std::collections::BTreeMap;

/// Whose move it is. The order is the window's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    /// Something waits for the person: a review, a question, a stuck terminal.
    NeedsYou,
    /// A member works on it, or Lila runs its check or makes its commit.
    Working,
    /// Waits for someone outside the team: the reporter's reply to a comment.
    Waiting,
    /// Its work waits to start: a worktree, the review backlog, a member, the quota.
    Queued,
    Delivered,
    Dropped,
}

impl Group {
    pub fn as_str(self) -> &'static str {
        match self {
            Group::NeedsYou => "needs_you",
            Group::Working => "working",
            Group::Waiting => "waiting",
            Group::Queued => "queued",
            Group::Delivered => "delivered",
            Group::Dropped => "dropped",
        }
    }
}

/// Why a queued task does not start yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Hold {
    /// Every worktree is taken.
    Worktree,
    /// Without worktrees: another chain has the project folder.
    Folder,
    /// Results wait for the person's review (`max_unreviewed`).
    Review,
    /// Its member's CLI is out of quota.
    Quota,
    /// Its member is busy with something else.
    Member,
    /// Its member's CLI is starting (in the chain's worktree, say) and has not said it is ready.
    Starting,
    /// Its member is not running (the team is stopped).
    Stopped,
}

/// An open inbox item, with the task it is about (for an item about a member,
/// lilad gives the member's current task).
#[derive(Debug, Clone, PartialEq)]
pub struct InboxFact {
    pub id: String,
    pub kind: String,
    pub task: Option<String>,
    pub created_ms: i64,
}

/// Where a request came from, when not from the person: an issue, how the PM
/// sorted it, the pull request it ended in.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct External {
    pub source: String,
    #[serde(rename = "ref")]
    pub rf: String,
    pub url: String,
    pub author: String,
    pub triage: Option<String>,
    pub pr: Option<String>,
    /// When a comment last went to the reporter.
    pub asked_ms: Option<i64>,
}

/// An open flow run.
#[derive(Debug, Clone, PartialEq)]
pub struct RunFact {
    pub anchor: String,
    pub checking: bool,
    pub updated_ms: i64,
}

pub struct Facts<'a> {
    pub tasks: &'a [Task],
    pub owner: &'a str,
    pub inbox: &'a [InboxFact],
    pub runs: &'a [RunFact],
    /// Chains whose commit is under way.
    pub committing: &'a [String],
    /// Queued tasks that wait, and why.
    pub held: &'a BTreeMap<String, Hold>,
    /// Chains in a worktree, and its number.
    pub slots: &'a BTreeMap<String, usize>,
    /// Requests that came from a source, by root task.
    pub external: &'a BTreeMap<String, External>,
}

/// What is happening to a request now.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Doing {
    /// The person has to look: the most pressing inbox item's kind.
    You { inbox: String },
    /// A member has a task in it (`state`: delivered, in_progress, blocked).
    Member { member: String, task: String, title: String, state: String },
    Check,
    Commit,
    /// Its work waits to start.
    Wait { hold: Hold },
    /// The reporter was asked something and nothing has happened since.
    Reply,
    Done,
    Dropped,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Request {
    pub id: String,
    pub title: String,
    /// Who asked: `human`, or the member (the PM) that started it.
    pub from: String,
    pub group: Group,
    pub doing: Doing,
    /// Since when it is in its group (for the window: "12 分钟").
    pub since_ms: i64,
    pub created_ms: i64,
    /// Its last change.
    pub updated_ms: i64,
    /// Open inbox items about it, most pressing first.
    pub inbox: Vec<String>,
    /// Decisions members took on the person's behalf, and points nobody checked: the review's core.
    pub decisions: usize,
    pub unverified: usize,
    pub tasks: usize,
    pub slot: Option<usize>,
    pub external: Option<External>,
}

/// Inbox kinds that ask the person to act, most pressing first.
const NEEDS: [&str; 10] = ["review", "triage", "question", "draft", "merge", "budget", "terminal", "startup", "stalled", "runaway"];

fn root_of<'a>(tasks: &'a [Task], t: &'a Task) -> &'a Task {
    let mut cur = t;
    // A broken parent link ends the walk where it is; chains are short.
    for _ in 0..64 {
        match cur.parent.as_deref().and_then(|p| tasks.iter().find(|x| x.id == p)) {
            Some(p) => cur = p,
            None => break,
        }
    }
    cur
}

/// Every request, in the window's order: by group, then the longest waiting
/// first (finished ones: the latest first).
pub fn requests(f: &Facts) -> Vec<Request> {
    let mut chains: BTreeMap<&str, Vec<&Task>> = BTreeMap::new();
    for t in f.tasks {
        chains.entry(root_of(f.tasks, t).id.as_str()).or_default().push(t);
    }
    let mut out: Vec<Request> = chains
        .into_iter()
        .filter_map(|(root, tasks)| {
            let r = tasks.iter().find(|t| t.id == root)?;
            Some(one(f, r, &tasks))
        })
        .collect();
    out.sort_by(|a, b| {
        a.group.cmp(&b.group).then_with(|| match a.group {
            Group::Delivered | Group::Dropped => b.updated_ms.cmp(&a.updated_ms),
            _ => a.since_ms.cmp(&b.since_ms),
        })
    });
    out
}

/// When the reporter was asked, if the request still waits for the answer: only the
/// PM's own task is open, and nothing started since the comment went out.
pub fn asked(f: &Facts, root: &Task, tasks: &[&Task]) -> Option<i64> {
    let at = f.external.get(&root.id)?.asked_ms?;
    let open = |t: &&&Task| !matches!(t.state, TaskState::Done | TaskState::Cancelled);
    let quiet = tasks.iter().filter(open).all(|t| t.id == root.id) && tasks.iter().all(|t| t.id == root.id || t.created_ms < at);
    (open(&&root) && root.state != TaskState::Queued && quiet && !f.committing.contains(&root.id) && !f.runs.iter().any(|r| r.anchor == root.id)).then_some(at)
}

fn one(f: &Facts, root: &Task, tasks: &[&Task]) -> Request {
    let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
    let mut inbox: Vec<&InboxFact> = f.inbox.iter().filter(|i| NEEDS.contains(&i.kind.as_str()) && i.task.as_deref().is_some_and(|t| ids.contains(&t))).collect();
    let rank = |k: &str| NEEDS.iter().position(|n| *n == k).unwrap_or(NEEDS.len());
    inbox.sort_by_key(|i| (rank(&i.kind), i.created_ms));
    let updated = tasks.iter().map(|t| t.updated_ms).max().unwrap_or(root.updated_ms);
    let active = |t: &&&Task| matches!(t.state, TaskState::Delivered | TaskState::InProgress | TaskState::Blocked);
    // A member's work over the PM's own task, which stays open while the team works under it.
    let working = tasks.iter().filter(active).filter(|t| t.to != f.owner).max_by_key(|t| t.updated_ms).or_else(|| tasks.iter().filter(active).max_by_key(|t| t.updated_ms));
    let run = f.runs.iter().filter(|r| ids.contains(&r.anchor.as_str())).find(|r| r.checking);
    let queued: Vec<&&Task> = tasks.iter().filter(|t| t.state == TaskState::Queued).collect();
    let (group, doing, since) = if let Some(first) = inbox.first() {
        (Group::NeedsYou, Doing::You { inbox: first.kind.clone() }, inbox.iter().map(|i| i.created_ms).min().unwrap_or(first.created_ms))
    } else if let Some(at) = asked(f, root, tasks) {
        (Group::Waiting, Doing::Reply, at)
    } else if let Some(t) = working {
        let doing = Doing::Member { member: t.to.clone(), task: t.id.clone(), title: t.title.clone(), state: t.state.as_str().to_owned() };
        (Group::Working, doing, t.updated_ms)
    } else if let Some(r) = run {
        (Group::Working, Doing::Check, r.updated_ms)
    } else if f.committing.iter().any(|c| c == &root.id) {
        (Group::Working, Doing::Commit, updated)
    } else if let Some(q) = queued.iter().min_by_key(|t| t.created_ms) {
        let hold = queued.iter().find_map(|t| f.held.get(&t.id).copied()).unwrap_or(Hold::Member);
        (Group::Queued, Doing::Wait { hold }, q.created_ms)
    } else if root.state == TaskState::Cancelled {
        (Group::Dropped, Doing::Dropped, updated)
    } else {
        (Group::Delivered, Doing::Done, updated)
    };
    Request {
        id: root.id.clone(),
        title: root.title.clone(),
        from: root.from.as_str().to_owned(),
        group,
        doing,
        since_ms: since,
        created_ms: root.created_ms,
        updated_ms: updated,
        inbox: inbox.iter().map(|i| i.id.clone()).collect(),
        decisions: tasks.iter().map(|t| t.outcome.decisions.len()).sum(),
        unverified: tasks.iter().map(|t| t.outcome.unverified.len()).sum(),
        tasks: tasks.len(),
        slot: f.slots.get(&root.id).copied(),
        external: f.external.get(&root.id).cloned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{Actor, Outcome};

    fn task(id: &str, parent: Option<&str>, from: &str, to: &str, state: TaskState, at: i64) -> Task {
        Task {
            id: id.into(),
            project: "shop".into(),
            title: format!("title {id}"),
            body: String::new(),
            state,
            from: Actor::parse(from),
            to: to.into(),
            parent: parent.map(Into::into),
            priority: 0,
            summary: None,
            block_reason: None,
            created_ms: at,
            updated_ms: at,
            brief: Default::default(),
            outcome: Outcome::default(),
            run: None,
        }
    }

    fn list(tasks: &[Task], inbox: &[InboxFact], runs: &[RunFact], committing: &[String], held: &BTreeMap<String, Hold>) -> Vec<(String, Group, Doing)> {
        let slots = BTreeMap::from([("T1".to_owned(), 1)]);
        let external = BTreeMap::new();
        let f = Facts { tasks, owner: "pm", inbox, runs, committing, held, slots: &slots, external: &external };
        requests(&f).into_iter().map(|r| (r.id, r.group, r.doing)).collect()
    }

    #[test]
    fn an_issue_waits_for_its_reporter_until_work_starts_again() {
        use TaskState::*;
        let ext = |asked| External { source: "github".into(), rf: "7".into(), url: String::new(), author: "alice".into(), triage: Some("ask".into()), pr: None, asked_ms: asked };
        let group = |tasks: &[Task], asked: Option<i64>| {
            let external = BTreeMap::from([("T1".to_owned(), ext(asked))]);
            let (slots, held) = (BTreeMap::new(), BTreeMap::new());
            let f = Facts { tasks, owner: "pm", inbox: &[], runs: &[], committing: &[], held: &held, slots: &slots, external: &external };
            let r = requests(&f).remove(0);
            (r.group, r.doing, r.since_ms)
        };
        let asked = [task("T1", None, "human", "pm", InProgress, 10)];
        assert_eq!(group(&asked, None).0, Group::Working, "not asked yet");
        assert_eq!(group(&asked, Some(20)), (Group::Waiting, Doing::Reply, 20));
        // The reply came (the person told the PM) and an engineer works on it.
        let again = [task("T1", None, "human", "pm", InProgress, 10), task("T2", Some("T1"), "pm", "engineer", InProgress, 30)];
        assert_eq!(group(&again, Some(20)).0, Group::Working);
        // Done under it since, the PM wraps up: no longer waiting.
        let wrapped = [task("T1", None, "human", "pm", InProgress, 10), task("T2", Some("T1"), "pm", "engineer", Done, 30)];
        assert_eq!(group(&wrapped, Some(20)).0, Group::Working);
    }

    #[test]
    fn each_chain_is_one_request_grouped_by_whose_move_it_is() {
        use TaskState::*;
        let tasks = [
            // T1: the person asked the PM; an engineer works under it.
            task("T1", None, "human", "pm", InProgress, 10),
            task("T2", Some("T1"), "pm", "engineer", InProgress, 20),
            // T3: done, a review waits.
            task("T3", None, "human", "engineer", Done, 30),
            // T4: queued for a worktree.
            task("T4", None, "pm", "engineer", Queued, 40),
            // T5: delivered and accepted; T6: cancelled.
            task("T5", None, "human", "architect", Done, 50),
            task("T6", None, "human", "architect", Cancelled, 60),
            // T7: its check runs.
            task("T7", None, "human", "engineer", Done, 70),
        ];
        let inbox = [
            InboxFact { id: "Q1".into(), kind: "review".into(), task: Some("T3".into()), created_ms: 35 },
            InboxFact { id: "Q2".into(), kind: "report".into(), task: Some("T5".into()), created_ms: 55 },
        ];
        let runs = [RunFact { anchor: "T7".into(), checking: true, updated_ms: 71 }];
        let held = BTreeMap::from([("T4".to_owned(), Hold::Worktree)]);
        let got = list(&tasks, &inbox, &runs, &[], &held);
        let member = |m: &str, t: &str| Doing::Member { member: m.into(), task: t.into(), title: format!("title {t}"), state: "in_progress".into() };
        assert_eq!(
            got,
            [
                ("T3".into(), Group::NeedsYou, Doing::You { inbox: "review".into() }),
                ("T1".into(), Group::Working, member("engineer", "T2")),
                ("T7".into(), Group::Working, Doing::Check),
                ("T4".into(), Group::Queued, Doing::Wait { hold: Hold::Worktree }),
                ("T5".into(), Group::Delivered, Doing::Done),
                ("T6".into(), Group::Dropped, Doing::Dropped),
            ]
        );
    }

    #[test]
    fn the_most_pressing_item_names_it_and_a_commit_keeps_it_working() {
        use TaskState::*;
        let tasks = [task("T1", None, "human", "pm", Done, 10), task("T2", Some("T1"), "pm", "engineer", Done, 20)];
        let inbox = [
            InboxFact { id: "Q2".into(), kind: "terminal".into(), task: Some("T2".into()), created_ms: 25 },
            InboxFact { id: "Q3".into(), kind: "question".into(), task: Some("T1".into()), created_ms: 30 },
        ];
        let slots = BTreeMap::from([("T1".to_owned(), 2)]);
        let held = BTreeMap::new();
        let external = BTreeMap::new();
        let f = Facts { tasks: &tasks, owner: "pm", inbox: &inbox, runs: &[], committing: &[], held: &held, slots: &slots, external: &external };
        let r = &requests(&f)[0];
        assert_eq!((r.group, &r.doing, r.since_ms, r.inbox.clone(), r.slot), (Group::NeedsYou, &Doing::You { inbox: "question".into() }, 25, vec!["Q3".into(), "Q2".into()], Some(2)));
        assert_eq!(list(&tasks, &[], &[], &["T1".into()], &held)[0].2, Doing::Commit);
    }
}
