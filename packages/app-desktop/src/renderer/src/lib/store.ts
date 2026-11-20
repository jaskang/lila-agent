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
  WatchSlot,
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
export type DraftImage = ImageInput & { id: string; name: string };
type Draft = { text: string; images: DraftImage[] };
type DeliveryMode = "steer" | "followUp";

type State = {
  connection: ConnectionState;
  connectionDetail?: string;
  projects: Project[];
  models: ModelInfo[];
  defaultModelId?: string;
  modelsError?: string;
  modelsLoading: boolean;
  sessions: Record<string, SessionSummary>;
  /** Parent session id → child sub-agent sessions, sorted by newest first. */
  childrenByParent: Record<string, SessionSummary[]>;
  views: Record<string, SessionView>;
  selectedId?: string;
  previewId?: string;
  newSession: NewSessionOptions;
  /** Composer text per session; `new` is the new-session composer. */
  drafts: Record<string, string>;
  /** Pending image attachments per session; preserved across session switches. */
  draftImages: Record<string, DraftImage[]>;
  sendingDrafts: Record<string, true>;
  paletteOpen: boolean;
  resourcesOpen: boolean;
  gitOpen: boolean;
};

export const NEW_DRAFT = "new";
const SELECTED_KEY = "lila.selectedSession";
const NEW_SESSION_KEY = "lila.newSession";
const OPEN_LIMIT = 300;
const EARLIER_LIMIT = 200;
const MAX_CACHED_VIEWS = 10;
const MAX_SCROLL_POSITIONS = 50;

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

export const EMPTY_VIEW: SessionView = Object.freeze({
  items: [],
  index: {},
  hasEarlier: false,
  loadingEarlier: false,
  loading: false,
});

export const emptyView = (): SessionView => ({ items: [], index: {}, hasEarlier: false, loadingEarlier: false, loading: false });

function buildChildrenByParent(sessions: Record<string, SessionSummary>): Record<string, SessionSummary[]> {
  const children: Record<string, SessionSummary[]> = {};
  for (const session of Object.values(sessions)) {
    if (session.parentId) (children[session.parentId] ??= []).push(session);
  }
  for (const list of Object.values(children)) list.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
  return children;
}

function withSessions(sessions: Record<string, SessionSummary>): Pick<State, "sessions" | "childrenByParent"> {
  return { sessions, childrenByParent: buildChildrenByParent(sessions) };
}

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
  modelsLoading: false,
  sessions: {},
  childrenByParent: {},
  views: {},
  selectedId: localStorage.getItem(SELECTED_KEY) ?? undefined,
  newSession: readNewSession(),
  drafts: {},
  draftImages: {},
  sendingDrafts: {},
  paletteOpen: false,
  resourcesOpen: false,
  gitOpen: false,
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

/** Most-recently-opened sessions, used to cap in-memory transcript views. */
const recentViews: string[] = [];
/** Per-session scrollTop cache, kept in memory so revisiting a session restores its reading position. */
const scrollPositions = new Map<string, number>();

function recordViewAccess(sessionId: string): void {
  const existing = recentViews.indexOf(sessionId);
  if (existing >= 0) recentViews.splice(existing, 1);
  recentViews.push(sessionId);
}

function pruneViews(): void {
  if (recentViews.length <= MAX_CACHED_VIEWS) return;
  const { selectedId, previewId } = get();
  const keep = new Set([selectedId, previewId, watching.main, watching.preview]);
  const drop: string[] = [];
  for (let i = 0; i < recentViews.length && recentViews.length - drop.length > MAX_CACHED_VIEWS; i++) {
    const candidate = recentViews[i]!;
    if (!keep.has(candidate)) drop.push(candidate);
  }
  if (!drop.length) return;
  const dropped = new Set(drop);
  for (let i = recentViews.length - 1; i >= 0; i--) {
    if (dropped.has(recentViews[i]!)) recentViews.splice(i, 1);
  }
  set((state) => {
    const views = { ...state.views };
    for (const id of dropped) delete views[id];
    return { views };
  });
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

/** Independent watches for the conversation and the read-only preview. */
const watching: Partial<Record<WatchSlot, string>> = {};
const opening = new Map<string, number>();
let openCounter = 0;
let selectionVersion = 0;
let newDraftTarget = { key: NEW_DRAFT };

function unwatchSession(slot: WatchSlot): void {
  const id = watching[slot];
  if (!id) return;
  delete watching[slot];
  buffer = buffer.filter((frame) => frame.sessionId !== id);
  if (!buffer.length && frameHandle !== undefined) {
    cancelAnimationFrame(frameHandle);
    frameHandle = undefined;
  }
  if (get().connection === "connected") {
    void client.call({ type: "session.unwatch", slot }).catch(() => {});
  }
}

function closePreview(): void {
  set({ previewId: undefined });
  unwatchSession("preview");
}

async function openSession(sessionId: string, slot: WatchSlot = "main"): Promise<void> {
  const token = ++openCounter;
  opening.set(sessionId, token);
  watching[slot] = sessionId;
  recordViewAccess(sessionId);
  flush();
  setView(sessionId, (view) => ({ ...view, loading: true, error: undefined }));
  try {
    const current = get().views[sessionId];
    const limit = current?.state?.access === "read-only" ? Math.min(1_000, Math.max(OPEN_LIMIT, current.items.length)) : OPEN_LIMIT;
    const { items, hasEarlier, state } = await client.call({ type: "session.open", sessionId, limit, slot });
    if (opening.get(sessionId) !== token || watching[slot] !== sessionId) return;
    // Frames queued before the snapshot are already part of it.
    buffer = buffer.filter((frame) => frame.sessionId !== sessionId);
    setView(sessionId, () => ({ items, index: indexOf(items), hasEarlier, loadingEarlier: false, state, loading: false }));
    pruneViews();
  } catch (error) {
    if (opening.get(sessionId) !== token || watching[slot] !== sessionId) return;
    setView(sessionId, (view) => ({ ...view, loading: false, error: message(error) }));
  } finally {
    if (opening.get(sessionId) === token) opening.delete(sessionId);
  }
}

// ---------------------------------------------------------------------------
// System notifications while the window is in the background.

function notify(title: string, body: string, sessionId?: string): void {
  if (document.hasFocus() || typeof Notification === "undefined" || Notification.permission !== "granted") return;
  const notification = new Notification(title, { body });
  notification.onclick = () => {
    if (sessionId && get().sessions[sessionId]) actions.select(sessionId);
    window.lila.focus();
  };
}

function notifyTransition(previous: SessionSummary | undefined, next: SessionSummary): void {
  if (!previous) return;
  const title = next.title || "新会话";
  if (next.pendingCount > previous.pendingCount) notify("需要你处理", title, next.id);
  else if (previous.status === "running" && next.status === "idle") notify("任务完成", title, next.id);
  else if (previous.status !== "error" && next.status === "error") notify("任务出错", title, next.id);
}

function handlePush(frame: PushMessage): void {
  switch (frame.type) {
    case "item":
    case "delta":
    case "state":
      if (frame.sessionId !== watching.main && frame.sessionId !== watching.preview) return;
      buffer.push(frame);
      frameHandle ??= requestAnimationFrame(flush);
      return;
    case "reset":
      if (frame.sessionId === watching.main) void openSession(frame.sessionId);
      else if (frame.sessionId === watching.preview) void openSession(frame.sessionId, "preview");
      return;
    case "projects":
      set({ projects: frame.projects });
      return;
    case "session": {
      const previous = get().sessions[frame.session.id];
      set((state) => withSessions({ ...state.sessions, [frame.session.id]: frame.session }));
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
  if (wasSelected || get().previewId === sessionId) closePreview();
  const idx = recentViews.indexOf(sessionId);
  if (idx >= 0) recentViews.splice(idx, 1);
  scrollPositions.delete(sessionId);
  set((state) => {
    const sessions = { ...state.sessions };
    delete sessions[sessionId];
    const views = { ...state.views };
    delete views[sessionId];
    const drafts = { ...state.drafts };
    delete drafts[sessionId];
    const draftImages = { ...state.draftImages };
    delete draftImages[sessionId];
    const sendingDrafts = { ...state.sendingDrafts };
    delete sendingDrafts[sessionId];
    return {
      ...withSessions(sessions),
      views,
      drafts,
      draftImages,
      sendingDrafts,
      ...(wasSelected ? { selectedId: undefined, resourcesOpen: false, gitOpen: false } : {}),
    };
  });
  if (wasSelected) {
    localStorage.removeItem(SELECTED_KEY);
    unwatchSession("main");
  }
}

async function loadModels(): Promise<void> {
  if (get().modelsLoading) return;
  set({ modelsLoading: true });
  try {
    const { models, defaultModelId } = await client.call({ type: "models.list" });
    set({ models, defaultModelId, modelsError: undefined });
  } catch (error) {
    set({ modelsError: message(error) });
  } finally {
    set({ modelsLoading: false });
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
    set({ projects, ...withSessions(byId), ...(missing ? { selectedId: undefined } : {}) });
    if (missing) {
      localStorage.removeItem(SELECTED_KEY);
      unwatchSession("main");
    }
    const { selectedId: current, previewId } = get();
    const preview = previewId ? byId[previewId] : undefined;
    const canPreview = preview?.readOnly && current && preview.parentId === current;
    if (previewId && !canPreview) closePreview();
    await Promise.all([
      current ? openSession(current) : undefined,
      canPreview ? openSession(preview.id, "preview") : undefined,
    ]);
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

function toDraftImages(images: ImageInput[]): DraftImage[] {
  return images.map((image, index) => ({
    ...image,
    id: crypto.randomUUID(),
    name: `图片 ${index + 1}`,
  }));
}

const EMPTY_DRAFT: Draft = { text: "", images: [] };

function readDraft(key: string, state = get()): Draft {
  return { text: state.drafts[key] ?? "", images: state.draftImages[key] ?? [] };
}

function writeDraft(state: State, key: string, draft: Draft): Pick<State, "drafts" | "draftImages"> {
  const drafts = { ...state.drafts };
  const draftImages = { ...state.draftImages };
  if (draft.text) drafts[key] = draft.text;
  else delete drafts[key];
  if (draft.images.length) draftImages[key] = draft.images;
  else delete draftImages[key];
  return { drafts, draftImages };
}

/** Reserving a submission immediately leaves a fresh draft for anything typed while awaiting core. */
function beginDraftSubmission(key: string): Draft | undefined {
  const state = get();
  const draft = readDraft(key, state);
  if (state.sendingDrafts[key] || (!draft.text.trim() && !draft.images.length)) return;
  set({ ...writeDraft(state, key, EMPTY_DRAFT), sendingDrafts: { ...state.sendingDrafts, [key]: true } });
  return draft;
}

function finishDraftSubmission(key: string, unsent?: Draft): void {
  set((state) => {
    if (!state.sendingDrafts[key]) return state;
    const sendingDrafts = { ...state.sendingDrafts };
    delete sendingDrafts[key];
    if (!unsent) return { sendingDrafts };
    const next = readDraft(key, state);
    return {
      ...writeDraft(state, key, {
        text: [unsent.text, next.text].filter(Boolean).join("\n\n"),
        images: [...unsent.images, ...next.images],
      }),
      sendingDrafts,
    };
  });
}

async function sendDraftContents(sessionId: string, draft: Draft, mode?: DeliveryMode): Promise<void> {
  let accepted = false;
  try {
    accepted = await actions.prompt(sessionId, draft.text.trim(), draft.images.map(({ mimeType, data }) => ({ mimeType, data })), mode);
  } finally {
    finishDraftSubmission(sessionId, accepted ? undefined : draft);
  }
}

export const actions = {
  select(sessionId: string | undefined): void {
    selectionVersion++;
    if (get().selectedId !== sessionId) closePreview();
    set({ selectedId: sessionId, resourcesOpen: false, gitOpen: false });
    if (sessionId) localStorage.setItem(SELECTED_KEY, sessionId);
    else localStorage.removeItem(SELECTED_KEY);
    if (!sessionId) {
      unwatchSession("main");
      return;
    }
    if (get().connection === "connected") void openSession(sessionId);
    else delete watching.main;
  },

  previewSubsession(sessionId: string): void {
    const { selectedId, sessions, connection, previewId } = get();
    const child = sessions[sessionId];
    if (!selectedId || !child?.readOnly || child.parentId !== selectedId || sessionId === previewId) return;
    unwatchSession("preview");
    set({ previewId: sessionId });
    if (connection === "connected") void openSession(sessionId, "preview");
  },

  closePreview,

  getScrollTop(sessionId: string): number | undefined {
    return scrollPositions.get(sessionId);
  },

  setScrollTop(sessionId: string, scrollTop: number): void {
    if (!Number.isFinite(scrollTop) || scrollTop < 0) return;
    if (scrollPositions.has(sessionId)) scrollPositions.delete(sessionId);
    scrollPositions.set(sessionId, scrollTop);
    if (scrollPositions.size > MAX_SCROLL_POSITIONS) {
      const oldest = scrollPositions.keys().next().value;
      if (oldest !== undefined) scrollPositions.delete(oldest);
    }
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

  /** An attachment read can finish after the composer moved into the created session. */
  captureDraftTarget(key: string): () => string {
    const target = key === NEW_DRAFT ? newDraftTarget : { key };
    return () => target.key;
  },

  setDraft(key: string, text: string): void {
    set((state) => ({ drafts: { ...state.drafts, [key]: text } }));
  },

  setDraftImages(key: string, images: DraftImage[] | ((current: DraftImage[]) => DraftImage[])): void {
    set((state) => {
      const current = state.draftImages[key] ?? [];
      const next = typeof images === "function" ? images(current) : images;
      const draftImages = { ...state.draftImages };
      if (next.length) draftImages[key] = next;
      else delete draftImages[key];
      return { draftImages };
    });
  },

  setPaletteOpen(open: boolean): void {
    set({ paletteOpen: open });
  },

  setResourcesOpen(open: boolean): void {
    set({ resourcesOpen: open });
  },

  setGitOpen(open: boolean): void {
    set({ gitOpen: open });
  },

  retryModels: loadModels,

  retrySession(sessionId: string): void {
    if (get().previewId === sessionId) void openSession(sessionId, "preview");
    else if (get().selectedId === sessionId) void openSession(sessionId);
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

  async removeProject(projectId: string): Promise<boolean> {
    const result = await attempt(() => client.call({ type: "projects.remove", projectId }));
    if (!result) return false;
    const state = get();
    const removedSessionIds = new Set(
      Object.values(state.sessions)
        .filter((session) => session.projectId === projectId)
        .map((session) => session.id),
    );
    for (let i = recentViews.length - 1; i >= 0; i--) {
      if (removedSessionIds.has(recentViews[i]!)) recentViews.splice(i, 1);
    }
    for (const id of removedSessionIds) scrollPositions.delete(id);
    const wasSelected = state.selectedId !== undefined && removedSessionIds.has(state.selectedId);
    const remainingProjects = state.projects.filter((project) => project.id !== projectId);
    const nextNewSession =
      state.newSession.projectId === projectId ? { ...state.newSession, projectId: remainingProjects.at(-1)?.id } : state.newSession;
    set((current) => {
      const sessions = { ...current.sessions };
      const views = { ...current.views };
      const drafts = { ...current.drafts };
      const draftImages = { ...current.draftImages };
      const sendingDrafts = { ...current.sendingDrafts };
      for (const id of removedSessionIds) {
        delete sessions[id];
        delete views[id];
        delete drafts[id];
        delete draftImages[id];
        delete sendingDrafts[id];
      }
      return {
        projects: remainingProjects,
        ...withSessions(sessions),
        views,
        drafts,
        draftImages,
        sendingDrafts,
        newSession: nextNewSession,
        ...(wasSelected ? { selectedId: undefined, resourcesOpen: false, gitOpen: false } : {}),
      };
    });
    if (wasSelected) {
      localStorage.removeItem(SELECTED_KEY);
      closePreview();
      unwatchSession("main");
    }
    if (nextNewSession !== state.newSession) localStorage.setItem(NEW_SESSION_KEY, JSON.stringify(nextNewSession));
    return true;
  },

  /** Creates a session, then hands the next draft to it while the submitted one waits for acceptance. */
  async createAndPrompt(projectId: string, modelId: string, thinkingLevel?: ThinkingLevel): Promise<void> {
    const { approvalMode } = get().newSession;
    const submitted = beginDraftSubmission(NEW_DRAFT);
    if (!submitted) return;
    const version = selectionVersion;
    const created = await attempt(() =>
      client.call({ type: "session.create", projectId, modelId, approvalMode, ...(thinkingLevel ? { thinkingLevel } : {}) }),
    );
    if (!created) {
      finishDraftSubmission(NEW_DRAFT, submitted);
      void loadModels();
      return;
    }
    const { session } = created;
    const followSession = selectionVersion === version && get().selectedId === undefined;
    if (followSession) {
      newDraftTarget.key = session.id;
      newDraftTarget = { key: NEW_DRAFT };
    }
    set((state) => {
      const sendingDrafts = { ...state.sendingDrafts, [session.id]: true as const };
      delete sendingDrafts[NEW_DRAFT];
      const transferred = followSession
        ? writeDraft({ ...state, ...writeDraft(state, NEW_DRAFT, EMPTY_DRAFT) }, session.id, readDraft(NEW_DRAFT, state))
        : {};
      return {
        ...withSessions({ ...state.sessions, [session.id]: session }),
        ...transferred,
        sendingDrafts,
        ...(followSession ? { selectedId: session.id } : {}),
      };
    });
    if (followSession) {
      localStorage.setItem(SELECTED_KEY, session.id);
      if (get().connection === "connected") await openSession(session.id);
    }
    await sendDraftContents(session.id, submitted);
  },

  async sendDraft(sessionId: string, mode?: DeliveryMode): Promise<void> {
    const draft = beginDraftSubmission(sessionId);
    if (draft) await sendDraftContents(sessionId, draft, mode);
  },

  async continueSubsession(sessionId: string): Promise<void> {
    const version = selectionVersion;
    const result = await attempt(() => client.call({ type: "session.fork", sessionId }));
    if (!result) return;
    const { session } = result;
    set((state) => withSessions({ ...state.sessions, [session.id]: session }));
    if (selectionVersion === version) actions.select(session.id);
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

  async configure(sessionId: string, patch: { modelId?: string; thinkingLevel?: ThinkingLevel; approvalMode?: ApprovalMode }): Promise<boolean> {
    const result = await attempt(() => client.call({ type: "session.configure", sessionId, ...patch }));
    if (!result) return false;
    setView(sessionId, (view) => ({ ...view, state: result.state }));
    return true;
  },

  async rename(sessionId: string, title: string): Promise<boolean> {
    const result = await attempt(() => client.call({ type: "session.rename", sessionId, title }));
    if (!result) return false;
    set((state) => withSessions({ ...state.sessions, [result.session.id]: result.session }));
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

  /** Moves the session back to before a user message; its text and images go back into the composer. */
  async rewind(sessionId: string, itemId: string, images: ImageInput[] = []): Promise<boolean> {
    const result = await attempt(() => client.call({ type: "session.rewind", sessionId, itemId }));
    if (!result) return false;
    actions.setDraft(sessionId, result.text);
    actions.setDraftImages(sessionId, toDraftImages(images));
    return true;
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

  async gitStatus(projectId: string) {
    return (await client.call({ type: "git.status", projectId })).status;
  },

  async gitDiff(projectId: string, path: string, staged?: boolean) {
    return (await client.call({ type: "git.diff", projectId, path, ...(staged !== undefined ? { staged } : {}) })).diff;
  },

  async searchTranscript(sessionId: string, query: string, offset: number) {
    return client.call({ type: "session.search", sessionId, query, offset });
  },

  async transcriptAround(sessionId: string, itemId: string) {
    return (await client.call({ type: "session.around", sessionId, itemId })).items;
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
