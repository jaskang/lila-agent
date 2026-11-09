//! SQLite persistence. Only the daemon writes; everything is scoped by project id.

use anyhow::{Context, Result, bail};
use lila_core::{Actor, Brief, Outcome, Task, TaskState, Transition, task};
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
CREATE TABLE IF NOT EXISTS runs (
    project TEXT NOT NULL,
    anchor TEXT NOT NULL,
    flow TEXT NOT NULL,
    step INTEGER NOT NULL DEFAULT 0,
    round INTEGER NOT NULL DEFAULT 0,
    state TEXT NOT NULL,
    created_ms INTEGER NOT NULL,
    updated_ms INTEGER NOT NULL,
    PRIMARY KEY (project, anchor)
);
CREATE TABLE IF NOT EXISTS sessions (
    project TEXT NOT NULL,
    id TEXT NOT NULL,
    member TEXT NOT NULL,
    runtime TEXT NOT NULL,
    file TEXT,
    started_ms INTEGER NOT NULL,
    PRIMARY KEY (project, id)
);
CREATE TABLE IF NOT EXISTS external (
    project TEXT NOT NULL,
    task TEXT NOT NULL,
    source TEXT NOT NULL,
    ref TEXT NOT NULL,
    url TEXT NOT NULL,
    author TEXT NOT NULL DEFAULT '',
    triage TEXT,
    size TEXT,
    why TEXT,
    plan TEXT,
    pr TEXT,
    created_ms INTEGER NOT NULL,
    PRIMARY KEY (project, source, ref)
);
CREATE TABLE IF NOT EXISTS session_chains (
    project TEXT NOT NULL,
    session TEXT NOT NULL,
    chain TEXT NOT NULL,
    last_ms INTEGER NOT NULL,
    PRIMARY KEY (project, session, chain)
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

/// A request that came from outside: the issue it is, how the PM sorted it, its PR.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExternalRow {
    /// The request's root task.
    pub task: String,
    /// `github`.
    pub source: String,
    /// The issue's number.
    pub rf: String,
    pub url: String,
    pub author: String,
    /// The PM's call: fix | ask | skip | dup.
    pub triage: Option<String>,
    pub size: Option<String>,
    pub why: Option<String>,
    pub plan: Option<String>,
    pub pr: Option<String>,
    pub created_ms: i64,
    /// When a comment last went to the reporter: the request then waits for their reply.
    pub asked_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemberRow {
    pub id: String,
    pub role: String,
    pub terminal: Option<String>,
    pub started_ms: Option<i64>,
    /// The CLI session its terminal runs (`sessions`), when Lila knows it.
    pub session: Option<String>,
}

/// A CLI session Lila started for a member.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    pub id: String,
    pub member: String,
    pub runtime: String,
    /// Where the CLI records it (for its context size).
    pub file: Option<String>,
    pub started_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewTask {
    pub title: String,
    pub body: String,
    pub from: Actor,
    pub to: String,
    pub parent: Option<String>,
    pub priority: i64,
    pub brief: Brief,
    /// A step of a flow run: its anchor and step index.
    pub run: Option<(String, usize)>,
}

/// A flow run: the steps one piece of work goes through (`lila_core::flow`).
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    /// The task that started it.
    pub anchor: String,
    pub flow: String,
    /// The step now under way.
    pub step: usize,
    /// Times work was sent back.
    pub round: u32,
    pub state: RunState,
    pub updated_ms: i64,
    /// Tokens the run may use; over it, it holds between steps.
    pub budget: Option<u64>,
    /// While held: what lilad was about to do next, for when the person lets it go on.
    pub hold: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    /// A member is on the current step.
    Running,
    /// lilad runs the check command.
    Checking,
    Done,
    /// Failed more than `max_rounds` times; the PM decides.
    Failed,
    Cancelled,
    /// Over its budget, between two steps: the person decides.
    Held,
}

impl RunState {
    pub fn as_str(self) -> &'static str {
        match self {
            RunState::Running => "running",
            RunState::Checking => "checking",
            RunState::Done => "done",
            RunState::Failed => "failed",
            RunState::Cancelled => "cancelled",
            RunState::Held => "held",
        }
    }

    pub fn parse(s: &str) -> RunState {
        match s {
            "checking" => RunState::Checking,
            "done" => RunState::Done,
            "failed" => RunState::Failed,
            "cancelled" => RunState::Cancelled,
            "held" => RunState::Held,
            _ => RunState::Running,
        }
    }

    pub fn is_open(self) -> bool {
        matches!(self, RunState::Running | RunState::Checking | RunState::Held)
    }
}

/// Columns added after the first release: `(table, column, declaration)`.
/// Missing ones are added when a database opens, so old databases keep working.
const ADDED_COLUMNS: &[(&str, &str, &str)] = &[
    ("tasks", "quote", "TEXT"),
    ("tasks", "accept", "TEXT NOT NULL DEFAULT ''"),
    ("tasks", "out_of_scope", "TEXT NOT NULL DEFAULT ''"),
    ("tasks", "decisions", "TEXT NOT NULL DEFAULT ''"),
    ("tasks", "unverified", "TEXT NOT NULL DEFAULT ''"),
    ("tasks", "run", "TEXT"),
    ("tasks", "run_step", "INTEGER"),
    ("tasks", "verdict", "TEXT"),
    ("members", "session", "TEXT"),
    ("runs", "budget", "INTEGER"),
    ("runs", "hold", "TEXT"),
    ("external", "asked_ms", "INTEGER"),
];

fn migrate(db: &Connection) -> Result<()> {
    db.execute_batch(SCHEMA)?;
    for (table, col, decl) in ADDED_COLUMNS {
        let has: bool = db.query_row(&format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1"), [col], |r| r.get::<_, i64>(0))? > 0;
        if !has {
            db.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {col} {decl}"))?;
        }
    }
    Ok(())
}

/// Lists live in one column, separated by the unit separator (like inbox options).
fn join_list(items: &[String]) -> String {
    items.iter().map(|i| i.trim()).filter(|i| !i.is_empty()).collect::<Vec<_>>().join("\u{1f}")
}

fn split_list(s: &str) -> Vec<String> {
    s.split('\u{1f}').filter(|s| !s.is_empty()).map(String::from).collect()
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

const TASK_COLS: &str = "project, id, title, body, state, from_actor, to_member, parent, priority, summary, block_reason, created_ms, updated_ms, seq, nudged_ms, stall_reported, quote, accept, out_of_scope, decisions, unverified, run, run_step, verdict";

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
            brief: Brief { quote: r.get(16)?, accept: split_list(&r.get::<_, String>(17)?), out_of_scope: split_list(&r.get::<_, String>(18)?) },
            outcome: Outcome {
                decisions: split_list(&r.get::<_, String>(19)?),
                unverified: split_list(&r.get::<_, String>(20)?),
                passed: r.get::<_, Option<String>>(23)?.map(|v| v == "pass"),
            },
            run: match (r.get::<_, Option<String>>(21)?, r.get::<_, Option<i64>>(22)?) {
                (Some(a), Some(s)) => Some((a, s as usize)),
                _ => None,
            },
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
        migrate(&db)?;
        Ok(Store { db })
    }

    pub fn in_memory() -> Result<Store> {
        let db = Connection::open_in_memory()?;
        migrate(&db)?;
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

    /// Forgets a project and everything Lila recorded for it. Its folder is not touched.
    pub fn remove_project(&self, project: &str) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        for table in ["members", "tasks", "task_events", "artifacts", "inbox", "outbox", "runs", "sessions", "session_chains", "external"] {
            tx.execute(&format!("DELETE FROM {table} WHERE project = ?1"), [project])?;
        }
        tx.execute("DELETE FROM projects WHERE id = ?1", [project])?;
        tx.commit()?;
        Ok(())
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
        let mut st = self.db.prepare("SELECT id, role, terminal, started_ms, session FROM members WHERE project = ?1 ORDER BY rowid")?;
        Ok(st
            .query_map([project], |r| Ok(MemberRow { id: r.get(0)?, role: r.get(1)?, terminal: r.get(2)?, started_ms: r.get(3)?, session: r.get(4)? }))?
            .collect::<rusqlite::Result<_>>()?)
    }

    // ---------- requests from outside (`[[sources]]`) ----------

    pub fn add_external(&self, project: &str, e: &ExternalRow) -> Result<()> {
        self.db.execute(
            "INSERT INTO external (project, task, source, ref, url, author, created_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![project, e.task, e.source, e.rf, e.url, e.author, e.created_ms],
        )?;
        Ok(())
    }

    fn external_where(&self, project: &str, cond: &str, p1: &str, p2: Option<&str>) -> Result<Vec<ExternalRow>> {
        let sql = format!("SELECT task, source, ref, url, author, triage, size, why, plan, pr, created_ms, asked_ms FROM external WHERE project = ?1 AND {cond} ORDER BY created_ms");
        let mut st = self.db.prepare(&sql)?;
        let row = |r: &rusqlite::Row| {
            Ok(ExternalRow {
                task: r.get(0)?,
                source: r.get(1)?,
                rf: r.get(2)?,
                url: r.get(3)?,
                author: r.get(4)?,
                triage: r.get(5)?,
                size: r.get(6)?,
                why: r.get(7)?,
                plan: r.get(8)?,
                pr: r.get(9)?,
                created_ms: r.get(10)?,
                asked_ms: r.get(11)?,
            })
        };
        let rows = match p2 {
            Some(p2) => st.query_map(params![project, p1, p2], row)?.collect::<rusqlite::Result<_>>()?,
            None => st.query_map(params![project, p1], row)?.collect::<rusqlite::Result<_>>()?,
        };
        Ok(rows)
    }

    /// The outside item a request (its root task) came from.
    pub fn external_for(&self, project: &str, task: &str) -> Result<Option<ExternalRow>> {
        Ok(self.external_where(project, "task = ?2", task, None)?.into_iter().next())
    }

    pub fn external_by_ref(&self, project: &str, source: &str, rf: &str) -> Result<Option<ExternalRow>> {
        Ok(self.external_where(project, "source = ?2 AND ref = ?3", source, Some(rf))?.into_iter().next())
    }

    /// Items a source brought in since `since_ms`.
    pub fn externals_since(&self, project: &str, source: &str, since_ms: i64) -> Result<Vec<ExternalRow>> {
        Ok(self.external_where(project, "source = ?2", source, None)?.into_iter().filter(|e| e.created_ms >= since_ms).collect())
    }

    pub fn externals(&self, project: &str) -> Result<Vec<ExternalRow>> {
        self.external_where(project, "?2 IS NOT NULL", "", None)
    }

    pub fn set_triage(&self, project: &str, task: &str, triage: &str, size: Option<&str>, why: &str, plan: Option<&str>) -> Result<()> {
        self.db.execute("UPDATE external SET triage = ?3, size = ?4, why = ?5, plan = ?6 WHERE project = ?1 AND task = ?2", params![project, task, triage, size, why, plan])?;
        Ok(())
    }

    pub fn set_asked(&self, project: &str, task: &str, at_ms: i64) -> Result<()> {
        self.db.execute("UPDATE external SET asked_ms = ?3 WHERE project = ?1 AND task = ?2", params![project, task, at_ms])?;
        Ok(())
    }

    pub fn set_pr(&self, project: &str, task: &str, url: &str) -> Result<()> {
        self.db.execute("UPDATE external SET pr = ?3 WHERE project = ?1 AND task = ?2", params![project, task, url])?;
        Ok(())
    }

    // ---------- sessions ----------

    /// Records a session Lila started (or found) and makes it the member's current one.
    pub fn add_session(&self, project: &str, s: &SessionRow) -> Result<()> {
        self.db.execute(
            "INSERT INTO sessions (project, id, member, runtime, file, started_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (project, id) DO UPDATE SET file = COALESCE(?5, file)",
            params![project, s.id, s.member, s.runtime, s.file, s.started_ms],
        )?;
        self.db.execute("UPDATE members SET session = ?3 WHERE project = ?1 AND id = ?2", params![project, s.member, s.id])?;
        Ok(())
    }

    pub fn session(&self, project: &str, id: &str) -> Result<Option<SessionRow>> {
        Ok(self
            .db
            .query_row(
                "SELECT id, member, runtime, file, started_ms FROM sessions WHERE project = ?1 AND id = ?2",
                params![project, id],
                |r| Ok(SessionRow { id: r.get(0)?, member: r.get(1)?, runtime: r.get(2)?, file: r.get(3)?, started_ms: r.get(4)? }),
            )
            .optional()?)
    }

    /// `session` was given work from `chain`.
    pub fn touch_chain(&self, project: &str, session: &str, chain: &str, now: i64) -> Result<()> {
        self.db.execute(
            "INSERT INTO session_chains (project, session, chain, last_ms) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (project, session, chain) DO UPDATE SET last_ms = ?4",
            params![project, session, chain, now],
        )?;
        Ok(())
    }

    /// The chain `session` last had work from.
    pub fn last_chain(&self, project: &str, session: &str) -> Result<Option<String>> {
        Ok(self
            .db
            .query_row(
                "SELECT chain FROM session_chains WHERE project = ?1 AND session = ?2 ORDER BY last_ms DESC, rowid DESC LIMIT 1",
                params![project, session],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// `member`'s session that last worked on `chain`.
    pub fn session_for_chain(&self, project: &str, member: &str, chain: &str) -> Result<Option<SessionRow>> {
        let id: Option<String> = self
            .db
            .query_row(
                "SELECT c.session FROM session_chains c JOIN sessions s ON s.project = c.project AND s.id = c.session
                 WHERE c.project = ?1 AND s.member = ?2 AND c.chain = ?3 ORDER BY c.last_ms DESC, c.rowid DESC LIMIT 1",
                params![project, member, chain],
                |r| r.get(0),
            )
            .optional()?;
        match id {
            Some(id) => self.session(project, &id),
            None => Ok(None),
        }
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
            "INSERT INTO tasks (project, id, seq, title, body, state, from_actor, to_member, parent, priority, created_ms, updated_ms, quote, accept, out_of_scope, run, run_step)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                project,
                id,
                n,
                new.title,
                new.body,
                TaskState::Queued.as_str(),
                new.from.as_str(),
                new.to,
                new.parent,
                new.priority,
                now,
                new.brief.quote.as_deref().map(str::trim).filter(|q| !q.is_empty()),
                join_list(&new.brief.accept),
                join_list(&new.brief.out_of_scope),
                new.run.as_ref().map(|(a, _)| a.as_str()),
                new.run.as_ref().map(|(_, s)| *s as i64)
            ],
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

    /// What a member reported with `done` besides its summary.
    pub fn set_outcome(&self, project: &str, id: &str, o: &Outcome) -> Result<()> {
        self.db.execute(
            "UPDATE tasks SET decisions = ?3, unverified = ?4, verdict = ?5 WHERE project = ?1 AND id = ?2",
            params![project, id.to_ascii_uppercase(), join_list(&o.decisions), join_list(&o.unverified), o.passed.map(|p| if p { "pass" } else { "fail" })],
        )?;
        Ok(())
    }

    // ---------- flow runs ----------

    /// Starts a run at `anchor`, which becomes its step 0.
    pub fn start_run(&self, project: &str, anchor: &str, flow: &str, budget: Option<u64>, now: i64) -> Result<Run> {
        self.db.execute(
            "INSERT INTO runs (project, anchor, flow, state, created_ms, updated_ms, budget) VALUES (?1, ?2, ?3, 'running', ?4, ?4, ?5)",
            params![project, anchor, flow, now, budget.map(|b| b as i64)],
        )?;
        self.db.execute("UPDATE tasks SET run = ?2, run_step = 0 WHERE project = ?1 AND id = ?2", params![project, anchor])?;
        Ok(self.run(project, anchor)?.expect("just inserted"))
    }

    fn run_from(r: &Row) -> rusqlite::Result<Run> {
        Ok(Run {
            anchor: r.get(0)?,
            flow: r.get(1)?,
            step: r.get::<_, i64>(2)? as usize,
            round: r.get::<_, i64>(3)? as u32,
            state: RunState::parse(&r.get::<_, String>(4)?),
            updated_ms: r.get(5)?,
            budget: r.get::<_, Option<i64>>(6)?.map(|b| b as u64),
            hold: r.get(7)?,
        })
    }

    pub fn run(&self, project: &str, anchor: &str) -> Result<Option<Run>> {
        Ok(self
            .db
            .query_row(
                "SELECT anchor, flow, step, round, state, updated_ms, budget, hold FROM runs WHERE project = ?1 AND anchor = ?2",
                params![project, anchor],
                Self::run_from,
            )
            .optional()?)
    }

    /// Every run of a project, oldest first.
    pub fn runs(&self, project: &str) -> Result<Vec<Run>> {
        let mut st = self.db.prepare("SELECT anchor, flow, step, round, state, updated_ms, budget, hold FROM runs WHERE project = ?1 ORDER BY created_ms, rowid")?;
        Ok(st.query_map([project], Self::run_from)?.collect::<rusqlite::Result<_>>()?)
    }

    /// A run's budget and what it holds for (None: not held).
    pub fn set_run_hold(&self, project: &str, anchor: &str, budget: Option<u64>, hold: Option<&str>) -> Result<()> {
        self.db.execute(
            "UPDATE runs SET budget = ?3, hold = ?4 WHERE project = ?1 AND anchor = ?2",
            params![project, anchor, budget.map(|b| b as i64), hold],
        )?;
        Ok(())
    }

    pub fn set_run(&self, project: &str, anchor: &str, step: usize, round: u32, state: RunState, now: i64) -> Result<()> {
        self.db.execute(
            "UPDATE runs SET step = ?3, round = ?4, state = ?5, updated_ms = ?6 WHERE project = ?1 AND anchor = ?2",
            params![project, anchor, step as i64, round as i64, state.as_str(), now],
        )?;
        Ok(())
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
        NewTask {
            title: "t".into(),
            body: "b".into(),
            from: Actor::Member("owner".into()),
            to: to.into(),
            parent: parent.map(Into::into),
            priority: 0,
            brief: Brief::default(),
            run: None,
        }
    }

    #[test]
    fn a_brief_and_an_outcome_round_trip() {
        let mut s = store();
        let mut n = new("engineer", None);
        n.brief = Brief { quote: Some("  加登录 ".into()), accept: vec!["401 on a wrong password".into(), " ".into()], out_of_scope: vec!["sign-up".into()] };
        let t = s.create_task("shop", n, 1).unwrap();
        assert_eq!(t.brief, Brief { quote: Some("加登录".into()), accept: vec!["401 on a wrong password".into()], out_of_scope: vec!["sign-up".into()] });
        assert!(t.outcome.is_empty());
        let o = Outcome { decisions: vec!["24h tokens".into()], unverified: vec!["load".into(), "real DB".into()], passed: Some(false) };
        s.set_outcome("shop", "t1", &o).unwrap();
        assert_eq!(s.task("shop", "T1").unwrap().unwrap().outcome, o);
        assert!(s.create_task("shop", new("engineer", None), 2).unwrap().brief.is_empty());
    }

    #[test]
    fn runs_track_their_step_round_and_state() {
        let mut s = store();
        s.create_task("shop", new("engineer", None), 1).unwrap();
        let run = s.start_run("shop", "T1", "code", Some(300_000), 2).unwrap();
        assert_eq!((run.step, run.round, run.state), (0, 0, RunState::Running));
        assert_eq!(s.task("shop", "T1").unwrap().unwrap().run, Some(("T1".into(), 0)));
        let mut step = new("tester", Some("T1"));
        step.run = Some(("T1".into(), 2));
        assert_eq!(s.create_task("shop", step, 3).unwrap().run, Some(("T1".into(), 2)));
        s.set_run("shop", "T1", 2, 1, RunState::Checking, 4).unwrap();
        let run = s.run("shop", "T1").unwrap().unwrap();
        assert_eq!((run.step, run.round, run.state, run.updated_ms), (2, 1, RunState::Checking, 4));
        assert!(run.state.is_open() && !RunState::Failed.is_open());
        assert_eq!(s.runs("shop").unwrap().len(), 1);
        assert!(s.run("shop", "T2").unwrap().is_none());
        assert_eq!(run.budget, Some(300_000));
        s.set_run_hold("shop", "T1", Some(600_000), Some("{\"at\":1}")).unwrap();
        let run = s.run("shop", "T1").unwrap().unwrap();
        assert_eq!((run.budget, run.hold.as_deref()), (Some(600_000), Some("{\"at\":1}")));
        assert!(RunState::Held.is_open());
    }

    #[test]
    fn the_session_ledger_knows_which_session_did_which_chain() {
        let s = store();
        s.set_member("shop", "engineer", "engineer", Some("t1"), Some(1)).unwrap();
        let row = |id: &str| SessionRow { id: id.into(), member: "engineer".into(), runtime: "claude".into(), file: Some(format!("/c/{id}.jsonl")), started_ms: 1 };
        s.add_session("shop", &row("s1")).unwrap();
        assert_eq!(s.members("shop").unwrap()[0].session.as_deref(), Some("s1"));
        s.touch_chain("shop", "s1", "T1", 10).unwrap();
        s.touch_chain("shop", "s1", "T5", 20).unwrap();
        assert_eq!(s.last_chain("shop", "s1").unwrap().as_deref(), Some("T5"));
        s.add_session("shop", &row("s2")).unwrap();
        s.touch_chain("shop", "s2", "T9", 30).unwrap();
        s.touch_chain("shop", "s1", "T1", 40).unwrap();
        assert_eq!(s.last_chain("shop", "s1").unwrap().as_deref(), Some("T1"), "touched again");
        assert_eq!(s.session_for_chain("shop", "engineer", "T5").unwrap().unwrap().id, "s1");
        assert_eq!(s.session_for_chain("shop", "engineer", "T9").unwrap().unwrap().file.as_deref(), Some("/c/s2.jsonl"));
        assert!(s.session_for_chain("shop", "tester", "T9").unwrap().is_none());
        // Found again (codex): the file fills in, nothing else changes.
        s.add_session("shop", &SessionRow { file: None, ..row("s2") }).unwrap();
        assert_eq!(s.session("shop", "s2").unwrap().unwrap().file.as_deref(), Some("/c/s2.jsonl"));
    }

    #[test]
    fn a_removed_project_leaves_nothing_behind() {
        let mut s = store();
        s.upsert_project("temp", "/repo/temp", 1).unwrap();
        for p in ["shop", "temp"] {
            s.create_task(p, new("eng", None), 1).unwrap();
            s.event(p, Some("T1"), "x", "made", "", 1).unwrap();
            s.add_inbox(p, "report", None, Some("T1"), "hi", &[], 1).unwrap();
        }
        s.remove_project("shop").unwrap();
        assert!(s.project("shop").unwrap().is_none());
        assert!(s.tasks("shop").unwrap().is_empty() && s.inbox("shop", true).unwrap().is_empty() && s.events("shop", None, 10).unwrap().is_empty());
        assert_eq!((s.tasks("temp").unwrap().len(), s.inbox("temp", true).unwrap().len()), (1, 1), "other projects stay");
    }

    #[test]
    fn old_databases_get_the_new_columns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lila.db");
        {
            // SCHEMA is still the first release's; the newer columns come from ADDED_COLUMNS.
            let db = Connection::open(&path).unwrap();
            db.execute_batch(SCHEMA).unwrap();
            db.execute("INSERT INTO projects (id, cwd, created_ms) VALUES ('shop', '/r', 0)", []).unwrap();
            db.execute(
                "INSERT INTO tasks (project, id, seq, title, body, state, from_actor, to_member, created_ms, updated_ms)
                 VALUES ('shop', 'T1', 1, 'old', '', 'queued', 'human', 'pm', 0, 0)",
                [],
            )
            .unwrap();
        }
        let s = Store::open(&path).unwrap();
        let t = s.task("shop", "T1").unwrap().unwrap();
        assert_eq!(t.title, "old");
        assert!(t.brief.is_empty() && t.outcome.is_empty());
        drop(s);
        Store::open(&path).unwrap();
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
