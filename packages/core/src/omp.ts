import type { ApprovalMode } from "@lila/protocol";

const MAX_RPC_FRAME_BYTES = 1_048_576;
const MAX_RPC_REASSEMBLED_BYTES = 67_108_864;
const STARTUP_TIMEOUT_MS = 10_000;
const STOP_TIMEOUT_MS = 1_500;

export type OmpFrame = Record<string, unknown> & { type: string };
type OmpProcess = Bun.Subprocess<"pipe", "pipe", "pipe">;

export interface OmpSessionOptions {
  cwd: string;
  approvalMode: ApprovalMode;
  modelId?: string;
  sessionFile?: string;
  onFrame: (frame: OmpFrame) => void;
  onExit: (code: number) => void;
}

type PendingChunks = {
  chunkId: string;
  count: number;
  byteLength: number;
  nextIndex: number;
  totalBytes: number;
  chunks: Buffer[];
};

class RpcChunkDecoder {
  #pending?: PendingChunks;

  push(frame: OmpFrame): OmpFrame | undefined {
    if (frame.type !== "rpc_chunk") {
      if (this.#pending) throw new Error("rpc chunk sequence interrupted");
      return frame;
    }

    const chunkId = frame.chunkId;
    const count = frame.count;
    const index = frame.index;
    const byteLength = frame.byteLength;
    const data = frame.data;
    if (
      typeof chunkId !== "string" ||
      chunkId.length === 0 ||
      typeof count !== "number" ||
      typeof index !== "number" ||
      typeof byteLength !== "number" ||
      typeof data !== "string" ||
      !Number.isSafeInteger(count) ||
      count < 1 ||
      count > MAX_RPC_REASSEMBLED_BYTES ||
      !Number.isSafeInteger(index) ||
      index < 0 ||
      index >= count ||
      !Number.isSafeInteger(byteLength) ||
      byteLength < 1 ||
      byteLength > MAX_RPC_REASSEMBLED_BYTES ||
      data.length === 0 ||
      data.length % 4 !== 0 ||
      !/^[A-Za-z0-9+/]*={0,2}$/.test(data)
    ) {
      throw new Error("invalid rpc chunk bounds");
    }

    const bytes = Buffer.from(data, "base64");
    if (bytes.length === 0 || bytes.length > byteLength) throw new Error("invalid rpc chunk length");

    let pending = this.#pending;
    if (!pending) {
      if (index !== 0) throw new Error("rpc chunks must start at index 0");
      pending = { chunkId, count, byteLength, nextIndex: 0, totalBytes: 0, chunks: [] };
      this.#pending = pending;
    }
    if (
      pending.chunkId !== chunkId ||
      pending.count !== count ||
      pending.byteLength !== byteLength ||
      pending.nextIndex !== index
    ) {
      throw new Error("rpc chunk sequence mismatch");
    }
    if (pending.totalBytes > byteLength - bytes.length) throw new Error("rpc chunk byte length exceeded");

    pending.chunks.push(bytes);
    pending.totalBytes += bytes.length;
    pending.nextIndex++;
    if (pending.nextIndex < pending.count) return undefined;
    if (pending.totalBytes !== pending.byteLength) throw new Error("rpc chunk byte length mismatch");

    this.#pending = undefined;
    let decoded: string;
    try {
      decoded = new TextDecoder("utf-8", { fatal: true }).decode(Buffer.concat(pending.chunks, pending.byteLength));
    } catch (error) {
      throw new Error(`invalid rpc chunk UTF-8: ${String(error)}`);
    }
    const parsed: unknown = JSON.parse(decoded);
    if (!isFrame(parsed)) throw new Error("invalid rpc chunk frame");
    return parsed;
  }
}

type PendingResponse = {
  resolve: (frame: OmpFrame) => void;
  reject: (error: Error) => void;
};

function asError(error: unknown): Error {
  return error instanceof Error ? error : new Error(String(error));
}

function isFrame(value: unknown): value is OmpFrame {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  return "type" in value && typeof value.type === "string";
}

function modelParts(modelId: string): { provider: string; modelId: string } {
  const separator = modelId.indexOf("::");
  if (separator <= 0 || separator === modelId.length - 2) {
    throw new Error(`modelId must use provider::id encoding: ${modelId}`);
  }
  return { provider: modelId.slice(0, separator), modelId: modelId.slice(separator + 2) };
}

export class OmpSession {
  readonly #options: OmpSessionOptions;
  #process?: OmpProcess;
  #readPromise?: Promise<void>;
  #readyPromise: Promise<void>;
  #resolveReady!: () => void;
  #rejectReady!: (error: Error) => void;
  #readySettled = false;
  #started = false;
  #initializationStarted = false;
  #stopping = false;
  #exitNotified = false;
  #pendingResponses = new Map<string, PendingResponse>();

  constructor(options: OmpSessionOptions) {
    this.#options = options;
    const ready = Promise.withResolvers<void>();
    this.#readyPromise = ready.promise;
    this.#resolveReady = () => {
      if (this.#readySettled) return;
      this.#readySettled = true;
      ready.resolve();
    };
    this.#rejectReady = (error: Error) => {
      if (this.#readySettled) return;
      this.#readySettled = true;
      ready.reject(error);
    };
  }

  async start(): Promise<void> {
    if (this.#started) return this.#readyPromise;
    this.#started = true;
    const command = [
      process.env.LILA_OMP_BIN ?? "omp",
      "--mode",
      "rpc-ui",
      "--approval-mode",
      this.#options.approvalMode,
      ...(this.#options.sessionFile ? ["--resume", this.#options.sessionFile] : []),
    ];
    try {
      this.#process = Bun.spawn(command, {
        cwd: this.#options.cwd,
        stdin: "pipe",
        stdout: "pipe",
        stderr: "pipe",
      });
      this.#readPromise = this.#readOutput();
      void this.#drainStderr(this.#process);
      let timeout: ReturnType<typeof setTimeout> | undefined;
      try {
        await Promise.race([
          this.#readyPromise,
          new Promise<never>((_, reject) => {
            timeout = setTimeout(() => reject(new Error("omp startup timed out")), STARTUP_TIMEOUT_MS);
          }),
        ]);
      } finally {
        if (timeout) clearTimeout(timeout);
      }
    } catch (error) {
      const startupError = asError(error);
      this.#fail(startupError);
      await this.#terminate();
      throw startupError;
    }
  }

  async prompt(id: string, message: string): Promise<void> {
    await this.#send({ id, type: "prompt", message });
  }

  async abort(id: string): Promise<void> {
    await this.#send({ id, type: "abort" });
  }

  async respond(id: string, response: Record<string, unknown>): Promise<void> {
    await this.#send({ type: "extension_ui_response", id, ...response });
  }

  async request(type: string, payload: Record<string, unknown> = {}): Promise<OmpFrame> {
    await this.#readyPromise;
    const id = crypto.randomUUID();
    return this.#request({ ...payload, id, type });
  }

  async stop(): Promise<void> {
    if (!this.#process) return;
    this.#stopping = true;
    this.#fail(new Error("omp session stopped"));
    await this.#terminate();
  }

  async #send(frame: Record<string, unknown>): Promise<void> {
    const process = this.#process;
    if (!process || this.#stopping) throw new Error("omp process is not running");
    await process.stdin.write(`${JSON.stringify(frame)}\n`);
    await process.stdin.flush();
  }

  #request(frame: Record<string, unknown>): Promise<OmpFrame> {
    const id = frame.id;
    if (typeof id !== "string" || id.length === 0) throw new Error("omp request requires an id");
    if (this.#pendingResponses.has(id)) throw new Error(`duplicate omp request id: ${id}`);
    const response = Promise.withResolvers<OmpFrame>();
    this.#pendingResponses.set(id, { resolve: response.resolve, reject: response.reject });
    void this.#send(frame).catch((error) => {
      const pending = this.#pendingResponses.get(id);
      if (!pending) return;
      this.#pendingResponses.delete(id);
      pending.reject(asError(error));
      this.#fail(asError(error));
    });
    return response.promise;
  }

  #handleFrame(frame: OmpFrame): void {
    if (frame.type === "ready" && !this.#initializationStarted) {
      this.#initializationStarted = true;
      void this.#initialize().catch((error) => {
        this.#fail(asError(error));
      });
    }

    if (frame.type === "response" && typeof frame.id === "string") {
      const pending = this.#pendingResponses.get(frame.id);
      if (pending) {
        this.#pendingResponses.delete(frame.id);
        if (frame.success === false) {
          pending.reject(new Error(typeof frame.error === "string" ? frame.error : "omp request failed"));
        } else if (frame.success === true) {
          pending.resolve(frame);
        } else {
          pending.reject(new Error("invalid omp response"));
        }
      }
    }
    try {
      this.#options.onFrame(frame);
    } catch (error) {
      this.#fail(asError(error));
    }
  }

  async #initialize(): Promise<void> {
    await this.#request({ id: "lila-negotiate", type: "negotiate_protocol", protocolVersion: 2 });
    await this.#request({ id: "lila-ask-dialog", type: "set_ask_dialog", enabled: true });
    if (this.#options.modelId) {
      const { provider, modelId } = modelParts(this.#options.modelId);
      await this.#request({ id: "lila-model", type: "set_model", provider, modelId });
    }
    this.#resolveReady();
  }

  async #readOutput(): Promise<void> {
    const process = this.#process;
    if (!process) return;
    const decoder = new TextDecoder();
    const chunks = new RpcChunkDecoder();
    let buffer = "";
    try {
      for await (const chunk of process.stdout) {
        buffer += decoder.decode(chunk, { stream: true });
        if (Buffer.byteLength(buffer) > MAX_RPC_FRAME_BYTES) throw new Error("omp frame exceeds transport limit");
        let newline = buffer.indexOf("\n");
        while (newline >= 0) {
          const line = buffer.slice(0, newline);
          buffer = buffer.slice(newline + 1);
          if (Buffer.byteLength(line) > MAX_RPC_FRAME_BYTES) throw new Error("omp frame exceeds transport limit");
          newline = buffer.indexOf("\n");
          if (!line.trim()) continue;
          const parsed: unknown = JSON.parse(line);
          if (!isFrame(parsed)) throw new Error("invalid omp frame");
          const decoded = chunks.push(parsed);
          if (decoded) this.#handleFrame(decoded);
        }
      }
      buffer += decoder.decode();
      if (buffer.trim()) throw new Error("omp stdout ended with an incomplete frame");
      this.#fail(new Error("omp stdout closed"));
    } catch (error) {
      this.#fail(asError(error));
      try {
        this.#options.onFrame({ type: "adapter_error", error: String(error) });
      } catch {
        // The process lifecycle must still settle if the consumer callback fails.
      }
    } finally {
      if (!this.#stopping) process.kill("SIGTERM");
      let code = await Promise.race([
        process.exited,
        new Promise<undefined>((resolve) => setTimeout(() => resolve(undefined), STOP_TIMEOUT_MS)),
      ]);
      if (code === undefined) {
        process.kill("SIGKILL");
        code = await Promise.race([
          process.exited,
          new Promise<undefined>((resolve) => setTimeout(() => resolve(undefined), STOP_TIMEOUT_MS)),
        ]);
      }
      if (code === undefined) code = -1;
      this.#fail(new Error(`omp exited with code ${code}`));
      this.#notifyExit(code);
    }
  }

  async #drainStderr(process: OmpProcess): Promise<void> {
    try {
      for await (const _chunk of process.stderr) {
        // Keep stderr flowing; RPC frames are the structured diagnostics channel.
      }
    } catch {
      // stderr closure is reported through process.exited/stdout lifecycle.
    }
  }

  async #terminate(): Promise<void> {
    const process = this.#process;
    if (!process) return;
    try {
      await process.stdin.end();
    } catch {
      // The process may already have closed stdin.
    }
    let exited = await Promise.race([
      process.exited.then(() => true),
      new Promise<false>((resolve) => setTimeout(() => resolve(false), STOP_TIMEOUT_MS)),
    ]);
    if (!exited) {
      process.kill("SIGTERM");
      exited = await Promise.race([
        process.exited.then(() => true),
        new Promise<false>((resolve) => setTimeout(() => resolve(false), STOP_TIMEOUT_MS)),
      ]);
    }
    if (!exited) {
      process.kill("SIGKILL");
      await Promise.race([process.exited, new Promise<void>((resolve) => setTimeout(resolve, STOP_TIMEOUT_MS))]);
    }
    if (this.#readPromise) {
      await Promise.race([this.#readPromise, new Promise<void>((resolve) => setTimeout(resolve, STOP_TIMEOUT_MS))]);
    }
  }

  #fail(error: Error): void {
    this.#rejectReady(error);
    for (const [id, pending] of this.#pendingResponses) {
      this.#pendingResponses.delete(id);
      pending.reject(error);
    }
  }

  #notifyExit(code: number): void {
    if (this.#exitNotified) return;
    this.#exitNotified = true;
    try {
      this.#options.onExit(code);
    } catch {
      // Do not turn an already-settled child lifecycle into an unhandled rejection.
    }
  }
}
