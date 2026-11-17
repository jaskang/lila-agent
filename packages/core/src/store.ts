import { mkdirSync } from "node:fs";
import { basename, dirname } from "node:path";
import type {
  ApprovalMode,
  Project,
  SessionEvent,
  SessionEventBody,
  SessionStatus,
} from "@lila/protocol";
import { Database } from "bun:sqlite";

const SCHEMA_VERSION = 1;

export interface SessionRecord {
  id: string;
  projectId: string;
  title: string;
  modelId?: string;
  approvalMode: ApprovalMode;
  status: SessionStatus;
  ompSessionFile?: string;
  updatedAt: string;
}

type SessionRow = {
  id: string;
  project_id: string;
  title: string;
  model_id: string | null;
  approval_mode: ApprovalMode;
  status: SessionStatus;
  omp_session_file: string | null;
  updated_at: string;
};

function toSession(row: SessionRow): SessionRecord {
  return {
    id: row.id,
    projectId: row.project_id,
    title: row.title,
    ...(row.model_id ? { modelId: row.model_id } : {}),
    approvalMode: row.approval_mode,
    status: row.status,
    ...(row.omp_session_file ? { ompSessionFile: row.omp_session_file } : {}),
    updatedAt: row.updated_at,
  };
}

const SESSION_COLUMNS = "id, project_id, title, model_id, approval_mode, status, omp_session_file, updated_at";

/** SQLite persistence for projects, sessions and the per-session event log. */
export class Store {
  readonly #db: Database;
  readonly #nextSeq = new Map<string, number>();

  constructor(path = ":memory:") {
    if (path !== ":memory:") mkdirSync(dirname(path), { recursive: true });
    this.#db = new Database(path, { strict: true });
    this.#db.run("PRAGMA journal_mode = WAL");
    this.#db.run("PRAGMA synchronous = NORMAL");
    const { user_version } = this.#db.query("PRAGMA user_version").get() as { user_version: number };
    if (user_version === 0) this.#createSchema();
    else if (user_version !== SCHEMA_VERSION) throw new Error(`Unsupported database schema version ${user_version}`);
  }

  #createSchema(): void {
    // The prototype schema had no version; keep its tables under a legacy_ prefix instead of dropping them.
    const legacy = this.#db
      .query("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'legacy_%' AND name NOT LIKE 'sqlite_%'")
      .all() as Array<{ name: string }>;
    for (const { name } of legacy) this.#db.run(`ALTER TABLE "${name}" RENAME TO "legacy_${name}"`);
    this.#db.run(`
      CREATE TABLE projects (
        id TEXT PRIMARY KEY,
        path TEXT NOT NULL UNIQUE,
        name TEXT NOT NULL,
        created_at TEXT NOT NULL
      );
      CREATE TABLE sessions (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL REFERENCES projects(id),
        title TEXT NOT NULL,
        model_id TEXT,
        approval_mode TEXT NOT NULL,
        status TEXT NOT NULL,
        omp_session_file TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
      );
      CREATE TABLE events (
        session_id TEXT NOT NULL,
        seq INTEGER NOT NULL,
        run_id TEXT,
        event_type TEXT NOT NULL,
        payload_json TEXT NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (session_id, seq)
      ) WITHOUT ROWID;
      PRAGMA user_version = ${SCHEMA_VERSION};
    `);
  }

  close(): void {
    this.#db.close();
  }

  listProjects(): Project[] {
    return this.#db.query("SELECT id, path, name FROM projects ORDER BY created_at ASC").all() as Project[];
  }

  getProject(id: string): Project | undefined {
    return (this.#db.query("SELECT id, path, name FROM projects WHERE id = ?").get(id) as Project | null) ?? undefined;
  }

  /** Adds a project for `path`, or returns the existing one. */
  addProject(path: string): Project {
    const existing = this.#db.query("SELECT id, path, name FROM projects WHERE path = ?").get(path) as Project | null;
    if (existing) return existing;
    const project = { id: crypto.randomUUID(), path, name: basename(path) || path };
    this.#db
      .query("INSERT INTO projects (id, path, name, created_at) VALUES (?, ?, ?, ?)")
      .run(project.id, project.path, project.name, new Date().toISOString());
    return project;
  }

  listSessions(): SessionRecord[] {
    return (this.#db.query(`SELECT ${SESSION_COLUMNS} FROM sessions ORDER BY updated_at DESC`).all() as SessionRow[]).map(
      toSession,
    );
  }

  getSession(id: string): SessionRecord | undefined {
    const row = this.#db.query(`SELECT ${SESSION_COLUMNS} FROM sessions WHERE id = ?`).get(id) as SessionRow | null;
    return row ? toSession(row) : undefined;
  }

  createSession(input: { projectId: string; modelId?: string; approvalMode: ApprovalMode }): SessionRecord {
    const now = new Date().toISOString();
    const id = crypto.randomUUID();
    this.#db
      .query(
        "INSERT INTO sessions (id, project_id, title, model_id, approval_mode, status, created_at, updated_at) VALUES (?, ?, '', ?, ?, 'idle', ?, ?)",
      )
      .run(id, input.projectId, input.modelId ?? null, input.approvalMode, now, now);
    return this.getSession(id)!;
  }

  updateSession(
    id: string,
    patch: Partial<Pick<SessionRecord, "title" | "approvalMode" | "status" | "ompSessionFile">> & {
      modelId?: string | null;
    },
  ): SessionRecord {
    const current = this.getSession(id);
    if (!current) throw new Error(`Session ${id} does not exist`);
    const modelId = patch.modelId === undefined ? current.modelId ?? null : patch.modelId;
    this.#db
      .query(
        "UPDATE sessions SET title = ?, model_id = ?, approval_mode = ?, status = ?, omp_session_file = ?, updated_at = ? WHERE id = ?",
      )
      .run(
        patch.title ?? current.title,
        modelId,
        patch.approvalMode ?? current.approvalMode,
        patch.status ?? current.status,
        patch.ompSessionFile ?? current.ompSessionFile ?? null,
        new Date().toISOString(),
        id,
      );
    return this.getSession(id)!;
  }

  appendEvent(sessionId: string, body: SessionEventBody, runId?: string): SessionEvent {
    let seq = this.#nextSeq.get(sessionId);
    if (seq === undefined) {
      const row = this.#db.query("SELECT COALESCE(MAX(seq), 0) AS seq FROM events WHERE session_id = ?").get(sessionId) as {
        seq: number;
      };
      seq = row.seq + 1;
    }
    this.#db
      .query("INSERT INTO events (session_id, seq, run_id, event_type, payload_json, created_at) VALUES (?, ?, ?, ?, ?, ?)")
      .run(sessionId, seq, runId ?? null, body.eventType, JSON.stringify(body.payload), new Date().toISOString());
    this.#nextSeq.set(sessionId, seq + 1);
    return { type: "event", sessionId, seq, ...(runId ? { runId } : {}), ...body } as SessionEvent;
  }

  listEventsAfter(sessionId: string, afterSeq: number): SessionEvent[] {
    const rows = this.#db
      .query("SELECT seq, run_id, event_type, payload_json FROM events WHERE session_id = ? AND seq > ? ORDER BY seq ASC")
      .all(sessionId, afterSeq) as Array<{ seq: number; run_id: string | null; event_type: string; payload_json: string }>;
    return rows.map(
      (row) =>
        ({
          type: "event",
          sessionId,
          seq: row.seq,
          ...(row.run_id ? { runId: row.run_id } : {}),
          eventType: row.event_type,
          payload: JSON.parse(row.payload_json),
        }) as SessionEvent,
    );
  }

  /** Last run of the session that has no terminal event. */
  unfinishedRunId(sessionId: string): string | undefined {
    const row = this.#db
      .query(
        "SELECT run_id, event_type FROM events WHERE session_id = ? AND event_type IN ('run.started', 'run.finished', 'run.aborted', 'run.failed', 'run.interrupted') ORDER BY seq DESC LIMIT 1",
      )
      .get(sessionId) as { run_id: string | null; event_type: string } | null;
    return row?.event_type === "run.started" ? row.run_id ?? undefined : undefined;
  }
}
