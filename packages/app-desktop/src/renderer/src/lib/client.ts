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

type Pending = { resolve: (data: unknown) => void; reject: (error: Error) => void };

const RETRY_DELAYS_MS = [300, 600, 1_200, 2_500, 5_000];

/** WebSocket client for core: authenticates with the token subprotocol, reconnects with backoff and pairs commands with responses. */
export class CoreClient {
  #socket?: WebSocket;
  #ready = false;
  #pending = new Map<string, Pending>();
  #attempt = 0;
  #timer?: number;
  #disposed = false;

  constructor(private readonly handlers: ClientHandlers) {}

  start(): void {
    this.#disposed = false;
    void this.#connect();
  }

  dispose(): void {
    this.#disposed = true;
    clearTimeout(this.#timer);
    const socket = this.#socket;
    this.#socket = undefined;
    socket?.close();
  }

  call<T extends CommandType>(command: Extract<CommandInput, { type: T }>): Promise<CommandResults[T]> {
    const socket = this.#socket;
    if (!socket || !this.#ready || socket.readyState !== WebSocket.OPEN) {
      return Promise.reject(new Error("与 core 的连接已断开，正在重连…"));
    }
    const requestId = crypto.randomUUID();
    const result = new Promise<CommandResults[T]>((resolve, reject) => {
      this.#pending.set(requestId, { resolve: resolve as (data: unknown) => void, reject });
    });
    socket.send(JSON.stringify({ ...command, requestId }));
    return result;
  }

  async #connect(): Promise<void> {
    if (this.#disposed) return;
    this.handlers.onState("connecting");
    let socket: WebSocket;
    try {
      const { url, token } = await window.lila.connection();
      if (this.#disposed) return;
      socket = new WebSocket(url, [`lila.${token}`]);
    } catch (error) {
      this.handlers.onState("disconnected", error instanceof Error ? error.message : String(error));
      this.#retry();
      return;
    }
    this.#socket = socket;
    this.#ready = false;
    socket.onmessage = (raw) => {
      if (this.#socket === socket) this.#receive(String(raw.data));
    };
    socket.onclose = () => {
      if (this.#socket !== socket) return;
      this.#socket = undefined;
      this.#ready = false;
      for (const pending of this.#pending.values()) pending.reject(new Error("与 core 的连接已断开"));
      this.#pending.clear();
      this.handlers.onState("disconnected");
      this.#retry();
    };
  }

  #retry(): void {
    if (this.#disposed) return;
    const delay = RETRY_DELAYS_MS[Math.min(this.#attempt, RETRY_DELAYS_MS.length - 1)];
    this.#attempt += 1;
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
        if (message.protocolVersion !== PROTOCOL_VERSION) {
          this.handlers.onState("incompatible", `core 协议版本 ${message.protocolVersion}，界面需要 ${PROTOCOL_VERSION}。`);
          this.#disposed = true;
          this.#socket?.close();
          return;
        }
        this.#attempt = 0;
        this.#ready = true;
        this.handlers.onState("connected");
        this.handlers.onReady();
        return;
      case "response": {
        const pending = this.#pending.get(message.requestId);
        if (!pending) return;
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
