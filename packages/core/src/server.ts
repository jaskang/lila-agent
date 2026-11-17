import { existsSync, statSync } from "node:fs";
import { resolve } from "node:path";
import {
  ClientCommand,
  PROTOCOL_VERSION,
  type CommandResults,
  type CommandType,
  type CoreResponse,
  type Model,
  type ProtocolMessage,
  type SessionEvent,
  type SessionSummary,
} from "@lila/protocol";
import type { Server, ServerWebSocket } from "bun";
import pkg from "../package.json" with { type: "json" };
import { createOmpSession, type CreateOmp } from "./omp.ts";
import { SessionRunner } from "./session.ts";
import { Store, type SessionRecord } from "./store.ts";
import { z } from "zod";

export interface CoreOptions {
  port: number;
  hostname?: string;
  databasePath: string;
  /** Clients must present this token in the `lila.<token>` WebSocket subprotocol. */
  token: string;
  createOmp?: CreateOmp;
  idleTimeoutMs?: number;
}

type SocketData = { watching?: string };
type Socket = ServerWebSocket<SocketData>;

export class UserError extends Error {}

const OmpModel = z.object({ id: z.string().min(1), provider: z.string().min(1), name: z.string().optional() });
const OmpModels = z.object({ models: z.array(OmpModel) });
const OmpState = z.object({ model: OmpModel.optional() });

function encode(message: ProtocolMessage): string {
  return JSON.stringify(message);
}

function directoryError(path: string): string | undefined {
  if (!existsSync(path)) return "目录不存在";
  if (!statSync(path).isDirectory()) return "这不是一个目录";
  return undefined;
}

export function startCore(options: CoreOptions): Server<SocketData> {
  const createOmp = options.createOmp ?? createOmpSession;
  const store = new Store(options.databasePath);
  const runners = new Map<string, SessionRunner>();
  const clients = new Set<Socket>();
  const hello = encode({ type: "hello", protocolVersion: PROTOCOL_VERSION, coreVersion: pkg.version });
  let models: Promise<CommandResults["models.list"]> | undefined;

  // A previous core process may have died mid-run; those runs cannot continue.
  for (const session of store.listSessions()) {
    if (session.status !== "running" && session.status !== "waiting") continue;
    const runId = store.unfinishedRunId(session.id);
    if (runId) {
      store.appendEvent(session.id, { eventType: "run.interrupted", payload: { reason: "core 重启，未完成的任务已中断。" } }, runId);
    }
    store.updateSession(session.id, { status: "interrupted" });
  }

  function summary(record: SessionRecord): SessionSummary {
    return {
      id: record.id,
      projectId: record.projectId,
      title: record.title,
      ...(record.modelId ? { modelId: record.modelId } : {}),
      approvalMode: record.approvalMode,
      status: record.status,
      updatedAt: record.updatedAt,
      pendingCount: runners.get(record.id)?.pendingCount ?? 0,
    };
  }

  function broadcastSession(record: SessionRecord): void {
    const message = encode({ type: "session", session: summary(record) });
    for (const client of clients) client.send(message);
  }

  function toWatchers(sessionId: string, message: string): void {
    for (const client of clients) if (client.data.watching === sessionId) client.send(message);
  }

  function runner(sessionId: string): SessionRunner {
    const existing = runners.get(sessionId);
    if (existing) return existing;
    const record = store.getSession(sessionId);
    if (!record) throw new UserError("会话不存在");
    const project = store.getProject(record.projectId);
    if (!project) throw new UserError("会话所属的项目不存在");
    const created = new SessionRunner(record, project.path, {
      createOmp,
      idleTimeoutMs: options.idleTimeoutMs ?? 10 * 60_000,
      emit: (body, runId) => toWatchers(sessionId, encode(store.appendEvent(sessionId, body, runId))),
      stream: (runId, body) => toWatchers(sessionId, encode({ type: "stream", sessionId, runId, ...body })),
      update: (patch) => {
        const updated = store.updateSession(sessionId, patch);
        broadcastSession(updated);
        return updated;
      },
    });
    runners.set(sessionId, created);
    return created;
  }

  async function discoverModels(): Promise<CommandResults["models.list"]> {
    const omp = createOmp({ cwd: process.cwd(), approvalMode: "yolo", onFrame: () => undefined, onExit: () => undefined });
    try {
      await omp.start();
      const [available, state] = await Promise.all([omp.request("get_available_models"), omp.request("get_state")]);
      const models: Model[] = OmpModels.parse(available.data).models.map((model) => ({
        id: `${model.provider}::${model.id}`,
        label: model.name ?? model.id,
      }));
      const current = OmpState.safeParse(state.data).data?.model;
      return { models, ...(current ? { defaultModelId: `${current.provider}::${current.id}` } : {}) };
    } finally {
      await omp.stop();
    }
  }

  async function execute(ws: Socket, command: ClientCommand): Promise<CommandResults[CommandType]> {
    switch (command.type) {
      case "projects.list":
        return { projects: store.listProjects() };
      case "projects.add": {
        const path = resolve(command.path);
        const error = directoryError(path);
        if (error) throw new UserError(`${error}：${path}`);
        return { project: store.addProject(path) };
      }
      case "models.list":
        models ??= discoverModels().catch((error) => {
          models = undefined;
          throw error;
        });
        return models;
      case "sessions.list":
        return { sessions: store.listSessions().map(summary) };
      case "session.create": {
        const project = store.getProject(command.projectId);
        if (!project) throw new UserError("项目不存在");
        const error = directoryError(project.path);
        if (error) throw new UserError(`项目${error}：${project.path}`);
        const record = store.createSession({
          projectId: project.id,
          ...(command.modelId ? { modelId: command.modelId } : {}),
          approvalMode: command.approvalMode ?? "yolo",
        });
        store.appendEvent(record.id, {
          eventType: "session.created",
          payload: {
            projectId: project.id,
            ...(record.modelId ? { modelId: record.modelId } : {}),
            approvalMode: record.approvalMode,
          },
        });
        broadcastSession(record);
        return { session: summary(record) };
      }
      case "session.configure": {
        const record = await runner(command.sessionId).configure({
          ...(command.modelId !== undefined ? { modelId: command.modelId } : {}),
          ...(command.approvalMode ? { approvalMode: command.approvalMode } : {}),
        });
        return { session: summary(record) };
      }
      case "message.send":
        return { runId: await runner(command.sessionId).send(command.text) };
      case "run.abort":
        await runner(command.sessionId).abort();
        return {};
      case "pending.respond":
        await runner(command.sessionId).respond(command.pendingId, command.answer);
        return {};
      case "events.resume": {
        if (!store.getSession(command.sessionId)) throw new UserError("会话不存在");
        // Switching the watch and reading the log happen in one synchronous step, so no event falls between them.
        ws.data.watching = command.sessionId;
        const events: SessionEvent[] = store.listEventsAfter(command.sessionId, command.afterSeq);
        const live = runners.get(command.sessionId)?.live();
        return { events, ...(live ? { live } : {}) };
      }
    }
  }

  async function handle(ws: Socket, raw: string): Promise<void> {
    let value: unknown;
    try {
      value = JSON.parse(raw);
    } catch {
      ws.send(encode({ type: "error", code: "invalid_json", message: "命令不是合法的 JSON" }));
      return;
    }
    const parsed = ClientCommand.safeParse(value);
    if (!parsed.success) {
      ws.send(encode({ type: "error", code: "invalid_command", message: parsed.error.message }));
      return;
    }
    const command = parsed.data;
    let response: CoreResponse;
    try {
      const data = await execute(ws, command);
      response = { type: "response", requestId: command.requestId, command: command.type, ok: true, data };
    } catch (error) {
      if (!(error instanceof UserError)) console.error(`[core] ${command.type} failed`, error);
      const message = error instanceof Error ? error.message : String(error);
      response = { type: "response", requestId: command.requestId, command: command.type, ok: false, error: message };
    }
    ws.send(encode(response));
  }

  const server = Bun.serve<SocketData>({
    hostname: options.hostname ?? "127.0.0.1",
    port: options.port,
    fetch(req, srv) {
      const { pathname } = new URL(req.url);
      if (pathname === "/health") return Response.json({ ok: true, coreVersion: pkg.version, protocolVersion: PROTOCOL_VERSION, pid: process.pid });
      if (pathname !== "/ws") return new Response("Not found", { status: 404 });
      // Web pages in any browser can reach localhost; only Lila's own windows (file:// or the dev server) may connect.
      const origin = req.headers.get("origin");
      if (origin && origin !== "null" && origin !== "file://" && !/^http:\/\/(localhost|127\.0\.0\.1):\d+$/.test(origin)) {
        return new Response("Forbidden origin", { status: 403 });
      }
      const protocols = (req.headers.get("sec-websocket-protocol") ?? "").split(",").map((item) => item.trim());
      const expected = `lila.${options.token}`;
      if (!protocols.includes(expected)) return new Response("Unauthorized", { status: 401 });
      if (srv.upgrade(req, { data: {}, headers: { "Sec-WebSocket-Protocol": expected } })) return undefined;
      return new Response("Expected a WebSocket upgrade", { status: 426 });
    },
    websocket: {
      open(ws) {
        clients.add(ws);
        ws.send(hello);
      },
      message(ws, message) {
        void handle(ws, String(message));
      },
      close(ws) {
        clients.delete(ws);
      },
    },
  });

  const stop = server.stop.bind(server);
  server.stop = async (closeActiveConnections?: boolean) => {
    await Promise.all([...runners.values()].map((item) => item.dispose()));
    await stop(closeActiveConnections);
    store.close();
  };
  return server;
}
