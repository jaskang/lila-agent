import { join } from "node:path";
import type {
  AgentSession,
  AgentSessionEvent,
  AgentSessionServices,
  ExtensionUIDialogOptions,
  ToolInfo,
} from "@earendil-works/pi-coding-agent";
import type {
  ClientCommand,
  PendingAnswer,
  PendingRequest,
  RunStatus,
  ServerMessage,
  SessionResources,
  SessionState,
  SessionStatus,
  SessionSummary,
  SlashCommand,
  SubagentProgress,
  TranscriptItem,
} from "@lila/protocol";
import { errorMessage, UserError } from "./errors.ts";
import type { LilaBridge, SubagentBusEvent } from "./lila-extension.ts";
import { PendingBridge } from "./pending-bridge.ts";
import type { Store } from "./store.ts";
import { SubagentTracker } from "./subagent-tracker.ts";
import {
  type AgentMessage,
  type AssistantItem,
  assistantItem,
  aroundTranscriptItem,
  buildSubagentToolInfo,
  contentText,
  countBranchIds,
  customItem,
  displayUserText,
  earlier,
  hasAssistantContent,
  parseUserItemId,
  projectBranch,
  responseOutcome,
  RUN_SETTLED,
  RUN_STARTED,
  snapshot,
  searchTranscript,
  type ToolItem,
  toolCallItems,
  transcriptTurns,
  type UserMessage,
  uniqueId,
  userItem,
  withToolResult,
} from "./transcript.ts";

/** How a live session reaches the connected clients. */
export interface Clients {
  toWatchers(sessionId: string, message: ServerMessage): void;
  broadcast(message: ServerMessage): void;
  isWatched(sessionId: string): boolean;
}

type Command<T extends ClientCommand["type"]> = Extract<ClientCommand, { type: T }>;
type Model = NonNullable<AgentSession["model"]>;
type ToolThrottle = { at: number; timer?: NodeJS.Timeout; latest?: ToolItem };

const TOOL_UPDATE_INTERVAL_MS = 100;
const TITLE_LIMIT = 60;

const BUILTIN_COMMANDS: SlashCommand[] = [
  { name: "compact", description: "压缩上下文，用摘要替换较早的对话", source: "builtin" },
  { name: "reload", description: "重新加载技能、提示词、扩展和 MCP 服务", source: "builtin" },
];

/** What `/` offers: Lila's built-ins, then what Pi's `prompt()` expands (extension commands, prompt templates, skills). */
export function slashCommands(
  extensionCommands: Iterable<{ name: string; description?: string | undefined }>,
  prompts: Iterable<{ name: string; description: string }>,
  skills: Iterable<{ name: string; description: string }>,
): SlashCommand[] {
  const commands = [...BUILTIN_COMMANDS];
  for (const command of extensionCommands) commands.push({ name: command.name, description: command.description ?? "", source: "extension" });
  for (const prompt of prompts) commands.push({ name: prompt.name, description: prompt.description, source: "prompt" });
  for (const skill of skills) commands.push({ name: `skill:${skill.name}`, description: skill.description, source: "skill" });
  return commands;
}

/** Matches pi-subagents' default auto-generated session name such as `Explore#9b7891d8`. */
export function parseAutoSubagentName(name: string | undefined): { type: string; idPrefix: string } | undefined {
  if (!name) return undefined;
  const match = /^(.+?)#([0-9a-f]{8})$/i.exec(name.trim());
  if (!match) return undefined;
  return { type: match[1]!, idPrefix: match[2]!.toLowerCase() };
}

function truncateTitle(raw: string): string {
  const line = raw.trim().split("\n")[0]?.trim() ?? "";
  if (!line) return "";
  return line.length > TITLE_LIMIT ? `${line.slice(0, TITLE_LIMIT)}…` : line;
}

export function sessionTitle(
  name: string | undefined,
  firstMessage: string | undefined,
  options?: { isSubsession?: boolean; description?: string | undefined },
): string {
  const trimmed = name?.trim();
  // Replace pi-subagents' opaque `Type#<hash>` auto-name with a human-readable description or prompt line.
  if (options?.isSubsession && parseAutoSubagentName(trimmed)) {
    const desc = options.description ? truncateTitle(options.description) : "";
    if (desc) return desc;
    const promptLine = firstMessage ? truncateTitle(displayUserText(firstMessage)) : "";
    if (promptLine) return promptLine;
  }
  if (trimmed) return trimmed;
  const line = firstMessage ? truncateTitle(displayUserText(firstMessage)) : "";
  return line || "新会话";
}

export function findModel(services: AgentSessionServices, modelId: string): Model | undefined {
  return services.modelRuntime.getAvailableSnapshot().find((model) => `${model.provider}/${model.id}` === modelId);
}

/** One opened Pi session: its AgentSession, pending user requests and the mapping of its events to frames. */
export class LiveSession implements LilaBridge {
  readonly id: string;
  readonly projectId: string;
  readonly allowedTools = new Set<string>();
  lastActivity = Date.now();
  readonly #store: Store;
  readonly #clients: Clients;
  readonly #onReload?: () => void;
  readonly #pending: PendingBridge;
  readonly #subagents: SubagentTracker;
  #session!: AgentSession;
  #services!: AgentSessionServices;
  #unsubscribe?: () => void;
  #access: SessionState["access"] = "starting";
  #startupError?: string;
  #initialization!: Promise<void>;
  #disposed = false;
  #statusLines: Record<string, string> = {};
  #retry: SessionState["retry"];
  #updatedAt = new Date().toISOString();
  #firstMessage: string | undefined;
  #idCounts = new Map<string, number>();
  #streamingPending: AssistantItem | undefined;
  #streamingId: string | undefined;
  #streamingSent = false;
  /** Tool items of the current run, with their newest output. */
  readonly #tools = new Map<string, ToolItem>();
  readonly #throttles = new Map<string, ToolThrottle>();
  #notices = 0;
  #lastSummary = "";
  #run: { startedAt: number; outcome: RunStatus } | undefined;

  constructor(options: {
    id: string;
    projectId: string;
    store: Store;
    clients: Clients;
    onReload?: () => void;
    onSubsessionsChanged?: () => void;
  }) {
    this.id = options.id;
    this.projectId = options.projectId;
    this.#store = options.store;
    this.#clients = options.clients;
    this.#onReload = options.onReload;
    this.#pending = new PendingBridge({
      onPendingChanged: () => {
        this.touch();
        this.#pushState();
        this.#broadcastSummary();
      },
      onNotify: (level, message) => this.#notify(level, message),
      onStatusLine: (key, value) => {
        if (value === undefined) delete this.#statusLines[key];
        else this.#statusLines[key] = value;
        this.#pushState();
      },
    });
    this.#subagents = new SubagentTracker({
      onStateChanged: (activeChanged) => {
        this.#pushState();
        if (activeChanged) this.#broadcastSummary();
      },
      onSubsessionsChanged: () => options.onSubsessionsChanged?.(),
    });
  }

  get session(): AgentSession {
    return this.#session;
  }

  get services(): AgentSessionServices {
    return this.#services;
  }

  get approvalMode() {
    return this.#store.approvalMode(this.id);
  }

  get busy(): boolean {
    return this.#access === "starting" || this.#session.isStreaming || this.#session.isCompacting || this.#pending.size > 0 || this.#subagents.hasActive();
  }

  attach(services: AgentSessionServices, session: AgentSession, updatedAt?: string): void {
    this.#services = services;
    this.#session = session;
    if (updatedAt) this.#updatedAt = updatedAt;
    for (const entry of session.sessionManager.getEntries()) {
      if (entry.type === "message" && entry.message.role === "user") {
        this.#firstMessage = contentText(entry.message.content);
        break;
      }
    }
    this.#syncIdCounts();
    this.#unsubscribe = session.subscribe((event) => this.#onEvent(event));
    // The host registers the session immediately. An extension can ask a question
    // during startup, so clients need to open it and answer before this resolves.
    this.#initialization = Promise.resolve()
      .then(() => this.#disposed ? undefined : session.bindExtensions({
        uiContext: this.#pending.createUIContext(),
        mode: "rpc",
        commandContextActions: {
          waitForIdle: () => session.waitForIdle(),
          reload: async () => {
            await session.reload();
            this.#onReload?.();
            this.#pushState();
          },
          // One Lila session is one Pi session; extensions cannot replace it from under the client.
          newSession: async () => ({ cancelled: true }),
          fork: async () => ({ cancelled: true }),
          switchSession: async () => ({ cancelled: true }),
          navigateTree: async () => ({ cancelled: true }),
        },
        onError: (error) => this.#notify("error", `扩展出错（${error.extensionPath}，${error.event}）：${error.error}`),
      }))
      .then(
        () => { this.#access = "ready"; },
        (error: unknown) => {
          this.#access = "failed";
          this.#startupError = errorMessage(error);
          this.#pending.cancelAll();
        },
      )
      .then(() => {
        if (this.#disposed) return;
        this.#pushState();
        this.#broadcastSummary();
      });
  }

  #requireReady(): void {
    if (this.#disposed) throw new UserError("会话已关闭");
    if (this.#access === "starting") throw new UserError("请先完成会话初始化");
    if (this.#access === "failed") throw new UserError(this.#startupError ?? "会话初始化失败");
  }

  touch(): void {
    this.lastActivity = Date.now();
  }

  #syncIdCounts(): void {
    this.#idCounts = countBranchIds(this.#session.sessionManager.getBranch());
  }

  // -------------------------------------------------------------------------
  // Snapshots

  get status(): SessionStatus {
    if (this.#pending.size > 0) return "waiting";
    if (this.#access === "failed") return "error";
    if (this.#session.isStreaming || this.#session.isCompacting || this.#subagents.hasActive()) return "running";
    const messages = this.#session.messages;
    for (let index = messages.length - 1; index >= 0; index--) {
      const message = messages[index];
      if (message?.role === "assistant") return message.stopReason === "error" ? "error" : "idle";
    }
    return "idle";
  }

  subagentByPrefix(idPrefix: string): SubagentProgress | undefined {
    return this.#subagents.byPrefix(idPrefix);
  }

  summary(): SessionSummary {
    return {
      id: this.id,
      projectId: this.projectId,
      title: sessionTitle(this.#session.sessionName, this.#firstMessage),
      updatedAt: this.#updatedAt,
      status: this.status,
      pendingCount: this.#pending.size,
    };
  }

  state(): SessionState {
    const session = this.#session;
    const usage = session.getContextUsage();
    const model = session.model;
    const subagents = this.#subagents.snapshot();
    return {
      status: this.status,
      access: this.#access,
      ...(this.#startupError ? { startupError: this.#startupError } : {}),
      runActive: session.isStreaming,
      ...(model && !(model.provider === "unknown" && model.id === "unknown") ? { modelId: `${model.provider}/${model.id}` } : {}),
      thinkingLevel: session.thinkingLevel,
      thinkingLevels: session.getAvailableThinkingLevels(),
      approvalMode: this.approvalMode,
      queue: { steering: [...session.getSteeringMessages()], followUp: [...session.getFollowUpMessages()] },
      ...(usage ? { context: { tokens: usage.tokens, window: usage.contextWindow, percent: usage.percent } } : {}),
      cost: session.getSessionStats().cost,
      compacting: session.isCompacting,
      ...(this.#retry ? { retry: this.#retry } : {}),
      pending: this.#pending.list(),
      statusLines: { ...this.#statusLines },
      ...(subagents ? { subagents } : {}),
    };
  }

  #items(): TranscriptItem[] {
    const running = this.#session.isStreaming;
    const items = projectBranch(this.#session.sessionManager.getBranch(), { running });
    if (!running) return items;
    const live = items.map((item) => (item.kind === "tool" ? (this.#tools.get(item.id) ?? item) : item));
    const partial = this.#session.state.streamingMessage;
    if (partial?.role === "assistant") {
      const item = assistantItem(partial, true);
      if (hasAssistantContent(item)) {
        live.push(this.#streamingId ? { ...item, id: this.#streamingId } : item);
      }
    }
    return live;
  }

  open(limit: number | undefined): { items: TranscriptItem[]; hasEarlier: boolean; state: SessionState } {
    this.touch();
    return { ...snapshot(this.#items(), limit), state: this.state() };
  }

  earlier(beforeId: string, limit: number): { items: TranscriptItem[]; hasEarlier: boolean } {
    return earlier(this.#items(), beforeId, limit);
  }

  search(query: string, offset: number) {
    return searchTranscript(this.#items(), query, offset);
  }

  turns(offset: number) {
    return transcriptTurns(this.#session.sessionManager.getBranch(), offset);
  }

  around(itemId: string): TranscriptItem[] {
    return aroundTranscriptItem(this.#items(), itemId);
  }

  // -------------------------------------------------------------------------
  // Commands

  async prompt(command: Command<"session.prompt">): Promise<void> {
    this.touch();
    const session = this.#session;
    await this.#initialization;
    this.#requireReady();
    if (command.mode && !session.isStreaming) throw new UserError("当前回复已结束，可以重新发送这条消息");
    const streamingBehavior = session.isStreaming ? (command.mode ?? "steer") : undefined;
    const images = command.images?.map((image) => ({ type: "image" as const, data: image.data, mimeType: image.mimeType }));
    // prompt() resolves when the whole run ends; answer the client once Pi accepted the input.
    const { promise, resolve, reject } = Promise.withResolvers<void>();
    let accepted = false;
    session
      .prompt(command.text, {
        source: "rpc",
        ...(images?.length ? { images } : {}),
        ...(streamingBehavior ? { streamingBehavior } : {}),
        preflightResult: () => {
          accepted = true;
          resolve();
        },
      })
      .then(
        () => resolve(),
        (error: unknown) => {
          if (!accepted) {
            const message = errorMessage(error);
            if (/^No API key found for the selected model|^No model selected/.test(message)) {
              return reject(new UserError("没有可用模型。请在终端运行 pi，在 Pi 中输入 /login，然后回到 Lila 刷新模型。"));
            }
            return reject(error);
          }
          console.error(`[core] run of ${this.id} failed`, error);
          this.#notify("error", errorMessage(error));
        },
      );
    await promise;
  }

  async abort(): Promise<void> {
    this.touch();
    this.#pending.cancelAll();
    await this.#session.abort();
  }

  async configure(command: Command<"session.configure">): Promise<SessionState> {
    this.touch();
    if (command.modelId) {
      let model = findModel(this.#services, command.modelId);
      if (!model) {
        await this.#services.modelRuntime.refresh({ allowNetwork: false });
        model = findModel(this.#services, command.modelId);
      }
      if (!model) throw new UserError(`找不到模型 ${command.modelId}，或它还没有配置凭据`);
      await this.#session.setModel(model);
    }
    if (command.thinkingLevel) this.#session.setThinkingLevel(command.thinkingLevel);
    if (command.approvalMode) this.#store.setApprovalMode(this.id, command.approvalMode);
    this.#pushState();
    return this.state();
  }

  rename(title: string): SessionSummary {
    this.#session.setSessionName(title);
    return this.#broadcastSummary(true);
  }

  compact(): void {
    this.#requireReady();
    this.touch();
    this.#session.compact().catch((error: unknown) => this.#notify("error", `压缩失败：${errorMessage(error)}`));
  }

  async rewind(itemId: string): Promise<string> {
    this.#requireReady();
    const session = this.#session;
    if (session.isStreaming || session.isCompacting) throw new UserError("请先停止当前回复再回退");
    const parsed = parseUserItemId(itemId);
    let seen = 0;
    const entry = parsed
      ? session.sessionManager.getBranch().find((candidate) => {
          if (candidate.type !== "message" || candidate.message.role !== "user" || candidate.message.timestamp !== parsed.timestamp) return false;
          return ++seen === parsed.occurrence;
        })
      : undefined;
    if (!entry || entry.type !== "message") throw new UserError("找不到这条消息，可能已经不在当前分支上");
    const result = await session.navigateTree(entry.id);
    if (result.cancelled) throw new UserError("回退被扩展取消了");
    this.#syncIdCounts();
    this.#clients.toWatchers(this.id, { type: "reset", sessionId: this.id });
    this.#pushState();
    return displayUserText(result.editorText ?? contentText((entry.message as UserMessage).content));
  }

  respond(pendingId: string, answer: PendingAnswer): void {
    this.#pending.respond(pendingId, answer);
  }

  commands(): SlashCommand[] {
    const session = this.#session;
    const extensionCommands = session.extensionRunner.getRegisteredCommands().map((command) => ({ name: command.invocationName, description: command.description }));
    return slashCommands(extensionCommands, session.promptTemplates, session.resourceLoader.getSkills().skills);
  }

  resources(): SessionResources {
    const session = this.#session;
    const loader = session.resourceLoader;
    const { cwd, agentDir } = this.#services;
    const extensions = loader.getExtensions();
    const skills = loader.getSkills();
    const prompts = loader.getPrompts();
    const active = new Set(session.getActiveToolNames());
    return {
      cwd,
      agentDir,
      mcpConfigPaths: [join(agentDir, "mcp.json"), join(cwd, ".pi", "mcp.json")],
      skills: skills.skills.map((skill) => ({ name: skill.name, description: skill.description, path: skill.filePath, scope: skill.sourceInfo.scope })),
      prompts: prompts.prompts.map((prompt) => ({ name: prompt.name, description: prompt.description, path: prompt.filePath })),
      extensions: extensions.extensions
        .filter((extension) => !extension.hidden)
        .map((extension) => ({ path: extension.path, tools: [...extension.tools.keys()], commands: [...extension.commands.keys()] })),
      tools: session.getAllTools().map((tool) => ({ name: tool.name, description: tool.description, source: toolSource(tool), active: active.has(tool.name) })),
      contextFiles: loader.getAgentsFiles().agentsFiles.map((file) => file.path),
      diagnostics: [
        ...this.#services.diagnostics.map((diagnostic) => diagnostic.message),
        ...extensions.errors.map((error) => `${error.path}: ${error.error}`),
        ...skills.diagnostics.map((diagnostic) => diagnostic.message),
        ...prompts.diagnostics.map((diagnostic) => diagnostic.message),
      ],
    };
  }

  async reload(): Promise<void> {
    this.#requireReady();
    if (this.#session.isStreaming || this.#session.isCompacting) throw new UserError("请等当前回复结束后再重新加载");
    await this.#session.reload();
    this.#onReload?.();
    this.#pushState();
  }

  async dispose(): Promise<void> {
    this.#disposed = true;
    this.#pending.close();
    for (const throttle of this.#throttles.values()) clearTimeout(throttle.timer);
    this.#throttles.clear();
    this.#subagents.dispose();
    try {
      await this.#session.abort();
      // Lets extensions such as MCP close their connections and child processes.
      if (this.#session.extensionRunner.hasHandlers("session_shutdown")) {
        await this.#session.extensionRunner.emit({ type: "session_shutdown", reason: "quit" });
      }
    } catch (error) {
      console.error(`[core] shutting down ${this.id} failed`, error);
    }
    this.#unsubscribe?.();
    this.#session.dispose();
    await this.#services.modelRuntime.settleCredentialWrites();
  }

  // -------------------------------------------------------------------------
  // Extension bridge hooks

  request(request: Omit<PendingRequest, "id">, options: ExtensionUIDialogOptions = {}): Promise<PendingAnswer | undefined> {
    return this.#pending.request(request, options);
  }

  onSubagentEvent(event: SubagentBusEvent): void {
    this.touch();
    this.#subagents.onEvent(event);
  }

  // -------------------------------------------------------------------------
  // Events → frames

  #send(item: TranscriptItem): void {
    this.#clients.toWatchers(this.id, { type: "item", sessionId: this.id, item });
  }

  #pushState(): void {
    if (this.#disposed) return;
    this.#clients.toWatchers(this.id, { type: "state", sessionId: this.id, state: this.state() });
  }

  #notify(level: "info" | "warning" | "error", message: string): void {
    this.#clients.broadcast({ type: "notify", sessionId: this.id, level, message });
  }

  #notice(tone: "info" | "warning" | "error", text: string): void {
    this.#send({ kind: "notice", id: `n:live-${++this.#notices}`, tone, text });
  }

  /** Broadcasts the list entry when something the sidebar shows changed. */
  #broadcastSummary(force = false): SessionSummary {
    const summary = this.summary();
    const key = `${summary.title}\n${summary.status}\n${summary.pendingCount}\n${summary.updatedAt}`;
    if (!this.#disposed && (force || key !== this.#lastSummary)) {
      this.#lastSummary = key;
      this.#clients.broadcast({ type: "session", session: summary });
    }
    return summary;
  }

  #setTool(item: ToolItem, throttle: boolean): void {
    const enriched = this.#subagents.syncToolItem(item);
    this.#tools.set(enriched.id, enriched);
    if (!throttle) {
      const pending = this.#throttles.get(enriched.id);
      clearTimeout(pending?.timer);
      this.#throttles.delete(enriched.id);
      this.#send(enriched);
      return;
    }
    let state = this.#throttles.get(enriched.id);
    if (!state) this.#throttles.set(enriched.id, (state = { at: 0 }));
    const wait = state.at + TOOL_UPDATE_INTERVAL_MS - Date.now();
    if (wait <= 0) {
      state.at = Date.now();
      state.latest = undefined;
      this.#send(enriched);
      return;
    }
    state.latest = enriched;
    const current = state;
    if (!current.timer) {
      current.timer = setTimeout(() => {
        current.timer = undefined;
        if (!current.latest) return;
        current.at = Date.now();
        this.#send(current.latest);
        current.latest = undefined;
      }, wait);
      current.timer.unref();
    }
  }

  #onEvent(event: AgentSessionEvent): void {
    this.touch();
    switch (event.type) {
      case "message_start":
        this.#messageStart(event.message);
        break;
      case "message_update": {
        const update = event.assistantMessageEvent;
        if ((update.type === "text_delta" || update.type === "thinking_delta") && update.delta) {
          if (!this.#streamingSent && this.#streamingPending) {
            this.#streamingId = uniqueId(this.#idCounts, this.#streamingPending.id);
            this.#streamingSent = true;
            this.#send({ ...this.#streamingPending, id: this.#streamingId });
          }
          if (this.#streamingId) {
            this.#clients.toWatchers(this.id, {
              type: "delta",
              sessionId: this.id,
              itemId: this.#streamingId,
              channel: update.type === "text_delta" ? "text" : "thinking",
              delta: update.delta,
            });
          }
        }
        break;
      }
      case "message_end":
        this.#messageEnd(event.message);
        break;
      case "tool_execution_start": {
        if (event.parentToolCallId) break;
        const args = event.args && typeof event.args === "object" ? (event.args as Record<string, unknown>) : {};
        const existing = this.#tools.get(event.toolCallId);
        const base: ToolItem =
          existing ?? {
            kind: "tool",
            id: event.toolCallId,
            name: event.toolName,
            args,
            status: "running",
            output: "",
            ...(event.toolName === "Agent" ? { subagent: buildSubagentToolInfo(args, undefined, "running", "") } : {}),
          };
        this.#setTool({ ...base, status: "running" }, false);
        break;
      }
      case "tool_execution_update": {
        if (event.parentToolCallId) break;
        const base = this.#tools.get(event.toolCallId);
        if (base) this.#setTool(withToolResult(base, event.partialResult, "running"), true);
        break;
      }
      case "tool_execution_end": {
        if (event.parentToolCallId) break;
        const base: ToolItem = this.#tools.get(event.toolCallId) ?? {
          kind: "tool",
          id: event.toolCallId,
          name: event.toolName,
          args: {},
          status: "running",
          output: "",
        };
        this.#setTool(withToolResult(base, event.result, event.isError ? "error" : "done", event.durationMs), false);
        break;
      }
      case "agent_start":
        if (!this.#run) {
          this.#run = { startedAt: performance.now(), outcome: "incomplete" };
          try {
            this.#session.sessionManager.appendCustomEntry(RUN_STARTED);
          } catch (error) {
            console.warn("[core] could not save Run start", error);
          }
        }
        this.#pushState();
        this.#broadcastSummary();
        break;
      case "agent_settled":
        if (this.#run) {
          const status = event.aborted ? "aborted" : this.#run.outcome;
          const durationMs = Math.max(0, Math.round(performance.now() - this.#run.startedAt));
          this.#run = undefined;
          try {
            const id = this.#session.sessionManager.appendCustomEntry(RUN_SETTLED, { status, durationMs });
            this.#send({ kind: "run", id: `run:${id}`, status, durationMs });
          } catch (error) {
            console.warn("[core] could not save Run outcome", error);
          }
        }
        for (const throttle of this.#throttles.values()) clearTimeout(throttle.timer);
        this.#throttles.clear();
        this.#tools.clear();
        this.#streamingId = undefined;
        this.#streamingPending = undefined;
        this.#streamingSent = false;
        this.#retry = undefined;
        this.#subagents.onAgentSettled();
        this.#pushState();
        this.#broadcastSummary();
        break;
      case "queue_update":
      case "thinking_level_changed":
        this.#pushState();
        break;
      case "session_info_changed":
        this.#broadcastSummary();
        break;
      case "compaction_start":
        this.#pushState();
        this.#broadcastSummary();
        break;
      case "compaction_end":
        // A manual compaction reports its failure through the compact command's notification.
        if (event.errorMessage && event.reason !== "manual") this.#notice("error", event.errorMessage);
        if (event.result) {
          this.#syncIdCounts();
          this.#clients.toWatchers(this.id, { type: "reset", sessionId: this.id });
        }
        this.#pushState();
        this.#broadcastSummary();
        break;
      case "auto_retry_start":
        this.#retry = { attempt: event.attempt, max: event.maxAttempts, message: event.errorMessage };
        this.#pushState();
        break;
      case "auto_retry_end":
        this.#retry = undefined;
        if (!event.success) this.#notice("error", `重试 ${event.attempt} 次后仍然失败：${event.finalError ?? "未知错误"}`);
        this.#pushState();
        break;
      default:
        break;
    }
  }

  #messageStart(message: AgentMessage): void {
    if (message.role === "user") {
      if (this.#run) this.#run.outcome = "incomplete";
      this.#firstMessage ??= contentText(message.content);
      const item = userItem(message);
      this.#send({ ...item, id: uniqueId(this.#idCounts, item.id) });
      this.#broadcastSummary();
    } else if (message.role === "assistant") {
      const item = assistantItem(message, true);
      this.#streamingPending = item;
      this.#streamingId = undefined;
      this.#streamingSent = false;
      if (hasAssistantContent(item)) {
        this.#streamingId = uniqueId(this.#idCounts, item.id);
        this.#streamingSent = true;
        this.#send({ ...item, id: this.#streamingId });
      }
    }
  }

  #messageEnd(message: AgentMessage): void {
    switch (message.role) {
      case "assistant": {
        if (this.#run) this.#run.outcome = responseOutcome(message.stopReason);
        const item = assistantItem(message, false);
        if (hasAssistantContent(item)) {
          const id = this.#streamingId ?? uniqueId(this.#idCounts, item.id);
          this.#send({ ...item, id });
        }
        this.#streamingId = undefined;
        this.#streamingPending = undefined;
        this.#streamingSent = false;
        for (const tool of toolCallItems(message)) this.#setTool(tool, false);
        this.#updatedAt = new Date().toISOString();
        this.#pushState();
        this.#broadcastSummary();
        break;
      }
      case "toolResult": {
        // Normally covered by tool_execution_end; this catches results of calls that never ran.
        const base = this.#tools.get(message.toolCallId);
        if (base?.status === "running") this.#setTool(withToolResult(base, message, message.isError ? "error" : "done", message.durationMs), false);
        break;
      }
      case "custom":
        if (message.display) this.#send(customItem(message, uniqueId(this.#idCounts, `c:${message.timestamp}`)));
        break;
      case "user":
        this.#updatedAt = new Date().toISOString();
        this.#broadcastSummary();
        break;
      default:
        break;
    }
  }
}

function toolSource(tool: ToolInfo): string {
  const mcp = tool.namespace?.name.replace(/^mcp__/, "") ?? /^mcp__(.+?)__/.exec(tool.name)?.[1];
  return mcp ?? tool.sourceInfo.source;
}
