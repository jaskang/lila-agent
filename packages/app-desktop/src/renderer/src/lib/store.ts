import { create } from "zustand";
import { toast } from "sonner";
import type {
  ApprovalMode,
  ImageInput,
  ModelInfo,
  PendingAnswer,
  Project,
  SessionState,
  SessionSummary,
  SlashCommand,
  ThinkingLevel,
  TranscriptItem,
} from "@lila/protocol";
import { CoreClient, type ConnectionState, type PushMessage } from "./client.ts";

export type SessionView = {
  items: TranscriptItem[];
  /** Item id → index in `items`. */
  index: Record<string, number>;
  hasEarlier: boolean;
  loadingEarlier: boolean;
  state?: SessionState;
  loading: boolean;
  error?: string;
};

/** Settings for a session that does not exist yet; remembered between new sessions. */
export type NewSessionOptions = { projectId?: string; modelId?: string; thinkingLevel?: ThinkingLevel; approvalMode: ApprovalMode };

type State = {
  connection: ConnectionState;
  connectionDetail?: string;
  projects: Project[];
  models: ModelInfo[];
  defaultModelId?: string;
  modelsError?: string;
  sessions: Record<string, SessionSummary>;
  views: Record<string, SessionView>;
  selectedId?: string;
  newSession: NewSessionOptions;
  /** Composer text per session; `new` is the new-session composer. */
  drafts: Record<string, string>;
  paletteOpen: boolean;
};

export const NEW_DRAFT = "new";
const SELECTED_KEY = "lila.selectedSession";
const NEW_SESSION_KEY = "lila.newSession";
const OPEN_LIMIT = 200;
const EARLIER_LIMIT = 200;

export const APPROVAL_LABEL: Record<ApprovalMode, string> = {
  auto: "自动执行",
  "ask-exec": "执行命令前询问",
  "ask-all": "修改和执行都询问",
};

export const APPROVAL_HINT: Record<ApprovalMode, string> = {
  auto: "所有工具直接执行，适合你信任的项目。",
  "ask-exec": "改文件不问，运行命令、MCP 和子 Agent 前先问你。",
  "ask-all": "改文件和运行命令前都先问你。",
};

export const THINKING_LABEL: Record<ThinkingLevel, string> = {
  off: "不思考",
  minimal: "最少思考",
  low: "少量思考",
  medium: "适中思考",
  high: "深入思考",
  xhigh: "很深入",
  max: "最深入",
};

export const emptyView = (): SessionView => ({ items: [], index: {}, hasEarlier: false, loadingEarlier: false, loading: false });

function readNewSession(): NewSessionOptions {
  try {
    const saved = JSON.parse(localStorage.getItem(NEW_SESSION_KEY) ?? "null") as Partial<NewSessionOptions> | null;
    if (saved && typeof saved === "object") return { ...saved, approvalMode: saved.approvalMode ?? "auto" };
  } catch {
    // A broken entry falls back to the defaults.
  }
  return { approvalMode: "auto" };
}

export const useLila = create<State>(() => ({
  connection: "connecting",
  projects: [],
  models: [],
  sessions: {},
  views: {},
  selectedId: localStorage.getItem(SELECTED_KEY) ?? undefined,
  newSession: readNewSession(),
  drafts: {},
  paletteOpen: false,
}));

const set = useLila.setState;
const get = useLila.getState;

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function report(error: unknown): void {
  toast.error(message(error));
}

function indexOf(items: TranscriptItem[]): Record<string, number> {
  const index: Record<string, number> = {};
  items.forEach((item, at) => {
    index[item.id] = at;
  });
  return index;
}

function setView(sessionId: string, update: (view: SessionView) => SessionView): void {
  set((state) => ({ views: { ...state.views, [sessionId]: update(state.views[sessionId] ?? emptyView()) } }));
}

// ---------------------------------------------------------------------------
// Live frames, merged per animation frame so a fast model cannot flood React with renders.

type SessionFrame = Extract<PushMessage, { type: "item" | "delta" | "state" }>;

function applyFrame(view: SessionView, frame: SessionFrame, items: TranscriptItem[], index: Record<string, number>): SessionView {
  if (frame.type === "state") return { ...view, state: frame.state };
  if (frame.type === "item") {
    const at = index[frame.item.id];
    if (at === undefined) {
      index[frame.item.id] = items.length;
      items.push(frame.item);
    } else {
      items[at] = frame.item;
    }
    return view;
  }
  const at = index[frame.itemId];
  const current = at === undefined ? undefined : items[at];
  if (current && current.kind !== "assistant") return view;
  const base = current ?? { kind: "assistant" as const, id: frame.itemId, text: "", thinking: "", streaming: true };
  const next = { ...base, [frame.channel]: base[frame.channel] + frame.delta };
  if (at === undefined) {
    index[frame.itemId] = items.length;
    items.push(next);
  } else {
    items[at] = next;
  }
  return view;
}

let buffer: SessionFrame[] = [];
let frameHandle: number | undefined;

function flush(): void {
  if (frameHandle !== undefined) cancelAnimationFrame(frameHandle);
  frameHandle = undefined;
  if (!buffer.length) return;
  const frames = buffer;
  buffer = [];
  set((state) => {
    const views = { ...state.views };
    const touched = new Map<string, { items: TranscriptItem[]; index: Record<string, number> }>();
    for (const frame of frames) {
      let view = views[frame.sessionId];
      if (!view) continue;
      let work = touched.get(frame.sessionId);
      if (!work) {
        work = { items: view.items.slice(), index: { ...view.index } };
        touched.set(frame.sessionId, work);
      }
      view = applyFrame(view, frame, work.items, work.index);
      views[frame.sessionId] = view;
    }
    for (const [sessionId, work] of touched) {
      const view = views[sessionId];
      if (view) views[sessionId] = { ...view, items: work.items, index: work.index };
    }
    return { views };
  });
}

/** The session this socket watches, and opens in flight (frames before the snapshot are already in it). */
let watching: string | undefined;
const opening = new Map<string, number>();
let openCounter = 0;

async function openSession(sessionId: string): Promise<void> {
  const token = ++openCounter;
  opening.set(sessionId, token);
  watching = sessionId;
  flush();
  setView(sessionId, (view) => ({ ...view, loading: true, error: undefined }));
  try {
    const { items, hasEarlier, state } = await client.call({ type: "session.open", sessionId, limit: OPEN_LIMIT });
    if (opening.get(sessionId) !== token) return;
    // Frames queued before the snapshot are already part of it.
    buffer = buffer.filter((frame) => frame.sessionId !== sessionId);
    setView(sessionId, () => ({ items, index: indexOf(items), hasEarlier, loadingEarlier: false, state, loading: false }));
  } catch (error) {
    if (opening.get(sessionId) !== token) return;
    setView(sessionId, (view) => ({ ...view, loading: false, error: message(error) }));
  } finally {
    if (opening.get(sessionId) === token) opening.delete(sessionId);
  }
}

// ---------------------------------------------------------------------------
// System notifications while the window is in the background.

function notify(title: string, body: string): void {
  if (document.hasFocus() || typeof Notification === "undefined" || Notification.permission !== "granted") return;
  const notification = new Notification(title, { body });
  notification.onclick = () => window.lila.focus();
}

function notifyTransition(previous: SessionSummary | undefined, next: SessionSummary): void {
  if (!previous) return;
  const title = next.title || "新会话";
  if (next.pendingCount > previous.pendingCount) notify("需要你处理", title);
  else if (previous.status === "running" && next.status === "idle") notify("任务完成", title);
  else if (previous.status !== "error" && next.status === "error") notify("任务出错", title);
}

function handlePush(frame: PushMessage): void {
  switch (frame.type) {
    case "item":
    case "delta":
    case "state":
      if (frame.sessionId !== watching) return;
      buffer.push(frame);
      frameHandle ??= requestAnimationFrame(flush);
      return;
    case "reset":
      if (frame.sessionId === watching) void openSession(frame.sessionId);
      return;
    case "session": {
      const previous = get().sessions[frame.session.id];
      set((state) => ({ sessions: { ...state.sessions, [frame.session.id]: frame.session } }));
      notifyTransition(previous, frame.session);
      return;
    }
    case "session.removed":
      forget(frame.sessionId);
      return;
    case "notify": {
      const show = frame.level === "error" ? toast.error : frame.level === "warning" ? toast.warning : toast.info;
      const title = frame.sessionId ? get().sessions[frame.sessionId]?.title : undefined;
      show(frame.message, title ? { description: title } : undefined);
    }
  }
}

function forget(sessionId: string): void {
  const wasSelected = get().selectedId === sessionId;
  set((state) => {
    const sessions = { ...state.sessions };
    delete sessions[sessionId];
    const views = { ...state.views };
    delete views[sessionId];
    return { sessions, views, ...(wasSelected ? { selectedId: undefined } : {}) };
  });
  if (wasSelected) {
    localStorage.removeItem(SELECTED_KEY);
    watching = undefined;
  }
}

async function loadModels(): Promise<void> {
  try {
    const { models, defaultModelId } = await client.call({ type: "models.list" });
    set({ models, defaultModelId, modelsError: undefined });
  } catch (error) {
    set({ modelsError: message(error) });
  }
}

async function onReady(): Promise<void> {
  void loadModels();
  try {
    const [{ projects }, { sessions }] = await Promise.all([
      client.call({ type: "projects.list" }),
      client.call({ type: "sessions.list" }),
    ]);
    const byId = Object.fromEntries(sessions.map((session) => [session.id, session]));
    const selectedId = get().selectedId;
    const missing = selectedId !== undefined && !byId[selectedId];
    set({ projects, sessions: byId, ...(missing ? { selectedId: undefined } : {}) });
    if (missing) localStorage.removeItem(SELECTED_KEY);
    const current = get().selectedId;
    if (current) await openSession(current);
  } catch (error) {
    report(error);
  }
}

export const client = new CoreClient({
  onState: (connection, connectionDetail) => set({ connection, connectionDetail }),
  onReady: () => void onReady(),
  onPush: handlePush,
});

// ---------------------------------------------------------------------------
// Actions.

async function attempt<T>(run: () => Promise<T>): Promise<T | undefined> {
  try {
    return await run();
  } catch (error) {
    report(error);
    return undefined;
  }
}

export const actions = {
  select(sessionId: string | undefined): void {
    set({ selectedId: sessionId });
    if (sessionId) localStorage.setItem(SELECTED_KEY, sessionId);
    else localStorage.removeItem(SELECTED_KEY);
    if (sessionId && get().connection === "connected") void openSession(sessionId);
  },

  /** Shows the new-session composer, optionally for one project. */
  startNew(projectId?: string): void {
    if (projectId) actions.setNewSession({ projectId });
    actions.select(undefined);
  },

  setNewSession(patch: Partial<NewSessionOptions>): void {
    const next = { ...get().newSession, ...patch };
    set({ newSession: next });
    localStorage.setItem(NEW_SESSION_KEY, JSON.stringify(next));
  },

  setDraft(key: string, text: string): void {
    set((state) => ({ drafts: { ...state.drafts, [key]: text } }));
  },

  setPaletteOpen(open: boolean): void {
    set({ paletteOpen: open });
  },

  retryModels: loadModels,

  retrySession(sessionId: string): void {
    void openSession(sessionId);
  },

  async addProject(): Promise<Project | undefined> {
    const path = await window.lila.pickDirectory();
    if (!path) return undefined;
    const result = await attempt(() => client.call({ type: "projects.add", path }));
    if (!result) return undefined;
    const { project } = result;
    set((state) => ({
      projects: state.projects.some((item) => item.id === project.id) ? state.projects : [...state.projects, project],
    }));
    return project;
  },

  /** Creates a session from the new-session composer and sends its first message. */
  async createAndPrompt(text: string, images: ImageInput[]): Promise<boolean> {
    const { projectId, modelId, thinkingLevel, approvalMode } = get().newSession;
    if (!projectId) return false;
    const created = await attempt(() =>
      client.call({
        type: "session.create",
        projectId,
        approvalMode,
        ...(modelId ? { modelId } : {}),
        ...(thinkingLevel ? { thinkingLevel } : {}),
      }),
    );
    if (!created) return false;
    const { session } = created;
    set((state) => ({ sessions: { ...state.sessions, [session.id]: session } }));
    actions.select(session.id);
    return actions.prompt(session.id, text, images);
  },

  async prompt(sessionId: string, text: string, images: ImageInput[] = [], mode?: "steer" | "followUp"): Promise<boolean> {
    const trimmed = text.trim();
    if (trimmed === "/compact") return Boolean(await actions.compact(sessionId));
    if (trimmed === "/reload") return Boolean(await actions.reload(sessionId));
    const result = await attempt(() =>
      client.call({ type: "session.prompt", sessionId, text, ...(images.length ? { images } : {}), ...(mode ? { mode } : {}) }),
    );
    return result !== undefined;
  },

  async abort(sessionId: string): Promise<void> {
    await attempt(() => client.call({ type: "session.abort", sessionId }));
  },

  async configure(sessionId: string, patch: { modelId?: string; thinkingLevel?: ThinkingLevel; approvalMode?: ApprovalMode }): Promise<void> {
    const result = await attempt(() => client.call({ type: "session.configure", sessionId, ...patch }));
    if (result) setView(sessionId, (view) => ({ ...view, state: result.state }));
  },

  async rename(sessionId: string, title: string): Promise<boolean> {
    const result = await attempt(() => client.call({ type: "session.rename", sessionId, title }));
    if (!result) return false;
    set((state) => ({ sessions: { ...state.sessions, [result.session.id]: result.session } }));
    return true;
  },

  async remove(sessionId: string): Promise<boolean> {
    const result = await attempt(() => client.call({ type: "session.delete", sessionId }));
    if (!result) return false;
    forget(sessionId);
    return true;
  },

  async compact(sessionId: string): Promise<boolean> {
    return (await attempt(() => client.call({ type: "session.compact", sessionId }))) !== undefined;
  },

  async reload(sessionId: string): Promise<boolean> {
    const done = (await attempt(() => client.call({ type: "session.reload", sessionId }))) !== undefined;
    if (done) toast.success("已重新加载技能、扩展和 MCP");
    return done;
  },

  /** Moves the session back to before a user message; its text goes back into the composer. */
  async rewind(sessionId: string, itemId: string): Promise<void> {
    const result = await attempt(() => client.call({ type: "session.rewind", sessionId, itemId }));
    if (result) actions.setDraft(sessionId, result.text);
  },

  async respond(sessionId: string, pendingId: string, answer: PendingAnswer): Promise<void> {
    await attempt(() => client.call({ type: "session.respond", sessionId, pendingId, answer }));
  },

  async commands(projectId: string, sessionId?: string): Promise<SlashCommand[]> {
    try {
      return (await client.call({ type: "commands.list", projectId, ...(sessionId ? { sessionId } : {}) })).commands;
    } catch {
      return [];
    }
  },

  async resources(sessionId: string) {
    return (await client.call({ type: "session.resources", sessionId })).resources;
  },

  async searchFiles(projectId: string, query: string): Promise<string[]> {
    try {
      return (await client.call({ type: "files.search", projectId, query, limit: 50 })).files;
    } catch {
      return [];
    }
  },

  async loadEarlier(sessionId: string): Promise<void> {
    const view = get().views[sessionId];
    const first = view?.items[0];
    if (!view || !first || !view.hasEarlier || view.loadingEarlier) return;
    setView(sessionId, (current) => ({ ...current, loadingEarlier: true }));
    try {
      const { items, hasEarlier } = await client.call({ type: "session.earlier", sessionId, beforeId: first.id, limit: EARLIER_LIMIT });
      setView(sessionId, (current) => {
        const merged = [...items.filter((item) => current.index[item.id] === undefined), ...current.items];
        return { ...current, items: merged, index: indexOf(merged), hasEarlier, loadingEarlier: false };
      });
    } catch (error) {
      report(error);
      setView(sessionId, (current) => ({ ...current, loadingEarlier: false }));
    }
  },
};
