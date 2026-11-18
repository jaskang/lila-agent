import { createServer, type IncomingMessage } from "node:http";
import type { AddressInfo } from "node:net";
import { ClientCommand, type CommandResults, type CommandType, PROTOCOL_VERSION, type ServerMessage } from "@lila/protocol";
import { type WebSocket, WebSocketServer } from "ws";
import pkg from "../package.json" with { type: "json" };
import { errorMessage, UserError } from "./errors.ts";
import { SessionHost } from "./host.ts";
import type { Clients } from "./live-session.ts";

export interface CoreOptions {
  port: number;
  hostname?: string;
  dataDir: string;
  token: string;
}

export interface Core {
  url: string;
  port: number;
  /** Disposes every live session, then closes the sockets and the listener. */
  stop(): Promise<void>;
}

const ALLOWED_ORIGIN = /^http:\/\/(localhost|127\.0\.0\.1)(:\d+)?$/;

function originAllowed(origin: string | undefined): boolean {
  return !origin || origin === "null" || origin === "file://" || ALLOWED_ORIGIN.test(origin);
}

export async function startCore(options: CoreOptions): Promise<Core> {
  const watching = new Map<WebSocket, string>();
  const send = (ws: WebSocket, message: ServerMessage) => {
    if (ws.readyState === ws.OPEN) ws.send(JSON.stringify(message));
  };
  const clients: Clients = {
    toWatchers(sessionId, message) {
      const data = JSON.stringify(message);
      for (const [ws, watched] of watching) if (watched === sessionId && ws.readyState === ws.OPEN) ws.send(data);
    },
    broadcast(message) {
      const data = JSON.stringify(message);
      for (const ws of wss.clients) if (ws.readyState === ws.OPEN) ws.send(data);
    },
    isWatched: (sessionId) => [...watching.values()].includes(sessionId),
  };
  const host = new SessionHost(options.dataDir, clients);
  const expectedProtocol = `lila.${options.token}`;

  async function execute(ws: WebSocket, command: ClientCommand): Promise<CommandResults[CommandType]> {
    switch (command.type) {
      case "projects.list":
        return { projects: [...host.store.projects] };
      case "projects.add":
        return { project: host.addProject(command.path) };
      case "projects.remove":
        await host.removeProject(command.projectId);
        return {};
      case "models.list":
        return await host.listModels();
      case "sessions.list":
        return { sessions: await host.listSessions() };
      case "session.create":
        return { session: await host.createSession(command) };
      case "files.search":
        return { files: await host.searchFiles(command.projectId, command.query, command.limit) };
      case "session.delete":
        await host.deleteSession(command.sessionId);
        return {};
      case "commands.list":
        return { commands: await host.commands(command.projectId, command.sessionId) };
      default:
        break;
    }
    const live = await host.live(command.sessionId);
    switch (command.type) {
      case "session.open": {
        watching.set(ws, command.sessionId);
        return live.open(command.limit);
      }
      case "session.earlier":
        return live.earlier(command.beforeId, command.limit);
      case "session.prompt":
        await live.prompt(command);
        return {};
      case "session.abort":
        await live.abort();
        return {};
      case "session.configure":
        return { state: await live.configure(command) };
      case "session.rename":
        return { session: live.rename(command.title) };
      case "session.compact":
        live.compact();
        return {};
      case "session.rewind":
        return { text: await live.rewind(command.itemId) };
      case "session.respond":
        live.respond(command.pendingId, command.answer);
        return {};
      case "session.resources":
        return { resources: live.resources() };
      case "session.reload":
        await live.reload();
        return {};
    }
  }

  async function handle(ws: WebSocket, raw: string): Promise<void> {
    let value: unknown;
    try {
      value = JSON.parse(raw);
    } catch {
      send(ws, { type: "error", code: "invalid_json", message: "命令不是合法的 JSON" });
      return;
    }
    const parsed = ClientCommand.safeParse(value);
    if (!parsed.success) {
      const requestId = (value as { requestId?: unknown } | null)?.requestId;
      if (typeof requestId === "string") send(ws, { type: "response", requestId, ok: false, error: `命令格式不对：${parsed.error.message}` });
      else send(ws, { type: "error", code: "invalid_command", message: parsed.error.message });
      return;
    }
    const command = parsed.data;
    try {
      send(ws, { type: "response", requestId: command.requestId, ok: true, data: await execute(ws, command) });
    } catch (error) {
      if (!(error instanceof UserError)) console.error(`[core] ${command.type} failed`, error);
      send(ws, { type: "response", requestId: command.requestId, ok: false, error: errorMessage(error) });
    }
  }

  const server = createServer((req, res) => {
    const { pathname } = new URL(req.url ?? "/", "http://localhost");
    if (pathname === "/health") {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify({ ok: true, coreVersion: pkg.version, protocolVersion: PROTOCOL_VERSION, pid: process.pid }));
      return;
    }
    res.writeHead(pathname === "/ws" ? 426 : 404).end();
  });

  const wss = new WebSocketServer({
    noServer: true,
    maxPayload: 64 * 1024 * 1024,
    handleProtocols: (protocols) => (protocols.has(expectedProtocol) ? expectedProtocol : false),
  });

  server.on("upgrade", (req: IncomingMessage, socket, head) => {
    const reject = (status: number, text: string) => {
      socket.end(`HTTP/1.1 ${status} ${text}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n`);
    };
    if (new URL(req.url ?? "/", "http://localhost").pathname !== "/ws") return reject(404, "Not Found");
    if (!originAllowed(req.headers.origin)) return reject(403, "Forbidden");
    const protocols = (req.headers["sec-websocket-protocol"] ?? "").split(",").map((item) => item.trim());
    if (!protocols.includes(expectedProtocol)) return reject(401, "Unauthorized");
    wss.handleUpgrade(req, socket, head, (ws) => {
      ws.on("message", (data) => void handle(ws, data.toString()));
      ws.on("close", () => watching.delete(ws));
      send(ws, { type: "hello", protocolVersion: PROTOCOL_VERSION, coreVersion: pkg.version });
    });
  });

  const hostname = options.hostname ?? "127.0.0.1";
  const { promise: listening, resolve, reject } = Promise.withResolvers<void>();
  server.once("error", reject);
  server.listen(options.port, hostname, () => resolve());
  await listening;
  const port = (server.address() as AddressInfo).port;

  return {
    url: `http://${hostname}:${port}`,
    port,
    async stop() {
      await host.dispose();
      for (const ws of wss.clients) ws.terminate();
      wss.close();
      const { promise, resolve: closed } = Promise.withResolvers<void>();
      server.close(() => closed());
      server.closeAllConnections();
      await promise;
    },
  };
}
