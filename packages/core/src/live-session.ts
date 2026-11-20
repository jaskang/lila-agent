import { randomUUID } from "node:crypto";
import { join } from "node:path";
import type {
  AgentSession,
  AgentSessionEvent,
  AgentSessionServices,
  ExtensionUIContext,
  ExtensionUIDialogOptions,
  ToolInfo,
} from "@earendil-works/pi-coding-agent";
import type {
  ClientCommand,
  PendingAnswer,
  PendingRequest,
  ServerMessage,
  SessionResources,
  SessionState,
  SessionStatus,
  SessionSummary,
  SlashCommand,
  SubagentProgress,
  SubagentStatus,
  TranscriptItem,
} from "@lila/protocol";
import { errorMessage, UserError } from "./errors.ts";
import type { LilaBridge, SubagentBusEvent } from "./lila-extension.ts";
import type { Store } from "./store.ts";
import {
  type AgentMessage,
  assistantItem,
  contentText,
  customItem,
  displayUserText,
  earlier,
  parseUserItemId,
  projectBranch,
  snapshot,
  type ToolItem,
  toolCallItems,
  type UserMessage,
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
type PendingEntry = { request: PendingRequest; settle(answer: PendingAnswer | undefined): void };
type ToolThrottle = { at: number; timer?: NodeJS.Timeout; latest?: ToolItem };

const TOOL_UPDATE_INTERVAL_MS = 100;
const SUBAGENT_PUSH_INTERVAL_MS = 200;
const MAX_SUBAGENT_ENTRIES = 50;
const TITLE_LIMIT = 60;

function normalizeSubagentStatus(raw: string | undefined): SubagentStatus {
  switch (raw) {
    case "queued":
    case "running":
    case "completed":
    case "steered":
    case "stopped":
    case "aborted":
    case "error":
      return raw;
    case "background":
      return "running";
    default:
      return "completed";
  }
}

function isActiveSubagent(status: SubagentStatus): boolean {
  return status === "queued" || status === "running";
}
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
  const match = /^([^\s#]+)#([0-9a-f]{8})$/i.exec(name.trim());
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

/** Identity styling: extensions that style text for the TUI get their plain text back. */
const plainTheme = new Proxy(
  {},
  { get: () => (...args: unknown[]) => args.findLast((arg) => typeof arg === "string") ?? "" },
) as ExtensionUIContext["theme"];

/** One opened Pi session: its AgentSession, pending user requests and the mapping of its events to frames. */
export class LiveSession implements LilaBridge {
  readonly id: string;
  readonly projectId: string;
  readonly parentId: string | undefined;
  readonly agentId: string | undefined;
  readonly #subagentDescription: string | undefined;
  readonly allowedTools = new Set<string>();
  lastActivity = Date.now();
  readonly #store: Store;
  readonly #clients: Clients;
  readonly #onReload?: () => void;
  readonly #onSubsessionsChanged?: () => void;
  #session!: AgentSession;
  #services!: AgentSessionServices;
  #unsubscribe?: () => void;
  readonly #pending = new Map<string, PendingEntry>();
  #statusLines: Record<string, string> = {};
  #retry: SessionState["retry"];
  #updatedAt = new Date().toISOString();
  #firstMessage: string | undefined;
  #streamingId: string | undefined;
  /** Tool items of the current run, with their newest output. */
  readonly #tools = new Map<string, ToolItem>();
  readonly #throttles = new Map<string, ToolThrottle>();
  readonly #subagents = new Map<string, SubagentProgress>();
  #subagentPushTimer: NodeJS.Timeout | undefined;
  #notices = 0;
  #lastSummary = "";

  constructor(options: {
    id: string;
    projectId: string;
    parentId?: string | undefined;
    agentId?: string | undefined;
    subagentDescription?: string | undefined;
    store: Store;
    clients: Clients;
    onReload?: () => void;
    onSubsessionsChanged?: () => void;
  }) {
    this.id = options.id;
    this.projectId = options.projectId;
    this.parentId = options.parentId;
    this.agentId = options.agentId;
    this.#subagentDescription = options.subagentDescription;
    this.#store = options.store;
    this.#clients = options.clients;
    this.#onReload = options.onReload;
    this.#onSubsessionsChanged = options.onSubsessionsChanged;
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
    if (this.#session.isStreaming || this.#session.isCompacting || this.#pending.size > 0) return true;
    for (const sub of this.#subagents.values()) if (isActiveSubagent(sub.status)) return true;
    return false;
  }

  async attach(services: AgentSessionServices, session: AgentSession, updatedAt?: string): Promise<void> {
    this.#services = services;
    this.#session = session;
    if (updatedAt) this.#updatedAt = updatedAt;
    for (const entry of session.sessionManager.getEntries()) {
      if (entry.type === "message" && entry.message.role === "user") {
        this.#firstMessage = contentText(entry.message.content);
        break;
      }
    }
    this.#unsubscribe = session.subscribe((event) => this.#onEvent(event));
    await session.bindExtensions({
      uiContext: this.#uiContext(),
      mode: "rpc",
      commandContextActions: {
        waitForIdle: () => this.#session.waitForIdle(),
        reload: async () => {
          await this.#session.reload();
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
    });
  }

  touch(): void {
    this.lastActivity = Date.now();
  }

  // -------------------------------------------------------------------------
  // Snapshots

  get status(): SessionStatus {
    if (this.#pending.size > 0) return "waiting";
    if (this.#session.isStreaming || this.#session.isCompacting) return "running";
    const messages = this.#session.messages;
    for (let index = messages.length - 1; index >= 0; index--) {
      const message = messages[index];
      if (message?.role === "assistant") return message.stopReason === "error" ? "error" : "idle";
    }
    return "idle";
  }

  subagentByPrefix(idPrefix: string): SubagentProgress | undefined {
    const lower = idPrefix.toLowerCase();
    for (const entry of this.#subagents.values()) {
      if (entry.id.toLowerCase().startsWith(lower)) return entry;
    }
    return undefined;
  }

  summary(): SessionSummary {
    const auto = this.parentId ? parseAutoSubagentName(this.#session.sessionName) : undefined;
    const resolvedAgentId = this.agentId ?? auto?.idPrefix;
    return {
      id: this.id,
      projectId: this.projectId,
      title: sessionTitle(this.#session.sessionName, this.#firstMessage, {
        isSubsession: Boolean(this.parentId),
        description: this.#subagentDescription,
      }),
      updatedAt: this.#updatedAt,
      status: this.status,
      pendingCount: this.#pending.size,
      ...(this.parentId ? { parentId: this.parentId } : {}),
      ...(resolvedAgentId ? { agentId: resolvedAgentId } : {}),
    };
  }

  state(): SessionState {
    const session = this.#session;
    const usage = session.getContextUsage();
    const model = session.model;
    return {
      status: this.status,
      ...(model ? { modelId: `${model.provider}/${model.id}` } : {}),
      thinkingLevel: session.thinkingLevel,
      thinkingLevels: session.getAvailableThinkingLevels(),
      approvalMode: this.approvalMode,
      queue: { steering: [...session.getSteeringMessages()], followUp: [...session.getFollowUpMessages()] },
      ...(usage ? { context: { tokens: usage.tokens, window: usage.contextWindow, percent: usage.percent } } : {}),
      cost: session.getSessionStats().cost,
      compacting: session.isCompacting,
      ...(this.#retry ? { retry: this.#retry } : {}),
      pending: Array.from(this.#pending.values()).map((entry) => entry.request),
      statusLines: { ...this.#statusLines },
      ...(this.#subagents.size ? { subagents: Array.from(this.#subagents.values(), (entry) => ({ ...entry })) } : {}),
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
      live.push(this.#streamingId ? { ...item, id: this.#streamingId } : item);
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

  // -------------------------------------------------------------------------
  // Commands

  async prompt(command: Command<"session.prompt">): Promise<void> {
    this.touch();
    const session = this.#session;
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
          if (!accepted) return reject(error);
          console.error(`[core] run of ${this.id} failed`, error);
          this.#notify("error", errorMessage(error));
        },
      );
    await promise;
  }

  async abort(): Promise<void> {
    this.touch();
    for (const entry of Array.from(this.#pending.values())) entry.settle(undefined);
    await this.#session.abort();
  }

  async configure(command: Command<"session.configure">): Promise<SessionState> {
    this.touch();
    if (command.modelId) {
      const model = findModel(this.#services, command.modelId);
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
    this.touch();
    this.#session.compact().catch((error: unknown) => this.#notify("error", `压缩失败：${errorMessage(error)}`));
  }

  async rewind(itemId: string): Promise<string> {
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
    this.#clients.toWatchers(this.id, { type: "reset", sessionId: this.id });
    this.#pushState();
    return displayUserText(result.editorText ?? contentText((entry.message as UserMessage).content));
  }

  respond(pendingId: string, answer: PendingAnswer): void {
    const entry = this.#pending.get(pendingId);
    if (!entry) throw new UserError("这个请求已经结束了");
    entry.settle(answer);
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
    if (this.#session.isStreaming || this.#session.isCompacting) throw new UserError("请等当前回复结束后再重新加载");
    await this.#session.reload();
    this.#onReload?.();
    this.#pushState();
  }

  async dispose(): Promise<void> {
    for (const entry of Array.from(this.#pending.values())) entry.settle(undefined);
    for (const throttle of this.#throttles.values()) clearTimeout(throttle.timer);
    this.#throttles.clear();
    clearTimeout(this.#subagentPushTimer);
    this.#subagentPushTimer = undefined;
    this.#subagents.clear();
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
  // Requests that wait for the user

  request(request: Omit<PendingRequest, "id">, options: ExtensionUIDialogOptions = {}): Promise<PendingAnswer | undefined> {
    const id = randomUUID();
    const { signal, timeout } = options;
    const { promise, resolve } = Promise.withResolvers<PendingAnswer | undefined>();
    if (signal?.aborted) {
      resolve(undefined);
      return promise;
    }
    let timer: NodeJS.Timeout | undefined;
    const onAbort = () => settle(undefined);
    const settle = (answer: PendingAnswer | undefined) => {
      if (!this.#pending.delete(id)) return;
      clearTimeout(timer);
      signal?.removeEventListener("abort", onAbort);
      this.#pendingChanged();
      resolve(answer);
    };
    this.#pending.set(id, { request: { ...request, id }, settle });
    signal?.addEventListener("abort", onAbort, { once: true });
    if (timeout) timer = setTimeout(onAbort, timeout);
    this.#pendingChanged();
    return promise;
  }

  #pendingChanged(): void {
    this.touch();
    this.#pushState();
    this.#broadcastSummary();
  }

  #uiContext(): ExtensionUIContext {
    const dialog = <T>(request: Omit<PendingRequest, "id">, options: ExtensionUIDialogOptions | undefined, map: (answer: PendingAnswer | undefined) => T) =>
      this.request(request, options).then(map);
    const text = (answer: PendingAnswer | undefined) => (answer?.type === "text" ? answer.value : undefined);
    return {
      select: (title, options, opts) => dialog({ kind: "select", title, options }, opts, (answer) => (answer?.type === "option" ? answer.value : undefined)),
      confirm: (title, message, opts) => dialog({ kind: "confirm", title, message, options: [] }, opts, (answer) => answer?.type === "confirm" && answer.confirmed),
      input: (title, placeholder, opts) => dialog({ kind: "input", title, options: [], ...(placeholder ? { placeholder } : {}) }, opts, text),
      editor: (title, prefill) => dialog({ kind: "editor", title, options: [], ...(prefill ? { prefill } : {}) }, undefined, text),
      notify: (message, type) => this.#notify(type ?? "info", message),
      setStatus: (key, value) => {
        // pi-subagents writes a TUI status line (`1 running agent`) that duplicates Lila's native sub-agent UI.
        if (key === "subagents") return;
        if (value === undefined) delete this.#statusLines[key];
        else this.#statusLines[key] = value;
        this.#pushState();
      },
      // TUI-only capabilities.
      onTerminalInput: () => () => {},
      setWorkingMessage: () => {},
      setWorkingVisible: () => {},
      setWorkingIndicator: () => {},
      setHiddenThinkingLabel: () => {},
      setWidget: () => {},
      setFooter: () => {},
      setHeader: () => {},
      setTitle: () => {},
      custom: async () => undefined as never,
      pasteToEditor: () => {},
      setEditorText: () => {},
      getEditorText: () => "",
      addAutocompleteProvider: () => {},
      setEditorComponent: () => {},
      getEditorComponent: () => undefined,
      theme: plainTheme,
      getAllThemes: () => [],
      getTheme: () => undefined,
      setTheme: () => ({ success: false, error: "Lila 不支持主题" }),
      getToolsExpanded: () => false,
      setToolsExpanded: () => {},
    };
  }

  // -------------------------------------------------------------------------
  // Sub-agent progress (`pi-subagents` event bus + foreground tool updates)

  onSubagentEvent(event: SubagentBusEvent): void {
    this.touch();
    switch (event.type) {
      case "created": {
        const existing = this.#subagents.get(event.id);
        this.#subagents.set(event.id, {
          id: event.id,
          type: event.agentType,
          description: event.description || existing?.description || "",
          status: existing && isActiveSubagent(existing.status) ? existing.status : "queued",
          background: event.isBackground,
          startedAt: existing?.startedAt ?? Date.now(),
          toolUses: existing?.toolUses ?? 0,
          turns: existing?.turns ?? 0,
          tokens: existing?.tokens ?? 0,
          cost: existing?.cost ?? 0,
          ...(existing?.model ? { model: existing.model } : {}),
          ...(existing?.activity ? { activity: existing.activity } : {}),
        });
        this.#scheduleSubagentPush(true);
        break;
      }
      case "started": {
        const existing = this.#subagents.get(event.id);
        this.#subagents.set(event.id, {
          id: event.id,
          type: event.agentType || existing?.type || "Agent",
          description: event.description || existing?.description || "",
          status: "running",
          background: existing?.background ?? false,
          startedAt: Date.now(),
          toolUses: existing?.toolUses ?? 0,
          turns: existing?.turns ?? 0,
          tokens: existing?.tokens ?? 0,
          cost: existing?.cost ?? 0,
          ...(existing?.model ? { model: existing.model } : {}),
          ...(existing?.activity ? { activity: existing.activity } : {}),
        });
        this.#trimSubagents();
        this.#scheduleSubagentPush(true);
        this.#onSubsessionsChanged?.();
        break;
      }
      case "usage": {
        if (event.depth !== undefined && event.depth > 1) return;
        const existing = this.#subagents.get(event.id) ?? {
          id: event.id,
          type: event.agentType,
          description: event.description,
          status: "running" as const,
          background: false,
          startedAt: Date.now(),
          toolUses: 0,
          turns: 0,
          tokens: 0,
          cost: 0,
        };
        const firstTurn = existing.turns === 0;
        existing.turns += 1;
        existing.tokens += event.tokens;
        existing.cost += event.cost;
        if (event.model) existing.model = event.model;
        if (existing.status === "queued") existing.status = "running";
        this.#subagents.set(event.id, existing);
        this.#scheduleSubagentPush(false);
        // Sub-agent session JSONL is flushed once its first message is written.
        if (firstTurn) this.#onSubsessionsChanged?.();
        break;
      }
      case "compacted": {
        const existing = this.#subagents.get(event.id);
        if (!existing) return;
        existing.activity = event.compactionCount ? `已压缩上下文（第 ${event.compactionCount} 次）` : "已压缩上下文";
        this.#scheduleSubagentPush(false);
        break;
      }
      case "steered": {
        const existing = this.#subagents.get(event.id);
        if (!existing) return;
        existing.activity = "已收到引导消息";
        this.#scheduleSubagentPush(false);
        break;
      }
      case "finished": {
        const existing = this.#subagents.get(event.id);
        const next: SubagentProgress = {
          id: event.id,
          type: event.agentType || existing?.type || "Agent",
          description: event.description || existing?.description || "",
          status: normalizeSubagentStatus(event.status),
          background: existing?.background ?? false,
          startedAt: existing?.startedAt ?? Date.now(),
          toolUses: event.toolUses ?? existing?.toolUses ?? 0,
          turns: existing?.turns ?? 0,
          tokens: event.tokens ?? existing?.tokens ?? 0,
          cost: event.cost ?? existing?.cost ?? 0,
          ...(existing?.model ? { model: existing.model } : {}),
          ...(event.durationMs !== undefined ? { durationMs: event.durationMs } : {}),
          ...(event.result ? { output: event.result } : {}),
          ...(event.error ? { error: event.error } : {}),
        };
        this.#subagents.set(event.id, next);
        this.#trimSubagents();
        this.#scheduleSubagentPush(true);
        this.#onSubsessionsChanged?.();
        break;
      }
      case "disposed":
        if (!this.#subagents.size) return;
        this.#subagents.clear();
        this.#scheduleSubagentPush(true);
        break;
    }
  }

  #trimSubagents(): void {
    if (this.#subagents.size <= MAX_SUBAGENT_ENTRIES) return;
    for (const [id, entry] of this.#subagents) {
      if (this.#subagents.size <= MAX_SUBAGENT_ENTRIES) break;
      if (!isActiveSubagent(entry.status)) this.#subagents.delete(id);
    }
  }

  #syncAgentToolProgress(item: ToolItem): void {
    if (item.name !== "Agent" || !item.details) return;
    const details = item.details;
    const id = typeof details.agentId === "string" && details.agentId ? details.agentId : undefined;
    if (!id) return;
    const existing = this.#subagents.get(id);
    const rawStatus = typeof details.status === "string" ? details.status : undefined;
    const incoming = rawStatus ? normalizeSubagentStatus(rawStatus) : existing?.status ?? (item.status === "running" ? "running" : item.status === "error" ? "error" : "completed");
    // Do not regress a sub-agent that has already finished back to running when its spawn tool result lands.
    const status = existing && !isActiveSubagent(existing.status) && isActiveSubagent(incoming) ? existing.status : incoming;
    const next: SubagentProgress = {
      id,
      type: (typeof details.subagentType === "string" && details.subagentType) || (typeof item.args.subagent_type === "string" && item.args.subagent_type) || existing?.type || "Agent",
      description: (typeof details.description === "string" && details.description) || (typeof item.args.description === "string" && item.args.description) || existing?.description || "",
      status,
      background: rawStatus === "background" || Boolean(item.args.run_in_background) || (existing?.background ?? false),
      startedAt: existing?.startedAt ?? Date.now(),
      toolUses: typeof details.toolUses === "number" && details.toolUses > (existing?.toolUses ?? 0) ? details.toolUses : (existing?.toolUses ?? 0),
      turns: typeof details.turnCount === "number" && details.turnCount > (existing?.turns ?? 0) ? details.turnCount : (existing?.turns ?? 0),
      tokens: existing?.tokens ?? 0,
      cost: typeof details.cost === "number" && details.cost > (existing?.cost ?? 0) ? details.cost : (existing?.cost ?? 0),
      ...(typeof details.modelName === "string" && details.modelName ? { model: details.modelName } : existing?.model ? { model: existing.model } : {}),
      ...(typeof details.activity === "string" && details.activity && isActiveSubagent(status) ? { activity: details.activity } : existing?.activity && isActiveSubagent(status) ? { activity: existing.activity } : {}),
      ...(typeof details.durationMs === "number" && details.durationMs > 0 && !isActiveSubagent(status) ? { durationMs: details.durationMs } : existing?.durationMs !== undefined ? { durationMs: existing.durationMs } : {}),
      ...(existing?.output ? { output: existing.output } : {}),
      ...(typeof details.error === "string" && details.error ? { error: details.error } : existing?.error ? { error: existing.error } : {}),
    };
    this.#subagents.set(id, next);
    this.#trimSubagents();
    this.#scheduleSubagentPush(!isActiveSubagent(status));
  }

  #scheduleSubagentPush(immediate: boolean): void {
    if (immediate) {
      clearTimeout(this.#subagentPushTimer);
      this.#subagentPushTimer = undefined;
      this.#pushState();
      return;
    }
    this.#subagentPushTimer ??= setTimeout(() => {
      this.#subagentPushTimer = undefined;
      this.#pushState();
    }, SUBAGENT_PUSH_INTERVAL_MS);
  }

  // -------------------------------------------------------------------------
  // Events → frames

  #send(item: TranscriptItem): void {
    this.#clients.toWatchers(this.id, { type: "item", sessionId: this.id, item });
  }

  #pushState(): void {
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
    if (force || key !== this.#lastSummary) {
      this.#lastSummary = key;
      this.#clients.broadcast({ type: "session", session: summary });
    }
    return summary;
  }

  #setTool(item: ToolItem, throttle: boolean): void {
    this.#tools.set(item.id, item);
    this.#syncAgentToolProgress(item);
    if (!throttle) {
      const pending = this.#throttles.get(item.id);
      clearTimeout(pending?.timer);
      this.#throttles.delete(item.id);
      this.#send(item);
      return;
    }
    let state = this.#throttles.get(item.id);
    if (!state) this.#throttles.set(item.id, (state = { at: 0 }));
    const wait = state.at + TOOL_UPDATE_INTERVAL_MS - Date.now();
    if (wait <= 0) {
      state.at = Date.now();
      state.latest = undefined;
      this.#send(item);
      return;
    }
    state.latest = item;
    const current = state;
    current.timer ??= setTimeout(() => {
      current.timer = undefined;
      if (!current.latest) return;
      current.at = Date.now();
      this.#send(current.latest);
      current.latest = undefined;
    }, wait);
  }

  #onEvent(event: AgentSessionEvent): void {
    this.touch();
    switch (event.type) {
      case "message_start":
        this.#messageStart(event.message);
        break;
      case "message_update": {
        const update = event.assistantMessageEvent;
        if ((update.type === "text_delta" || update.type === "thinking_delta") && this.#streamingId) {
          this.#clients.toWatchers(this.id, {
            type: "delta",
            sessionId: this.id,
            itemId: this.#streamingId,
            channel: update.type === "text_delta" ? "text" : "thinking",
            delta: update.delta,
          });
        }
        break;
      }
      case "message_end":
        this.#messageEnd(event.message);
        break;
      case "tool_execution_start": {
        if (event.parentToolCallId) break;
        const args = event.args && typeof event.args === "object" ? (event.args as Record<string, unknown>) : {};
        const base = this.#tools.get(event.toolCallId) ?? { kind: "tool", id: event.toolCallId, name: event.toolName, args, status: "running", output: "" };
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
        const base = this.#tools.get(event.toolCallId) ?? { kind: "tool", id: event.toolCallId, name: event.toolName, args: {}, status: "running", output: "" };
        this.#setTool(withToolResult(base, event.result, event.isError ? "error" : "done", event.durationMs), false);
        break;
      }
      case "agent_start":
        this.#pushState();
        this.#broadcastSummary();
        break;
      case "agent_settled":
        for (const throttle of this.#throttles.values()) clearTimeout(throttle.timer);
        this.#throttles.clear();
        this.#tools.clear();
        this.#streamingId = undefined;
        this.#retry = undefined;
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
        if (event.result) this.#clients.toWatchers(this.id, { type: "reset", sessionId: this.id });
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
      this.#firstMessage ??= contentText(message.content);
      this.#send(userItem(message));
      this.#broadcastSummary();
    } else if (message.role === "assistant") {
      const item = assistantItem(message, true);
      this.#streamingId = item.id;
      this.#send(item);
    }
  }

  #messageEnd(message: AgentMessage): void {
    switch (message.role) {
      case "assistant": {
        const item = assistantItem(message, false);
        this.#send(this.#streamingId ? { ...item, id: this.#streamingId } : item);
        this.#streamingId = undefined;
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
        if (message.display) this.#send(customItem(message, `c:${message.timestamp}`));
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
