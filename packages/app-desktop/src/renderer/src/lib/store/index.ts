import { create } from "zustand";
import { toast } from "sonner";
import type {
  ApprovalMode,
  AssistantTask,
  ImageInput,
  PendingAnswer,
  Project,
  ScheduleCreateInput,
  ScheduledJob,
  ScheduleUpdatePatch,
  SessionSummary,
  SlashCommand,
  TaskCreateInput,
  TaskUpdatePatch,
  ThinkingLevel,
  WatchSlot,
} from "@lila/protocol";
import { CoreClient, type PushMessage } from "../client.ts";
import type { FileChange } from "../transcript-layout.ts";
import { readDraftImages, saveDraftImages } from "./draft-images.ts";
import { createSettingsActions } from "./settings-actions.ts";
import {
  attempt,
  message,
  persistDrafts,
  readNewSession,
  readNotificationsEnabled,
  readSavedDrafts,
  report,
  safeLocalGet,
  safeLocalRemove,
  safeLocalSet,
  toDraftImages,
  withSessions,
} from "./storage.ts";
import { applyFrameBatch, createViewCache, indexOf, type SessionFrame } from "./transcript.ts";
import {
  type AutomationTab,
  DEFAULT_APPROVAL_KEY,
  type DeliveryMode,
  type Draft,
  type DraftImage,
  EARLIER_LIMIT,
  EMPTY_DRAFT,
  emptyView,
  type FileHistoryEntry,
  NEW_DRAFT,
  NEW_SESSION_KEY,
  type NewSessionOptions,
  OPEN_LIMIT,
  SELECTED_KEY,
  type SessionView,
  type State,
} from "./types.ts";

export {
  APPROVAL_LABEL,
  EMPTY_VIEW,
  NEW_DRAFT,
  emptyView,
  type AuthLoginFlowState,
  type AutomationTab,
  type DraftImage,
  type FileHistoryEntry,
  type NewSessionOptions,
  type ReplyReviewState,
  type RightPanelKind,
  type SessionView,
  type SettingsTab,
} from "./types.ts";

const savedDraftImages = await readDraftImages();

export const useLila = create<State>(() => ({
  connection: "connecting",
  projects: [],
  models: [],
  modelsLoading: false,
  sessions: {},
  childrenByParent: {},
  views: {},
  selectedId: safeLocalGet(SELECTED_KEY) ?? undefined,
  ...readNewSession(),
  drafts: readSavedDrafts(),
  draftImages: savedDraftImages,
  sendingDrafts: {},
  paletteOpen: false,
  automationOpen: false,
  automationTab: "tasks",
  automationCreateToken: 0,
  settingsOpen: false,
  settingsTab: "general",
  confirmSettingsDiscard: false,
  settingsDirty: { mcp: false, memory: false, capabilities: false },
  notificationsEnabled: readNotificationsEnabled(),
  authLogin: undefined,
  rightPanel: undefined,
  activeFile: undefined,
  activeChangePath: undefined,
  gitChangeCounts: {},
  unreadCompletedIds: {},
  schedules: [],
  tasks: [],
  workspaceRefreshSeq: 0,
  commandsRefreshSeq: 0,
}));

const set = useLila.setState;
const get = useLila.getState;
const viewCache = createViewCache();

function setView(sessionId: string, update: (view: SessionView) => SessionView): void {
  set((state) => ({ views: { ...state.views, [sessionId]: update(state.views[sessionId] ?? emptyView()) } }));
}

let buffer: SessionFrame[] = [];
let frameHandle: number | undefined;
const watching: Partial<Record<WatchSlot, string>> = {};
const opening = new Map<string, number>();
let openCounter = 0;
let selectionVersion = 0;
let newDraftTarget = { key: NEW_DRAFT };

function flush(): void {
  if (frameHandle !== undefined) cancelAnimationFrame(frameHandle);
  frameHandle = undefined;
  if (!buffer.length) return;
  const frames = buffer;
  buffer = [];
  set((state) => ({ views: applyFrameBatch(state.views, frames) }));
}

function pruneViews(): void {
  const { selectedId, previewId } = get();
  const drop = viewCache.collectPrunable(new Set([selectedId, previewId, watching.main, watching.preview]));
  if (!drop.length) return;
  set((state) => {
    const views = { ...state.views };
    for (const id of drop) delete views[id];
    return { views };
  });
}

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
  set((state) => ({
    previewId: undefined,
    ...(state.rightPanel === "subsession" ? { rightPanel: undefined } : {}),
  }));
  unwatchSession("preview");
}

async function openSession(sessionId: string, slot: WatchSlot = "main"): Promise<void> {
  const token = ++openCounter;
  opening.set(sessionId, token);
  watching[slot] = sessionId;
  viewCache.recordAccess(sessionId);
  flush();
  setView(sessionId, (view) => ({ ...view, loading: true, error: undefined }));
  try {
    const current = get().views[sessionId];
    const limit = current?.state?.access === "read-only" ? Math.min(1_000, Math.max(OPEN_LIMIT, current.items.length)) : OPEN_LIMIT;
    const { items, hasEarlier, state } = await client.call({ type: "session.open", sessionId, limit, slot });
    if (opening.get(sessionId) !== token || watching[slot] !== sessionId) return;
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

function notify(title: string, body: string, sessionId?: string): void {
  if (!get().notificationsEnabled || document.hasFocus() || typeof Notification === "undefined" || Notification.permission !== "granted") return;
  const notification = new Notification(title, { body });
  notification.onclick = () => {
    window.lila.focus();
    actions.closeSettingsThen(() => {
      actions.setPaletteOpen(false);
      const target = sessionId ? get().sessions[sessionId] : undefined;
      if (!target) return;
      if (target.readOnly && target.parentId && get().sessions[target.parentId]) {
        actions.select(target.parentId);
        actions.previewSubsession(target.id);
      } else if (!target.readOnly) {
        actions.select(target.id);
      }
    });
  };
}

function notifyTransition(previous: SessionSummary | undefined, next: SessionSummary): void {
  if (!previous || next.parentId || next.readOnly) return;
  const title = next.title || "新会话";
  if (next.pendingCount > previous.pendingCount) notify("需要你处理", title, next.id);
  else if (previous.status === "running" && next.status === "idle") notify("任务完成", title, next.id);
  else if (previous.status !== "error" && next.status === "error") notify("任务出错", title, next.id);
}

function forget(sessionId: string): void {
  const wasSelected = get().selectedId === sessionId;
  if (wasSelected || get().previewId === sessionId) closePreview();
  viewCache.remove(sessionId);
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
    const unreadCompletedIds = { ...state.unreadCompletedIds };
    delete unreadCompletedIds[sessionId];
    return {
      ...withSessions(sessions),
      views,
      drafts,
      draftImages,
      sendingDrafts,
      unreadCompletedIds,
      ...(wasSelected ? { selectedId: undefined, rightPanel: undefined, activeFile: undefined } : {}),
    };
  });
  if (wasSelected) {
    safeLocalRemove(SELECTED_KEY);
    unwatchSession("main");
  }
  persistComposerDrafts();
}

let queuedModelsLoad: { refresh: boolean } | undefined;

async function loadModels(refresh = false): Promise<void> {
  if (get().modelsLoading) {
    queuedModelsLoad = { refresh: Boolean(queuedModelsLoad?.refresh || refresh) };
    return;
  }
  set({ modelsLoading: true });
  try {
    const { models, defaultModelId } = await client.call({ type: "models.list", ...(refresh ? { refresh: true } : {}) });
    set({ models, defaultModelId, modelsError: undefined });
  } catch (error) {
    set({ modelsError: message(error) });
  } finally {
    set({ modelsLoading: false });
    if (queuedModelsLoad) {
      const next = queuedModelsLoad;
      queuedModelsLoad = undefined;
      void loadModels(next.refresh);
    }
  }
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
    case "schedules":
      set({ schedules: frame.schedules });
      return;
    case "tasks":
      set({ tasks: frame.tasks });
      return;
    case "session": {
      const previous = get().sessions[frame.session.id];
      const finishedInBackground =
        !frame.session.parentId &&
        !frame.session.readOnly &&
        previous?.status === "running" &&
        frame.session.status === "idle" &&
        get().selectedId !== frame.session.id;
      set((state) => ({
        ...withSessions({ ...state.sessions, [frame.session.id]: frame.session }),
        ...(finishedInBackground
          ? { unreadCompletedIds: { ...state.unreadCompletedIds, [frame.session.id]: true as const } }
          : {}),
      }));
      notifyTransition(previous, frame.session);
      return;
    }
    case "session.removed":
      forget(frame.sessionId);
      return;
    case "auth.login": {
      const current = get().authLogin;
      if (!current || current.loginId !== frame.loginId) return;
      const event = frame.event;
      switch (event.type) {
        case "auth_url":
          set({ authLogin: { ...current, authUrl: { url: event.url, ...(event.instructions ? { instructions: event.instructions } : {}) }, deviceCode: undefined } });
          return;
        case "device_code":
          set({ authLogin: { ...current, deviceCode: { userCode: event.userCode, verificationUri: event.verificationUri }, authUrl: undefined } });
          return;
        case "info":
          set({ authLogin: { ...current, info: { message: event.message, ...(event.links ? { links: event.links } : {}) } } });
          return;
        case "progress":
          set({ authLogin: { ...current, progress: event.message } });
          return;
        case "prompt":
          set({ authLogin: { ...current, prompt: event.prompt } });
          return;
        case "prompt_cleared":
          if (current.prompt?.promptId === event.promptId) set({ authLogin: { ...current, prompt: undefined } });
          return;
        case "done":
          set({ authLogin: { ...current, status: "done", prompt: undefined, progress: undefined, error: undefined } });
          void loadModels();
          return;
        case "error":
          set({ authLogin: { ...current, status: "error", prompt: undefined, progress: undefined, error: event.message } });
          return;
      }
      return;
    }
    case "notify": {
      const show = frame.level === "error" ? toast.error : frame.level === "warning" ? toast.warning : toast.info;
      const title = frame.sessionId ? get().sessions[frame.sessionId]?.title : undefined;
      show(frame.message, title ? { description: title } : undefined);
    }
  }
}

async function onReady(): Promise<void> {
  void loadModels();
  try {
    const [{ projects }, { sessions }, { schedules }, { tasks }] = await Promise.all([
      client.call({ type: "projects.list" }),
      client.call({ type: "sessions.list" }),
      client.call({ type: "schedules.list" }),
      client.call({ type: "tasks.list" }),
    ]);
    const byId = Object.fromEntries(sessions.map((session) => [session.id, session]));
    const selectedId = get().selectedId;
    const missing = selectedId !== undefined && !byId[selectedId];
    set({ projects, schedules, tasks, ...withSessions(byId), ...(missing ? { selectedId: undefined } : {}) });
    if (missing) {
      safeLocalRemove(SELECTED_KEY);
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

function mergeDrafts(first: Draft, next: Draft): Draft {
  return {
    text: [first.text, next.text].filter(Boolean).join("\n\n"),
    images: [...first.images, ...next.images],
  };
}

let lastSavedImages = savedDraftImages;
let lastSavedSubmissions = get().sendingDrafts;

function persistComposerDrafts(): void {
  const { drafts, draftImages, sendingDrafts } = get();
  const savedText = { ...drafts };
  for (const [key, draft] of Object.entries(sendingDrafts)) {
    savedText[key] = [draft.text, drafts[key]].filter(Boolean).join("\n\n");
  }
  persistDrafts(savedText);

  if (lastSavedImages === draftImages && lastSavedSubmissions === sendingDrafts) return;
  lastSavedImages = draftImages;
  lastSavedSubmissions = sendingDrafts;
  const savedImages = { ...draftImages };
  for (const [key, draft] of Object.entries(sendingDrafts)) {
    if (draft.images.length) savedImages[key] = [...draft.images, ...(draftImages[key] ?? [])];
  }
  void saveDraftImages(savedImages);
}

function beginDraftSubmission(key: string): Draft | undefined {
  const state = get();
  const draft = readDraft(key, state);
  if (state.sendingDrafts[key] || (!draft.text.trim() && !draft.images.length)) return;
  set({ ...writeDraft(state, key, EMPTY_DRAFT), sendingDrafts: { ...state.sendingDrafts, [key]: draft } });
  persistComposerDrafts();
  return draft;
}

function finishDraftSubmission(key: string, accepted: boolean): void {
  set((state) => {
    const submitted = state.sendingDrafts[key];
    if (!submitted) return state;
    const sendingDrafts = { ...state.sendingDrafts };
    delete sendingDrafts[key];
    if (accepted) return { sendingDrafts };
    return {
      ...writeDraft(state, key, mergeDrafts(submitted, readDraft(key, state))),
      sendingDrafts,
    };
  });
  persistComposerDrafts();
}

async function sendDraftContents(sessionId: string, draft: Draft, mode?: DeliveryMode): Promise<void> {
  let accepted = false;
  try {
    accepted = await actions.prompt(
      sessionId,
      draft.text.trim(),
      draft.images.map(({ mimeType, data }) => ({ mimeType, data })),
      mode,
    );
  } finally {
    finishDraftSubmission(sessionId, accepted);
  }
}

function setNewSession(patch: Partial<NewSessionOptions>): void {
  const next = { ...get().newSession, ...patch };
  set({ newSession: next });
  safeLocalSet(NEW_SESSION_KEY, JSON.stringify(next));
}

const settingsActions = createSettingsActions({ client, get, set, loadModels, setNewSession });

export const actions = {
  ...settingsActions,

  select(sessionId: string | undefined): void {
    selectionVersion++;
    const prev = get();
    const prevProjectId = prev.selectedId ? prev.sessions[prev.selectedId]?.projectId : undefined;
    const nextProjectId = sessionId ? prev.sessions[sessionId]?.projectId : undefined;
    const sameProject = Boolean(prevProjectId && prevProjectId === nextProjectId);
    if (prev.selectedId !== sessionId) closePreview();
    set((state) => {
      const unreadCompletedIds = sessionId && state.unreadCompletedIds[sessionId]
        ? (() => {
            const next = { ...state.unreadCompletedIds };
            delete next[sessionId];
            return next;
          })()
        : state.unreadCompletedIds;
      return {
        selectedId: sessionId,
        unreadCompletedIds,
        ...(prev.selectedId !== sessionId ? { replyReview: undefined, ...(prev.rightPanel === "review" ? { rightPanel: undefined } : {}) } : {}),
        ...(!sessionId || !sameProject ? { rightPanel: undefined, activeFile: undefined, replyReview: undefined } : {}),
      };
    });
    if (sessionId) safeLocalSet(SELECTED_KEY, sessionId);
    else safeLocalRemove(SELECTED_KEY);
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
    set({ previewId: sessionId, rightPanel: "subsession" });
    if (connection === "connected") void openSession(sessionId, "preview");
  },

  closePreview,

  toggleRightPanel(kind: "files" | "changes"): void {
    if (get().rightPanel === kind) {
      actions.closeRightPanel();
      return;
    }
    closePreview();
    set({ rightPanel: kind });
  },

  openRightPanel(kind: "files" | "changes"): void {
    closePreview();
    set({ rightPanel: kind, returnToPreviewId: undefined });
  },

  openChangesForFile(path: string): void {
    const prevPreview = get().rightPanel === "subsession" ? get().previewId : get().returnToPreviewId;
    closePreview();
    set({ rightPanel: "changes", activeChangePath: path, returnToPreviewId: prevPreview });
  },

  setActiveChangePath(path: string | undefined): void {
    set({ activeChangePath: path });
  },

  openFilePreview(path: string, range?: { line?: number | undefined; endLine?: number | undefined }): void {
    const { activeFile, rightPanel, previewId, returnToPreviewId } = get();
    const prevEntry: FileHistoryEntry | undefined =
      activeFile && activeFile.path !== path
        ? {
            path: activeFile.path,
            ...(activeFile.line !== undefined ? { line: activeFile.line } : {}),
            ...(activeFile.endLine !== undefined ? { endLine: activeFile.endLine } : {}),
            ...(activeFile.scrollTop !== undefined ? { scrollTop: activeFile.scrollTop } : {}),
          }
        : undefined;
    const history = prevEntry ? [...activeFile!.history.slice(-19), prevEntry] : (activeFile?.history ?? []);
    const nextReturnPreview = rightPanel === "subsession" ? previewId : returnToPreviewId;
    closePreview();
    set({
      rightPanel: "files",
      returnToPreviewId: nextReturnPreview,
      activeFile: {
        path,
        ...(range?.line !== undefined ? { line: range.line } : {}),
        ...(range?.endLine !== undefined ? { endLine: range.endLine } : {}),
        history,
      },
    });
  },

  setActiveFileScrollTop(scrollTop: number): void {
    const { activeFile } = get();
    if (!activeFile) return;
    set({ activeFile: { ...activeFile, scrollTop } });
  },

  backFilePreview(): void {
    const { activeFile } = get();
    if (!activeFile?.history.length) return;
    const previous = activeFile.history[activeFile.history.length - 1]!;
    set({
      activeFile: {
        path: previous.path,
        ...(previous.line !== undefined ? { line: previous.line } : {}),
        ...(previous.endLine !== undefined ? { endLine: previous.endLine } : {}),
        ...(previous.scrollTop !== undefined ? { scrollTop: previous.scrollTop } : {}),
        history: activeFile.history.slice(0, -1),
      },
    });
  },

  openReplyReview(sessionId: string, changes: FileChange[], path: string): void {
    const { rightPanel, previewId, returnToPreviewId } = get();
    const nextReturnPreview = rightPanel === "subsession" ? previewId : returnToPreviewId;
    closePreview();
    set({ rightPanel: "review", replyReview: { sessionId, changes, path }, returnToPreviewId: nextReturnPreview });
  },

  returnToSubsession(): void {
    const target = get().returnToPreviewId;
    if (!target || !get().sessions[target]) {
      set({ returnToPreviewId: undefined });
      return;
    }
    set({ returnToPreviewId: undefined });
    actions.previewSubsession(target);
  },

  selectReplyReviewPath(path: string): void {
    const { replyReview } = get();
    if (!replyReview) return;
    set({ replyReview: { ...replyReview, path } });
  },

  closeRightPanel(): void {
    if (get().previewId) closePreview();
    else set({ rightPanel: undefined, replyReview: undefined, returnToPreviewId: undefined });
  },

  getScrollTop: (sessionId: string) => viewCache.getScrollTop(sessionId),
  setScrollTop: (sessionId: string, scrollTop: number | undefined) => viewCache.setScrollTop(sessionId, scrollTop),

  startNew(projectId?: string): void {
    set({ newSessionApprovalMode: undefined });
    if (projectId) setNewSession({ projectId });
    actions.select(undefined);
  },

  setNewSession,

  setNewSessionApprovalMode(approvalMode: ApprovalMode): void {
    set({ newSessionApprovalMode: approvalMode });
  },

  setDefaultApprovalMode(approvalMode: ApprovalMode): void {
    set({ defaultApprovalMode: approvalMode });
    safeLocalSet(DEFAULT_APPROVAL_KEY, approvalMode);
  },

  captureDraftTarget(key: string): () => string {
    const target = key === NEW_DRAFT ? newDraftTarget : { key };
    return () => target.key;
  },

  setDraft(key: string, text: string): void {
    const drafts = { ...get().drafts, [key]: text };
    set({ drafts });
    persistComposerDrafts();
  },

  mentionFileInDraft(path: string): void {
    const state = get();
    const key = state.selectedId ?? NEW_DRAFT;
    const current = state.drafts[key] ?? "";
    const spacer = current && !/\s$/.test(current) ? " " : "";
    const drafts = { ...state.drafts, [key]: `${current}${spacer}@${path} ` };
    set({ drafts });
    persistComposerDrafts();
  },

  appendToDraft(snippet: string): void {
    const state = get();
    const key = state.selectedId ?? NEW_DRAFT;
    const current = state.drafts[key] ?? "";
    const next = current.trimEnd() ? `${current.trimEnd()}\n\n${snippet}` : snippet;
    const drafts = { ...state.drafts, [key]: next };
    set({ drafts });
    persistComposerDrafts();
  },

  clearUnreadCompleted(sessionId: string): void {
    if (!get().unreadCompletedIds[sessionId]) return;
    set((state) => {
      const unreadCompletedIds = { ...state.unreadCompletedIds };
      delete unreadCompletedIds[sessionId];
      return { unreadCompletedIds };
    });
  },

  bumpWorkspaceRefresh(): void {
    set((state) => ({ workspaceRefreshSeq: state.workspaceRefreshSeq + 1 }));
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
    persistComposerDrafts();
  },

  setPaletteOpen(open: boolean): void {
    if (open && get().settingsOpen) settingsActions.closeSettingsThen(() => set({ paletteOpen: true }));
    else set({ paletteOpen: open });
  },

  openAutomation(tab?: AutomationTab, createNew = false): void {
    const openNow = () =>
      set((state) => ({
        automationOpen: true,
        automationTab: tab ?? state.automationTab,
        paletteOpen: false,
        ...(createNew ? { automationCreateToken: state.automationCreateToken + 1 } : {}),
      }));
    if (get().settingsOpen) settingsActions.closeSettingsThen(openNow);
    else openNow();
  },

  setAutomationOpen(open: boolean): void {
    set({ automationOpen: open });
  },

  setAutomationTab(tab: AutomationTab): void {
    set({ automationTab: tab });
  },

  async createSchedule(input: ScheduleCreateInput): Promise<ScheduledJob | undefined> {
    const result = await attempt(() => client.call({ type: "schedules.create", input }));
    if (result) toast.success(`已创建定时任务「${result.schedule.name}」`);
    return result?.schedule;
  },

  async updateSchedule(id: string, patch: ScheduleUpdatePatch): Promise<ScheduledJob | undefined> {
    const result = await attempt(() => client.call({ type: "schedules.update", id, patch }));
    return result?.schedule;
  },

  async deleteSchedule(id: string): Promise<boolean> {
    const ok = await attempt(() => client.call({ type: "schedules.delete", id }));
    if (ok) toast.success("已删除定时任务");
    return Boolean(ok);
  },

  async runSchedule(id: string): Promise<{ schedule: ScheduledJob; sessionId?: string } | undefined> {
    const result = await attempt(() => client.call({ type: "schedules.run", id }));
    if (result) toast.success(`已触发「${result.schedule.name}」`);
    return result;
  },

  async createTask(input: TaskCreateInput): Promise<AssistantTask | undefined> {
    const result = await attempt(() => client.call({ type: "tasks.create", input }));
    if (result) toast.success(`已记录任务「${result.task.title}」`);
    return result?.task;
  },

  async updateTask(id: string, patch: TaskUpdatePatch): Promise<AssistantTask | undefined> {
    const result = await attempt(() => client.call({ type: "tasks.update", id, patch }));
    return result?.task;
  },

  async deleteTask(id: string): Promise<boolean> {
    const ok = await attempt(() => client.call({ type: "tasks.delete", id }));
    if (ok) toast.success("已删除任务");
    return Boolean(ok);
  },

  async startTask(options: {
    id: string;
    projectId?: string;
    useWorktree?: boolean;
    branch?: string;
    modelId?: string;
    thinkingLevel?: ThinkingLevel;
    approvalMode?: ApprovalMode;
  }): Promise<boolean> {
    const result = await attempt(() => client.call({ type: "tasks.start", ...options }));
    if (!result) return false;
    set((state) => ({
      automationOpen: false,
      ...(result.project && !state.projects.some((item) => item.id === result.project!.id)
        ? { projects: [...state.projects, result.project] }
        : {}),
      ...withSessions({ ...state.sessions, [result.session.id]: result.session }),
    }));
    actions.select(result.session.id);
    toast.success(options.useWorktree ? "已在隔离 Worktree 中启动任务" : "已启动任务执行");
    return true;
  },

  reconnectNow(): void {
    client.reconnectNow();
  },

  setGitChangeCount(projectId: string, count: number | undefined): void {
    set((state) => ({ gitChangeCounts: { ...state.gitChangeCounts, [projectId]: count } }));
  },

  retryModels: () => loadModels(true),

  async retrySession(sessionId: string): Promise<void> {
    if (get().views[sessionId]?.state?.access === "failed") {
      const ok = await attempt(() => client.call({ type: "session.retry", sessionId }));
      if (ok) toast.success("正在重新初始化会话");
      return;
    }
    if (get().previewId === sessionId) void openSession(sessionId, "preview");
    else if (get().selectedId === sessionId) void openSession(sessionId);
  },

  async addProject(): Promise<Project | undefined> {
    const path = await attempt(() => window.lila.pickDirectory("project"));
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
    viewCache.removeMany(removedSessionIds);
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
      const unreadCompletedIds = { ...current.unreadCompletedIds };
      for (const id of removedSessionIds) {
        delete sessions[id];
        delete views[id];
        delete drafts[id];
        delete draftImages[id];
        delete sendingDrafts[id];
        delete unreadCompletedIds[id];
      }
      return {
        projects: remainingProjects,
        ...withSessions(sessions),
        views,
        drafts,
        draftImages,
        sendingDrafts,
        unreadCompletedIds,
        newSession: nextNewSession,
        ...(wasSelected ? { selectedId: undefined, rightPanel: undefined, activeFile: undefined } : {}),
      };
    });
    if (wasSelected) {
      safeLocalRemove(SELECTED_KEY);
      closePreview();
      unwatchSession("main");
    }
    if (nextNewSession !== state.newSession) safeLocalSet(NEW_SESSION_KEY, JSON.stringify(nextNewSession));
    persistComposerDrafts();
    return true;
  },

  async createAndPrompt(projectId: string, modelId: string, thinkingLevel?: ThinkingLevel): Promise<void> {
    const { newSessionApprovalMode, defaultApprovalMode } = get();
    const approvalMode = newSessionApprovalMode ?? defaultApprovalMode;
    const submitted = beginDraftSubmission(NEW_DRAFT);
    if (!submitted) return;
    const version = selectionVersion;
    const created = await attempt(() =>
      client.call({ type: "session.create", projectId, modelId, approvalMode, ...(thinkingLevel ? { thinkingLevel } : {}) }),
    );
    if (!created) {
      finishDraftSubmission(NEW_DRAFT, false);
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
      const sendingDrafts = { ...state.sendingDrafts, [session.id]: submitted };
      delete sendingDrafts[NEW_DRAFT];
      const transferred = followSession
        ? writeDraft({ ...state, ...writeDraft(state, NEW_DRAFT, EMPTY_DRAFT) }, session.id, readDraft(NEW_DRAFT, state))
        : {};
      return {
        ...withSessions({ ...state.sessions, [session.id]: session }),
        ...transferred,
        sendingDrafts,
        ...(followSession ? { selectedId: session.id, newSessionApprovalMode: undefined } : {}),
      };
    });
    persistComposerDrafts();
    if (followSession) {
      safeLocalSet(SELECTED_KEY, session.id);
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

  async forkFromMessage(sessionId: string, itemId: string): Promise<boolean> {
    const version = selectionVersion;
    const result = await attempt(() => client.call({ type: "session.fork", sessionId, itemId }));
    if (!result) return false;
    const { session } = result;
    set((state) => withSessions({ ...state.sessions, [session.id]: session }));
    if (selectionVersion === version) actions.select(session.id);
    toast.success("已分叉出新会话");
    return true;
  },

  async updateQueue(
    sessionId: string,
    action: "remove" | "promote" | "clear",
    kind?: "steering" | "followUp",
    index?: number,
  ): Promise<boolean> {
    const result = await attempt(() =>
      client.call({
        type: "session.queue.update",
        sessionId,
        action,
        ...(kind ? { kind } : {}),
        ...(index !== undefined ? { index } : {}),
      }),
    );
    if (!result) return false;
    setView(sessionId, (view) => ({ ...view, state: result.state }));
    return true;
  },

  async prompt(sessionId: string, text: string, images: ImageInput[] = [], mode?: "steer" | "followUp"): Promise<boolean> {
    const trimmed = text.trim();
    if (trimmed === "/compact" || trimmed.startsWith("/compact ")) {
      const instructions = trimmed.slice("/compact".length).trim();
      return Boolean(await actions.compact(sessionId, instructions || undefined));
    }
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

  async compact(sessionId: string, instructions?: string): Promise<boolean> {
    return (await attempt(() => client.call({ type: "session.compact", sessionId, ...(instructions ? { instructions } : {}) }))) !== undefined;
  },

  async updateApprovalRules(
    sessionId: string,
    scope: "session" | "project",
    action: "remove" | "clear",
    pattern?: string,
  ): Promise<boolean> {
    const result = await attempt(() =>
      client.call({ type: "session.rules.update", sessionId, scope, action, ...(pattern ? { pattern } : {}) }),
    );
    if (!result) return false;
    setView(sessionId, (view) => ({ ...view, state: result.state }));
    return true;
  },

  async stopSubagent(sessionId: string, agentId: string): Promise<boolean> {
    const result = await attempt(() => client.call({ type: "session.subagent.stop", sessionId, agentId }));
    if (result !== undefined) toast.info("已发送停止信号给子 Agent");
    return result !== undefined;
  },

  async steerSubagent(sessionId: string, agentId: string, text: string): Promise<boolean> {
    const result = await attempt(() => client.call({ type: "session.subagent.steer", sessionId, agentId, message: text }));
    if (result !== undefined) toast.success("已向子 Agent 发送引导消息");
    return result !== undefined;
  },

  async reload(sessionId: string): Promise<boolean> {
    const done = (await attempt(() => client.call({ type: "session.reload", sessionId }))) !== undefined;
    if (done) {
      set((state) => ({ commandsRefreshSeq: state.commandsRefreshSeq + 1, settingsDirty: { mcp: false, memory: false, capabilities: false } }));
      toast.success("已重新加载技能、扩展和 MCP");
    }
    return done;
  },

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

  async commands(projectId: string, sessionId?: string): Promise<SlashCommand[] | undefined> {
    try {
      return (await client.call({ type: "commands.list", projectId, ...(sessionId ? { sessionId } : {}) })).commands;
    } catch {
      return undefined;
    }
  },

  async searchFiles(projectId: string, query: string): Promise<string[]> {
    try {
      return (await client.call({ type: "files.search", projectId, query, limit: 50 })).files;
    } catch {
      return [];
    }
  },

  async listFiles(projectId: string, refresh = false) {
    return client.call({ type: "files.list", projectId, ...(refresh ? { refresh } : {}) });
  },

  async readFile(projectId: string, path: string) {
    return (await client.call({ type: "files.read", projectId, path })).file;
  },

  async gitStatus(projectId: string) {
    return (await client.call({ type: "git.status", projectId })).status;
  },

  async gitDiff(projectId: string, path: string, staged?: boolean) {
    return (await client.call({ type: "git.diff", projectId, path, ...(staged !== undefined ? { staged } : {}) })).diff;
  },

  async gitStage(projectId: string, paths?: string[]) {
    const { status } = await client.call({ type: "git.stage", projectId, ...(paths?.length ? { paths } : {}) });
    actions.setGitChangeCount(projectId, status.isRepo ? status.files.length : undefined);
    return status;
  },

  async gitUnstage(projectId: string, paths?: string[]) {
    const { status } = await client.call({ type: "git.unstage", projectId, ...(paths?.length ? { paths } : {}) });
    actions.setGitChangeCount(projectId, status.isRepo ? status.files.length : undefined);
    return status;
  },

  async gitDiscard(projectId: string, path: string) {
    const { status } = await client.call({ type: "git.discard", projectId, path });
    actions.setGitChangeCount(projectId, status.isRepo ? status.files.length : undefined);
    actions.bumpWorkspaceRefresh();
    return status;
  },

  async gitCommit(projectId: string, commitMessage: string) {
    const res = await client.call({ type: "git.commit", projectId, message: commitMessage });
    actions.setGitChangeCount(projectId, res.status.isRepo ? res.status.files.length : undefined);
    actions.bumpWorkspaceRefresh();
    return res;
  },

  async createWorktreeProject(projectId: string, branch?: string): Promise<{ project: Project; branch: string } | undefined> {
    const result = await attempt(() =>
      client.call({ type: "git.worktree.create", projectId, ...(branch?.trim() ? { branch: branch.trim() } : {}) }),
    );
    if (!result) return undefined;
    set((state) => ({
      projects: state.projects.some((item) => item.id === result.project.id) ? state.projects : [...state.projects, result.project],
    }));
    toast.success(`已创建隔离 Worktree 分支 ${result.branch}`);
    return result;
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
