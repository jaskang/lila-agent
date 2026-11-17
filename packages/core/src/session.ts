import { isAbsolute, relative } from "node:path";
import type {
  FileDiff,
  LiveSnapshot,
  PendingAnswer,
  PendingKind,
  PendingQuestion,
  SessionEventBody,
  StreamMessage,
} from "@lila/protocol";
import type { CreateOmp, OmpFrame, OmpRuntime } from "./omp.ts";
import type { SessionRecord } from "./store.ts";

type DistributiveOmit<T, K extends PropertyKey> = T extends unknown ? Omit<T, K> : never;
export type StreamBody = DistributiveOmit<StreamMessage, "type" | "sessionId" | "runId">;
export type SessionPatch = Partial<Pick<SessionRecord, "title" | "approvalMode" | "status" | "ompSessionFile">> & {
  modelId?: string | null;
};

export interface RunnerContext {
  createOmp: CreateOmp;
  /** Persists the event and pushes it to clients watching the session. */
  emit(body: SessionEventBody, runId?: string): void;
  /** Pushes a transient frame to clients watching the session. */
  stream(runId: string, body: StreamBody): void;
  /** Persists the patch and broadcasts the session's list entry. */
  update(patch: SessionPatch): SessionRecord;
  /** Stop an idle omp process after this long; the next message resumes it from its session file. */
  idleTimeoutMs: number;
}

type PendingEntry = { rawId: string; kind: PendingKind; options: string[] };
type LiveMessage = { text: string; thinking: string };
type LiveTool = { toolName: string; output: string; diffs: FileDiff[] };

/** omp methods that wait for an answer; everything else in `extension_ui_request` is a notification. */
const DIALOG_METHODS: Record<string, true> = { confirm: true, select: true, input: true, editor: true, ask: true };
const MAX_ARG_CHARS = 4_000;
const MAX_OUTPUT_CHARS = 20_000;
const MAX_LIVE_OUTPUT_CHARS = 8_000;

function text(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function record(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined;
}

function strings(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((item) => {
    if (typeof item === "string") return [item];
    const label = text(record(item)?.label);
    return label ? [label] : [];
  });
}

function clipMiddle(value: string, max: number): string {
  if (value.length <= max) return value;
  const head = Math.floor(max * 0.2);
  const tail = max - head;
  return `${value.slice(0, head)}\n… 省略 ${value.length - max} 个字符 …\n${value.slice(-tail)}`;
}

function clipTail(value: string, max: number): string {
  return value.length <= max ? value : `…\n${value.slice(-max)}`;
}

function clipArgs(args: unknown): Record<string, unknown> {
  const value = record(args) ?? {};
  return Object.fromEntries(
    Object.entries(value).map(([key, item]) => [key, typeof item === "string" ? clipMiddle(item, MAX_ARG_CHARS) : item]),
  );
}

/** Joins the text blocks of an omp tool result (`{ content: [{ type: "text", text }] }`). */
function resultText(result: unknown): string {
  const content = record(result)?.content;
  if (!Array.isArray(content)) return typeof result === "string" ? result : "";
  return content
    .flatMap((block) => (record(block)?.type === "text" ? [text(record(block)?.text) ?? ""] : []))
    .join("\n");
}

function messageText(message: Record<string, unknown>, type: "text" | "thinking"): string {
  const content = message.content;
  if (!Array.isArray(content)) return "";
  return content
    .flatMap((block) => {
      const value = record(block);
      return value?.type === type ? [text(value[type]) ?? ""] : [];
    })
    .join("")
    .trim();
}

function errorMessage(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  if (message.includes("Model not found")) return "所选模型已不可用，请在顶部重新选择模型后再发送。";
  if (message.includes("ENOENT") || message.includes("Executable not found")) return "找不到 omp 可执行文件，请确认 omp 已安装并在 PATH 中。";
  return message;
}

function answerLabel(answer: PendingAnswer): string | undefined {
  switch (answer.type) {
    case "cancel":
      return undefined;
    case "option":
    case "text":
      return clipMiddle(answer.value, 200);
    case "confirm":
      return answer.confirmed ? "确认" : "拒绝";
    case "ask":
      return answer.answers.map((item) => [...item.selectedOptions, item.customInput].filter(Boolean).join("、")).join("；");
  }
}

/**
 * One Lila session: owns its omp process, turns omp RPC frames into Lila events,
 * and serializes the commands that change it.
 */
export class SessionRunner {
  #record: SessionRecord;
  readonly #cwd: string;
  readonly #ctx: RunnerContext;
  #omp?: OmpRuntime;
  #starting = false;
  #queue: Promise<unknown> = Promise.resolve();
  #idleTimer?: ReturnType<typeof setTimeout>;

  #runId?: string;
  #lastError?: string;
  #lastToolCallId?: string;
  readonly #pending = new Map<string, PendingEntry>();
  readonly #pendingByRawId = new Map<string, string>();
  readonly #liveMessages = new Map<string, LiveMessage>();
  readonly #liveTools = new Map<string, LiveTool>();

  constructor(record: SessionRecord, cwd: string, ctx: RunnerContext) {
    this.#record = record;
    this.#cwd = cwd;
    this.#ctx = ctx;
  }

  get pendingCount(): number {
    return this.#pending.size;
  }

  live(): LiveSnapshot | undefined {
    if (!this.#runId) return undefined;
    return {
      runId: this.#runId,
      messages: [...this.#liveMessages].map(([messageId, message]) => ({ messageId, ...message })),
      tools: [...this.#liveTools].map(([toolCallId, tool]) => ({ toolCallId, ...tool })),
    };
  }

  send(message: string): Promise<string> {
    return this.#serial(async () => {
      if (this.#runId) throw new Error("上一个任务还在运行，请等它结束或先停止。");
      this.#clearIdleTimer();
      const runId = crypto.randomUUID();
      this.#runId = runId;
      this.#lastError = undefined;
      if (!this.#record.title) this.#update({ title: message.replace(/\s+/g, " ").trim().slice(0, 60) });
      this.#ctx.emit({ eventType: "message.user", payload: { text: message } }, runId);
      this.#ctx.emit({ eventType: "run.started", payload: {} }, runId);
      this.#update({ status: "running" });
      try {
        const omp = await this.#ensureOmp();
        await omp.prompt(runId, message);
      } catch (error) {
        if (this.#runId === runId) this.#finishRun({ eventType: "run.failed", payload: { error: errorMessage(error) } });
      }
      return runId;
    });
  }

  abort(): Promise<void> {
    return this.#serial(async () => {
      if (!this.#runId) throw new Error("没有正在运行的任务。");
      if (!this.#omp) {
        this.#finishRun({ eventType: "run.aborted", payload: {} });
        return;
      }
      await this.#omp.abort(crypto.randomUUID());
    });
  }

  respond(pendingId: string, answer: PendingAnswer): Promise<void> {
    return this.#serial(async () => {
      const pending = this.#pending.get(pendingId);
      const omp = this.#omp;
      if (!pending || !omp) throw new Error("这个请求已经失效。");
      const response = this.#toOmpResponse(pending, answer);
      // Record the answer before omp can react to it, so the log keeps cause before effect.
      this.#resolvePending(pendingId, answer.type === "cancel" ? "cancelled" : "answered", answerLabel(answer));
      await omp.respond(pending.rawId, response);
    });
  }

  configure(patch: Pick<SessionPatch, "modelId" | "approvalMode">): Promise<SessionRecord> {
    return this.#serial(async () => {
      if (this.#runId) throw new Error("任务运行中不能修改模型或审批方式。");
      // Model and approval mode are omp start parameters; the next message restarts omp from its session file.
      await this.#stopOmp();
      const record = this.#update(patch);
      this.#ctx.emit({
        eventType: "session.updated",
        payload: { ...(record.modelId ? { modelId: record.modelId } : {}), approvalMode: record.approvalMode },
      });
      return record;
    });
  }

  dispose(): Promise<void> {
    this.#clearIdleTimer();
    return this.#stopOmp();
  }

  #serial<T>(task: () => Promise<T>): Promise<T> {
    const result = this.#queue.then(task);
    this.#queue = result.catch(() => undefined);
    return result;
  }

  #update(patch: SessionPatch): SessionRecord {
    this.#record = this.#ctx.update(patch);
    return this.#record;
  }

  async #ensureOmp(): Promise<OmpRuntime> {
    if (this.#omp) return this.#omp;
    const omp: OmpRuntime = this.#ctx.createOmp({
      cwd: this.#cwd,
      approvalMode: this.#record.approvalMode,
      ...(this.#record.modelId ? { modelId: this.#record.modelId } : {}),
      ...(this.#record.ompSessionFile ? { sessionFile: this.#record.ompSessionFile } : {}),
      onFrame: (frame) => {
        if (this.#omp === omp) this.#handleFrame(omp, frame);
      },
      onExit: (code) => {
        if (this.#omp === omp && !this.#starting) this.#handleExit(code);
      },
    });
    this.#omp = omp;
    this.#starting = true;
    try {
      await omp.start();
      const state = record((await omp.request("get_state")).data);
      const sessionFile = text(state?.sessionFile);
      if (sessionFile && sessionFile !== this.#record.ompSessionFile) this.#update({ ompSessionFile: sessionFile });
      return omp;
    } catch (error) {
      if (this.#omp === omp) this.#omp = undefined;
      await omp.stop().catch(() => undefined);
      throw error;
    } finally {
      this.#starting = false;
    }
  }

  async #stopOmp(): Promise<void> {
    const omp = this.#omp;
    if (!omp) return;
    this.#omp = undefined;
    await omp.stop();
  }

  #clearIdleTimer(): void {
    clearTimeout(this.#idleTimer);
    this.#idleTimer = undefined;
  }

  #scheduleIdleStop(): void {
    this.#clearIdleTimer();
    this.#idleTimer = setTimeout(() => {
      void this.#serial(async () => {
        if (!this.#runId) await this.#stopOmp();
      });
    }, this.#ctx.idleTimeoutMs);
    this.#idleTimer.unref?.();
  }

  #handleFrame(omp: OmpRuntime, frame: OmpFrame): void {
    const runId = this.#runId;
    switch (frame.type) {
      case "message_update": {
        const update = record(frame.assistantMessageEvent);
        const delta = text(update?.delta);
        const channel = update?.type === "text_delta" ? "text" : update?.type === "thinking_delta" ? "thinking" : undefined;
        if (!runId || !channel || !delta || typeof frame.messageId !== "string") return;
        const messageId = `${runId}:${frame.messageId}`;
        const live = this.#liveMessages.get(messageId) ?? { text: "", thinking: "" };
        live[channel] += delta;
        this.#liveMessages.set(messageId, live);
        this.#ctx.stream(runId, { kind: "assistant.delta", messageId, channel, delta });
        return;
      }
      case "message_end": {
        const message = record(frame.message);
        if (!runId || message?.role !== "assistant" || typeof frame.messageId !== "string") return;
        const messageId = `${runId}:${frame.messageId}`;
        this.#liveMessages.delete(messageId);
        if (message.stopReason === "error") this.#lastError = text(message.errorMessage) ?? this.#lastError;
        const body = messageText(message, "text");
        const thinking = messageText(message, "thinking");
        if (!body && !thinking) return;
        this.#ctx.emit(
          {
            eventType: "assistant.message",
            payload: { messageId, text: body, thinking, ...(text(message.model) ? { model: text(message.model) } : {}) },
          },
          runId,
        );
        return;
      }
      case "tool_stream_update": {
        const toolCallId = text(frame.toolCallId);
        const files = record(frame.update)?.files;
        if (!runId || !toolCallId || !Array.isArray(files)) return;
        const tool = this.#liveTool(toolCallId, text(frame.toolName));
        tool.diffs = this.#diffs(files);
        this.#ctx.stream(runId, { kind: "tool.progress", toolCallId, toolName: tool.toolName, diffs: tool.diffs });
        return;
      }
      case "tool_execution_start": {
        const toolCallId = text(frame.toolCallId);
        if (!runId || !toolCallId) return;
        this.#lastToolCallId = toolCallId;
        const tool = this.#liveTool(toolCallId, text(frame.toolName));
        this.#ctx.emit(
          {
            eventType: "tool.started",
            payload: {
              toolCallId,
              toolName: tool.toolName,
              ...(text(frame.intent) ? { intent: text(frame.intent) } : {}),
              args: clipArgs(frame.args),
            },
          },
          runId,
        );
        return;
      }
      case "tool_execution_update": {
        const toolCallId = text(frame.toolCallId);
        if (!runId || !toolCallId) return;
        const tool = this.#liveTool(toolCallId, text(frame.toolName));
        tool.output = clipTail(resultText(frame.partialResult), MAX_LIVE_OUTPUT_CHARS);
        this.#ctx.stream(runId, { kind: "tool.progress", toolCallId, toolName: tool.toolName, output: tool.output });
        return;
      }
      case "tool_execution_end": {
        const toolCallId = text(frame.toolCallId);
        if (!runId || !toolCallId) return;
        const live = this.#liveTools.get(toolCallId);
        this.#liveTools.delete(toolCallId);
        const details = record(record(frame.result)?.details);
        const finalDiff = text(details?.diff);
        const diffs = finalDiff
          ? this.#diffs([{ path: details?.path, diff: finalDiff, op: details?.op }])
          : (live?.diffs ?? []);
        this.#ctx.emit(
          {
            eventType: "tool.finished",
            payload: {
              toolCallId,
              isError: frame.isError === true,
              output: clipMiddle(resultText(frame.result), MAX_OUTPUT_CHARS),
              diffs,
            },
          },
          runId,
        );
        return;
      }
      case "extension_ui_request":
        this.#handleUiRequest(omp, frame);
        return;
      case "prompt_result": {
        if (!runId || (typeof frame.id === "string" && frame.id !== runId)) return;
        if (frame.status === "completed") this.#finishRun({ eventType: "run.finished", payload: {} });
        else if (frame.status === "aborted") this.#finishRun({ eventType: "run.aborted", payload: {} });
        else
          this.#finishRun({
            eventType: "run.failed",
            payload: { error: text(frame.error) ?? this.#lastError ?? "模型返回了错误，任务未完成。" },
          });
        return;
      }
      case "adapter_error":
        this.#lastError = text(frame.error);
        return;
    }
  }

  #handleUiRequest(omp: OmpRuntime, frame: OmpFrame): void {
    const method = text(frame.method) ?? "";
    if (method === "cancel") {
      const pendingId = this.#pendingByRawId.get(text(frame.targetId) ?? "");
      if (pendingId) this.#resolvePending(pendingId, "cancelled");
      return;
    }
    const rawId = text(frame.id);
    if (!rawId || !DIALOG_METHODS[method]) return;
    const runId = this.#runId;
    if (!runId) {
      // Nobody is watching a run that does not exist; let omp continue instead of hanging.
      void omp.respond(rawId, { cancelled: true }).catch(() => undefined);
      return;
    }
    const rawTitle = text(frame.title) ?? "";
    const options = strings(frame.options);
    const isApproval = method === "select" && rawTitle.startsWith("Allow tool:");
    const kind: PendingKind = isApproval ? "approval" : method === "editor" ? "input" : (method as PendingKind);
    const [firstLine = "", ...restLines] = rawTitle.split("\n");
    const message = isApproval ? restLines.join("\n") : text(frame.message);
    const questions: PendingQuestion[] = Array.isArray(frame.questions)
      ? frame.questions.flatMap((item, index) => {
          const value = record(item);
          if (!value) return [];
          return [
            {
              id: text(value.id) ?? `q${index}`,
              question: text(value.question) ?? text(value.title) ?? "",
              options: strings(value.options),
              multiple: value.multiSelect === true || value.multiple === true,
            },
          ];
        })
      : [];
    const pendingId = crypto.randomUUID();
    this.#pending.set(pendingId, { rawId, kind, options });
    this.#pendingByRawId.set(rawId, pendingId);
    this.#ctx.emit(
      {
        eventType: "pending.created",
        payload: {
          pendingId,
          kind,
          title: isApproval ? firstLine : rawTitle,
          ...(message ? { message } : {}),
          options,
          questions,
          ...(isApproval && this.#lastToolCallId ? { toolCallId: this.#lastToolCallId } : {}),
          ...(text(frame.placeholder) ? { placeholder: text(frame.placeholder) } : {}),
          ...(text(frame.prefill ?? frame.defaultValue) ? { defaultValue: text(frame.prefill ?? frame.defaultValue) } : {}),
        },
      },
      runId,
    );
    this.#update({ status: "waiting" });
  }

  #toOmpResponse(pending: PendingEntry, answer: PendingAnswer): Record<string, unknown> {
    if (answer.type === "cancel") return { cancelled: true };
    if ((pending.kind === "approval" || pending.kind === "select") && answer.type === "option") {
      if (!pending.options.includes(answer.value)) throw new Error("选项不在可选范围内。");
      return { value: answer.value };
    }
    if (pending.kind === "confirm" && answer.type === "confirm") return { confirmed: answer.confirmed };
    if (pending.kind === "input" && answer.type === "text") return { value: answer.value };
    if (pending.kind === "ask" && answer.type === "ask") return { answers: answer.answers };
    throw new Error("回答的类型与请求不匹配。");
  }

  #resolvePending(pendingId: string, status: "answered" | "cancelled" | "interrupted", answer?: string): void {
    const pending = this.#pending.get(pendingId);
    if (!pending) return;
    this.#pending.delete(pendingId);
    this.#pendingByRawId.delete(pending.rawId);
    this.#ctx.emit(
      { eventType: "pending.resolved", payload: { pendingId, status, ...(answer ? { answer } : {}) } },
      this.#runId,
    );
    if (this.#runId) this.#update({ status: this.#pending.size > 0 ? "waiting" : "running" });
  }

  #liveTool(toolCallId: string, toolName: string | undefined): LiveTool {
    let tool = this.#liveTools.get(toolCallId);
    if (!tool) {
      tool = { toolName: toolName ?? "tool", output: "", diffs: [] };
      this.#liveTools.set(toolCallId, tool);
    }
    return tool;
  }

  #diffs(files: unknown[]): FileDiff[] {
    return files.flatMap((file) => {
      const value = record(file);
      const path = text(value?.path);
      const diff = text(value?.diff);
      if (!path || diff === undefined) return [];
      const shown = isAbsolute(path) ? relative(this.#cwd, path) || path : path;
      return [{ path: shown.startsWith("..") ? path : shown, diff, ...(text(value?.op) ? { op: text(value?.op) } : {}) }];
    });
  }

  /** Ends the active run; unfinished output is persisted so the log stays complete without deltas. */
  #finishRun(terminal: Extract<SessionEventBody, { eventType: `run.${"finished" | "aborted" | "failed" | "interrupted"}` }>): void {
    const runId = this.#runId;
    if (!runId) return;
    const pendingStatus = terminal.eventType === "run.interrupted" ? "interrupted" : "cancelled";
    for (const pendingId of this.#pending.keys()) this.#resolvePending(pendingId, pendingStatus);
    for (const [messageId, message] of this.#liveMessages) {
      if (message.text || message.thinking) {
        this.#ctx.emit({ eventType: "assistant.message", payload: { messageId, text: message.text, thinking: message.thinking } }, runId);
      }
    }
    for (const [toolCallId, tool] of this.#liveTools) {
      this.#ctx.emit(
        { eventType: "tool.finished", payload: { toolCallId, isError: true, output: tool.output || "已中断", diffs: tool.diffs } },
        runId,
      );
    }
    this.#liveMessages.clear();
    this.#liveTools.clear();
    this.#runId = undefined;
    this.#lastToolCallId = undefined;
    this.#ctx.emit(terminal, runId);
    const status = {
      "run.finished": "finished",
      "run.aborted": "aborted",
      "run.failed": "failed",
      "run.interrupted": "interrupted",
    } as const;
    this.#update({ status: status[terminal.eventType] });
    this.#scheduleIdleStop();
  }

  #handleExit(code: number): void {
    this.#omp = undefined;
    const detail = this.#lastError ? `：${this.#lastError}` : "";
    this.#finishRun({ eventType: "run.interrupted", payload: { reason: `omp 进程意外退出（退出码 ${code}）${detail}` } });
  }
}
