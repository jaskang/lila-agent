import { existsSync, statSync } from "node:fs";
import { resolve } from "node:path";
import { ClientCommand, PROTOCOL_VERSION, type CommandResults, type CommandType, type CoreResponse, type ProtocolMessage } from "@lila/protocol";
import type { Server, ServerWebSocket } from "bun";
import pkg from "../package.json" with { type: "json" };
import { DurableCore } from "./durable.ts";

export interface CoreOptions {
  port: number;
  hostname?: string;
  databasePath: string;
  token: string;
}

type SocketData = { watching?: string };
type Socket = ServerWebSocket<SocketData>;

export class UserError extends Error {}

function encode(message: ProtocolMessage): string { return JSON.stringify(message); }
function directoryError(path: string): string | undefined {
  if (!existsSync(path)) return "目录不存在";
  if (!statSync(path).isDirectory()) return "这不是一个目录";
  return undefined;
}

export async function startCore(options: CoreOptions): Promise<Server<SocketData>> {
  const clients = new Set<Socket>();
  let server!: Server<SocketData>;
  let core: DurableCore;
  const toWatchers = (sessionId: string, message: string) => {
    for (const client of clients) if (client.data.watching === sessionId) client.send(message);
  };
  const broadcastSession = async (sessionId: string) => {
    try {
      const session = await core.getSession(sessionId);
      const message = encode({ type: "session", session });
      for (const client of clients) client.send(message);
    } catch {
      // A deleted session is no longer present in the sidebar.
    }
  };

  core = await DurableCore.open(options.databasePath, {
    emit: toWatchers,
    stream: (sessionId, runId, message) => toWatchers(sessionId, encode({ ...message, type: "stream", sessionId, runId })),
    update: (sessionId) => void broadcastSession(sessionId),
  });

  const hello = encode({ type: "hello", protocolVersion: PROTOCOL_VERSION, coreVersion: pkg.version });
  async function execute(ws: Socket, command: ClientCommand): Promise<CommandResults[CommandType]> {
    switch (command.type) {
      case "projects.list": return { projects: await core.listProjects() };
      case "projects.add": {
        const path = resolve(command.path);
        const error = directoryError(path);
        if (error) throw new UserError(`${error}：${path}`);
        return { project: await core.addProject({ id: crypto.randomUUID(), path, name: path.split("/").filter(Boolean).at(-1) ?? path }) };
      }
      case "models.list": {
        const models = core.models;
        return { models: [...models], ...(models[0] ? { defaultModelId: models[0].id } : {}) };
      }
      case "sessions.list": return { sessions: await core.listSessions() };
      case "session.create": {
        const session = await core.createSession(command.projectId, command.modelId, command.approvalMode ?? "yolo");
        broadcastSession(session.id);
        return { session };
      }
      case "session.configure": return { session: await core.configure(command.sessionId, { ...(command.modelId !== undefined ? { modelId: command.modelId } : {}), ...(command.approvalMode ? { approvalMode: command.approvalMode } : {}) }) };
      case "session.rename": return { session: await core.rename(command.sessionId, command.title) };
      case "session.delete": await core.delete(command.sessionId); return {};
      case "message.send": return { runId: await core.send(command.sessionId, command.text, command.attachments ?? []) };
      case "run.abort": await core.abort(command.sessionId); return {};
      case "pending.respond": await core.respond(command.sessionId, command.pendingId, command.answer); return {};
      case "events.resume": {
        ws.data.watching = command.sessionId;
        return await core.resumeEvents(command.sessionId, command.afterSeq);
      }
      case "events.before": return await core.eventsBefore(command.sessionId, command.beforeSeq, command.limit);
    }
  }

  async function handle(ws: Socket, raw: string): Promise<void> {
    let value: unknown;
    try { value = JSON.parse(raw); } catch { ws.send(encode({ type: "error", code: "invalid_json", message: "命令不是合法的 JSON" })); return; }
    const parsed = ClientCommand.safeParse(value);
    if (!parsed.success) { ws.send(encode({ type: "error", code: "invalid_command", message: parsed.error.message })); return; }
    const command = parsed.data;
    let response: CoreResponse;
    try {
      response = { type: "response", requestId: command.requestId, command: command.type, ok: true, data: await execute(ws, command) };
    } catch (error) {
      if (!(error instanceof UserError)) console.error(`[core] ${command.type} failed`, error);
      response = { type: "response", requestId: command.requestId, command: command.type, ok: false, error: error instanceof Error ? error.message : String(error) };
    }
    ws.send(encode(response));
  }

  server = Bun.serve<SocketData>({
    hostname: options.hostname ?? "127.0.0.1",
    port: options.port,
    fetch(req, srv) {
      const { pathname } = new URL(req.url);
      if (pathname === "/health") return Response.json({ ok: true, coreVersion: pkg.version, protocolVersion: PROTOCOL_VERSION, pid: process.pid });
      if (pathname !== "/ws") return new Response("Not found", { status: 404 });
      const origin = req.headers.get("origin");
      if (origin && origin !== "null" && origin !== "file://" && !/^http:\/\/(localhost|127\.0\.0\.1):\d+$/.test(origin)) return new Response("Forbidden origin", { status: 403 });
      const expected = `lila.${options.token}`;
      const protocols = (req.headers.get("sec-websocket-protocol") ?? "").split(",").map((item) => item.trim());
      if (!protocols.includes(expected)) return new Response("Unauthorized", { status: 401 });
      if (srv.upgrade(req, { data: {}, headers: { "Sec-WebSocket-Protocol": expected } })) return undefined;
      return new Response("Expected a WebSocket upgrade", { status: 426 });
    },
    websocket: {
      open(ws) { clients.add(ws); ws.send(hello); },
      message(ws, message) { void handle(ws, String(message)); },
      close(ws) { clients.delete(ws); },
    },
  });
  const stop = server.stop.bind(server);
  server.stop = async (closeActiveConnections?: boolean) => { await stop(closeActiveConnections); await core.close(); };
  return server;
}
