import { PROTOCOL_VERSION, type ClientCommand, type CommandResults, type CommandType, type ServerMessage } from "@lila/protocol";
import type { LilaShell } from "../../../preload/index.ts";

declare global {
  interface Window {
    lila: LilaShell;
  }
}

export type ConnectionState = "connecting" | "connected" | "disconnected" | "incompatible";

type DistributiveOmit<T, K extends PropertyKey> = T extends unknown ? Omit<T, K> : never;
export type CommandInput = DistributiveOmit<ClientCommand, "requestId">;

/** Frames core pushes on its own, everything except `hello` and `response`. */
export type PushMessage = Exclude<ServerMessage, { type: "hello" | "response" }>;

export interface ClientHandlers {
  onState(state: ConnectionState, detail?: string): void;
  /** Called after every (re)connect, once the hello is accepted. */
  onReady(): void;
  onPush(message: PushMessage): void;
}

type Pending = { resolve: (data: unknown) => void; reject: (error: Error) => void; timer: number };

const RETRY_DELAYS_MS = [300, 600, 1_200, 2_500, 5_000];
const DEFAULT_CALL_TIMEOUT_MS = 30_000;
const LONG_CALL_TIMEOUT_MS = 120_000;
const HELLO_TIMEOUT_MS = 8_000;
const HEARTBEAT_INTERVAL_MS = 25_000;
const HEARTBEAT_TIMEOUT_MS = 10_000;

const LONG_COMMANDS = new Set<CommandType>([
  "models.list",
  "quotas.list",
  "packages.install",
  "packages.remove",
  "packages.update",
  "auth.login.start",
  "session.reload",
  "workspace.reload",
  "session.compact",
  "usage.summary",
]);

/** WebSocket client for core: authenticates with the token subprotocol, reconnects with backoff and pairs commands with responses. */
export class CoreClient {
  #socket?: WebSocket;
  #ready = false;
  #connecting = false;
  #pending = new Map<string, Pending>();
  #attempt = 0;
  #timer?: number;
  #helloTimer?: number;
  #heartbeatTimer?: number;
  #lastError?: string;
  #disposed = false;

  constructor(private readonly handlers: ClientHandlers) {}

  start(): void {
    if (!this.#disposed && (this.#connecting || this.#socket)) return;
    this.#disposed = false;
    void this.#connect();
  }

  reconnectNow(): void {
    this.#disposed = false;
    this.#attempt = 0;
    this.#clearTimers();
    this.#rejectAll("正在重新连接 core…");
    const socket = this.#socket;
    this.#socket = undefined;
    this.#ready = false;
    this.#connecting = false;
    socket?.close();
    void this.#connect();
  }

  dispose(): void {
    this.#disposed = true;
    this.#connecting = false;
    this.#ready = false;
    this.#clearTimers();
    this.#rejectAll("客户端已关闭");
    const socket = this.#socket;
    this.#socket = undefined;
    socket?.close();
  }

  call<T extends CommandType>(command: Extract<CommandInput, { type: T }>, timeoutMs?: number): Promise<CommandResults[T]> {
    const socket = this.#socket;
    if (!socket || !this.#ready || socket.readyState !== WebSocket.OPEN) {
      return Promise.reject(new Error("与 core 的连接已断开，正在重连…"));
    }
    const requestId = crypto.randomUUID();
    const limit = timeoutMs ?? (LONG_COMMANDS.has(command.type) ? LONG_CALL_TIMEOUT_MS : DEFAULT_CALL_TIMEOUT_MS);
    const result = new Promise<CommandResults[T]>((resolve, reject) => {
      const timer = window.setTimeout(() => {
        if (!this.#pending.delete(requestId)) return;
        reject(new Error(`请求 ${command.type} 超时，请稍后重试`));
      }, limit);
      this.#pending.set(requestId, { resolve: resolve as (data: unknown) => void, reject, timer });
    });
    try {
      socket.send(JSON.stringify({ ...command, requestId }));
    } catch (error) {
      const pending = this.#pending.get(requestId);
      if (pending) {
        clearTimeout(pending.timer);
        this.#pending.delete(requestId);
      }
      return Promise.reject(error instanceof Error ? error : new Error(String(error)));
    }
    return result;
  }

  #clearTimers(): void {
    clearTimeout(this.#timer);
    clearTimeout(this.#helloTimer);
    clearInterval(this.#heartbeatTimer);
    this.#timer = undefined;
    this.#helloTimer = undefined;
    this.#heartbeatTimer = undefined;
  }

  #rejectAll(message: string): void {
    for (const pending of this.#pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(new Error(message));
    }
    this.#pending.clear();
  }

  #startHeartbeat(socket: WebSocket): void {
    clearInterval(this.#heartbeatTimer);
    this.#heartbeatTimer = window.setInterval(() => {
      if (this.#socket !== socket || !this.#ready || socket.readyState !== WebSocket.OPEN) return;
      void this.call({ type: "ping" }, HEARTBEAT_TIMEOUT_MS).catch(() => {
        if (this.#socket === socket) {
          this.#lastError = "core 心跳超时，正在重新连接…";
          socket.close();
        }
      });
    }, HEARTBEAT_INTERVAL_MS);
  }

  async #connect(): Promise<void> {
    if (this.#disposed || this.#connecting) return;
    this.#connecting = true;
    this.handlers.onState("connecting", this.#lastError);
    let socket: WebSocket;
    try {
      const { url, token } = await window.lila.connection();
      if (this.#disposed) {
        this.#connecting = false;
        return;
      }
      socket = new WebSocket(url, [`lila.${token}`]);
    } catch (error) {
      this.#connecting = false;
      this.#lastError = error instanceof Error ? error.message : String(error);
      this.handlers.onState("disconnected", this.#lastError);
      this.#retry();
      return;
    }
    this.#socket = socket;
    this.#ready = false;
    clearTimeout(this.#helloTimer);
    this.#helloTimer = window.setTimeout(() => {
      if (this.#socket === socket && !this.#ready) {
        this.#lastError = "连接 core 握手超时，正在重试…";
        socket.close();
      }
    }, HELLO_TIMEOUT_MS);

    socket.onmessage = (raw) => {
      if (this.#socket === socket) this.#receive(String(raw.data));
    };
    socket.onclose = () => {
      if (this.#socket !== socket) return;
      this.#socket = undefined;
      this.#ready = false;
      this.#connecting = false;
      clearTimeout(this.#helloTimer);
      clearInterval(this.#heartbeatTimer);
      this.#helloTimer = undefined;
      this.#heartbeatTimer = undefined;
      this.#rejectAll("与 core 的连接已断开");
      this.handlers.onState("disconnected", this.#lastError);
      this.#retry();
    };
  }

  #retry(): void {
    if (this.#disposed) return;
    const delay = RETRY_DELAYS_MS[Math.min(this.#attempt, RETRY_DELAYS_MS.length - 1)];
    this.#attempt += 1;
    clearTimeout(this.#timer);
    this.#timer = window.setTimeout(() => void this.#connect(), delay);
  }

  #receive(raw: string): void {
    let message: ServerMessage;
    try {
      message = JSON.parse(raw) as ServerMessage;
    } catch {
      console.warn("[lila] unreadable message from core", raw.slice(0, 200));
      return;
    }
    switch (message.type) {
      case "hello":
        clearTimeout(this.#helloTimer);
        this.#helloTimer = undefined;
        this.#connecting = false;
        if (message.protocolVersion !== PROTOCOL_VERSION) {
          this.handlers.onState("incompatible", `core 协议版本 ${message.protocolVersion}，界面需要 ${PROTOCOL_VERSION}。`);
          this.#disposed = true;
          this.#socket?.close();
          return;
        }
        this.#attempt = 0;
        this.#lastError = undefined;
        this.#ready = true;
        if (this.#socket) this.#startHeartbeat(this.#socket);
        this.handlers.onState("connected");
        this.handlers.onReady();
        return;
      case "response": {
        const pending = this.#pending.get(message.requestId);
        if (!pending) return;
        clearTimeout(pending.timer);
        this.#pending.delete(message.requestId);
        if (message.ok) pending.resolve(message.data);
        else pending.reject(new Error(message.error));
        return;
      }
      case "error":
        console.warn(`[lila] core error ${message.code}: ${message.message}`);
        return;
      default:
        if (this.#ready) this.handlers.onPush(message);
    }
  }
}
