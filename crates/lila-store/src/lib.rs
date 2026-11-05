//! SQLite persistence. Only the daemon writes; everything is scoped by project id.

use anyhow::{Context, Result, bail};
use lila_core::{Actor, Task, TaskState, Transition, task};
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::path::Path;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    cwd TEXT NOT NULL UNIQUE,
    created_ms INTEGER NOT NULL,
    next_task INTEGER NOT NULL DEFAULT 1,
    next_question INTEGER NOT NULL DEFAULT 1,
    paused INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS members (
    project TEXT NOT NULL,
    id TEXT NOT NULL,
    role TEXT NOT NULL,
    terminal TEXT,
    started_ms INTEGER,
    PRIMARY KEY (project, id)
);
CREATE TABLE IF NOT EXISTS tasks (
    project TEXT NOT NULL,
    id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    state TEXT NOT NULL,
    from_actor TEXT NOT NULL,
    to_member TEXT NOT NULL,
    parent TEXT,
    priority INTEGER NOT NULL DEFAULT 0,
    summary TEXT,
    block_reason TEXT,
    created_ms INTEGER NOT NULL,
    updated_ms INTEGER NOT NULL,
    nudged_ms INTEGER,
    stall_reported INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project, id)
);
CREATE TABLE IF NOT EXISTS task_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project TEXT NOT NULL,
    task TEXT,
    at_ms INTEGER NOT NULL,
    actor TEXT NOT NULL,
    kind TEXT NOT NULL,
    detail TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS artifacts (
    project TEXT NOT NULL,
    task TEXT NOT NULL,
    path TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS inbox (
    project TEXT NOT NULL,
    id TEXT NOT NULL,
    kind TEXT NOT NULL,
    member TEXT,
    task TEXT,
    text TEXT NOT NULL,
    options TEXT NOT NULL DEFAULT '[]',
    answer TEXT,
    created_ms INTEGER NOT NULL,
    resolved_ms INTEGER,
    PRIMARY KEY (project, id)
);
CREATE TABLE IF NOT EXISTS outbox (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project TEXT NOT NULL,
    member TEXT NOT NULL,
    text TEXT NOT NULL,
    created_ms INTEGER NOT NULL
);
"#;

pub struct Store {
    db: Connection,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectRow {
    pub id: String,
    pub cwd: String,
    pub paused: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemberRow {
    pub id: String,
    pub role: String,
    pub terminal: Option<String>,
    pub started_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewTask {
    pub title: String,
    pub body: String,
    pub from: Actor,
    pub to: String,
    pub parent: Option<String>,
    pub priority: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskMeta {
    pub seq: i64,
    pub nudged_ms: Option<i64>,
    pub stall_reported: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub at_ms: i64,
    pub task: Option<String>,
    pub actor: String,
    pub kind: String,
    pub detail: String,
}

/// Something that needs the person.
#[derive(Debug, Clone, PartialEq)]
pub struct InboxItem {
    pub id: String,
    /// question | terminal | startup | stalled | report | lost
    pub kind: String,
    pub member: Option<String>,
    pub task: Option<String>,
    pub text: String,
    pub options: Vec<String>,
    pub answer: Option<String>,
    pub created_ms: i64,
    pub resolved_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutMessage {
    pub id: i64,
    pub member: String,
    pub text: String,
}

const TASK_COLS: &str =
    "project, id, title, body, state, from_actor, to_member, parent, priority, summary, block_reason, created_ms, updated_ms, seq, nudged_ms, stall_reported";

fn task_row(r: &Row) -> rusqlite::Result<(Task, TaskMeta)> {
    let state: String = r.get(4)?;
    let from: String = r.get(5)?;
    Ok((
        Task {
            project: r.get(0)?,
            id: r.get(1)?,
            title: r.get(2)?,
            body: r.get(3)?,
            state: TaskState::parse(&state).unwrap_or(TaskState::Queued),
            from: Actor::parse(&from),
            to: r.get(6)?,
            parent: r.get(7)?,
            priority: r.get(8)?,
            summary: r.get(9)?,
            block_reason: r.get(10)?,
            created_ms: r.get(11)?,
            updated_ms: r.get(12)?,
        },
        TaskMeta { seq: r.get(13)?, nudged_ms: r.get(14)?, stall_reported: r.get::<_, i64>(15)? != 0 },
    ))
}

fn inbox_row(r: &Row) -> rusqlite::Result<InboxItem> {
    let options: String = r.get(6)?;
    Ok(InboxItem {
        id: r.get(0)?,
        kind: r.get(1)?,
        member: r.get(2)?,
        task: r.get(3)?,
        text: r.get(4)?,
        answer: r.get(5)?,
        options: options.split('\u{1f}').filter(|s| !s.is_empty()).map(String::from).collect(),
        created_ms: r.get(7)?,
        resolved_ms: r.get(8)?,
    })
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        let db = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "busy_timeout", 5000)?;
        db.execute_batch(SCHEMA)?;
        Ok(Store { db })
    }

    pub fn in_memory() -> Result<Store> {
        let db = Connection::open_in_memory()?;
        db.execute_batch(SCHEMA)?;
        Ok(Store { db })
    }

    /// Rows this connection has changed since it opened: goes up on every write,
    /// so the daemon can tell watchers that something changed.
    pub fn total_changes(&self) -> u64 {
        self.db.total_changes()
    }

    // ---------- projects ----------

    /// Registers a project, or confirms an existing one. One id per folder.
    pub fn upsert_project(&self, id: &str, cwd: &str, now: i64) -> Result<()> {
        if let Some(p) = self.project_by_cwd(cwd)?
            && p.id != id
        {
            bail!("{cwd} is already registered as project {:?}; rename it back in .lila/project.toml", p.id);
        }
        if let Some(p) = self.project(id)?
            && p.cwd != cwd
        {
            bail!("a project named {id:?} already lives in {}; choose another name in .lila/project.toml", p.cwd);
        }
        self.db.execute("INSERT OR IGNORE INTO projects (id, cwd, created_ms) VALUES (?1, ?2, ?3)", params![id, cwd, now])?;
        Ok(())
    }

    fn project_from(r: &Row) -> rusqlite::Result<ProjectRow> {
        Ok(ProjectRow { id: r.get(0)?, cwd: r.get(1)?, paused: r.get::<_, i64>(2)? != 0 })
    }

    pub fn project(&self, id: &str) -> Result<Option<ProjectRow>> {
        Ok(self.db.query_row("SELECT id, cwd, paused FROM projects WHERE id = ?1", [id], Self::project_from).optional()?)
    }

    pub fn project_by_cwd(&self, cwd: &str) -> Result<Option<ProjectRow>> {
        Ok(self.db.query_row("SELECT id, cwd, paused FROM projects WHERE cwd = ?1", [cwd], Self::project_from).optional()?)
    }

    pub fn projects(&self) -> Result<Vec<ProjectRow>> {
        let mut st = self.db.prepare("SELECT id, cwd, paused FROM projects ORDER BY id")?;
        Ok(st.query_map([], Self::project_from)?.collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_paused(&self, project: &str, paused: bool) -> Result<()> {
        self.db.execute("UPDATE projects SET paused = ?2 WHERE id = ?1", params![project, paused as i64])?;
        Ok(())
    }

    // ---------- members ----------

    pub fn set_member(&self, project: &str, id: &str, role: &str, terminal: Option<&str>, started_ms: Option<i64>) -> Result<()> {
        self.db.execute(
            "INSERT INTO members (project, id, role, terminal, started_ms) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (project, id) DO UPDATE SET role = ?3, terminal = ?4, started_ms = ?5",
            params![project, id, role, terminal, started_ms],
        )?;
        Ok(())
    }

    pub fn members(&self, project: &str) -> Result<Vec<MemberRow>> {
        let mut st = self.db.prepare("SELECT id, role, terminal, started_ms FROM members WHERE project = ?1 ORDER BY rowid")?;
        Ok(st
            .query_map([project], |r| Ok(MemberRow { id: r.get(0)?, role: r.get(1)?, terminal: r.get(2)?, started_ms: r.get(3)? }))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn remove_member(&self, project: &str, id: &str) -> Result<()> {
        self.db.execute("DELETE FROM members WHERE project = ?1 AND id = ?2", params![project, id])?;
        Ok(())
    }

    // ---------- tasks ----------

    pub fn create_task(&mut self, project: &str, new: NewTask, now: i64) -> Result<Task> {
        let tx = self.db.transaction()?;
        let n: i64 = tx
            .query_row("SELECT next_task FROM projects WHERE id = ?1", [project], |r| r.get(0))
            .optional()?
            .with_context(|| format!("unknown project {project}"))?;
        tx.execute("UPDATE projects SET next_task = next_task + 1 WHERE id = ?1", [project])?;
        let id = format!("T{n}");
        tx.execute(
            "INSERT INTO tasks (project, id, seq, title, body, state, from_actor, to_member, parent, priority, created_ms, updated_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
            params![project, id, n, new.title, new.body, TaskState::Queued.as_str(), new.from.as_str(), new.to, new.parent, new.priority, now],
        )?;
        tx.execute(
            "INSERT INTO task_events (project, task, at_ms, actor, kind, detail) VALUES (?1, ?2, ?3, ?4, 'created', ?5)",
            params![project, id, now, new.from.as_str(), format!("→ {}: {}", new.to, new.title)],
        )?;
        tx.commit()?;
        Ok(self.task(project, &id)?.expect("just inserted"))
    }

    pub fn task(&self, project: &str, id: &str) -> Result<Option<Task>> {
        Ok(self.task_with_meta(project, id)?.map(|(t, _)| t))
    }

    pub fn task_with_meta(&self, project: &str, id: &str) -> Result<Option<(Task, TaskMeta)>> {
        let sql = format!("SELECT {TASK_COLS} FROM tasks WHERE project = ?1 AND id = ?2");
        Ok(self.db.query_row(&sql, params![project, id.to_ascii_uppercase()], task_row).optional()?)
    }

    pub fn tasks(&self, project: &str) -> Result<Vec<(Task, TaskMeta)>> {
        let sql = format!("SELECT {TASK_COLS} FROM tasks WHERE project = ?1 ORDER BY seq");
        let mut st = self.db.prepare(&sql)?;
        Ok(st.query_map([project], task_row)?.collect::<rusqlite::Result<_>>()?)
    }

    /// Moves a task through the state machine and records who did it.
    pub fn transition(&self, project: &str, id: &str, t: Transition, actor: &Actor, detail: &str, now: i64) -> Result<Task> {
        let task = self.task(project, id)?.with_context(|| format!("no task {id} in project {project}"))?;
        let next = task::apply(task.state, t).map_err(|e| anyhow::anyhow!("{}: {e}", task.id))?;
        let (summary, reason) = match t {
            Transition::Done => (Some(detail), None),
            Transition::Block => (None, Some(detail)),
            _ => (None, None),
        };
        self.db.execute(
            "UPDATE tasks SET state = ?3, updated_ms = ?4,
                summary = COALESCE(?5, summary),
                block_reason = CASE WHEN ?3 = 'blocked' THEN ?6 ELSE block_reason END,
                nudged_ms = CASE WHEN ?3 IN ('queued', 'delivered') THEN NULL ELSE nudged_ms END,
                stall_reported = CASE WHEN ?3 IN ('queued', 'delivered') THEN 0 ELSE stall_reported END
             WHERE project = ?1 AND id = ?2",
            params![project, task.id, next.as_str(), now, summary, reason],
        )?;
        self.event(project, Some(&task.id), actor.as_str(), t.as_str(), detail, now)?;
        Ok(self.task(project, &task.id)?.expect("exists"))
    }

    /// Replaces a task's title and body (only while it is queued: what was typed cannot change).
    pub fn set_task_text(&self, project: &str, id: &str, title: &str, body: &str, now: i64) -> Result<()> {
        self.db.execute(
            "UPDATE tasks SET title = ?3, body = ?4, updated_ms = ?5 WHERE project = ?1 AND id = ?2",
            params![project, id.to_ascii_uppercase(), title, body, now],
        )?;
        Ok(())
    }

    /// Parent, grandparent, ... of a task (nearest first).
    pub fn ancestors(&self, project: &str, id: &str) -> Result<Vec<Task>> {
        let mut out = Vec::new();
        let mut cur = self.task(project, id)?.and_then(|t| t.parent);
        while let Some(p) = cur {
            let Some(t) = self.task(project, &p)? else { break };
            if out.len() > 1000 {
                bail!("task parent chain loops at {p}");
            }
            cur = t.parent.clone();
            out.push(t);
        }
        Ok(out)
    }

    pub fn set_nudged(&self, project: &str, id: &str, now: i64) -> Result<()> {
        self.db.execute("UPDATE tasks SET nudged_ms = ?3 WHERE project = ?1 AND id = ?2", params![project, id, now])?;
        Ok(())
    }

    pub fn set_stall_reported(&self, project: &str, id: &str) -> Result<()> {
        self.db.execute("UPDATE tasks SET stall_reported = 1 WHERE project = ?1 AND id = ?2", params![project, id])?;
        Ok(())
    }

    /// How many ancestors a task has (T1 → T4 → T9: T9 has depth 2).
    pub fn depth(&self, project: &str, id: &str) -> Result<u32> {
        let mut depth = 0;
        let mut cur = self.task(project, id)?.and_then(|t| t.parent);
        while let Some(p) = cur {
            depth += 1;
            if depth > 1000 {
                bail!("task parent chain loops at {p}");
            }
            cur = self.task(project, &p)?.and_then(|t| t.parent);
        }
        Ok(depth)
    }

    pub fn add_artifacts(&self, project: &str, task: &str, paths: &[String]) -> Result<()> {
        for p in paths {
            self.db.execute("INSERT INTO artifacts (project, task, path) VALUES (?1, ?2, ?3)", params![project, task, p])?;
        }
        Ok(())
    }

    pub fn artifacts(&self, project: &str, task: &str) -> Result<Vec<String>> {
        let mut st = self.db.prepare("SELECT path FROM artifacts WHERE project = ?1 AND task = ?2 ORDER BY rowid")?;
        Ok(st.query_map(params![project, task], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
    }

    // ---------- events ----------

    pub fn event(&self, project: &str, task: Option<&str>, actor: &str, kind: &str, detail: &str, now: i64) -> Result<()> {
        self.db.execute(
            "INSERT INTO task_events (project, task, at_ms, actor, kind, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![project, task, now, actor, kind, detail],
        )?;
        Ok(())
    }

    /// Newest last. `task` narrows to one task.
    pub fn events(&self, project: &str, task: Option<&str>, limit: usize) -> Result<Vec<Event>> {
        let mut st = self.db.prepare(
            "SELECT at_ms, task, actor, kind, detail FROM (
                SELECT * FROM task_events WHERE project = ?1 AND (?2 IS NULL OR task = ?2) ORDER BY id DESC LIMIT ?3
             ) ORDER BY id",
        )?;
        Ok(st
            .query_map(params![project, task, limit as i64], |r| {
                Ok(Event { at_ms: r.get(0)?, task: r.get(1)?, actor: r.get(2)?, kind: r.get(3)?, detail: r.get(4)? })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    // ---------- inbox ----------

    #[allow(clippy::too_many_arguments)]
    pub fn add_inbox(
        &mut self,
        project: &str,
        kind: &str,
        member: Option<&str>,
        task: Option<&str>,
        text: &str,
        options: &[String],
        now: i64,
    ) -> Result<InboxItem> {
        let tx = self.db.transaction()?;
        let n: i64 = tx.query_row("SELECT next_question FROM projects WHERE id = ?1", [project], |r| r.get(0))?;
        tx.execute("UPDATE projects SET next_question = next_question + 1 WHERE id = ?1", [project])?;
        let id = format!("Q{n}");
        tx.execute(
            "INSERT INTO inbox (project, id, kind, member, task, text, options, created_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![project, id, kind, member, task, text, options.join("\u{1f}"), now],
        )?;
        tx.commit()?;
        Ok(self.inbox_item(project, &id)?.expect("just inserted"))
    }

    pub fn inbox_item(&self, project: &str, id: &str) -> Result<Option<InboxItem>> {
        Ok(self
            .db
            .query_row(
                "SELECT id, kind, member, task, text, answer, options, created_ms, resolved_ms FROM inbox WHERE project = ?1 AND id = ?2",
                params![project, id.to_ascii_uppercase()],
                inbox_row,
            )
            .optional()?)
    }

    pub fn inbox(&self, project: &str, include_resolved: bool) -> Result<Vec<InboxItem>> {
        let mut st = self.db.prepare(
            "SELECT id, kind, member, task, text, answer, options, created_ms, resolved_ms FROM inbox
             WHERE project = ?1 AND (?2 OR resolved_ms IS NULL) ORDER BY created_ms, rowid",
        )?;
        Ok(st.query_map(params![project, include_resolved], inbox_row)?.collect::<rusqlite::Result<_>>()?)
    }

    pub fn resolve_inbox(&self, project: &str, id: &str, answer: Option<&str>, now: i64) -> Result<()> {
        let n = self.db.execute(
            "UPDATE inbox SET resolved_ms = ?3, answer = COALESCE(?4, answer) WHERE project = ?1 AND id = ?2 AND resolved_ms IS NULL",
            params![project, id.to_ascii_uppercase(), now, answer],
        )?;
        if n == 0 {
            bail!("no open inbox item {id}");
        }
        Ok(())
    }

    /// Open items of a kind for a member (to avoid duplicates and to close them when the cause ends).
    pub fn open_inbox_for(&self, project: &str, kind: &str, member: &str) -> Result<Vec<InboxItem>> {
        Ok(self.inbox(project, false)?.into_iter().filter(|i| i.kind == kind && i.member.as_deref() == Some(member)).collect())
    }

    // ---------- outbox: notes waiting for their member to accept input ----------

    pub fn push_outbox(&self, project: &str, member: &str, text: &str, now: i64) -> Result<()> {
        self.db.execute("INSERT INTO outbox (project, member, text, created_ms) VALUES (?1, ?2, ?3, ?4)", params![project, member, text, now])?;
        Ok(())
    }

    pub fn outbox(&self, project: &str) -> Result<Vec<OutMessage>> {
        let mut st = self.db.prepare("SELECT id, member, text FROM outbox WHERE project = ?1 ORDER BY id")?;
        Ok(st.query_map([project], |r| Ok(OutMessage { id: r.get(0)?, member: r.get(1)?, text: r.get(2)? }))?.collect::<rusqlite::Result<_>>()?)
    }

    pub fn pop_outbox(&self, id: i64) -> Result<()> {
        self.db.execute("DELETE FROM outbox WHERE id = ?1", [id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let s = Store::in_memory().unwrap();
        s.upsert_project("shop", "/repo/shop", 1).unwrap();
        s
    }

    fn new(to: &str, parent: Option<&str>) -> NewTask {
        NewTask { title: "t".into(), body: "b".into(), from: Actor::Member("owner".into()), to: to.into(), parent: parent.map(Into::into), priority: 0 }
    }

    #[test]
    fn projects_are_one_per_folder() {
        let s = store();
        s.upsert_project("shop", "/repo/shop", 2).unwrap();
        assert!(s.upsert_project("other", "/repo/shop", 2).unwrap_err().to_string().contains("already registered"));
        assert!(s.upsert_project("shop", "/elsewhere", 2).unwrap_err().to_string().contains("already lives"));
        assert_eq!(s.projects().unwrap().len(), 1);
    }

    #[test]
    fn tasks_number_per_project_and_move_through_states() {
        let mut s = store();
        s.upsert_project("lab", "/repo/lab", 1).unwrap();
        assert_eq!(s.create_task("shop", new("engineer", None), 10).unwrap().id, "T1");
        assert_eq!(s.create_task("shop", new("engineer", None), 11).unwrap().id, "T2");
        assert_eq!(s.create_task("lab", new("engineer", None), 12).unwrap().id, "T1");

        let owner = Actor::Member("owner".into());
        let eng = Actor::Member("engineer".into());
        s.transition("shop", "T1", Transition::Deliver, &Actor::Human, "", 20).unwrap();
        s.transition("shop", "t1", Transition::Start, &eng, "", 21).unwrap();
        let t = s.transition("shop", "T1", Transition::Block, &eng, "need key", 22).unwrap();
        assert_eq!((t.state, t.block_reason.as_deref()), (TaskState::Blocked, Some("need key")));
        let t = s.transition("shop", "T1", Transition::Done, &eng, "shipped", 23).unwrap();
        assert_eq!((t.state, t.summary.as_deref()), (TaskState::Done, Some("shipped")));
        assert!(s.transition("shop", "T1", Transition::Start, &owner, "", 24).unwrap_err().to_string().contains("closed"));

        let kinds: Vec<String> = s.events("shop", Some("T1"), 50).unwrap().into_iter().map(|e| e.kind).collect();
        assert_eq!(kinds, ["created", "delivered", "started", "blocked", "done"]);
        assert_eq!(s.events("shop", None, 2).unwrap().len(), 2);
    }

    #[test]
    fn requeue_clears_stall_tracking() {
        let mut s = store();
        s.create_task("shop", new("engineer", None), 1).unwrap();
        s.transition("shop", "T1", Transition::Deliver, &Actor::Human, "", 2).unwrap();
        s.set_nudged("shop", "T1", 3).unwrap();
        s.set_stall_reported("shop", "T1").unwrap();
        s.transition("shop", "T1", Transition::Requeue, &Actor::Human, "lost", 4).unwrap();
        let (_, meta) = s.task_with_meta("shop", "T1").unwrap().unwrap();
        assert_eq!((meta.nudged_ms, meta.stall_reported), (None, false));
    }

    #[test]
    fn depth_follows_parents() {
        let mut s = store();
        s.create_task("shop", new("a", None), 1).unwrap();
        s.create_task("shop", new("b", Some("T1")), 1).unwrap();
        s.create_task("shop", new("c", Some("T2")), 1).unwrap();
        assert_eq!(s.depth("shop", "T1").unwrap(), 0);
        assert_eq!(s.depth("shop", "T3").unwrap(), 2);
        let ids: Vec<String> = s.ancestors("shop", "T3").unwrap().into_iter().map(|t| t.id).collect();
        assert_eq!(ids, ["T2", "T1"]);
    }

    #[test]
    fn task_text_can_change() {
        let mut s = store();
        s.create_task("shop", new("a", None), 1).unwrap();
        s.set_task_text("shop", "t1", "New title", "New body", 5).unwrap();
        let t = s.task("shop", "T1").unwrap().unwrap();
        assert_eq!((t.title.as_str(), t.body.as_str(), t.updated_ms), ("New title", "New body", 5));
    }

    #[test]
    fn inbox_and_outbox() {
        let mut s = store();
        let q = s.add_inbox("shop", "question", Some("owner"), None, "Stripe or Adyen?", &["Stripe".into(), "Adyen".into()], 5).unwrap();
        assert_eq!((q.id.as_str(), q.options.len()), ("Q1", 2));
        s.add_inbox("shop", "terminal", Some("tester"), None, "approve Bash", &[], 6).unwrap();
        assert_eq!(s.open_inbox_for("shop", "terminal", "tester").unwrap().len(), 1);
        s.resolve_inbox("shop", "q1", Some("Stripe"), 7).unwrap();
        assert!(s.resolve_inbox("shop", "Q1", None, 8).is_err());
        assert_eq!(s.inbox("shop", false).unwrap().len(), 1);
        assert_eq!(s.inbox("shop", true).unwrap()[0].answer.as_deref(), Some("Stripe"));

        s.push_outbox("shop", "owner", "hello", 1).unwrap();
        let out = s.outbox("shop").unwrap();
        assert_eq!(out[0].text, "hello");
        s.pop_outbox(out[0].id).unwrap();
        assert!(s.outbox("shop").unwrap().is_empty());
    }

    #[test]
    fn opens_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lila.db");
        Store::open(&path).unwrap().upsert_project("p", "/x", 1).unwrap();
        assert_eq!(Store::open(&path).unwrap().projects().unwrap()[0].id, "p");
    }
}
