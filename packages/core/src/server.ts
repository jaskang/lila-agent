import { resolve } from "node:path";

import {
  ClientCommand,
  DEFAULT_CORE_PORT,
  PROTOCOL_VERSION,
  ServerHello,
  type CoreResponse,
  type SessionEvent,
  type SessionEventType,
} from "@lila/protocol";
import pkg from "../package.json" with { type: "json" };
import { OmpSession } from "./omp.ts";
import { SessionStore, type SessionRecord } from "./store.ts";

type Socket = { send(data: string): void };

interface PendingItem {
  rawId: string;
  method: string;
}

interface ActiveSession {
  record: SessionRecord;
  projectPath: string;
  omp?: OmpSession;
  pending: Map<string, PendingItem>;
  runId?: string;
}

export interface CoreOptions {
  port?: number;
  hostname?: string;
  databasePath?: string;
  projectPath?: string;
}

type ModelOption = { id: string; label: string };

export function startCore({
  port = DEFAULT_CORE_PORT,
  hostname = "127.0.0.1",
  databasePath = process.env.LILA_DATABASE_PATH ?? ":memory:",
  projectPath = process.env.LILA_PROJECT_PATH ?? process.cwd(),
}: CoreOptions = {}) {
  const clients = new Set<Socket>();
  const store = new SessionStore(databasePath);
  const activeSessions = new Map<string, ActiveSession>();
  const hello = ServerHello.parse({
    type: "hello",
    protocolVersion: PROTOCOL_VERSION,
    coreVersion: pkg.version,
  });

  store.ensureProject("workspace", projectPath);
  let modelCatalogPromise: Promise<ModelOption[]> | undefined;

  async function discoverModels(): Promise<ModelOption[]> {
    const omp = new OmpSession({
      cwd: projectPath,
      approvalMode: "yolo",
      onFrame: () => undefined,
      onExit: () => undefined,
    });
    try {
      await omp.start();
      const response = await omp.request("get_available_models");
      const data = response.data;
      if (!data || typeof data !== "object") return [];
      const models = (data as Record<string, unknown>).models;
      if (!Array.isArray(models)) return [];
      return models.flatMap((item) => {
        if (!item || typeof item !== "object") return [];
        const value = item as Record<string, unknown>;
        if (typeof value.id !== "string") return [];
        const provider = typeof value.provider === "string" ? value.provider : "magpie";
        const label = typeof value.name === "string" ? value.name : value.id;
        return [{ id: `${provider}::${value.id}`, label }];
      });
    } finally {
      await omp.stop();
    }
  }

  function send(ws: Socket, message: CoreResponse | SessionEvent | Record<string, unknown>): void {
    ws.send(JSON.stringify(message));
  }

  function respond(
    ws: Socket,
    requestId: string,
    command: string,
    data?: Record<string, unknown>,
    error?: string,
  ): void {
    send(ws, {
      type: "response",
      requestId,
      command,
      success: !error,
      ...(data ? { data } : {}),
      ...(error ? { error } : {}),
    });
  }

  function fail(ws: Socket, code: string, message: string, requestId?: string): void {
    send(ws, { type: "error", code, message, ...(requestId ? { requestId } : {}) });
  }

  function emit(
    session: ActiveSession,
    eventType: SessionEventType,
    payload: Record<string, unknown>,
    options: { requestId?: string; runId?: string; pendingId?: string } = {},
  ): SessionEvent {
    const event = store.appendEvent({
      sessionId: session.record.id,
      eventType,
      payload,
      ...options,
    });
    const encoded = JSON.stringify(event);
    for (const client of clients) client.send(encoded);
    return event;
  }

  function getActiveSession(sessionId: string): ActiveSession | undefined {
    const existing = activeSessions.get(sessionId);
    if (existing) return existing;
    const record = store.getSession(sessionId);
    if (!record) return undefined;
    const project = store.getProject(record.projectId);
    if (!project) return undefined;
    const active = { record, projectPath: project.path, pending: new Map<string, PendingItem>() };
    activeSessions.set(sessionId, active);
    return active;
  }

  function updateStatus(session: ActiveSession, status: SessionRecord["status"]): void {
    session.record.status = status;
    store.setSessionStatus(session.record.id, status);
  }

  function failRun(session: ActiveSession, message: string): void {
    if (!session.runId) return;
    const runId = session.runId;
    updateStatus(session, "failed");
    emit(session, "run.failed", { message }, { runId });
    session.runId = undefined;
  }

  function handleOmpFrame(session: ActiveSession, frame: Record<string, unknown> & { type: string }): void {
    const runId = session.runId;
    if (frame.type === "message_update" && runId) {
      const update = frame.assistantMessageEvent as { type?: string; delta?: string } | undefined;
      if (update?.delta) {
        emit(
          session,
          "assistant.delta",
          { messageId: frame.messageId, kind: update.type ?? "unknown", delta: update.delta },
          { runId },
        );
      }
      return;
    }

    if (frame.type === "message_end" && runId) {
      const message = frame.message as Record<string, unknown> | undefined;
      if (message?.role === "assistant") {
        emit(session, "assistant.completed", { messageId: frame.messageId, message }, { runId });
      }
      return;
    }

    if (frame.type === "tool_execution_start" && runId) {
      emit(
        session,
        "tool.started",
        { toolCallId: frame.toolCallId, toolName: frame.toolName, args: frame.args, intent: frame.intent },
        { runId },
      );
      return;
    }

    if (frame.type === "tool_execution_update" && runId) {
      emit(session, "tool.output", { toolCallId: frame.toolCallId, result: frame.partialResult }, { runId });
      return;
    }

    if (frame.type === "tool_execution_end" && runId) {
      emit(
        session,
        "tool.finished",
        { toolCallId: frame.toolCallId, result: frame.result, isError: frame.isError },
        { runId },
      );
      return;
    }

    if (frame.type === "extension_ui_request" && runId && typeof frame.id === "string") {
      const pendingId = crypto.randomUUID();
      const method = typeof frame.method === "string" ? frame.method : "unknown";
      session.pending.set(pendingId, { rawId: frame.id, method });
      updateStatus(session, "waiting");
      emit(
        session,
        "pending.created",
        {
          method,
          title: frame.title,
          message: frame.message,
          options: frame.options,
          questions: frame.questions,
          placeholder: frame.placeholder,
          defaultValue: frame.defaultValue,
        },
        { runId, pendingId },
      );
      return;
    }

    if (frame.type === "prompt_result" && runId) {
      const status = frame.status;
      if (status === "completed") {
        updateStatus(session, "finished");
        emit(session, "run.finished", { status }, { runId });
      } else if (status === "aborted") {
        updateStatus(session, "aborted");
        emit(session, "run.aborted", { status }, { runId });
      } else {
        updateStatus(session, "failed");
        emit(session, "run.failed", { status, error: frame.error }, { runId });
      }
      session.runId = undefined;
      return;
    }

    if (frame.type === "adapter_error") {
      failRun(session, String(frame.error ?? "omp adapter error"));
    }
  }

  function handleOmpExit(session: ActiveSession, code: number): void {
    session.omp = undefined;
    for (const pendingId of session.pending.keys()) {
      emit(session, "pending.resolved", { status: "interrupted" }, { pendingId, runId: session.runId });
    }
    session.pending.clear();
    if (session.runId) {
      const runId = session.runId;
      updateStatus(session, "interrupted");
      emit(session, "run.interrupted", { exitCode: code }, { runId });
      session.runId = undefined;
    }
  }

  async function startOmp(session: ActiveSession): Promise<void> {
    const omp = new OmpSession({
      cwd: session.projectPath,
      approvalMode: session.record.approvalMode,
      modelId: session.record.modelId,
      sessionFile: session.record.sessionFile,
      onFrame: (frame) => handleOmpFrame(session, frame),
      onExit: (code) => handleOmpExit(session, code),
    });
    session.omp = omp;
    await omp.start();
    const state = await omp.request("get_state");
    const stateData = state.data && typeof state.data === "object" ? state.data as Record<string, unknown> : undefined;
    const sessionFile = typeof stateData?.sessionFile === "string" ? stateData.sessionFile : undefined;
    if (sessionFile) {
      session.record.sessionFile = sessionFile;
      store.setSessionFile(session.record.id, sessionFile);
    }
  }
  function toOmpResponse(pending: PendingItem, answer: unknown): Record<string, unknown> {
    const value = answer && typeof answer === "object" ? answer as Record<string, unknown> : undefined;
    if (value?.cancelled === true) return { cancelled: true };
    if (pending.method === "select") return { value: typeof value?.value === "string" ? value.value : String(answer) };
    if (pending.method === "confirm") return { confirmed: value?.confirmed === true || answer === true };
    if (pending.method === "input" || pending.method === "editor") {
      return { value: typeof value?.value === "string" ? value.value : String(answer) };
    }
    if (pending.method === "ask") {
      return { answers: Array.isArray(value?.answers) ? value.answers : Array.isArray(answer) ? answer : [] };
    }
    return { cancelled: true };
  }


  async function handleCommand(ws: Socket, input: string): Promise<void> {
    let value: unknown;
    try {
      value = JSON.parse(input);
    } catch {
      fail(ws, "invalid_json", "Command must be valid JSON");
      return;
    }

    const result = ClientCommand.safeParse(value);
    if (!result.success) {
      fail(ws, "invalid_command", result.error.message);
      return;
    }

    const command = result.data;
    if (command.type === "projects.list") {
      respond(ws, command.requestId, command.type, { projects: store.listProjects() });
      return;
    }
    if (command.type === "projects.add") {
      const path = resolve(command.path);
      const project = store.addProject(crypto.randomUUID(), path);
      respond(ws, command.requestId, command.type, { project });
      return;
    }
    if (command.type === "models.list") {
      try {
        modelCatalogPromise ??= discoverModels();
        respond(ws, command.requestId, command.type, { models: await modelCatalogPromise });
      } catch (error) {
        modelCatalogPromise = undefined;
        respond(ws, command.requestId, command.type, undefined, error instanceof Error ? error.message : String(error));
      }
      return;
    }
    if (command.type === "sessions.list") {
      respond(ws, command.requestId, command.type, { sessions: store.listSessions(command.projectId) });
      return;
    }
    if (command.type === "events.resume") {
      if (!store.getSession(command.sessionId)) {
        fail(ws, "session_not_found", "Session does not exist");
        return;
      }
      for (const event of store.listEventsAfter(command.sessionId, command.lastEventSeq)) send(ws, event);
      return;
    }
    if (command.type === "session.create") {
      const project = store.getProject(command.projectId);
      if (!project) {
        fail(ws, "project_not_found", "Project does not exist", command.requestId);
        return;
      }
      const record = store.createSession({
        id: crypto.randomUUID(),
        projectId: command.projectId,
        ...(command.modelId ? { modelId: command.modelId } : {}),
        approvalMode: command.approvalMode ?? "yolo",
      });
      const active = { record, projectPath: project.path, pending: new Map<string, PendingItem>() };
      activeSessions.set(record.id, active);
      emit(
        active,
        "session.created",
        {
          projectId: record.projectId,
          ...(record.modelId ? { modelId: record.modelId } : {}),
          approvalMode: record.approvalMode,
        },
        { requestId: command.requestId },
      );
      respond(ws, command.requestId, command.type, { sessionId: record.id });
      return;
    }

    const active = getActiveSession(command.sessionId);
    if (!active) {
      fail(ws, "session_not_found", "Session does not exist", command.requestId);
      return;
    }

    if (command.type === "session.configure") {
      if (active.runId) {
        respond(ws, command.requestId, command.type, undefined, "Cannot configure a running session");
        return;
      }
      if (active.omp) {
        await active.omp.stop();
        active.omp = undefined;
      }
      const record = store.configureSession(command.sessionId, {
        ...(Object.hasOwn(command, "modelId") ? { modelId: command.modelId } : {}),
        ...(command.approvalMode ? { approvalMode: command.approvalMode } : {}),
      });
      if (!record) {
        fail(ws, "session_not_found", "Session does not exist", command.requestId);
        return;
      }
      active.record = record;
      emit(active, "session.updated", { modelId: record.modelId, approvalMode: record.approvalMode }, { requestId: command.requestId });
      respond(ws, command.requestId, command.type, { session: record });
      return;
    }


    if (command.type === "message.send") {
      if (active.runId) {
        respond(ws, command.requestId, command.type, undefined, "Session is already running");
        return;
      }
      const runId = crypto.randomUUID();
      active.runId = runId;
      updateStatus(active, "running");
      emit(active, "message.user", { text: command.text }, { requestId: command.requestId, runId });
      emit(active, "run.started", {}, { requestId: command.requestId, runId });
      try {
        if (!active.omp) await startOmp(active);
        await active.omp?.prompt(runId, command.text);
        respond(ws, command.requestId, command.type, { runId });
      } catch (error) {
        failRun(active, error instanceof Error ? error.message : String(error));
        respond(ws, command.requestId, command.type, undefined, String(error));
      }
      return;
    }

    if (command.type === "run.abort") {
      if (active.runId !== command.runId || !active.omp) {
        respond(ws, command.requestId, command.type, undefined, "Run is not active");
        return;
      }
      await active.omp.abort(command.requestId);
      respond(ws, command.requestId, command.type, { accepted: true });
      return;
    }

    const pending = active.pending.get(command.pendingId);
    if (!pending || !active.omp) {
      respond(ws, command.requestId, command.type, undefined, "Pending item is no longer active");
      return;
    }
    await active.omp.respond(pending.rawId, toOmpResponse(pending, command.answer));
    active.pending.delete(command.pendingId);
    updateStatus(active, active.pending.size > 0 ? "waiting" : "running");
    emit(active, "pending.resolved", { status: "answered" }, { pendingId: command.pendingId, runId: active.runId });
    respond(ws, command.requestId, command.type, { accepted: true });
  }

  return Bun.serve({
    hostname,
    port,
    fetch(req, server) {
      const { pathname } = new URL(req.url);
      if (pathname === "/health") {
        return Response.json({ ok: true, coreVersion: pkg.version, pid: process.pid });
      }
      if (pathname === "/ws") {
        if (server.upgrade(req)) return;
        return new Response("Expected a WebSocket upgrade", { status: 426 });
      }
      return new Response("Not found", { status: 404 });
    },
    websocket: {
      open(ws) {
        clients.add(ws);
        console.log(`[core] client connected (${ws.remoteAddress})`);
        ws.send(JSON.stringify(hello));
      },
      message(ws, message) {
        void handleCommand(ws, String(message));
      },
      close(ws, code) {
        clients.delete(ws);
        console.log(`[core] client disconnected (${ws.remoteAddress}, code ${code})`);
      },
    },
  });
}
