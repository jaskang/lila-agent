import { timingSafeEqual } from "node:crypto";
import { createServer, type IncomingMessage } from "node:http";
import type { AddressInfo } from "node:net";
import { ClientCommand, type CommandResults, type CommandType, MAX_CORE_MESSAGE_BYTES, PROTOCOL_VERSION, type ServerMessage, type WatchSlot } from "@lila/protocol";
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
const MAX_CLIENTS = 64;
const AUTH_WINDOW_MS = 60_000;
const AUTH_MAX_FAILURES = 20;
const MAX_AUTH_IPS = 1_000;

function isLoopbackHost(hostname: string): boolean {
  const normalized = hostname.trim().toLowerCase();
  return normalized === "127.0.0.1" || normalized === "localhost" || normalized === "::1" || normalized === "[::1]";
}

function originAllowed(origin: string | undefined, allowNullOrigin: boolean): boolean {
  if (!origin || origin === "file://" || ALLOWED_ORIGIN.test(origin)) return true;
  return allowNullOrigin && origin === "null";
}

function protocolMatches(candidate: string, expected: Buffer): boolean {
  const actual = Buffer.from(candidate);
  return actual.length === expected.length && timingSafeEqual(actual, expected);
}

export async function startCore(options: CoreOptions): Promise<Core> {
  const hostname = options.hostname ?? "127.0.0.1";
  const loopback = isLoopbackHost(hostname);
  const watching = { main: new Map<WebSocket, string>(), preview: new Map<WebSocket, string>() };
  const watchSeq = { main: new WeakMap<WebSocket, number>(), preview: new WeakMap<WebSocket, number>() };
  const authFailures = new Map<string, { count: number; resetAt: number }>();

  const nextWatchSeq = (ws: WebSocket, slot: WatchSlot): number => {
    const next = (watchSeq[slot].get(ws) ?? 0) + 1;
    watchSeq[slot].set(ws, next);
    return next;
  };

  const unwatch = (ws: WebSocket, slot: WatchSlot) => {
    nextWatchSeq(ws, slot);
    watching[slot].delete(ws);
  };

  const isAuthRateLimited = (ip: string, now: number): boolean => {
    const entry = authFailures.get(ip);
    if (!entry) return false;
    if (now >= entry.resetAt) {
      authFailures.delete(ip);
      return false;
    }
    return entry.count >= AUTH_MAX_FAILURES;
  };

  const recordAuthFailure = (ip: string, now: number): void => {
    const existing = authFailures.get(ip);
    if (existing && now < existing.resetAt) {
      existing.count += 1;
      return;
    }
    if (authFailures.size >= MAX_AUTH_IPS) {
      for (const [key, entry] of authFailures) {
        if (now >= entry.resetAt) authFailures.delete(key);
      }
      if (authFailures.size >= MAX_AUTH_IPS) {
        const oldest = authFailures.keys().next().value;
        if (oldest !== undefined) authFailures.delete(oldest);
      }
    }
    authFailures.set(ip, { count: 1, resetAt: now + AUTH_WINDOW_MS });
  };

  const send = (ws: WebSocket, message: ServerMessage) => {
    if (ws.readyState === ws.OPEN) ws.send(JSON.stringify(message));
  };
  const clients: Clients = {
    toWatchers(sessionId, message) {
      const data = JSON.stringify(message);
      for (const ws of wss.clients) {
        if ((watching.main.get(ws) === sessionId || watching.preview.get(ws) === sessionId) && ws.readyState === ws.OPEN) ws.send(data);
      }
    },
    broadcast(message) {
      const data = JSON.stringify(message);
      for (const ws of wss.clients) if (ws.readyState === ws.OPEN) ws.send(data);
    },
    isWatched: (sessionId) => [...watching.main.values(), ...watching.preview.values()].includes(sessionId),
  };
  const host = new SessionHost(options.dataDir, clients);
  const expectedProtocol = `lila.${options.token}`;
  const expectedProtocolBytes = Buffer.from(expectedProtocol);

  async function execute(ws: WebSocket, command: ClientCommand): Promise<CommandResults[CommandType]> {
    switch (command.type) {
      case "ping":
        return { ok: true };
      case "projects.list":
        return { projects: [...host.store.projects] };
      case "projects.add":
        return { project: host.addProject(command.path) };
      case "projects.remove":
        await host.removeProject(command.projectId);
        return {};
      case "models.list":
        return await host.listModels(command.refresh ?? false);
      case "provider.models.set":
        return await host.setProviderModels(command.providerId, command.modelIds);
      case "settings.get":
        return { settings: await host.getSettings() };
      case "settings.update":
        return {
          settings: await host.updateSettings({
            defaultModelId: command.defaultModelId,
            defaultThinkingLevel: command.defaultThinkingLevel,
            autoCompaction: command.autoCompaction,
            autoRetry: command.autoRetry,
          }),
        };
      case "providers.list":
        return { providers: await host.listProviders() };
      case "auth.login.start": {
        let loginId = "";
        loginId = await host.startProviderLogin(ws, command.providerId, command.method, (event) => {
          send(ws, { type: "auth.login", loginId, providerId: command.providerId, event });
        });
        return { loginId };
      }
      case "auth.login.respond":
        host.respondProviderLogin(command.loginId, command.promptId, command.value);
        return {};
      case "auth.login.cancel":
        host.cancelProviderLogin(command.loginId);
        return {};
      case "auth.logout":
        await host.logoutProvider(command.providerId);
        return {};
      case "mcp.list":
        return { config: host.listMcp(command.projectId) };
      case "mcp.upsert":
        return { config: await host.upsertMcp(command.scope, command.name, command.config, command.projectId) };
      case "mcp.import":
        return { config: await host.importMcp(command.scope, command.entries, command.projectId) };
      case "mcp.toggle":
        return {
          config: await host.toggleMcp(
            command.scope,
            command.name,
            { enabled: command.enabled, exposure: command.exposure },
            command.projectId,
          ),
        };
      case "mcp.remove":
        return { config: await host.removeMcp(command.scope, command.name, command.projectId) };
      case "capabilities.get":
        return { capabilities: await host.getCapabilities(command.projectId, command.sessionId) };
      case "memory.get":
        return { memory: await host.getMemory(command.projectId, command.sessionId) };
      case "memory.add":
        return { memory: await host.addMemory(command.target, command.content) };
      case "memory.replace":
        return { memory: await host.replaceMemory(command.target, command.oldText, command.newContent) };
      case "memory.remove":
        return { memory: await host.removeMemory(command.target, command.oldText) };
      case "memory.config.update":
        return {
          memory: await host.updateMemorySettings({
            enabled: command.enabled,
            memoryCharLimit: command.memoryCharLimit,
            userCharLimit: command.userCharLimit,
            dreamingEnabled: command.dreamingEnabled,
            dreamingMinHours: command.dreamingMinHours,
            dreamingMinTurns: command.dreamingMinTurns,
            dreamingModelId: command.dreamingModelId,
          }),
        };
      case "packages.install":
        return { capabilities: await host.installPackage(command.source, command.scope, command.projectId) };
      case "packages.remove":
        return { capabilities: await host.removePackage(command.source, command.scope, command.projectId) };
      case "packages.update":
        return { capabilities: await host.updatePackages(command.source, command.projectId) };
      case "workspace.reload":
        await host.reloadWorkspace(command.projectId, command.sessionId);
        return {};
      case "usage.summary":
        return { summary: await host.getUsageSummary(command.range, command.onlyConfigured ?? true) };
      case "quotas.list":
        return { quotas: await host.listQuotas(command.refresh ?? false) };
      case "sessions.list":
        return { sessions: await host.listSessions() };
      case "session.create":
        return { session: await host.createSession(command) };
      case "session.unwatch":
        unwatch(ws, command.slot ?? "main");
        return {};
      case "files.search":
        return { files: await host.searchFiles(command.projectId, command.query, command.limit) };
      case "files.list":
        return await host.listProjectFiles(command.projectId, command.refresh);
      case "files.read":
        return { file: await host.readProjectFile(command.projectId, command.path) };
      case "git.status":
        return { status: await host.gitStatus(command.projectId) };
      case "git.diff":
        return { diff: await host.gitDiff(command.projectId, command.path, command.staged) };
      case "git.stage":
        return { status: await host.gitStage(command.projectId, command.paths) };
      case "git.unstage":
        return { status: await host.gitUnstage(command.projectId, command.paths) };
      case "git.discard":
        return { status: await host.gitDiscard(command.projectId, command.path) };
      case "git.commit":
        return await host.gitCommit(command.projectId, command.message);
      case "git.worktree.create":
        return await host.createWorktreeProject(command.projectId, command.branch);
      case "session.delete":
        await host.deleteSession(command.sessionId);
        return {};
      case "commands.list":
        return { commands: await host.commands(command.projectId, command.sessionId) };
      case "session.fork":
        return { session: await host.forkSession(command.sessionId, command.itemId) };
      case "session.retry":
        await host.retrySession(command.sessionId);
        return {};
      case "session.open": {
        const slot = command.slot ?? "main";
        const seq = nextWatchSeq(ws, slot);
        watching[slot].delete(ws);
        const view = await host.view(command.sessionId, true, slot === "preview");
        const snapshot = view.open(command.limit);
        if (ws.readyState === ws.OPEN && watchSeq[slot].get(ws) === seq) watching[slot].set(ws, command.sessionId);
        return snapshot;
      }
      case "session.earlier":
        return (await host.view(command.sessionId)).earlier(command.beforeId, command.limit);
      case "session.search":
        return (await host.view(command.sessionId)).search(command.query, command.offset);
      case "session.turns":
        return (await host.view(command.sessionId)).turns(command.offset);
      case "session.around":
        return { items: (await host.view(command.sessionId)).around(command.itemId) };
      default:
        break;
    }
    const live = await host.live(command.sessionId);
    switch (command.type) {
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
        live.compact(command.instructions);
        return {};
      case "session.rewind":
        return { text: await live.rewind(command.itemId) };
      case "session.respond":
        live.respond(command.pendingId, command.answer);
        return {};
      case "session.rules.update":
        return { state: live.updateRules(command.scope, command.action, command.pattern) };
      case "session.subagent.stop":
        await live.stopSubagent(command.agentId);
        return {};
      case "session.subagent.steer":
        await live.steerSubagent(command.agentId, command.message);
        return {};
      case "session.queue.update":
        return { state: await live.updateQueue(command.action, command.kind, command.index) };
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
      res.end(
        JSON.stringify({
          ok: true,
          app: "lila-core",
          coreVersion: pkg.version,
          protocolVersion: PROTOCOL_VERSION,
          ...(loopback ? { pid: process.pid } : {}),
        }),
      );
      return;
    }
    res.writeHead(pathname === "/ws" ? 426 : 404).end();
  });

  const wss = new WebSocketServer({
    noServer: true,
    maxPayload: MAX_CORE_MESSAGE_BYTES,
    handleProtocols: (protocols) => {
      for (const candidate of protocols) if (protocolMatches(candidate, expectedProtocolBytes)) return expectedProtocol;
      return false;
    },
  });

  server.on("upgrade", (req: IncomingMessage, socket, head) => {
    const reject = (status: number, text: string) => {
      socket.end(`HTTP/1.1 ${status} ${text}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n`);
    };
    if (new URL(req.url ?? "/", "http://localhost").pathname !== "/ws") return reject(404, "Not Found");
    if (!originAllowed(req.headers.origin, loopback)) return reject(403, "Forbidden");
    if (wss.clients.size >= MAX_CLIENTS) return reject(503, "Service Unavailable");
    const remoteIp = req.socket.remoteAddress ?? "unknown";
    const now = Date.now();
    if (isAuthRateLimited(remoteIp, now)) return reject(429, "Too Many Requests");
    const protocols = (req.headers["sec-websocket-protocol"] ?? "").split(",").map((item) => item.trim());
    if (!protocols.some((candidate) => protocolMatches(candidate, expectedProtocolBytes))) {
      recordAuthFailure(remoteIp, now);
      return reject(401, "Unauthorized");
    }
    authFailures.delete(remoteIp);
    wss.handleUpgrade(req, socket, head, (ws) => {
      ws.on("message", (data) => void handle(ws, data.toString()));
      ws.on("close", () => {
        unwatch(ws, "main");
        unwatch(ws, "preview");
        host.cancelLoginsForOwner(ws);
      });
      send(ws, { type: "hello", protocolVersion: PROTOCOL_VERSION, coreVersion: pkg.version });
    });
  });

  const { promise: listening, resolve, reject } = Promise.withResolvers<void>();
  server.once("error", reject);
  server.listen(options.port, hostname, () => resolve());
  await listening;
  const port = (server.address() as AddressInfo).port;
  const urlHost = hostname.includes(":") && !hostname.startsWith("[") ? `[${hostname}]` : hostname;

  return {
    url: `http://${urlHost}:${port}`,
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
