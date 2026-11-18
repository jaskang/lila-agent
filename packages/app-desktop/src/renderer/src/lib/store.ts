import { create } from "zustand";
import { toast } from "sonner";
import type {
  ApprovalMode,
  Attachment,
  AttachmentMeta,
  FileDiff,
  LiveSnapshot,
  Model,
  PendingAnswer,
  PendingKind,
  PendingQuestion,
  Project,
  SessionEvent,
  SessionSummary,
  StreamMessage,
} from "@lila/protocol";
import { CoreClient, type ConnectionState } from "./client.ts";

export type TranscriptItem =
  | { kind: "user"; id: string; text: string; attachments?: AttachmentMeta[] }
  | { kind: "assistant"; id: string; text: string; thinking: string; streaming: boolean }
  | {
      kind: "tool";
      id: string;
      toolName: string;
      intent?: string;
      args: Record<string, unknown>;
      status: "running" | "done" | "error";
      output: string;
      diffs: FileDiff[];
    }
  | { kind: "notice"; id: string; tone: "info" | "warning" | "error"; text: string }
  | { kind: "decision"; id: string; title: string; status: "answered" | "cancelled" | "interrupted"; answer?: string };

export type OpenPending = {
  pendingId: string;
  kind: PendingKind;
  title: string;
  message?: string;
  options: string[];
  questions: PendingQuestion[];
  toolCallId?: string;
  placeholder?: string;
  defaultValue?: string;
};

export type SessionView = {
  lastSeq: number;
  firstSeq: number;
  items: TranscriptItem[];
  /** Item id → index in `items`. */
  index: Record<string, number>;
  pending: OpenPending[];
  runId?: string;
  loaded: boolean;
  loading: boolean;
  loadError?: string;
  hasEarlier: boolean;
  loadingEarlier: boolean;
};

type State = {
  connection: ConnectionState;
  connectionDetail?: string;
  projects: Project[];
  models: Model[];
  defaultModelId?: string;
  modelsError?: string;
  sessions: Record<string, SessionSummary>;
  views: Record<string, SessionView>;
  selectedId?: string;
};

const emptyView = (): SessionView => ({
  lastSeq: 0,
  firstSeq: 0,
  items: [],
  index: {},
  pending: [],
  loaded: false,
  loading: false,
  hasEarlier: false,
  loadingEarlier: false,
});
const SELECTED_KEY = "lila.selectedSession";

export const useLila = create<State>(() => ({
  connection: "connecting",
  projects: [],
  models: [],
  sessions: {},
  views: {},
  selectedId: localStorage.getItem(SELECTED_KEY) ?? undefined,
}));

// ---------------------------------------------------------------------------
// Reducer: pure updates of one session view.

function upsert(view: SessionView, item: TranscriptItem): SessionView {
  const at = view.index[item.id];
  if (at === undefined) {
    return { ...view, items: [...view.items, item], index: { ...view.index, [item.id]: view.items.length } };
  }
  const items = view.items.slice();
  items[at] = item;
  return { ...view, items };
}

function patch<K extends TranscriptItem["kind"]>(
  view: SessionView,
  id: string,
  kind: K,
  update: (item: Extract<TranscriptItem, { kind: K }>) => Extract<TranscriptItem, { kind: K }>,
): SessionView {
  const at = view.index[id];
  const item = at === undefined ? undefined : view.items[at];
  if (!item || item.kind !== kind) return view;
  return upsert(view, update(item as Extract<TranscriptItem, { kind: K }>));
}

function rebuildIndex(items: TranscriptItem[]): Record<string, number> {
  return Object.fromEntries(items.map((item, index) => [item.id, index]));
}

function prependEvents(view: SessionView, events: SessionEvent[], models: Model[], hasEarlier: boolean): SessionView {
  let historical = emptyView();
  for (const event of events) historical = applyEvent(historical, event, models);
  const existing = new Set(view.items.map((item) => item.id));
  const items = [...historical.items.filter((item) => !existing.has(item.id)), ...view.items];
  return {
    ...view,
    firstSeq: events[0]?.seq ?? view.firstSeq,
    hasEarlier,
    items,
    index: rebuildIndex(items),
  };
}

export function applyEvent(view: SessionView, event: SessionEvent, models: Model[]): SessionView {
  if (event.seq <= view.lastSeq) return view;
  let next: SessionView = { ...view, lastSeq: event.seq };
  const id = `e${event.seq}`;
  switch (event.eventType) {
    case "session.created":
      return next;
    case "session.updated": {
      const label = models.find((model) => model.id === event.payload.modelId)?.label ?? "默认模型";
      return upsert(next, { kind: "notice", id, tone: "info", text: `已切换到 ${label} · ${APPROVAL_LABEL[event.payload.approvalMode]}` });
    }
    case "message.user":
      return upsert(next, { kind: "user", id, text: event.payload.text, ...(event.payload.attachments ? { attachments: event.payload.attachments } : {}) });
    case "run.started":
      return { ...next, runId: event.runId };
    case "assistant.message": {
      const { messageId, text, thinking } = event.payload;
      return upsert(next, { kind: "assistant", id: messageId, text, thinking, streaming: false });
    }
    case "tool.started": {
      const { toolCallId, toolName, intent, args } = event.payload;
      const existing = next.items[next.index[toolCallId] ?? -1];
      return upsert(next, {
        kind: "tool",
        id: toolCallId,
        toolName,
        ...(intent ? { intent } : {}),
        args,
        status: "running",
        output: existing?.kind === "tool" ? existing.output : "",
        diffs: existing?.kind === "tool" ? existing.diffs : [],
      });
    }
    case "tool.finished":
      return patch(next, event.payload.toolCallId, "tool", (tool) => ({
        ...tool,
        status: event.payload.isError ? "error" : "done",
        output: event.payload.output,
        diffs: event.payload.diffs.length ? event.payload.diffs : tool.diffs,
      }));
    case "pending.created":
      return { ...next, pending: [...next.pending, event.payload] };
    case "pending.resolved": {
      const open = next.pending.find((item) => item.pendingId === event.payload.pendingId);
      next = { ...next, pending: next.pending.filter((item) => item.pendingId !== event.payload.pendingId) };
      if (!open) return next;
      return upsert(next, {
        kind: "decision",
        id,
        title: open.title,
        status: event.payload.status,
        ...(event.payload.answer ? { answer: event.payload.answer } : {}),
      });
    }
    case "run.finished":
      return endRun(next);
    case "run.aborted":
      return upsert(endRun(next), { kind: "notice", id, tone: "warning", text: "已停止" });
    case "run.failed":
      return upsert(endRun(next), { kind: "notice", id, tone: "error", text: event.payload.error });
    case "run.interrupted":
      return upsert(endRun(next), { kind: "notice", id, tone: "warning", text: event.payload.reason });
  }
}

function endRun(view: SessionView): SessionView {
  const items = view.items.map((item) => {
    if (item.kind === "assistant" && item.streaming) return { ...item, streaming: false };
    if (item.kind === "tool" && item.status === "running") return { ...item, status: "error" as const };
    return item;
  });
  const { runId: _, ...rest } = view;
  return { ...rest, items, pending: [] };
}

export function applyStream(view: SessionView, message: StreamMessage): SessionView {
  if (message.runId !== view.runId) return view;
  if (message.kind === "assistant.delta") {
    const at = view.index[message.messageId];
    const current = at === undefined ? undefined : view.items[at];
    if (current && (current.kind !== "assistant" || !current.streaming)) return view;
    const base = current ?? { kind: "assistant" as const, id: message.messageId, text: "", thinking: "", streaming: true };
    return upsert(view, { ...base, [message.channel]: base[message.channel] + message.delta });
  }
  const at = view.index[message.toolCallId];
  const current = at === undefined ? undefined : view.items[at];
  if (current && (current.kind !== "tool" || current.status !== "running")) return view;
  const base =
    current ??
    ({ kind: "tool", id: message.toolCallId, toolName: message.toolName, args: {}, status: "running", output: "", diffs: [] } as const);
  return upsert(view, {
    ...base,
    ...(message.output !== undefined ? { output: message.output } : {}),
    ...(message.diffs ? { diffs: message.diffs } : {}),
  });
}

function applyLive(view: SessionView, live: LiveSnapshot): SessionView {
  let next: SessionView = { ...view, runId: live.runId };
  for (const message of live.messages) {
    next = upsert(next, { kind: "assistant", id: message.messageId, text: message.text, thinking: message.thinking, streaming: true });
  }
  for (const tool of live.tools) {
    next = applyStream(next, { type: "stream", sessionId: "", runId: live.runId, kind: "tool.progress", ...tool });
  }
  return next;
}

// ---------------------------------------------------------------------------
// Actions.

export const APPROVAL_LABEL: Record<ApprovalMode, string> = {
  yolo: "自动执行",
  write: "执行命令前询问",
  "always-ask": "修改和执行都询问",
};

const set = useLila.setState;
const get = useLila.getState;

function setView(sessionId: string, update: (view: SessionView) => SessionView): void {
  set((state) => ({ views: { ...state.views, [sessionId]: update(state.views[sessionId] ?? emptyView()) } }));
}

function report(error: unknown): void {
  toast.error(error instanceof Error ? error.message : String(error));
}

/** Streams are merged per animation frame so a fast model cannot flood React with renders. */
let streamBuffer: StreamMessage[] = [];
let frame: number | undefined;
function flushStream(): void {
  if (frame !== undefined) cancelAnimationFrame(frame);
  frame = undefined;
  if (!streamBuffer.length) return;
  const buffered = streamBuffer;
  streamBuffer = [];
  set((state) => {
    const views = { ...state.views };
    for (const message of buffered) {
      const view = views[message.sessionId];
      if (view) views[message.sessionId] = applyStream(view, message);
    }
    return { views };
  });
}

/** Live frames that arrive while a resume request is in flight wait for its replay. */
const resuming = new Map<string, Array<SessionEvent | StreamMessage>>();

function notify(title: string, body: string): void {
  if (document.hasFocus()) return;
  const notification = new Notification(title, { body, silent: false });
  notification.onclick = () => window.lila.focus();
}

function handleEvent(event: SessionEvent): void {
  const queue = resuming.get(event.sessionId);
  if (queue) {
    queue.push(event);
    return;
  }
  flushStream();
  const view = get().views[event.sessionId];
  if (!view) return;
  if (event.seq > view.lastSeq + 1) {
    void resume(event.sessionId);
    return;
  }
  setView(event.sessionId, (current) => applyEvent(current, event, get().models));
  const title = get().sessions[event.sessionId]?.title || "Lila";
  if (event.eventType === "pending.created") notify(`需要你处理 · ${title}`, event.payload.title);
  if (event.eventType === "run.finished") notify("任务完成", title);
  if (event.eventType === "run.failed") notify("任务失败", event.payload.error);
}

function handleStream(message: StreamMessage): void {
  const queue = resuming.get(message.sessionId);
  if (queue) {
    queue.push(message);
    return;
  }
  streamBuffer.push(message);
  frame ??= requestAnimationFrame(flushStream);
}

async function resume(sessionId: string): Promise<void> {
  if (resuming.has(sessionId)) return;
  resuming.set(sessionId, []);
  setView(sessionId, (current) => ({ ...current, loading: true, loadError: undefined }));
  try {
    const afterSeq = get().views[sessionId]?.lastSeq ?? 0;
    const { events, live, hasEarlier } = await client.call({ type: "events.resume", sessionId, afterSeq });
    const models = get().models;
    setView(sessionId, (current) => {
      let next = afterSeq === 0 ? emptyView() : current;
      for (const event of events) next = applyEvent(next, event, models);
      if (live) next = applyLive(next, live);
      return {
        ...next,
        firstSeq: afterSeq === 0 ? (events[0]?.seq ?? 0) : current.firstSeq,
        hasEarlier: afterSeq === 0 ? hasEarlier : current.hasEarlier,
        loaded: true,
        loading: false,
        loadError: undefined,
      };
    });
  } catch (error) {
    report(error);
    setView(sessionId, (current) => ({
      ...current,
      loading: false,
      loadError: error instanceof Error ? error.message : String(error),
    }));
  } finally {
    const queued = resuming.get(sessionId) ?? [];
    resuming.delete(sessionId);
    for (const item of queued) {
      if (item.type === "event") handleEvent(item);
      else handleStream(item);
    }
  }
}

async function loadModels(): Promise<void> {
  try {
    const { models, defaultModelId } = await client.call({ type: "models.list" });
    set({ models, ...(defaultModelId ? { defaultModelId } : {}), modelsError: undefined });
  } catch (error) {
    set({ modelsError: error instanceof Error ? error.message : String(error) });
  }
}

export const client = new CoreClient({
  onState: (connection, connectionDetail) => set({ connection, connectionDetail }),
  onReady: () => {
    void (async () => {
      try {
        const [{ projects }, { sessions }] = await Promise.all([
          client.call({ type: "projects.list" }),
          client.call({ type: "sessions.list" }),
        ]);
        const byId = Object.fromEntries(sessions.map((session) => [session.id, session]));
        const selectedId = get().selectedId;
        set({ projects, sessions: byId, ...(selectedId && !byId[selectedId] ? { selectedId: undefined } : {}) });
        const current = get().selectedId;
        if (current) await resume(current);
      } catch (error) {
        report(error);
      }
    })();
    if (!get().models.length) void loadModels();
  },
  onEvent: handleEvent,
  onStream: handleStream,
  onSession: ({ session }) => set((state) => ({ sessions: { ...state.sessions, [session.id]: session } })),
});

export const actions = {
  select(sessionId: string | undefined): void {
    set({ selectedId: sessionId });
    if (sessionId) localStorage.setItem(SELECTED_KEY, sessionId);
    else localStorage.removeItem(SELECTED_KEY);
    if (sessionId && get().connection === "connected") void resume(sessionId);
  },

  async addProject(): Promise<Project | undefined> {
    const path = await window.lila.pickDirectory();
    if (!path) return undefined;
    try {
      const { project } = await client.call({ type: "projects.add", path });
      set((state) => ({
        projects: state.projects.some((item) => item.id === project.id) ? state.projects : [...state.projects, project],
      }));
      return project;
    } catch (error) {
      report(error);
      return undefined;
    }
  },

  async createSession(projectId: string, modelId: string | undefined, approvalMode: ApprovalMode): Promise<void> {
    try {
      const { session } = await client.call({
        type: "session.create",
        projectId,
        approvalMode,
        ...(modelId ? { modelId } : {}),
      });
      set((state) => ({ sessions: { ...state.sessions, [session.id]: session } }));
      actions.select(session.id);
    } catch (error) {
      report(error);
    }
  },

  async configure(sessionId: string, patch: { modelId?: string | null; approvalMode?: ApprovalMode }): Promise<void> {
    try {
      const { session } = await client.call({ type: "session.configure", sessionId, ...patch });
      set((state) => ({ sessions: { ...state.sessions, [session.id]: session } }));
    } catch (error) {
      report(error);
    }
  },

  async renameSession(sessionId: string, title: string): Promise<boolean> {
    try {
      const { session } = await client.call({ type: "session.rename", sessionId, title });
      set((state) => ({ sessions: { ...state.sessions, [session.id]: session } }));
      return true;
    } catch (error) {
      report(error);
      return false;
    }
  },

  async deleteSession(sessionId: string): Promise<boolean> {
    try {
      const wasSelected = get().selectedId === sessionId;
      await client.call({ type: "session.delete", sessionId });
      set((state) => {
        const sessions = { ...state.sessions };
        delete sessions[sessionId];
        const views = { ...state.views };
        delete views[sessionId];
        return { sessions, views, ...(wasSelected ? { selectedId: undefined } : {}) };
      });
      if (wasSelected) localStorage.removeItem(SELECTED_KEY);
      return true;
    } catch (error) {
      report(error);
      return false;
    }
  },

  retrySession(sessionId: string): void {
    void resume(sessionId);
  },

  async loadEarlier(sessionId: string): Promise<void> {
    const view = get().views[sessionId];
    if (!view || !view.hasEarlier || view.loadingEarlier || view.firstSeq <= 1) return;
    setView(sessionId, (current) => ({ ...current, loadingEarlier: true }));
    try {
      const { events, hasEarlier } = await client.call({
        type: "events.before",
        sessionId,
        beforeSeq: view.firstSeq,
        limit: 200,
      });
      const models = get().models;
      setView(sessionId, (current) => ({ ...prependEvents(current, events, models, hasEarlier), loadingEarlier: false }));
    } catch (error) {
      report(error);
      setView(sessionId, (current) => ({ ...current, loadingEarlier: false }));
    }
  },

  async send(sessionId: string, text: string, attachments: Attachment[] = []): Promise<boolean> {
    try {
      await client.call({ type: "message.send", sessionId, text, ...(attachments.length ? { attachments } : {}) });
      return true;
    } catch (error) {
      report(error);
      return false;
    }
  },

  async abort(sessionId: string): Promise<void> {
    try {
      await client.call({ type: "run.abort", sessionId });
    } catch (error) {
      report(error);
    }
  },

  async respond(sessionId: string, pendingId: string, answer: PendingAnswer): Promise<void> {
    try {
      await client.call({ type: "pending.respond", sessionId, pendingId, answer });
    } catch (error) {
      report(error);
    }
  },

  retryModels: loadModels,
};

export { emptyView };
