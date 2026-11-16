import { mkdirSync } from "node:fs";
import { dirname } from "node:path";
import { ApprovalMode, SessionEvent, type SessionEventType } from "@lila/protocol";
import { Database } from "bun:sqlite";

export interface SessionRecord {
  id: string;
  projectId: string;
  modelId?: string;
  sessionFile?: string;
  approvalMode: ApprovalMode;
  status: "idle" | "running" | "waiting" | "finished" | "failed" | "aborted" | "interrupted";
}

export interface CreateSessionInput {
  id: string;
  projectId: string;
  modelId?: string;
  approvalMode: ApprovalMode;
}

export interface ProjectRecord {
  id: string;
  path: string;
}

export class SessionStore {
  readonly #db: Database;

  constructor(path = ":memory:") {
    if (path !== ":memory:") mkdirSync(dirname(path), { recursive: true });
    this.#db = new Database(path);
    this.#db.exec(`
      PRAGMA journal_mode = WAL;
      CREATE TABLE IF NOT EXISTS projects (
        id TEXT PRIMARY KEY,
        path TEXT NOT NULL,
        created_at TEXT NOT NULL
      );
      CREATE TABLE IF NOT EXISTS sessions (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        model_id TEXT,
        session_file TEXT,
        approval_mode TEXT NOT NULL,
        status TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
      );
      CREATE TABLE IF NOT EXISTS events (
        session_id TEXT NOT NULL,
        seq INTEGER NOT NULL,
        event_type TEXT NOT NULL,
        payload_json TEXT NOT NULL,
        request_id TEXT,
        run_id TEXT,
        pending_id TEXT,
        created_at TEXT NOT NULL,
        PRIMARY KEY (session_id, seq)
      );
      CREATE TABLE IF NOT EXISTS pending_items (
        id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL,
        run_id TEXT,
        status TEXT NOT NULL,
        payload_json TEXT NOT NULL,
        created_at TEXT NOT NULL,
        resolved_at TEXT
      );
      CREATE TABLE IF NOT EXISTS command_requests (
        session_id TEXT NOT NULL,
        request_id TEXT NOT NULL,
        command_type TEXT NOT NULL,
        response_json TEXT NOT NULL,
        created_at TEXT NOT NULL,
        PRIMARY KEY (session_id, request_id)
      );
    `);
    try {
      this.#db.exec("ALTER TABLE sessions ADD COLUMN session_file TEXT");
    } catch {
      // Existing databases already have the column.
    }
  }

  close(): void {
    this.#db.close();
  }

  addProject(id: string, path: string): ProjectRecord {
    this.ensureProject(id, path);
    return { id, path };
  }

  listProjects(): ProjectRecord[] {
    return this.#db.prepare("SELECT id, path FROM projects ORDER BY created_at ASC").all() as ProjectRecord[];
  }

  getProject(id: string): ProjectRecord | undefined {
    const row = this.#db.prepare("SELECT id, path FROM projects WHERE id = ?").get(id) as ProjectRecord | null;
    return row ?? undefined;
  }

  listSessions(projectId?: string): SessionRecord[] {
    const rows = projectId
      ? this.#db
          .prepare(
            "SELECT id, project_id, model_id, session_file, approval_mode, status FROM sessions WHERE project_id = ? ORDER BY created_at ASC",
          )
          .all(projectId)
      : this.#db
          .prepare("SELECT id, project_id, model_id, session_file, approval_mode, status FROM sessions ORDER BY created_at ASC")
          .all();
    return (rows as Array<{
      id: string;
      project_id: string;
      model_id: string | null;
      session_file: string | null;
      approval_mode: string;
      status: SessionRecord["status"];
    }>).flatMap((row) => {
      const approvalMode = ApprovalMode.safeParse(row.approval_mode);
      if (!approvalMode.success) return [];
      return [
        {
          id: row.id,
          projectId: row.project_id,
          ...(row.model_id ? { modelId: row.model_id } : {}),
          ...(row.session_file ? { sessionFile: row.session_file } : {}),
          approvalMode: approvalMode.data,
          status: row.status,
        },
      ];
    });
  }

  configureSession(id: string, input: { modelId?: string | null; approvalMode?: ApprovalMode }): SessionRecord | undefined {
    const current = this.getSession(id);
    if (!current) return undefined;
    this.#db
      .prepare("UPDATE sessions SET model_id = ?, approval_mode = ?, updated_at = ? WHERE id = ?")
      .run(input.modelId === undefined ? current.modelId ?? null : input.modelId, input.approvalMode ?? current.approvalMode, new Date().toISOString(), id);
    return this.getSession(id);
  }

  ensureProject(id: string, path: string): void {
    this.#db
      .prepare("INSERT OR IGNORE INTO projects (id, path, created_at) VALUES (?, ?, ?)")
      .run(id, path, new Date().toISOString());
  }

  createSession(input: CreateSessionInput): SessionRecord {
    const now = new Date().toISOString();
    this.#db
      .prepare(
        "INSERT INTO sessions (id, project_id, model_id, session_file, approval_mode, status, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
      )
      .run(input.id, input.projectId, input.modelId ?? null, null, input.approvalMode, "idle", now, now);
    return {
      id: input.id,
      projectId: input.projectId,
      ...(input.modelId ? { modelId: input.modelId } : {}),
      approvalMode: input.approvalMode,
      status: "idle",
    };
  }

  getSession(id: string): SessionRecord | undefined {
    const row = this.#db
      .prepare(
        "SELECT id, project_id, model_id, session_file, approval_mode, status FROM sessions WHERE id = ?",
      )
      .get(id) as
      | {
          id: string;
          project_id: string;
          model_id: string | null;
          session_file: string | null;
          approval_mode: string;
          status: SessionRecord["status"];
        }
      | null;
    if (!row) return undefined;
    const approvalMode = ApprovalMode.safeParse(row.approval_mode);
    if (!approvalMode.success) throw new Error(`Invalid approval mode in session ${id}`);
    return {
      id: row.id,
      projectId: row.project_id,
      ...(row.model_id ? { modelId: row.model_id } : {}),
      ...(row.session_file ? { sessionFile: row.session_file } : {}),
      approvalMode: approvalMode.data,
      status: row.status,
    };
  }

  setSessionStatus(id: string, status: SessionRecord["status"]): void {
    this.#db
      .prepare("UPDATE sessions SET status = ?, updated_at = ? WHERE id = ?")
      .run(status, new Date().toISOString(), id);
  }

  setSessionFile(id: string, sessionFile: string): void {
    this.#db
      .prepare("UPDATE sessions SET session_file = ?, updated_at = ? WHERE id = ?")
      .run(sessionFile, new Date().toISOString(), id);
  }

  appendEvent(input: {
    sessionId: string;
    eventType: SessionEventType;
    payload: Record<string, unknown>;
    requestId?: string;
    runId?: string;
    pendingId?: string;
  }): SessionEvent {
    const next = this.#db
      .prepare("SELECT COALESCE(MAX(seq), 0) + 1 AS seq FROM events WHERE session_id = ?")
      .get(input.sessionId) as { seq: number };
    const event = SessionEvent.parse({
      type: "event",
      sessionId: input.sessionId,
      seq: next.seq,
      eventType: input.eventType,
      payload: input.payload,
      ...(input.requestId ? { requestId: input.requestId } : {}),
      ...(input.runId ? { runId: input.runId } : {}),
      ...(input.pendingId ? { pendingId: input.pendingId } : {}),
    });
    this.#db
      .prepare(
        "INSERT INTO events (session_id, seq, event_type, payload_json, request_id, run_id, pending_id, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
      )
      .run(
        event.sessionId,
        event.seq,
        event.eventType,
        JSON.stringify(event.payload),
        event.requestId ?? null,
        event.runId ?? null,
        event.pendingId ?? null,
        new Date().toISOString(),
      );
    return event;
  }

  listEventsAfter(sessionId: string, seq: number): SessionEvent[] {
    const rows = this.#db
      .prepare(
        "SELECT seq, event_type, payload_json, request_id, run_id, pending_id FROM events WHERE session_id = ? AND seq > ? ORDER BY seq ASC",
      )
      .all(sessionId, seq) as Array<{
      seq: number;
      event_type: SessionEventType;
      payload_json: string;
      request_id: string | null;
      run_id: string | null;
      pending_id: string | null;
    }>;
    return rows.map((row) =>
      SessionEvent.parse({
        type: "event",
        sessionId,
        seq: row.seq,
        eventType: row.event_type,
        payload: JSON.parse(row.payload_json),
        ...(row.request_id ? { requestId: row.request_id } : {}),
        ...(row.run_id ? { runId: row.run_id } : {}),
        ...(row.pending_id ? { pendingId: row.pending_id } : {}),
      }),
    );
  }
}
