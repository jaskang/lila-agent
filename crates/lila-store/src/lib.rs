//! SQLite persistence. Only the daemon writes. Projects hold threads; a thread
//! holds the conversation items omp produced and the words waiting to be delivered.

use anyhow::{Context, Result, bail};
use lila_core::{Thread, Tier};
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::path::Path;

/// Bumped when the schema changes. A database from before threads (v3) has `user_version` 0
/// and tables of tasks and members: it is moved aside and a new one starts empty. Later changes
/// only add columns (`ADDED_COLUMNS`), which `migrate` adds to a table that lacks them.
/// 5: threads got `worktree`, `base_branch`, `base_commit`, `merged_ms`.
/// 6: the `checkpoints` table, and `outbox.hold` (a note that goes out only with a message).
/// 7: the `thread_rules` table (what a person allowed "always" for a thread).
/// 8: omp is the only engine: `threads.engine` is gone; a thread that ran on another (Claude Code, Codex) keeps its name in `legacy`, ended.
const VERSION: i64 = 8;

/// Columns added to a table after it first existed: an older database gets them by `ALTER TABLE`.
const ADDED_COLUMNS: [(&str, &str, &str); 6] = [
    ("threads", "legacy", "TEXT"),
    ("threads", "worktree", "INTEGER NOT NULL DEFAULT 0"),
    ("threads", "base_branch", "TEXT"),
    ("threads", "base_commit", "TEXT"),
    ("threads", "merged_ms", "INTEGER"),
    ("outbox", "hold", "INTEGER NOT NULL DEFAULT 0"),
];

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    cwd TEXT NOT NULL UNIQUE,
    created_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS threads (
    id TEXT PRIMARY KEY,
    project TEXT NOT NULL,
    title TEXT NOT NULL,
    legacy TEXT,
    model TEXT NOT NULL,
    effort TEXT,
    tier TEXT NOT NULL,
    cwd TEXT NOT NULL,
    session TEXT,
    terminal TEXT,
    created_ms INTEGER NOT NULL,
    active_ms INTEGER NOT NULL,
    started_ms INTEGER,
    ended_ms INTEGER,
    archived INTEGER NOT NULL DEFAULT 0,
    worktree INTEGER NOT NULL DEFAULT 0,
    base_branch TEXT,
    base_commit TEXT,
    merged_ms INTEGER
);
CREATE INDEX IF NOT EXISTS threads_project ON threads (project, active_ms);
CREATE TABLE IF NOT EXISTS conv_items (
    thread TEXT NOT NULL,
    item TEXT NOT NULL,
    seq INTEGER NOT NULL,
    at_ms INTEGER NOT NULL,
    body TEXT NOT NULL,
    PRIMARY KEY (thread, item)
);
CREATE INDEX IF NOT EXISTS conv_items_seq ON conv_items (thread, seq);
CREATE TABLE IF NOT EXISTS outbox (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    thread TEXT NOT NULL,
    text TEXT NOT NULL,
    attachments TEXT NOT NULL DEFAULT '[]',
    created_ms INTEGER NOT NULL,
    hold INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS thread_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    thread TEXT NOT NULL,
    at_ms INTEGER NOT NULL,
    actor TEXT NOT NULL,
    kind TEXT NOT NULL,
    detail TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS thread_events_thread ON thread_events (thread, id);
CREATE TABLE IF NOT EXISTS checkpoints (
    thread TEXT NOT NULL,
    n INTEGER NOT NULL,
    kind TEXT NOT NULL,
    at_ms INTEGER NOT NULL,
    sha TEXT NOT NULL,
    tree TEXT NOT NULL,
    head TEXT,
    branch TEXT,
    text TEXT NOT NULL DEFAULT '',
    item TEXT,
    PRIMARY KEY (thread, n)
);
CREATE TABLE IF NOT EXISTS thread_rules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    thread TEXT NOT NULL,
    kind TEXT NOT NULL,
    key TEXT NOT NULL,
    created_ms INTEGER NOT NULL,
    UNIQUE (thread, kind, key)
);
"#;

pub struct Store {
    db: Connection,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectRow {
    pub id: String,
    pub cwd: String,
    pub created_ms: i64,
}

/// What a new thread needs; its id comes from the caller.
#[derive(Debug, Clone)]
pub struct NewThread {
    pub id: String,
    pub project: String,
    pub title: String,
    pub model: String,
    pub effort: Option<String>,
    pub tier: Tier,
    pub cwd: String,
    /// The thread works in a worktree of its own: the branch the project folder was on and its commit then.
    pub worktree: Option<Base>,
}

/// Where a worktree thread started: what its work is merged back onto.
#[derive(Debug, Clone, PartialEq)]
pub struct Base {
    pub branch: String,
    pub commit: String,
}

/// Words waiting for a thread that cannot take them yet (still starting).
#[derive(Debug, Clone, PartialEq)]
pub struct OutMessage {
    pub id: i64,
    pub thread: String,
    pub text: String,
    /// The `lila_agent::Attachment`s, as JSON.
    pub attachments: String,
    /// A note for the agent that goes out with the next message, never by itself.
    pub hold: bool,
}

/// What a checkpoint was taken for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointKind {
    /// Before a turn began.
    Turn,
    /// Before files were put back to an earlier checkpoint: what that restore is undone by.
    Restore,
}

impl CheckpointKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CheckpointKind::Turn => "turn",
            CheckpointKind::Restore => "restore",
        }
    }
}

/// A snapshot of a thread's folder, kept alive by a ref in its repository (`lila::checkpoint`).
#[derive(Debug, Clone, PartialEq)]
pub struct Checkpoint {
    pub thread: String,
    /// The thread's own count, from 1; never used twice.
    pub n: i64,
    pub kind: CheckpointKind,
    pub at_ms: i64,
    /// The snapshot's commit, and its tree.
    pub sha: String,
    pub tree: String,
    /// The folder's HEAD and branch then (none: no commit yet, detached HEAD).
    pub head: Option<String>,
    pub branch: Option<String>,
    /// The words of the message the turn began with (for a restore: what it was done to).
    pub text: String,
    /// The conversation item of that message: none until it is found, empty once it is known to have none.
    pub item: Option<String>,
}

impl Checkpoint {
    /// The item this checkpoint is attached to, once found.
    pub fn matched_item(&self) -> Option<&str> {
        self.item.as_deref().filter(|i| !i.is_empty())
    }
}

/// What a person allowed "always" for a thread (`lila::rules`): kept with the thread, so it survives a restart and a resume.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleRow {
    pub id: i64,
    pub thread: String,
    pub kind: String,
    pub key: String,
    pub created_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub at_ms: i64,
    pub thread: String,
    pub actor: String,
    pub kind: String,
    pub detail: String,
}

fn open_db(path: &Path) -> Result<Connection> {
    let db = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
    db.pragma_update(None, "journal_mode", "WAL")?;
    db.pragma_update(None, "busy_timeout", 5000)?;
    Ok(db)
}

fn migrate(db: &Connection) -> Result<()> {
    db.execute_batch(SCHEMA)?;
    // A table made by an earlier version has fewer columns: add what is missing, whatever the version said.
    for (table, name, ddl) in ADDED_COLUMNS {
        let have: Vec<String> = db.prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        if !have.iter().any(|c| c == name) {
            db.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {name} {ddl}"))?;
        }
    }
    // A database from when threads chose an engine: the ones that did not run on omp are ended and cannot be resumed.
    let have: Vec<String> = db.prepare("SELECT name FROM pragma_table_info('threads')")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    if have.iter().any(|c| c == "engine") {
        db.execute_batch(
            "BEGIN;
             UPDATE threads SET legacy = engine WHERE engine <> 'omp' AND legacy IS NULL;
             UPDATE threads SET terminal = NULL, ended_ms = COALESCE(ended_ms, active_ms) WHERE legacy IS NOT NULL AND terminal IS NOT NULL;
             ALTER TABLE threads DROP COLUMN engine;
             COMMIT;",
        )?;
    }
    db.pragma_update(None, "user_version", VERSION)?;
    Ok(())
}

/// A database made before threads: no version, but tables of tasks.
fn is_v3(db: &Connection) -> Result<bool> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let tasks: i64 = db.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'tasks'", [], |r| r.get(0))?;
    Ok(version == 0 && tasks > 0)
}

fn thread_from(r: &Row) -> rusqlite::Result<Thread> {
    let tier: String = r.get(6)?;
    Ok(Thread {
        id: r.get(0)?,
        project: r.get(1)?,
        title: r.get(2)?,
        legacy: r.get(3)?,
        model: r.get(4)?,
        effort: r.get(5)?,
        tier: Tier::parse(&tier).unwrap_or_default(),
        cwd: r.get(7)?,
        session: r.get(8)?,
        terminal: r.get(9)?,
        created_ms: r.get(10)?,
        active_ms: r.get(11)?,
        started_ms: r.get(12)?,
        ended_ms: r.get(13)?,
        archived: r.get::<_, i64>(14)? != 0,
        worktree: r.get::<_, i64>(15)? != 0,
        base_branch: r.get(16)?,
        base_commit: r.get(17)?,
        merged_ms: r.get(18)?,
    })
}

const THREAD_COLS: &str = "id, project, title, legacy, model, effort, tier, cwd, session, terminal, created_ms, active_ms, started_ms, ended_ms, archived, worktree, base_branch, base_commit, merged_ms";

impl Store {
    /// Opens (making it if needed) the database. A v3 one is moved to `<name>.v3.bak` first: its
    /// tasks and members mean nothing to threads, and the new database starts empty.
    pub fn open(path: &Path) -> Result<Store> {
        let mut db = open_db(path)?;
        if is_v3(&db)? {
            drop(db);
            let mut bak = path.as_os_str().to_owned();
            bak.push(".v3.bak");
            let bak = std::path::PathBuf::from(bak);
            if bak.exists() {
                // An earlier v3 file is kept: this one is not older than it by anything we know, keep both.
                let mut n = 2;
                while std::path::PathBuf::from(format!("{}.{n}", bak.display())).exists() {
                    n += 1;
                }
                std::fs::rename(path, format!("{}.{n}", bak.display()))?;
            } else {
                std::fs::rename(path, &bak)?;
            }
            for ext in ["-wal", "-shm"] {
                let mut side = path.as_os_str().to_owned();
                side.push(ext);
                let _ = std::fs::remove_file(side);
            }
            db = open_db(path)?;
        }
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
        Ok(ProjectRow { id: r.get(0)?, cwd: r.get(1)?, created_ms: r.get(2)? })
    }

    pub fn project(&self, id: &str) -> Result<Option<ProjectRow>> {
        Ok(self.db.query_row("SELECT id, cwd, created_ms FROM projects WHERE id = ?1", [id], Self::project_from).optional()?)
    }

    pub fn project_by_cwd(&self, cwd: &str) -> Result<Option<ProjectRow>> {
        Ok(self.db.query_row("SELECT id, cwd, created_ms FROM projects WHERE cwd = ?1", [cwd], Self::project_from).optional()?)
    }

    pub fn projects(&self) -> Result<Vec<ProjectRow>> {
        let mut st = self.db.prepare("SELECT id, cwd, created_ms FROM projects ORDER BY id")?;
        Ok(st.query_map([], Self::project_from)?.collect::<rusqlite::Result<_>>()?)
    }

    /// Forgets a project and everything Lila recorded for it. Its folder is not touched.
    pub fn remove_project(&self, project: &str) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        for table in ["conv_items", "outbox", "thread_events", "checkpoints", "thread_rules"] {
            tx.execute(&format!("DELETE FROM {table} WHERE thread IN (SELECT id FROM threads WHERE project = ?1)"), [project])?;
        }
        tx.execute("DELETE FROM threads WHERE project = ?1", [project])?;
        tx.execute("DELETE FROM projects WHERE id = ?1", [project])?;
        tx.commit()?;
        Ok(())
    }

    // ---------- threads ----------

    pub fn create_thread(&self, new: NewThread, now: i64) -> Result<Thread> {
        self.db.execute(
            "INSERT INTO threads (id, project, title, model, effort, tier, cwd, created_ms, active_ms, worktree, base_branch, base_commit) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9, ?10, ?11)",
            params![new.id, new.project, new.title, new.model, new.effort, new.tier.as_str(), new.cwd, now, new.worktree.is_some(), new.worktree.as_ref().map(|b| &b.branch), new.worktree.as_ref().map(|b| &b.commit)],
        )?;
        Ok(self.thread(&new.id)?.expect("just inserted"))
    }

    pub fn thread(&self, id: &str) -> Result<Option<Thread>> {
        Ok(self.db.query_row(&format!("SELECT {THREAD_COLS} FROM threads WHERE id = ?1"), [id], thread_from).optional()?)
    }

    /// A project's threads, the most recently active first. Archived ones only on request.
    pub fn threads(&self, project: &str, archived: bool) -> Result<Vec<Thread>> {
        let sql = format!("SELECT {THREAD_COLS} FROM threads WHERE project = ?1 AND (?2 OR archived = 0) ORDER BY active_ms DESC, id");
        let mut st = self.db.prepare(&sql)?;
        Ok(st.query_map(params![project, archived], thread_from)?.collect::<rusqlite::Result<_>>()?)
    }

    /// Every thread that was started and not ended: what lilad finds its terminals for after a restart.
    pub fn live_threads(&self) -> Result<Vec<Thread>> {
        let sql = format!("SELECT {THREAD_COLS} FROM threads WHERE terminal IS NOT NULL ORDER BY created_ms");
        let mut st = self.db.prepare(&sql)?;
        Ok(st.query_map([], thread_from)?.collect::<rusqlite::Result<_>>()?)
    }

    /// omp runs in `terminal` now (started at `now`); a new run clears `ended_ms`.
    pub fn set_running(&self, id: &str, terminal: &str, now: i64) -> Result<()> {
        self.db.execute("UPDATE threads SET terminal = ?2, started_ms = ?3, ended_ms = NULL WHERE id = ?1", params![id, terminal, now])?;
        Ok(())
    }

    /// Its omp process is gone.
    pub fn set_ended(&self, id: &str, now: i64) -> Result<()> {
        self.db.execute("UPDATE threads SET terminal = NULL, ended_ms = ?2 WHERE id = ?1", params![id, now])?;
        Ok(())
    }

    pub fn set_session(&self, id: &str, session: &str) -> Result<()> {
        self.db.execute("UPDATE threads SET session = ?2 WHERE id = ?1 AND session IS NOT ?2", params![id, session])?;
        Ok(())
    }

    pub fn set_title(&self, id: &str, title: &str) -> Result<()> {
        self.db.execute("UPDATE threads SET title = ?2 WHERE id = ?1", params![id, title])?;
        Ok(())
    }

    pub fn set_tier(&self, id: &str, tier: Tier) -> Result<()> {
        self.db.execute("UPDATE threads SET tier = ?2 WHERE id = ?1", params![id, tier.as_str()])?;
        Ok(())
    }

    /// What the thread runs on next time it starts (a running one keeps its own until then).
    pub fn set_model(&self, id: &str, model: &str, effort: Option<&str>) -> Result<()> {
        self.db.execute("UPDATE threads SET model = ?2, effort = ?3 WHERE id = ?1", params![id, model, effort])?;
        Ok(())
    }

    /// The thread's worktree work went into its base branch at `now`; the worktree is removed.
    pub fn set_merged(&self, id: &str, now: i64) -> Result<()> {
        self.db.execute("UPDATE threads SET merged_ms = ?2 WHERE id = ?1", params![id, now])?;
        Ok(())
    }

    pub fn set_archived(&self, id: &str, archived: bool) -> Result<()> {
        self.db.execute("UPDATE threads SET archived = ?2 WHERE id = ?1", params![id, archived])?;
        Ok(())
    }

    /// A message or reply happened at `now`.
    pub fn touch_thread(&self, id: &str, now: i64) -> Result<()> {
        self.db.execute("UPDATE threads SET active_ms = ?2 WHERE id = ?1 AND active_ms < ?2", params![id, now])?;
        Ok(())
    }

    pub fn remove_thread(&self, id: &str) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        for table in ["conv_items", "outbox", "thread_events", "checkpoints", "thread_rules"] {
            tx.execute(&format!("DELETE FROM {table} WHERE thread = ?1"), [id])?;
        }
        tx.execute("DELETE FROM threads WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(())
    }

    // ---------- conversation items ----------

    /// Stores conversation items of a thread (`(id, at_ms, body)`). Each new or changed one gets the
    /// thread's next seq, so readers ask for what came after the last one they saw; an unchanged one
    /// is left alone (read again after a restart, it changes nothing). Returns the thread's last seq.
    pub fn put_items(&self, thread: &str, items: &[(String, i64, String)]) -> Result<i64> {
        let tx = self.db.unchecked_transaction()?;
        let mut seq: i64 = tx.query_row("SELECT COALESCE(MAX(seq), 0) FROM conv_items WHERE thread = ?1", params![thread], |r| r.get(0))?;
        for (item, at_ms, body) in items {
            let old: Option<String> = tx.query_row("SELECT body FROM conv_items WHERE thread = ?1 AND item = ?2", params![thread, item], |r| r.get(0)).optional()?;
            if old.as_deref() == Some(body.as_str()) {
                continue;
            }
            seq += 1;
            tx.execute(
                "INSERT INTO conv_items (thread, item, seq, at_ms, body) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (thread, item) DO UPDATE SET seq = ?3, at_ms = ?4, body = ?5",
                params![thread, item, seq, at_ms, body],
            )?;
        }
        tx.commit()?;
        Ok(seq)
    }

    /// A thread's items with a seq after `after`, oldest first: `(seq, body)`.
    /// The latest `limit` when there are more.
    pub fn items(&self, thread: &str, after: i64, limit: usize) -> Result<Vec<(i64, String)>> {
        let mut st = self.db.prepare(
            "SELECT seq, body FROM (SELECT seq, body, at_ms FROM conv_items WHERE thread = ?1 AND seq > ?2
             ORDER BY at_ms DESC, seq DESC LIMIT ?3) ORDER BY at_ms, seq",
        )?;
        let rows = st.query_map(params![thread, after, limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The thread's last seq (0: nothing yet).
    pub fn last_seq(&self, thread: &str) -> Result<i64> {
        Ok(self.db.query_row("SELECT COALESCE(MAX(seq), 0) FROM conv_items WHERE thread = ?1", [thread], |r| r.get(0))?)
    }

    // ---------- events: what lilad did on its own (answers by the tier, starts, ends) ----------

    pub fn event(&self, thread: &str, actor: &str, kind: &str, detail: &str, now: i64) -> Result<()> {
        self.db.execute("INSERT INTO thread_events (thread, at_ms, actor, kind, detail) VALUES (?1, ?2, ?3, ?4, ?5)", params![thread, now, actor, kind, detail])?;
        Ok(())
    }

    /// The latest `limit` events of a thread, oldest first.
    pub fn events(&self, thread: &str, limit: usize) -> Result<Vec<Event>> {
        let mut st = self.db.prepare("SELECT at_ms, thread, actor, kind, detail FROM (SELECT * FROM thread_events WHERE thread = ?1 ORDER BY id DESC LIMIT ?2) ORDER BY id")?;
        let rows = st.query_map(params![thread, limit as i64], |r| Ok(Event { at_ms: r.get(0)?, thread: r.get(1)?, actor: r.get(2)?, kind: r.get(3)?, detail: r.get(4)? }))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---------- rules: what was allowed "always" ----------

    /// A thread's rules, oldest first.
    pub fn rules(&self, thread: &str) -> Result<Vec<RuleRow>> {
        let mut st = self.db.prepare("SELECT id, thread, kind, key, created_ms FROM thread_rules WHERE thread = ?1 ORDER BY id")?;
        let rows = st.query_map([thread], |r| Ok(RuleRow { id: r.get(0)?, thread: r.get(1)?, kind: r.get(2)?, key: r.get(3)?, created_ms: r.get(4)? }))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Adds a rule (one that is there already stays as it is). Returns whether it is new.
    pub fn add_rule(&self, thread: &str, kind: &str, key: &str, now: i64) -> Result<bool> {
        Ok(self.db.execute("INSERT OR IGNORE INTO thread_rules (thread, kind, key, created_ms) VALUES (?1, ?2, ?3, ?4)", params![thread, kind, key, now])? > 0)
    }

    /// Removes a rule of the thread; returns it, if there was one.
    pub fn remove_rule(&self, thread: &str, id: i64) -> Result<Option<RuleRow>> {
        let row = self.rules(thread)?.into_iter().find(|r| r.id == id);
        if row.is_some() {
            self.db.execute("DELETE FROM thread_rules WHERE thread = ?1 AND id = ?2", params![thread, id])?;
        }
        Ok(row)
    }

    /// Removes all of a thread's rules; returns them.
    pub fn clear_rules(&self, thread: &str) -> Result<Vec<RuleRow>> {
        let rows = self.rules(thread)?;
        self.db.execute("DELETE FROM thread_rules WHERE thread = ?1", [thread])?;
        Ok(rows)
    }

    // ---------- checkpoints ----------

    /// The latest number a thread's checkpoints reached (0: none yet); numbers are not used again.
    pub fn last_checkpoint(&self, thread: &str) -> Result<i64> {
        Ok(self.db.query_row("SELECT COALESCE(MAX(n), 0) FROM checkpoints WHERE thread = ?1", [thread], |r| r.get(0))?)
    }

    pub fn add_checkpoint(&self, c: &Checkpoint) -> Result<()> {
        self.db.execute(
            "INSERT INTO checkpoints (thread, n, kind, at_ms, sha, tree, head, branch, text, item) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![c.thread, c.n, c.kind.as_str(), c.at_ms, c.sha, c.tree, c.head, c.branch, c.text, c.item],
        )?;
        Ok(())
    }

    fn checkpoint_from(r: &Row) -> rusqlite::Result<Checkpoint> {
        let kind: String = r.get(2)?;
        Ok(Checkpoint {
            thread: r.get(0)?,
            n: r.get(1)?,
            kind: if kind == "restore" { CheckpointKind::Restore } else { CheckpointKind::Turn },
            at_ms: r.get(3)?,
            sha: r.get(4)?,
            tree: r.get(5)?,
            head: r.get(6)?,
            branch: r.get(7)?,
            text: r.get(8)?,
            item: r.get(9)?,
        })
    }

    const CHECKPOINT_COLS: &'static str = "thread, n, kind, at_ms, sha, tree, head, branch, text, item";

    pub fn checkpoint(&self, thread: &str, n: i64) -> Result<Option<Checkpoint>> {
        Ok(self.db.query_row(&format!("SELECT {} FROM checkpoints WHERE thread = ?1 AND n = ?2", Self::CHECKPOINT_COLS), params![thread, n], Self::checkpoint_from).optional()?)
    }

    /// A thread's checkpoints, the newest first.
    pub fn checkpoints(&self, thread: &str) -> Result<Vec<Checkpoint>> {
        let mut st = self.db.prepare(&format!("SELECT {} FROM checkpoints WHERE thread = ?1 ORDER BY n DESC", Self::CHECKPOINT_COLS))?;
        Ok(st.query_map([thread], Self::checkpoint_from)?.collect::<rusqlite::Result<_>>()?)
    }

    /// The checkpoint found its item (empty: its message never became one).
    pub fn set_checkpoint_item(&self, thread: &str, n: i64, item: &str) -> Result<()> {
        self.db.execute("UPDATE checkpoints SET item = ?3 WHERE thread = ?1 AND n = ?2", params![thread, n, item])?;
        Ok(())
    }

    /// Forgets the checkpoints numbered `n` or lower (their refs are deleted by the caller).
    pub fn remove_checkpoints_up_to(&self, thread: &str, n: i64) -> Result<()> {
        self.db.execute("DELETE FROM checkpoints WHERE thread = ?1 AND n <= ?2", params![thread, n])?;
        Ok(())
    }

    pub fn remove_checkpoints(&self, thread: &str) -> Result<()> {
        self.db.execute("DELETE FROM checkpoints WHERE thread = ?1", [thread])?;
        Ok(())
    }

    /// The thread's user-message-like items from `since_ms` on, oldest first: `(item id, at_ms, body)`.
    pub fn items_since(&self, thread: &str, since_ms: i64) -> Result<Vec<(String, i64, String)>> {
        let mut st = self.db.prepare("SELECT item, at_ms, body FROM conv_items WHERE thread = ?1 AND at_ms >= ?2 AND body LIKE '%\"kind\":\"user_message\"%' ORDER BY at_ms, seq")?;
        Ok(st.query_map(params![thread, since_ms], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?)
    }

    // ---------- outbox: words waiting for their thread to accept input ----------

    pub fn push_outbox(&self, thread: &str, text: &str, attachments: &str, now: i64) -> Result<()> {
        self.db.execute("INSERT INTO outbox (thread, text, attachments, created_ms) VALUES (?1, ?2, ?3, ?4)", params![thread, text, attachments, now])?;
        Ok(())
    }

    /// A note for the agent that waits for the person's next message and goes out with it (never starts a turn by itself).
    pub fn push_note(&self, thread: &str, text: &str, now: i64) -> Result<()> {
        self.db.execute("INSERT INTO outbox (thread, text, attachments, created_ms, hold) VALUES (?1, ?2, '[]', ?3, 1)", params![thread, text, now])?;
        Ok(())
    }

    pub fn outbox(&self, thread: &str) -> Result<Vec<OutMessage>> {
        let mut st = self.db.prepare("SELECT id, thread, text, attachments, hold FROM outbox WHERE thread = ?1 ORDER BY id")?;
        Ok(st.query_map([thread], |r| Ok(OutMessage { id: r.get(0)?, thread: r.get(1)?, text: r.get(2)?, attachments: r.get(3)?, hold: r.get::<_, i64>(4)? != 0 }))?.collect::<rusqlite::Result<_>>()?)
    }

    /// Every thread that has words waiting (a held note alone is not words).
    pub fn outbox_threads(&self) -> Result<Vec<String>> {
        let mut st = self.db.prepare("SELECT DISTINCT thread FROM outbox WHERE hold = 0 ORDER BY thread")?;
        Ok(st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
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

    fn new(id: &str) -> NewThread {
        NewThread { id: id.into(), project: "shop".into(), title: "first".into(), model: "gpt".into(), effort: Some("low".into()), tier: Tier::Full, cwd: "/repo/shop".into(), worktree: None }
    }

    #[test]
    fn a_project_has_one_folder_and_one_id() {
        let s = store();
        s.upsert_project("shop", "/repo/shop", 2).unwrap();
        assert!(s.upsert_project("other", "/repo/shop", 2).is_err());
        assert!(s.upsert_project("shop", "/elsewhere", 2).is_err());
        assert_eq!(s.projects().unwrap().len(), 1);
    }

    #[test]
    fn a_thread_lives_from_start_to_end_and_back() {
        let s = store();
        let t = s.create_thread(new("t00000001"), 10).unwrap();
        assert_eq!((t.legacy.clone(), t.tier, t.terminal.clone(), t.session.clone()), (None, Tier::Full, None, None));
        s.set_running(&t.id, "t5", 20).unwrap();
        s.set_session(&t.id, "sess-1").unwrap();
        let t = s.thread("t00000001").unwrap().unwrap();
        assert_eq!((t.terminal.as_deref(), t.session.as_deref(), t.started_ms, t.ended_ms), (Some("t5"), Some("sess-1"), Some(20), None));
        assert_eq!(s.live_threads().unwrap().len(), 1);
        s.set_ended(&t.id, 30).unwrap();
        assert!(s.live_threads().unwrap().is_empty());
        let t = s.thread("t00000001").unwrap().unwrap();
        assert_eq!((t.terminal, t.ended_ms, t.session.as_deref()), (None, Some(30), Some("sess-1")), "the session stays: it resumes");
        s.set_running(&t.id, "t6", 40).unwrap();
        assert_eq!(s.thread("t00000001").unwrap().unwrap().ended_ms, None, "a new run is not ended");
    }

    #[test]
    fn threads_list_by_activity_and_archived_ones_hide() {
        let s = store();
        for (i, id) in ["t00000001", "t00000002", "t00000003"].iter().enumerate() {
            s.create_thread(new(id), 10 + i as i64).unwrap();
        }
        s.touch_thread("t00000001", 100).unwrap();
        s.touch_thread("t00000001", 50).unwrap();
        s.set_archived("t00000002", true).unwrap();
        let ids = |archived| s.threads("shop", archived).unwrap().into_iter().map(|t| t.id).collect::<Vec<_>>();
        assert_eq!(ids(false), ["t00000001", "t00000003"]);
        assert_eq!(ids(true).len(), 3);
        s.set_tier("t00000003", Tier::ReadOnly).unwrap();
        s.set_model("t00000003", "gpt-x", None).unwrap();
        let t = s.thread("t00000003").unwrap().unwrap();
        assert_eq!((t.tier, t.model.as_str(), t.effort), (Tier::ReadOnly, "gpt-x", None));
    }

    #[test]
    fn items_get_a_seq_per_thread_and_unchanged_ones_keep_theirs() {
        let s = store();
        s.create_thread(new("t00000001"), 1).unwrap();
        s.create_thread(new("t00000002"), 1).unwrap();
        let row = |i: &str, at, body: &str| (i.to_owned(), at, body.to_owned());
        assert_eq!(s.put_items("t00000001", &[row("a", 1, "x"), row("b", 2, "y")]).unwrap(), 2);
        assert_eq!(s.put_items("t00000001", &[row("a", 1, "x"), row("b", 2, "y2")]).unwrap(), 3, "only the changed one moves");
        assert_eq!(s.put_items("t00000002", &[row("a", 1, "z")]).unwrap(), 1, "each thread counts its own");
        let after = s.items("t00000001", 2, 10).unwrap();
        assert_eq!(after, [(3, "y2".to_owned())]);
        assert_eq!(s.items("t00000001", 0, 1).unwrap().len(), 1, "the latest when there are many");
        assert_eq!(s.last_seq("t00000001").unwrap(), 3);
    }

    #[test]
    fn removing_a_project_takes_its_threads_and_what_hangs_on_them() {
        let s = store();
        s.create_thread(new("t00000001"), 1).unwrap();
        s.put_items("t00000001", &[("a".into(), 1, "x".into())]).unwrap();
        s.push_outbox("t00000001", "hi", "[]", 1).unwrap();
        s.event("t00000001", "lilad", "started", "", 1).unwrap();
        s.remove_project("shop").unwrap();
        assert!(s.thread("t00000001").unwrap().is_none() && s.items("t00000001", 0, 9).unwrap().is_empty() && s.outbox("t00000001").unwrap().is_empty() && s.events("t00000001", 9).unwrap().is_empty());
        assert!(s.projects().unwrap().is_empty());
    }

    #[test]
    fn a_worktree_thread_remembers_where_it_started_and_when_it_was_merged() {
        let s = store();
        let t = s.create_thread(NewThread { worktree: Some(Base { branch: "main".into(), commit: "abc123".into() }), cwd: "/data/worktrees/shop/t00000001".into(), ..new("t00000001") }, 1).unwrap();
        assert_eq!((t.worktree, t.base_branch.as_deref(), t.base_commit.as_deref(), t.merged_ms), (true, Some("main"), Some("abc123"), None));
        s.set_merged("t00000001", 50).unwrap();
        assert_eq!(s.thread("t00000001").unwrap().unwrap().merged_ms, Some(50));
        let plain = s.create_thread(new("t00000002"), 1).unwrap();
        assert_eq!((plain.worktree, plain.base_branch, plain.base_commit), (false, None, None));
    }

    #[test]
    fn a_version_4_database_gains_the_worktree_columns_and_loses_the_engines_that_are_gone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lila.db");
        {
            let old = Connection::open(&path).unwrap();
            old.execute_batch(
                "CREATE TABLE projects (id TEXT PRIMARY KEY, cwd TEXT NOT NULL UNIQUE, created_ms INTEGER NOT NULL);
                 CREATE TABLE threads (id TEXT PRIMARY KEY, project TEXT NOT NULL, title TEXT NOT NULL, engine TEXT NOT NULL, model TEXT NOT NULL, effort TEXT, tier TEXT NOT NULL,
                    cwd TEXT NOT NULL, session TEXT, terminal TEXT, created_ms INTEGER NOT NULL, active_ms INTEGER NOT NULL, started_ms INTEGER, ended_ms INTEGER, archived INTEGER NOT NULL DEFAULT 0);
                 INSERT INTO projects VALUES ('shop', '/repo/shop', 1);
                 INSERT INTO threads (id, project, title, engine, model, tier, cwd, terminal, created_ms, active_ms) VALUES ('t00000001', 'shop', 'old', 'claude', 'm', 'full', '/repo/shop', 'term-1', 1, 7);
                 INSERT INTO threads (id, project, title, engine, model, tier, cwd, terminal, created_ms, active_ms) VALUES ('t00000003', 'shop', 'codex', 'codex', 'm', 'full', '/repo/shop', NULL, 1, 1);
                 INSERT INTO threads (id, project, title, engine, model, tier, cwd, terminal, created_ms, active_ms) VALUES ('t00000004', 'shop', 'mine', 'omp', 'm', 'full', '/repo/shop', 'term-4', 1, 1);
                 PRAGMA user_version = 4;",
            )
            .unwrap();
        }
        let s = Store::open(&path).unwrap();
        let t = s.thread("t00000001").unwrap().unwrap();
        assert_eq!((t.title.as_str(), t.worktree, t.base_branch, t.merged_ms), ("old", false, None, None), "the old thread is as it was");
        // Claude Code's and Codex's threads are ended for good (their terminals are not theirs to keep); omp's go on.
        assert_eq!((t.legacy.as_deref(), t.terminal, t.ended_ms), (Some("claude"), None, Some(7)));
        assert_eq!(s.thread("t00000003").unwrap().unwrap().legacy.as_deref(), Some("codex"));
        let omp = s.thread("t00000004").unwrap().unwrap();
        assert_eq!((omp.legacy, omp.terminal.as_deref(), omp.ended_ms), (None, Some("term-4"), None));
        s.create_thread(NewThread { worktree: Some(Base { branch: "main".into(), commit: "c".into() }), ..new("t00000002") }, 2).unwrap();
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.thread("t00000002").unwrap().unwrap().base_branch.as_deref(), Some("main"), "opening it again changes nothing");
        assert_eq!(s.db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0)).unwrap(), VERSION);
    }

    fn checkpoint(n: i64) -> Checkpoint {
        Checkpoint { thread: "t00000001".into(), n, kind: CheckpointKind::Turn, at_ms: n * 10, sha: format!("sha{n}"), tree: format!("tree{n}"), head: Some("h".into()), branch: Some("main".into()), text: format!("turn {n}"), item: None }
    }

    #[test]
    fn checkpoints_are_numbered_per_thread_found_by_item_and_dropped_with_the_thread() {
        let s = store();
        s.create_thread(new("t00000001"), 1).unwrap();
        assert_eq!(s.last_checkpoint("t00000001").unwrap(), 0);
        for n in 1..=4 {
            s.add_checkpoint(&checkpoint(n)).unwrap();
        }
        assert!(s.add_checkpoint(&checkpoint(2)).is_err(), "a number is used once");
        assert_eq!(s.checkpoints("t00000001").unwrap().iter().map(|c| c.n).collect::<Vec<_>>(), [4, 3, 2, 1], "newest first");
        s.set_checkpoint_item("t00000001", 2, "u2").unwrap();
        s.set_checkpoint_item("t00000001", 3, "").unwrap();
        let (two, three) = (s.checkpoint("t00000001", 2).unwrap().unwrap(), s.checkpoint("t00000001", 3).unwrap().unwrap());
        assert_eq!((two.matched_item(), three.item.as_deref(), three.matched_item()), (Some("u2"), Some(""), None));
        // Dropping the old ones keeps the numbers going up.
        s.remove_checkpoints_up_to("t00000001", 2).unwrap();
        assert_eq!(s.checkpoints("t00000001").unwrap().len(), 2);
        s.remove_checkpoints_up_to("t00000001", 9).unwrap();
        assert_eq!(s.last_checkpoint("t00000001").unwrap(), 0);
        // The user messages of a thread, from a time on.
        let row = |i: &str, at, body: &str| (i.to_owned(), at, body.to_owned());
        s.put_items("t00000001", &[row("a", 5, r#"{"id":"a","kind":"user_message","text":"x"}"#), row("b", 9, r#"{"id":"b","kind":"agent_message","text":"y"}"#), row("c", 12, r#"{"id":"c","kind":"user_message","text":"z"}"#)]).unwrap();
        assert_eq!(s.items_since("t00000001", 6).unwrap().iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["c"]);
        s.add_checkpoint(&checkpoint(1)).unwrap();
        s.remove_thread("t00000001").unwrap();
        assert!(s.checkpoints("t00000001").unwrap().is_empty());
    }

    #[test]
    fn rules_belong_to_a_thread_are_kept_once_and_go_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lila.db");
        {
            let s = Store::open(&path).unwrap();
            s.upsert_project("shop", "/repo/shop", 1).unwrap();
            s.create_thread(new("t00000001"), 1).unwrap();
            s.create_thread(new("t00000002"), 1).unwrap();
            assert!(s.add_rule("t00000001", "command", "cargo test", 10).unwrap());
            assert!(!s.add_rule("t00000001", "command", "cargo test", 11).unwrap(), "once");
            assert!(s.add_rule("t00000001", "edit", "", 12).unwrap());
            assert!(s.add_rule("t00000002", "command", "cargo test", 13).unwrap(), "another thread's own");
        }
        // Kept across opening it again (lilad restarted).
        let s = Store::open(&path).unwrap();
        let rules = s.rules("t00000001").unwrap();
        assert_eq!(rules.iter().map(|r| (r.kind.as_str(), r.key.as_str(), r.created_ms)).collect::<Vec<_>>(), [("command", "cargo test", 10), ("edit", "", 12)]);
        assert!(s.remove_rule("t00000001", 999).unwrap().is_none());
        assert!(s.remove_rule("t00000002", rules[0].id).unwrap().is_none(), "not another thread's");
        assert_eq!(s.remove_rule("t00000001", rules[0].id).unwrap().map(|r| r.key), Some("cargo test".into()));
        assert_eq!(s.rules("t00000001").unwrap().len(), 1);
        assert_eq!(s.clear_rules("t00000001").unwrap().len(), 1);
        assert!(s.rules("t00000001").unwrap().is_empty() && s.rules("t00000002").unwrap().len() == 1);
        s.remove_thread("t00000002").unwrap();
        assert!(s.rules("t00000002").unwrap().is_empty());
    }

    #[test]
    fn a_version_6_database_gains_the_rules_table_and_keeps_its_threads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lila.db");
        {
            let s = Store::open(&path).unwrap();
            s.upsert_project("shop", "/repo/shop", 1).unwrap();
            s.create_thread(new("t00000001"), 1).unwrap();
            s.db.execute_batch("DROP TABLE thread_rules; PRAGMA user_version = 6;").unwrap();
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(s.thread("t00000001").unwrap().unwrap().title, "first");
        assert!(s.rules("t00000001").unwrap().is_empty());
        assert!(s.add_rule("t00000001", "command", "ls", 5).unwrap());
        assert_eq!(s.db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0)).unwrap(), VERSION);
    }

    #[test]
    fn a_held_note_is_not_words_and_a_version_5_database_gains_what_checkpoints_need() {
        let s = store();
        s.push_note("t1", "note", 1).unwrap();
        assert!(s.outbox_threads().unwrap().is_empty(), "a note alone is not delivered");
        s.push_outbox("t1", "words", "[]", 2).unwrap();
        assert_eq!(s.outbox_threads().unwrap(), ["t1"]);
        assert_eq!(s.outbox("t1").unwrap().iter().map(|m| (m.text.as_str(), m.hold)).collect::<Vec<_>>(), [("note", true), ("words", false)]);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lila.db");
        {
            let old = Connection::open(&path).unwrap();
            old.execute_batch(
                "CREATE TABLE projects (id TEXT PRIMARY KEY, cwd TEXT NOT NULL UNIQUE, created_ms INTEGER NOT NULL);
                 CREATE TABLE outbox (id INTEGER PRIMARY KEY AUTOINCREMENT, thread TEXT NOT NULL, text TEXT NOT NULL, attachments TEXT NOT NULL DEFAULT '[]', created_ms INTEGER NOT NULL);
                 INSERT INTO outbox (thread, text, created_ms) VALUES ('t1', 'waiting', 1);
                 PRAGMA user_version = 5;",
            )
            .unwrap();
        }
        let s = Store::open(&path).unwrap();
        let q = s.outbox("t1").unwrap();
        assert_eq!((q.len(), q[0].hold), (1, false), "the old words are words");
        s.add_checkpoint(&checkpoint(1)).unwrap();
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.checkpoints("t00000001").unwrap().len(), 1, "opening it again changes nothing");
        assert_eq!(s.db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0)).unwrap(), VERSION);
    }

    #[test]
    fn the_outbox_keeps_order_and_a_v3_database_is_set_aside() {
        let s = store();
        s.push_outbox("t1", "one", "[]", 1).unwrap();
        s.push_outbox("t1", "two", "[]", 2).unwrap();
        let q = s.outbox("t1").unwrap();
        assert_eq!(q.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), ["one", "two"]);
        assert_eq!(s.outbox_threads().unwrap(), ["t1"]);
        s.pop_outbox(q[0].id).unwrap();
        assert_eq!(s.outbox("t1").unwrap().len(), 1);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lila.db");
        {
            let old = Connection::open(&path).unwrap();
            old.execute_batch("CREATE TABLE tasks (id TEXT); CREATE TABLE projects (id TEXT, cwd TEXT, created_ms INTEGER, paused INTEGER); INSERT INTO tasks VALUES ('T1');").unwrap();
        }
        let s = Store::open(&path).unwrap();
        s.upsert_project("shop", "/repo", 1).unwrap();
        assert!(dir.path().join("lila.db.v3.bak").exists(), "the old tasks are not lost");
        drop(s);
        assert_eq!(Store::open(&path).unwrap().projects().unwrap().len(), 1, "a v4 database is opened as it is");
    }
}
