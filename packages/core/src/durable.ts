import { BACKGROUND_CONTEXT, type Context } from "@earendil-works/chord/context";
import { ModelRuntime } from "@earendil-works/pi-coding-agent";
import { NodeExecutionEnv } from "@earendil-works/pi-durable/env/node";
import { Harness, createRegistry, defineDoc, defineExtension, hook, ToolTask, type Conversation, type ConversationView } from "@earendil-works/pi-durable";
import { CodingTools as DurableCodingTools } from "@earendil-works/pi-durable/tools";
import { openNodeSqliteStorage } from "@earendil-works/pi-durable/storage/sqlite/node";
import type {
  ApprovalMode,
  FileDiff,
  Model,
  PendingAnswer,
  Project,
  SessionEvent,
  SessionEventBody,
  SessionStatus,
  SessionSummary,
  StreamMessage,
} from "@lila/protocol";
import type { AgentChange, ModelRef } from "@earendil-works/pi-durable";

// Durable owns the transcript, task scheduler, storage, recovery and live documents.
// These Lila-specific documents only project the product vocabulary onto Durable state.
export const ProjectCatalogDoc = (defineDoc as any)({
  kind: "lila.projects",
  version: 1,
  scope: "session",
  initial: () => ({ projects: [] as Project[] }),
  checkpointWhen: () => true,
}) as any;

export const SessionCatalogDoc = (defineDoc as any)({
  kind: "lila.sessions",
  version: 1,
  scope: "session",
  initial: () => ({ ids: [] }),
  checkpointWhen: () => true,
}) as any;

export type SessionMeta = {
  projectId: string;
  title: string;
  modelId?: string;
  approvalMode: ApprovalMode;
  status: SessionStatus;
  updatedAt: string;
};

export const SessionMetaDoc = (defineDoc as any)({
  kind: "lila.session",
  version: 1,
  scope: "conversation",
  history: "latest",
  fork: "initial",
  initial: (): SessionMeta => ({
    projectId: "",
    title: "",
    approvalMode: "yolo",
    status: "idle",
    updatedAt: new Date().toISOString(),
  }),
  checkpointWhen: () => true,
}) as any;

type Projection = {
  events: SessionEvent[];
  seenEntryIds: number[];
  nextSeq: number;
  activeRunId?: string;
};

export const ProjectionDoc = (defineDoc as any)({
  kind: "lila.projection",
  version: 1,
  scope: "conversation",
  history: "latest",
  fork: "initial",
  initial: (): Projection => ({ events: [], seenEntryIds: [], nextSeq: 1 }),
  checkpointWhen: () => true,
}) as any;

type ApprovalState = {
  requests: Record<string, { status: "pending" | "approved" | "denied"; title: string }>;
  allowedTools: string[];
};

export const ApprovalDoc = (defineDoc as any)({
  kind: "lila.approvals",
  version: 1,
  scope: "conversation",
  history: "latest",
  fork: "initial",
  initial: (): ApprovalState => ({ requests: {}, allowedTools: [] }),
  checkpointWhen: () => true,
}) as any;

type DurableCallbacks = {
  emit(sessionId: string, message: string): void;
  stream(sessionId: string, runId: string, message: StreamMessage): void;
  update(sessionId: string): void;
};

type ApprovalBridge = {
  request(input: { conversationId: number; pendingId: string; toolName: string; title: string; callId: string }): Promise<void>;
};

function approvalNeeded(mode: ApprovalMode, toolName: string): boolean {
  if (mode === "yolo") return false;
  if (mode === "always-ask") return !["read", "grep", "find", "ls"].includes(toolName);
  return toolName === "bash" || toolName === "write" || toolName === "edit" || toolName.startsWith("mcp__") || toolName === "subagent" || toolName.startsWith("bg_");
}

function approvalExtension(bridge: ApprovalBridge): any {
  return defineExtension({
    name: "lila-approval",
    hooks: [hook(ToolTask, {
      beforeTool: async (call: any, api: any, context: Context) => {
        const meta = await api.snapshot(SessionMetaDoc, api.conversationId, context) as SessionMeta | undefined;
        if (!meta || !approvalNeeded(meta.approvalMode, call.name)) return undefined;
        const approvals = await api.snapshot(ApprovalDoc, api.conversationId, context) as ApprovalState | undefined;
        if (approvals?.allowedTools?.includes(call.name)) return undefined;
        const callId = String(call.id ?? call.callId);
        const pendingId = `${api.conversationId}:${callId}`;
        await bridge.request({ conversationId: api.conversationId, pendingId, toolName: call.name, title: `Allow tool: ${call.name}`, callId });
        for (;;) {
          const current = await api.snapshot(ApprovalDoc, api.conversationId, context) as ApprovalState | undefined;
          const status = current?.requests?.[pendingId]?.status;
          if (status === "approved") return undefined;
          if (status === "denied") return { block: "用户拒绝了工具调用。" };
          await new Promise((resolve) => setTimeout(resolve, 250));
        }
      },
    })],
  });
}

type PendingApproval = {
  sessionId: string;
  pendingId: string;
  title: string;
  toolName: string;
  runId?: string;
};

function asConversationId(value: string): number {
  const id = Number(value);
  if (!Number.isSafeInteger(id) || id <= 0) throw new Error("无效的 durable 会话 ID。");
  return id;
}

function now(): string {
  return new Date().toISOString();
}

function modelRef(modelId?: string): ModelRef | undefined {
  if (!modelId) return undefined;
  const separator = modelId.includes("::") ? "::" : "/";
  const index = modelId.indexOf(separator);
  if (index <= 0 || index === modelId.length - separator.length) throw new Error(`无效的模型 ID：${modelId}`);
  return { provider: modelId.slice(0, index), modelId: modelId.slice(index + separator.length) };
}


function blockText(content: unknown): string {
  if (!Array.isArray(content)) return typeof content === "string" ? content : "";
  return content
    .flatMap((item) => {
      if (!item || typeof item !== "object" || Array.isArray(item)) return [];
      const block = item as Record<string, unknown>;
      return block.type === "text" && typeof block.text === "string" ? [block.text] : [];
    })
    .join("\n");
}

function messageText(content: unknown, type: "text" | "thinking"): string {
  if (!Array.isArray(content)) return "";
  return content
    .flatMap((item) => {
      if (!item || typeof item !== "object" || Array.isArray(item)) return [];
      const block = item as Record<string, unknown>;
      return block.type === type && typeof block[type] === "string" ? [block[type]] : [];
    })
    .join("")
    .trim();
}

function clip(value: string, max: number): string {
  return value.length <= max ? value : `${value.slice(0, max)}\n… 已省略 ${value.length - max} 个字符 …`;
}

function diffsFromToolResult(result: Record<string, unknown>, args?: Record<string, unknown>): FileDiff[] {
  const details = result.details && typeof result.details === "object" && !Array.isArray(result.details)
    ? (result.details as Record<string, unknown>)
    : undefined;
  const diff = typeof details?.diff === "string" ? details.diff : undefined;
  const path = typeof details?.path === "string" ? details.path : typeof args?.path === "string" ? args.path : undefined;
  if (diff !== undefined && path) return [{ path, diff, ...(typeof details?.op === "string" ? { op: details.op } : {}) }];
  return [];
}

function toolResultPayload(entry: any): { toolCallId: string; toolName: string; output: string; diffs: FileDiff[]; isError: boolean } | undefined {
  const message = entry.model?.[0];
  if (!message || message.role !== "toolResult") return undefined;
  return {
    toolCallId: String(message.toolCallId),
    toolName: String(message.toolName),
    output: clip(blockText(message.content), 20_000),
    diffs: diffsFromToolResult(message, undefined),
    isError: message.isError === true,
  };
}

function assistantMessagePayload(entry: any, sessionId: string): SessionEventBody[] {
  const message = entry.model?.[0];
  if (!message || message.role !== "assistant") return [];
  const messageId = `${sessionId}:${entry.id}`;
  const events: SessionEventBody[] = [];
  const text = messageText(message.content, "text");
  const thinking = messageText(message.content, "thinking");
  if (text || thinking) {
    events.push({
      eventType: "assistant.message",
      payload: { messageId, text, thinking, ...(typeof message.model === "string" ? { model: message.model } : {}) },
    });
  }
  for (const block of Array.isArray(message.content) ? message.content : []) {
    if (!block || block.type !== "toolCall") continue;
    events.push({
      eventType: "tool.started",
      payload: {
        toolCallId: String(block.id),
        toolName: String(block.name),
        args: block.arguments && typeof block.arguments === "object" ? block.arguments : {},
      },
    });
  }
  return events;
}


export class DurableCore {
  readonly #context = BACKGROUND_CONTEXT;
  readonly #callbacks: DurableCallbacks;
  readonly #models: ModelRuntime;
  readonly #extensions: any;
  readonly #harness: any;
  readonly #conversations = new Map<string, Conversation>();
  readonly #watchReady = new Map<string, Promise<void>>();
  readonly #watchHandles = new Map<string, { stop(): void }>();
  readonly #live = new Map<string, any>();
  readonly #approvals = new Map<string, PendingApproval>();

  private constructor(
    models: ModelRuntime,
    harness: any,
    callbacks: DurableCallbacks,
    extensions: any,
  ) {
    this.#models = models;
    this.#harness = harness;
    this.#callbacks = callbacks;
    this.#extensions = extensions;
  }

  static async open(databasePath: string, callbacks: DurableCallbacks): Promise<DurableCore> {
    const storage = await openNodeSqliteStorage(databasePath);
    const models = await ModelRuntime.create({ allowModelNetwork: false });
    const registry = createRegistry();
    registry.install(DurableCodingTools);
    let durable!: DurableCore;
    const bridge: ApprovalBridge = { request: async () => undefined };
    const approvals = approvalExtension(bridge);
    registry.install(approvals);
    const core = await Harness.open(storage, {
      models,
      registry,
      settings: { extensions: [DurableCodingTools, approvals] },
      env: ({ cwd }) => new NodeExecutionEnv({ cwd: cwd ?? process.cwd() }),
      onReport: (report: any) => console.error("[core] durable report", report.kind, report.error ?? report),
    }, BACKGROUND_CONTEXT);
    durable = new DurableCore(models, core, callbacks, [DurableCodingTools, approvals]);
    bridge.request = (input) => durable.#createApproval(input);
    durable.#harness.resume();
    return durable;
  }

  get models(): readonly Model[] {
    return this.#models.getModels().map((item: any) => ({ id: `${item.provider}/${item.id}`, label: item.name ?? item.id }));
  }

  async close(): Promise<void> {
    for (const watch of this.#watchHandles.values()) watch.stop();
    await this.#harness.close(this.#context);
    // Harness.close closes the underlying durable storage.
  }

  async listProjects(): Promise<Project[]> {
    return (await this.#harness.snapshot(ProjectCatalogDoc, this.#context))?.projects ?? [];
  }

  async addProject(project: Project): Promise<Project> {
    await this.#harness.commit(async (tx: any) => {
      const catalog = await tx.doc(ProjectCatalogDoc);
      if (!catalog.projects.some((item: Project) => item.path === project.path)) catalog.projects.push(project);
    }, this.#context);
    return project;
  }

  async listSessions(): Promise<SessionSummary[]> {
    const sessions: SessionSummary[] = [];
    const catalog = await this.#harness.snapshot(SessionCatalogDoc, this.#context) as { ids?: string[] } | undefined;
    for (const sessionId of catalog?.ids ?? []) {
      try { sessions.push(await this.getSession(sessionId)); } catch { /* removed sessions are filtered out */ }
    }
    return sessions.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
  }

  async createSession(projectId: string, modelId: string | undefined, approvalMode: ApprovalMode): Promise<SessionSummary> {
    const project = (await this.listProjects()).find((item) => item.id === projectId);
    if (!project) throw new Error("项目不存在");
    const agent: AgentChange = { cwd: project.path, extensions: this.#extensions, ...(modelRef(modelId) ? { model: modelRef(modelId) } : {}) };
    const conversation = await this.#harness.createConversation({
      ownership: { kind: "ownerless" },
      agent,
      init: async (tx: any, conversationId: any) => {
        const meta = await tx.doc(SessionMetaDoc, conversationId);
        meta.projectId = projectId;
        meta.approvalMode = approvalMode;
        meta.updatedAt = now();
        await tx.doc(ProjectionDoc, conversationId);
        await tx.doc(ApprovalDoc, conversationId);
      },
    }, this.#context);
    const sessionId = String(conversation.id);
    this.#conversations.set(sessionId, conversation);
    await this.#harness.commit(async (tx: any) => {
      const catalog = await tx.doc(SessionCatalogDoc);
      if (!catalog.ids.includes(sessionId)) catalog.ids.push(sessionId);
      const projection = await tx.doc(ProjectionDoc, conversation.id as any) as Projection;
      await this.#appendProjection(tx, projection, sessionId, undefined, { eventType: "session.created", payload: { projectId, ...(modelId ? { modelId } : {}), approvalMode } });
    }, this.#context);
    await this.#ensureWatch(sessionId, conversation.id as any);
    return this.#summary(sessionId, conversation.id as any);
  }

  async configure(sessionId: string, patch: { modelId?: string | null; approvalMode?: ApprovalMode }): Promise<SessionSummary> {
    const conversation = await this.#conversation(sessionId);
    const current = await this.#meta(conversation);
    await conversation.configure({
      ...(patch.modelId !== undefined ? { model: patch.modelId ? modelRef(patch.modelId) : null } : {}),
      ...(current ? { cwd: undefined } : {}),
    }, this.#context);
    await this.#harness.commit(async (tx: any) => {
      const meta = await tx.doc(SessionMetaDoc, conversation.id as any);
      if (patch.modelId !== undefined) meta.modelId = patch.modelId ?? undefined;
      if (patch.approvalMode) meta.approvalMode = patch.approvalMode;
      meta.updatedAt = now();
    }, this.#context);
    this.#callbacks.update(sessionId);
    return this.#summary(sessionId, conversation.id as any);
  }

  async rename(sessionId: string, title: string): Promise<SessionSummary> {
    const conversation = await this.#conversation(sessionId);
    await this.#harness.commit(async (tx: any) => {
      const meta = await tx.doc(SessionMetaDoc, conversation.id as any);
      meta.title = title;
      meta.updatedAt = now();
    }, this.#context);
    this.#callbacks.update(sessionId);
    return this.#summary(sessionId, conversation.id as any);
  }

  async delete(sessionId: string): Promise<void> {
    const conversation = await this.#conversation(sessionId);
    await this.#harness.commit(async (tx: any) => {
      const meta = await tx.doc(SessionMetaDoc, conversation.id as any);
      meta.projectId = "";
      meta.status = "idle";
      meta.updatedAt = now();
    }, this.#context);
    this.#callbacks.update(sessionId);
  }

  async send(sessionId: string, text: string, attachments: Array<{ name: string; mimeType: string; data: string }> = []): Promise<string> {
    const conversation = await this.#conversation(sessionId);
    const runId = crypto.randomUUID();
    const content: any = attachments.some((item) => item.mimeType.startsWith("image/"))
      ? [{ type: "text", text: `${text}${this.#textAttachments(attachments)}` }, ...attachments.filter((item) => item.mimeType.startsWith("image/")).map((item) => ({ type: "image", data: item.data, mimeType: item.mimeType }))]
      : `${text}${this.#textAttachments(attachments)}`;
    await this.#harness.commit(async (tx: any) => {
      await conversation.submit(tx, { type: "input", content });
      const meta = await tx.doc(SessionMetaDoc, conversation.id as any);
      const projection = await tx.doc(ProjectionDoc, conversation.id as any);
      meta.status = "running";
      meta.updatedAt = now();
      if (!meta.title) meta.title = text.replace(/\s+/g, " ").trim().slice(0, 60);
      projection.activeRunId = runId;
      const attachmentMeta = attachments.map(({ name, mimeType }) => ({ name, mimeType }));
      await this.#appendProjection(tx, projection, sessionId, runId, { eventType: "message.user", payload: { text, ...(attachmentMeta.length ? { attachments: attachmentMeta } : {}) } });
      await this.#appendProjection(tx, projection, sessionId, runId, { eventType: "run.started", payload: {} });
    }, this.#context);
    void this.#monitor(sessionId, conversation, runId);
    return runId;
  }

  async abort(sessionId: string): Promise<void> {
    const conversation = await this.#conversation(sessionId);
    await conversation.abort(this.#context);
    const meta = await this.#meta(conversation);
    if (meta?.status === "running") {
      await this.#finish(sessionId, conversation, "run.aborted", {});
    }
  }

  async respond(sessionId: string, pendingId: string, answer: PendingAnswer): Promise<void> {
    const conversation = await this.#conversation(sessionId);
    if (!pendingId.startsWith(`${sessionId}:`)) throw new Error("这个请求已经失效。");
    const approved = answer.type === "option" && answer.value.startsWith("Approve");
    const toolName = this.#approvals.get(pendingId)?.toolName;
    await this.#harness.commit(async (tx: any) => {
      const approvals = await tx.doc(ApprovalDoc, conversation.id as any);
      const request = approvals.requests[pendingId];
      if (request) request.status = approved ? "approved" : "denied";
      if (approved && answer.type === "option" && answer.value.includes("for this session") && toolName && !approvals.allowedTools.includes(toolName)) approvals.allowedTools.push(toolName);
      const projection = await tx.doc(ProjectionDoc, conversation.id as any);
      await this.#appendProjection(tx, projection, sessionId, this.#approvals.get(pendingId)?.runId ?? projection.activeRunId, {
        eventType: "pending.resolved",
        payload: { pendingId, status: approved ? "answered" : "cancelled", answer: approved ? "Approve" : "Deny" },
      });
    }, this.#context);
    this.#callbacks.update(sessionId);
  }

  async #createApproval(input: { conversationId: number; pendingId: string; toolName: string; title: string; callId: string }): Promise<void> {
    const sessionId = String(input.conversationId);
    const conversation = await this.#conversation(sessionId);
    const projection = await this.#projection(conversation.id);
    this.#approvals.set(input.pendingId, { sessionId, pendingId: input.pendingId, title: input.title, toolName: input.toolName, runId: projection.activeRunId });
    await this.#harness.commit(async (tx: any) => {
      const approvals = await tx.doc(ApprovalDoc, conversation.id);
      approvals.allowedTools ??= [];
      if (approvals.requests[input.pendingId]?.status === "approved" || approvals.requests[input.pendingId]?.status === "pending") return;
      approvals.requests[input.pendingId] = { status: "pending", title: input.title };
      const current = await tx.doc(ProjectionDoc, conversation.id) as Projection;
      await this.#appendProjection(tx, current, sessionId, projection.activeRunId, {
        eventType: "pending.created",
        payload: { pendingId: input.pendingId, kind: "approval", title: input.title, options: ["Approve", `Approve ${input.toolName} for this session`, "Deny"], questions: [], toolCallId: input.callId },
      });
    }, this.#context);
    this.#callbacks.update(sessionId);
  }

  async resumeEvents(sessionId: string, afterSeq: number): Promise<{ events: SessionEvent[]; live?: any; hasEarlier: boolean }> {
    const conversation = await this.#conversation(sessionId);
    await this.#ensureWatch(sessionId, conversation.id as any);
    // The watch is live before this command returns. Its first value is projected asynchronously.
    const projection = (await this.#harness.snapshot(ProjectionDoc, conversation.id as any, this.#context) as Projection | undefined) ?? { events: [], seenEntryIds: [], nextSeq: 1 };
    const events = afterSeq === 0 ? projection.events.slice(-200) : projection.events.filter((event) => event.seq > afterSeq);
    return { events, hasEarlier: Boolean(events[0] && events[0].seq > 1), ...(this.#live.has(sessionId) ? { live: this.#live.get(sessionId) } : {}) };
  }

  async eventsBefore(sessionId: string, beforeSeq: number, limit: number): Promise<{ events: SessionEvent[]; hasEarlier: boolean }> {
    const conversation = await this.#conversation(sessionId);
    const projection = (await this.#harness.snapshot(ProjectionDoc, conversation.id as any, this.#context) as Projection | undefined) ?? { events: [], seenEntryIds: [], nextSeq: 1 };
    const events = projection.events.filter((event) => event.seq < beforeSeq).slice(-limit);
    return { events, hasEarlier: Boolean(events[0] && events[0].seq > 1) };
  }

  async getSession(sessionId: string): Promise<SessionSummary> {
    const conversation = await this.#conversation(sessionId);
    return this.#summary(sessionId, conversation.id as any);
  }

  async #conversation(sessionId: string): Promise<Conversation> {
    const existing = this.#conversations.get(sessionId);
    if (existing) return existing;
    const conversation = await this.#harness.conversation(asConversationId(sessionId) as any, this.#context);
    if (!conversation) throw new Error("会话不存在");
    this.#conversations.set(sessionId, conversation);
    await this.#ensureWatch(sessionId, conversation.id as any);
    return conversation;
  }

  async #meta(conversation: Conversation): Promise<SessionMeta | undefined> {
    return this.#harness.snapshot(SessionMetaDoc, conversation.id as any, this.#context);
  }

  async #projection(conversationId: any): Promise<Projection> {
    return (await this.#harness.snapshot(ProjectionDoc, conversationId, this.#context) as Projection | undefined) ?? { events: [], seenEntryIds: [], nextSeq: 1 };
  }

  async #summary(sessionId: string, conversationId: any): Promise<SessionSummary> {
    const meta = await this.#harness.snapshot(SessionMetaDoc, conversationId, this.#context);
    if (!meta?.projectId) throw new Error("会话不存在");
    const approvals = await this.#harness.snapshot(ApprovalDoc, conversationId, this.#context) as { requests?: Record<string, { status: string }> } | undefined;
    return {
      id: sessionId,
      projectId: meta.projectId,
      title: meta.title,
      ...(meta.modelId ? { modelId: meta.modelId } : {}),
      approvalMode: meta.approvalMode,
      status: meta.status,
      updatedAt: meta.updatedAt,
      pendingCount: Object.values(approvals?.requests ?? {}).filter((item: any) => item.status === "pending").length,
    };
  }

  #textAttachments(attachments: Array<{ name: string; mimeType: string; data: string }>): string {
    return attachments.filter((item) => !item.mimeType.startsWith("image/")).map((item) => {
      const bytes = Buffer.from(item.data, "base64");
      if (bytes.length > 200_000) throw new Error(`附件过大：${item.name}`);
      return `\n\n--- 附件：${item.name} ---\n\n${bytes.toString("utf8")}`;
    }).join("");
  }

  async #monitor(sessionId: string, conversation: Conversation, _runId: string): Promise<void> {
    try {
      await conversation.waitForIdle(this.#context);
      const meta = await this.#meta(conversation);
      if (meta?.status === "running") await this.#finish(sessionId, conversation, "run.finished", {});
    } catch (error) {
      await this.#finish(sessionId, conversation, "run.failed", { error: String(error) });
    }
  }

  async #finish(sessionId: string, conversation: Conversation, eventType: "run.finished" | "run.aborted" | "run.failed", payload: any): Promise<void> {
    await this.#harness.commit(async (tx: any) => {
      const meta = await tx.doc(SessionMetaDoc, conversation.id as any);
      const projection = await tx.doc(ProjectionDoc, conversation.id as any);
      meta.status = eventType === "run.finished" ? "finished" : eventType === "run.aborted" ? "aborted" : "failed";
      meta.updatedAt = now();
      await this.#appendProjection(tx, projection, sessionId, projection.activeRunId, { eventType, payload });
    }, this.#context);
    this.#callbacks.update(sessionId);
  }

  async #appendProjection(tx: any, projection: any, sessionId: string, runId: string | undefined, body: SessionEventBody): Promise<void> {
    const event = { type: "event", sessionId, seq: projection.nextSeq++, ...(runId ? { runId } : {}), ...body } as SessionEvent;
    projection.events.push(event);
    this.#callbacks.emit(sessionId, JSON.stringify(event));
  }

  async #ensureWatch(sessionId: string, conversationId: any): Promise<void> {
    if (this.#watchHandles.has(sessionId)) return;
    let readyResolve!: () => void;
    const ready = new Promise<void>((resolve) => { readyResolve = resolve; });
    this.#watchReady.set(sessionId, ready);
    const conversation = this.#conversations.get(sessionId) ?? await this.#harness.conversation(conversationId, this.#context);
    if (!conversation) throw new Error("会话不存在");
    this.#conversations.set(sessionId, conversation);
    const watch = await conversation.watch(this.#context, { min: 1, max: 1000 });
    watch.start(async (call: any) => {
      if (call.kind !== "value") return;
      readyResolve();
      readyResolve = () => undefined;
      await this.#projectView(sessionId, conversation, call.value as ConversationView);
    });
    this.#watchHandles.set(sessionId, { stop: () => watch.stop() });
  }

  async #projectView(sessionId: string, conversation: Conversation, view: ConversationView): Promise<void> {
    const projection = await this.#harness.snapshot(ProjectionDoc, conversation.id as any, this.#context) as Projection | undefined;
    if (!projection) return;
    const live = (view as any).docs?.["pi.live"] as any;
    if (live && projection.activeRunId) {
      const message = live.generation?.message;
      const text = messageText(message?.content, "text");
      const thinking = messageText(message?.content, "thinking");
      const previous = this.#live.get(sessionId) as any;
      const messageId = `${sessionId}:live`;
      if (text.length > (previous?.messages?.[0]?.text?.length ?? 0)) this.#callbacks.stream(sessionId, projection.activeRunId, { kind: "assistant.delta", messageId, channel: "text", delta: text.slice(previous?.messages?.[0]?.text?.length ?? 0) } as any);
      if (thinking.length > (previous?.messages?.[0]?.thinking?.length ?? 0)) this.#callbacks.stream(sessionId, projection.activeRunId, { kind: "assistant.delta", messageId, channel: "thinking", delta: thinking.slice(previous?.messages?.[0]?.thinking?.length ?? 0) } as any);
      const tools = Array.isArray(live.tools) ? live.tools.map((tool: any) => ({ toolCallId: String(tool.callId), toolName: String(tool.name), output: typeof tool.output === "string" ? tool.output : "", diffs: [] })) : [];
      for (const tool of tools) this.#callbacks.stream(sessionId, projection.activeRunId, { kind: "tool.progress", toolCallId: tool.toolCallId, toolName: tool.toolName, ...(tool.output ? { output: tool.output } : {}) } as any);
      this.#live.set(sessionId, { runId: projection.activeRunId, messages: [{ messageId, text, thinking }], tools });
    }
    const seen = new Set(projection.seenEntryIds);
    const runId = projection.activeRunId;
    const newBodies: SessionEventBody[] = [];
    const newIds: number[] = [];
    for (const entry of view.entries as any[]) {
      if (seen.has(Number(entry.id))) continue;
      newIds.push(Number(entry.id));
      if (entry.kind === "pi.assistant") newBodies.push(...assistantMessagePayload(entry, sessionId));
      else if (entry.kind === "pi.tool-result") {
        const payload = toolResultPayload(entry);
        if (payload) newBodies.push({ eventType: "tool.finished", payload });
      }
    }
    if (newBodies.length === 0 && newIds.length === 0) return;
    await this.#harness.commit(async (tx: any) => {
      const current = await tx.doc(ProjectionDoc, conversation.id as any);
      for (const id of newIds) current.seenEntryIds.push(id);
      for (const body of newBodies) await this.#appendProjection(tx, current, sessionId, runId, body);
    }, this.#context);
    this.#callbacks.update(sessionId);
  }
}

