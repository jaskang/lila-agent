import { randomUUID } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, renameSync, statSync, unlinkSync } from "node:fs";
import { basename, join, resolve } from "node:path";
import { type AgentSessionServices, createAgentSessionFromServices, SessionManager } from "@earendil-works/pi-coding-agent";
import type {
  ApprovalMode,
  AuthLoginEvent,
  AuthMethod,
  CapabilitiesSnapshot,
  CoreSettings,
  GitFileDiff,
  GitStatus,
  McpConfigSnapshot,
  McpExposure,
  McpScope,
  McpServerInput,
  MemorySnapshot,
  MemoryTarget,
  ModelInfo,
  PackageScope,
  Project,
  ProjectFileContent,
  ProviderAuthInfo,
  ProviderQuotaItem,
  SessionSummary,
  ThinkingLevel,
  UsageRange,
  UsageSummary,
} from "@lila/protocol";
import { UserError } from "./errors.ts";
import { createServices, ExtensionsService } from "./extensions-service.ts";
import { gitDiff as readGitDiff, gitStatus as readGitStatus } from "./git.ts";
import { type Clients, findModel, LiveSession, parseAutoSubagentName, sessionTitle, slashCommands } from "./live-session.ts";
import type { MemoryConfigPatch } from "./memory.ts";
import { SettingsService, settleServices } from "./settings-service.ts";
import { Store } from "./store.ts";
import { readSubsessionView, recordedSubagentIds } from "./subsession-view.ts";
import { invalidateFileCache, listTreeFiles, readPreviewFile, searchProjectFiles } from "./workspace-service.ts";

const IDLE_DISPOSE_MS = 15 * 60_000;
const SWEEP_INTERVAL_MS = 60_000;
const SUBSESSION_REFRESH_MS = 750;

type IndexedSession = { id: string; projectId: string; path: string; summary: SessionSummary };
type CachedServices = { promise: Promise<AgentSessionServices>; lastActivity: number };

function moveToTrash(source: string, destination: string): void {
  try {
    renameSync(source, destination);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "EXDEV") throw error;
    copyFileSync(source, destination);
    unlinkSync(source);
  }
}

/** Projects, the index of their Pi sessions, and the live AgentSessions of opened ones. */
export class SessionHost {
  readonly store: Store;
  readonly #dataDir: string;
  readonly #clients: Clients;
  readonly #live = new Map<string, LiveSession>();
  readonly #opening = new Map<string, Promise<LiveSession>>();
  readonly #openingProjects = new Map<string, string>();
  readonly #deletingSessions = new Set<string>();
  readonly #removingProjects = new Set<string>();
  #index = new Map<string, IndexedSession>();
  readonly #projectServices = new Map<string, CachedServices>();
  readonly #subsessionSyncTimers = new Map<string, { projectId: string; timer: NodeJS.Timeout }>();
  readonly #sweeper: NodeJS.Timeout;
  readonly #subsessionRefresh: NodeJS.Timeout;
  readonly #watchedSubsessions = new Map<string, string>();
  readonly #settings: SettingsService;
  readonly #extensions: ExtensionsService;
  #sweeping = false;

  constructor(dataDir: string, clients: Clients) {
    this.#dataDir = dataDir;
    this.#clients = clients;
    this.store = new Store(dataDir);
    this.#settings = new SettingsService(dataDir, () => this.#syncLiveRuntime());
    this.#extensions = new ExtensionsService({
      project: (projectId) => this.#project(projectId),
      servicesForCapabilities: (projectId, sessionId) => this.#servicesForCapabilities(projectId, sessionId),
      invalidateProjectServices: (projectId) => this.#invalidateProjectServices(projectId),
      reloadIdleSessions: (projectId, sessionId) => this.#reloadIdleSessions(projectId, sessionId),
    });
    this.#sweeper = setInterval(() => void this.#sweep(), SWEEP_INTERVAL_MS);
    this.#sweeper.unref();
    this.#subsessionRefresh = setInterval(() => this.#refreshSubsessionViews(), SUBSESSION_REFRESH_MS).unref();
  }

  // -------------------------------------------------------------------------
  // Projects

  addProject(path: string): Project {
    const resolved = resolve(path);
    if (!existsSync(resolved)) throw new UserError(`目录不存在：${resolved}`);
    if (!statSync(resolved).isDirectory()) throw new UserError(`这不是一个目录：${resolved}`);
    const existing = this.store.projects.find((project) => project.path === resolved);
    if (existing) return existing;
    const project: Project = { id: randomUUID(), path: resolved, name: basename(resolved) || resolved };
    this.store.addProject(project);
    this.#clients.broadcast({ type: "projects", projects: [...this.store.projects] });
    return project;
  }

  async removeProject(projectId: string): Promise<void> {
    const project = this.store.project(projectId);
    this.#removingProjects.add(projectId);
    try {
      for (const [parentId, entry] of Array.from(this.#subsessionSyncTimers.entries())) {
        if (entry.projectId === projectId) {
          clearTimeout(entry.timer);
          this.#subsessionSyncTimers.delete(parentId);
        }
      }
      const pendingOpens: Array<Promise<unknown>> = [];
      for (const [sessionId, promise] of this.#opening) {
        const openProjectId = this.#openingProjects.get(sessionId) ?? this.#index.get(sessionId)?.projectId;
        if (!openProjectId || openProjectId === projectId) pendingOpens.push(promise.catch(() => undefined));
      }
      if (pendingOpens.length) await Promise.all(pendingOpens);

      for (const live of Array.from(this.#live.values())) if (live.projectId === projectId) await this.#dispose(live);
      const cached = this.#projectServices.get(projectId);
      if (cached) {
        this.#projectServices.delete(projectId);
        await settleServices(cached.promise);
      }
      for (const [id, entry] of Array.from(this.#index.entries())) {
        if (entry.projectId === projectId) this.#index.delete(id);
      }
      if (project) invalidateFileCache(project.path);
      this.store.removeProject(projectId);
      this.#clients.broadcast({ type: "projects", projects: [...this.store.projects] });
    } finally {
      this.#removingProjects.delete(projectId);
    }
  }

  #project(projectId: string): Project {
    const project = this.store.project(projectId);
    if (!project) throw new UserError("项目不存在，可能已经被移除");
    return project;
  }

  // -------------------------------------------------------------------------
  // Service caches & delegations

  #invalidateCaches(projectId?: string): void {
    this.#settings.invalidate();
    if (projectId) {
      const cached = this.#projectServices.get(projectId);
      if (cached) {
        this.#projectServices.delete(projectId);
        void settleServices(cached.promise);
      }
    }
  }

  #invalidateProjectServices(projectId?: string): void {
    if (projectId) {
      this.#invalidateCaches(projectId);
      return;
    }
    this.#invalidateCaches();
    for (const [id, cached] of Array.from(this.#projectServices.entries())) {
      this.#projectServices.delete(id);
      void settleServices(cached.promise);
    }
  }

  async #reloadIdleSessions(projectId?: string, sessionId?: string): Promise<void> {
    this.#invalidateProjectServices(projectId);
    const targets = sessionId
      ? [this.#live.get(sessionId)].filter((live): live is LiveSession => Boolean(live))
      : Array.from(this.#live.values()).filter((live) => !projectId || live.projectId === projectId);
    await Promise.all(targets.map((live) => live.reloadIfIdle().catch(() => false)));
  }

  async #syncLiveRuntime(): Promise<void> {
    await Promise.all(Array.from(this.#live.values(), (live) => live.refreshSettingsAndAuth()));
  }

  async #servicesForCapabilities(projectId?: string, sessionId?: string): Promise<{ services: AgentSessionServices; cwd?: string }> {
    if (sessionId) {
      const live = this.#live.get(sessionId);
      if (live) return { services: live.services, cwd: live.services.cwd };
    }
    if (projectId) {
      const project = this.#project(projectId);
      const existingLive = Array.from(this.#live.values()).find((live) => live.projectId === project.id);
      if (existingLive) return { services: existingLive.services, cwd: project.path };
      let entry = this.#projectServices.get(project.id);
      if (!entry) {
        const promise = createServices(project.path);
        promise.catch(() => {
          if (this.#projectServices.get(project.id)?.promise === promise) this.#projectServices.delete(project.id);
        });
        entry = { promise, lastActivity: Date.now() };
        this.#projectServices.set(project.id, entry);
      } else {
        entry.lastActivity = Date.now();
      }
      return { services: await entry.promise, cwd: project.path };
    }
    return { services: await this.#settings.getModelServices().promise };
  }

  listModels(forceRefresh = false): Promise<{ models: ModelInfo[]; defaultModelId?: string }> {
    return this.#settings.listModels(forceRefresh);
  }
  setProviderModels(providerId: string, modelIds: string[] | null): Promise<{ models: ModelInfo[]; defaultModelId?: string }> {
    return this.#settings.setProviderModels(providerId, modelIds);
  }
  getSettings(): Promise<CoreSettings> {
    return this.#settings.getSettings();
  }
  updateSettings(patch: {
    defaultModelId?: string | undefined;
    defaultThinkingLevel?: ThinkingLevel | undefined;
    autoCompaction?: boolean | undefined;
    autoRetry?: boolean | undefined;
  }): Promise<CoreSettings> {
    return this.#settings.updateSettings(patch);
  }
  listProviders(): Promise<ProviderAuthInfo[]> {
    return this.#settings.listProviders();
  }
  startProviderLogin(owner: unknown, providerId: string, method: AuthMethod, emit: (event: AuthLoginEvent) => void): Promise<string> {
    return this.#settings.startProviderLogin(owner, providerId, method, emit);
  }
  respondProviderLogin(loginId: string, promptId: string, value: string): void {
    this.#settings.respondProviderLogin(loginId, promptId, value);
  }
  cancelProviderLogin(loginId: string): void {
    this.#settings.cancelProviderLogin(loginId);
  }
  cancelLoginsForOwner(owner: unknown): void {
    this.#settings.cancelLoginsForOwner(owner);
  }
  logoutProvider(providerId: string): Promise<void> {
    return this.#settings.logoutProvider(providerId);
  }
  getUsageSummary(range: UsageRange, onlyConfigured = true): Promise<UsageSummary> {
    return this.#settings.getUsageSummary(range, onlyConfigured);
  }
  listQuotas(refresh = false): Promise<ProviderQuotaItem[]> {
    return this.#settings.listQuotas(refresh);
  }

  listMcp(projectId?: string): McpConfigSnapshot {
    return this.#extensions.listMcp(projectId);
  }
  upsertMcp(scope: McpScope, name: string, config: McpServerInput, projectId?: string): Promise<McpConfigSnapshot> {
    return this.#extensions.upsertMcp(scope, name, config, projectId);
  }
  toggleMcp(
    scope: McpScope,
    name: string,
    patch: { enabled?: boolean | undefined; exposure?: McpExposure | undefined },
    projectId?: string,
  ): Promise<McpConfigSnapshot> {
    return this.#extensions.toggleMcp(scope, name, patch, projectId);
  }
  removeMcp(scope: McpScope, name: string, projectId?: string): Promise<McpConfigSnapshot> {
    return this.#extensions.removeMcp(scope, name, projectId);
  }
  getCapabilities(projectId?: string, sessionId?: string): Promise<CapabilitiesSnapshot> {
    return this.#extensions.getCapabilities(projectId, sessionId);
  }
  installPackage(source: string, scope: PackageScope, projectId?: string): Promise<CapabilitiesSnapshot> {
    return this.#extensions.installPackage(source, scope, projectId);
  }
  removePackage(source: string, scope: PackageScope, projectId?: string): Promise<CapabilitiesSnapshot> {
    return this.#extensions.removePackage(source, scope, projectId);
  }
  updatePackages(source?: string, projectId?: string): Promise<CapabilitiesSnapshot> {
    return this.#extensions.updatePackages(source, projectId);
  }
  async reloadWorkspace(projectId?: string, sessionId?: string): Promise<void> {
    if (sessionId) {
      const live = this.#live.get(sessionId);
      if (live) {
        await live.reload();
        return;
      }
    }
    await this.#reloadIdleSessions(projectId);
  }
  getMemory(projectId?: string, sessionId?: string): Promise<MemorySnapshot> {
    return this.#extensions.getMemory(projectId, sessionId);
  }
  addMemory(target: MemoryTarget, content: string): Promise<MemorySnapshot> {
    return this.#extensions.addMemory(target, content);
  }
  replaceMemory(target: MemoryTarget, oldText: string, newContent: string): Promise<MemorySnapshot> {
    return this.#extensions.replaceMemory(target, oldText, newContent);
  }
  removeMemory(target: MemoryTarget, oldText: string): Promise<MemorySnapshot> {
    return this.#extensions.removeMemory(target, oldText);
  }
  updateMemorySettings(patch: MemoryConfigPatch): Promise<MemorySnapshot> {
    return this.#extensions.updateMemorySettings(patch);
  }

  // -------------------------------------------------------------------------
  // Session index

  #enrichSubsession(
    summary: SessionSummary,
    parentId: string | undefined,
    rawName: string | undefined,
    firstMessage: string | undefined,
    agentId: string,
  ): void {
    if (parentId) summary.parentId = parentId;
    const liveSub = parentId ? this.#live.get(parentId)?.subagentByPrefix(agentId) : undefined;
    summary.agentId = liveSub?.id ?? agentId;
    summary.readOnly = true;
    summary.title = sessionTitle(rawName, firstMessage, { isSubsession: true, description: liveSub?.description });
    if (liveSub) {
      if (liveSub.status === "running" || liveSub.status === "queued") summary.status = "running";
      else if (liveSub.status === "error" || liveSub.status === "aborted" || liveSub.status === "stopped") summary.status = "error";
      else summary.status = "idle";
    }
  }

  async listSessions(): Promise<SessionSummary[]> {
    const scanned = new Map<string, IndexedSession>();
    const parents = new Map<string, { parentPath: string; name?: string | undefined; firstMessage?: string | undefined }>();
    const projects = [...this.store.projects];
    await Promise.all(
      projects.map(async (project) => {
        if (this.#removingProjects.has(project.id)) return;
        for (const info of await SessionManager.list(project.path)) {
          if (this.#deletingSessions.has(info.id) || this.#removingProjects.has(project.id)) continue;
          const firstMessage = info.messageCount > 0 ? info.firstMessage : undefined;
          if (info.parentSessionPath) parents.set(info.id, { parentPath: info.parentSessionPath, name: info.name, firstMessage });
          scanned.set(info.id, {
            id: info.id,
            projectId: project.id,
            path: info.path,
            summary: {
              id: info.id,
              projectId: project.id,
              title: sessionTitle(info.name, firstMessage),
              updatedAt: info.modified.toISOString(),
              status: "idle",
              pendingCount: 0,
            },
          });
        }
      }),
    );
    const nextIndex = new Map<string, IndexedSession>();
    for (const [id, entry] of scanned) {
      if (this.#deletingSessions.has(id) || this.#removingProjects.has(entry.projectId) || !this.store.project(entry.projectId)) continue;
      nextIndex.set(id, entry);
    }
    for (const [id, prev] of this.#index) {
      if (nextIndex.has(id)) continue;
      if (this.#deletingSessions.has(id) || this.#removingProjects.has(prev.projectId) || !this.store.project(prev.projectId)) continue;
      if (this.#live.has(id) || existsSync(prev.path)) nextIndex.set(id, prev);
    }
    const idByPath = new Map(Array.from(nextIndex.values(), (entry) => [entry.path, entry.id]));
    const recordedByParent = new Map<string, Map<string, string>>();
    for (const [id, meta] of parents) {
      const entry = nextIndex.get(id);
      if (!entry) continue;
      let agentId = parseAutoSubagentName(meta.name)?.idPrefix;
      if (!agentId) {
        let recorded = recordedByParent.get(meta.parentPath);
        if (!recorded) {
          recorded = recordedSubagentIds(meta.parentPath);
          recordedByParent.set(meta.parentPath, recorded);
        }
        agentId = recorded.get(entry.path);
      }
      if (agentId) this.#enrichSubsession(entry.summary, idByPath.get(meta.parentPath), meta.name, meta.firstMessage, agentId);
    }
    this.#index = nextIndex;
    const summaries = new Map(Array.from(nextIndex.values(), (entry) => [entry.id, entry.summary]));
    for (const live of this.#live.values()) {
      if (this.#deletingSessions.has(live.id) || this.#removingProjects.has(live.projectId)) continue;
      summaries.set(live.id, live.summary());
    }
    return Array.from(summaries.values()).sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
  }

  async commands(projectId: string, sessionId?: string) {
    if (sessionId) return (await this.live(sessionId)).commands();
    const { services } = await this.#servicesForCapabilities(projectId);
    const loader = services.resourceLoader;
    const extensionCommands = loader.getExtensions().extensions.flatMap((extension) => Array.from(extension.commands.values()));
    return slashCommands(extensionCommands, loader.getPrompts().prompts, loader.getSkills().skills);
  }

  // -------------------------------------------------------------------------
  // Live sessions

  async createSession(options: { projectId: string; modelId?: string; thinkingLevel?: ThinkingLevel; approvalMode?: ApprovalMode }): Promise<SessionSummary> {
    const project = this.#project(options.projectId);
    const manager = SessionManager.create(project.path);
    this.store.setApprovalMode(manager.getSessionId(), options.approvalMode ?? "auto");
    try {
      const live = await this.#start(project, manager, options);
      const summary = live.summary();
      this.#clients.broadcast({ type: "session", session: summary });
      return summary;
    } catch (error) {
      this.store.forgetSession(manager.getSessionId());
      throw error;
    }
  }

  async live(sessionId: string): Promise<LiveSession> {
    if (this.#deletingSessions.has(sessionId)) throw new UserError("会话正在删除");
    const live = this.#live.get(sessionId);
    if (live) {
      live.touch();
      return live;
    }
    let opening = this.#opening.get(sessionId);
    if (!opening) {
      const knownProjectId = this.#index.get(sessionId)?.projectId;
      if (knownProjectId) this.#openingProjects.set(sessionId, knownProjectId);
      opening = this.#open(sessionId).finally(() => {
        this.#opening.delete(sessionId);
        this.#openingProjects.delete(sessionId);
      });
      this.#opening.set(sessionId, opening);
    }
    return opening;
  }

  async #entry(sessionId: string): Promise<IndexedSession> {
    let entry = this.#index.get(sessionId);
    if (!entry || !existsSync(entry.path)) {
      await this.listSessions();
      entry = this.#index.get(sessionId);
    }
    if (!entry || this.#deletingSessions.has(sessionId)) throw new UserError("会话不存在，可能已经被删除");
    if (this.#removingProjects.has(entry.projectId)) throw new UserError("项目正在移除");
    return entry;
  }

  #subsessionStatus(entry: IndexedSession): SessionSummary["status"] {
    const { parentId, agentId, status } = entry.summary;
    const progress = parentId && agentId ? this.#live.get(parentId)?.subagentByPrefix(agentId) : undefined;
    if (!progress) return status;
    if (progress.status === "queued" || progress.status === "running") return "running";
    if (progress.status === "completed" || progress.status === "steered") return "idle";
    return "error";
  }

  async view(sessionId: string, watch = false, preview = false): Promise<Pick<LiveSession, "open" | "earlier" | "search" | "turns" | "around">> {
    const live = this.#live.get(sessionId);
    if (live && !preview) return await this.live(sessionId);
    const entry = await this.#entry(sessionId);
    if (!entry.summary.readOnly) {
      if (preview) throw new UserError("侧边预览仅支持子 Agent 会话");
      return await this.live(sessionId);
    }
    const status = this.#subsessionStatus(entry);
    if (watch) this.#watchedSubsessions.set(sessionId, this.#subsessionVersion(entry, status));
    const project = this.#project(entry.projectId);
    return readSubsessionView(entry.path, project.path, status, this.store.approvalMode(sessionId));
  }

  async forkSession(sessionId: string, itemId?: string): Promise<SessionSummary> {
    const entry = await this.#entry(sessionId);
    if (!itemId) return await this.forkSubsession(sessionId);
    if (entry.summary.readOnly) throw new UserError("子 Agent 会话请直接使用「继续为新会话」");
    const live = await this.live(sessionId);
    const entryId = live.findUserEntryId(itemId);
    if (!entryId) throw new UserError("找不到这条消息，无法从此处分叉");
    const branchPath = live.session.sessionManager.createBranchedSession(entryId);
    if (!branchPath) throw new UserError("无法创建分叉会话文件");
    const manager = SessionManager.open(branchPath);
    const title = `分叉：${entry.summary.title}`;
    manager.appendSessionInfo(title);
    const summary: SessionSummary = {
      id: manager.getSessionId(),
      projectId: entry.projectId,
      title,
      updatedAt: new Date().toISOString(),
      status: "idle",
      pendingCount: 0,
    };
    this.store.setApprovalMode(summary.id, this.store.approvalMode(sessionId));
    this.#index.set(summary.id, { id: summary.id, projectId: entry.projectId, path: branchPath, summary });
    this.#clients.broadcast({ type: "session", session: summary });
    return summary;
  }

  async retrySession(sessionId: string): Promise<void> {
    const existing = this.#live.get(sessionId);
    if (existing) {
      if (existing.access !== "failed") return;
      await this.#dispose(existing, false);
    }
    await this.live(sessionId);
    this.#clients.toWatchers(sessionId, { type: "reset", sessionId });
  }

  async forkSubsession(sessionId: string): Promise<SessionSummary> {
    const entry = await this.#entry(sessionId);
    if (!entry.summary.readOnly) throw new UserError("只有子 Agent 会话可以用此方式继续");
    if (this.#subsessionStatus(entry) === "running") throw new UserError("请等子 Agent 结束后再继续");
    const project = this.#project(entry.projectId);
    const manager = SessionManager.forkFrom(entry.path, project.path);
    const title = `继续：${entry.summary.title}`;
    manager.appendSessionInfo(title);
    const summary: SessionSummary = {
      id: manager.getSessionId(),
      projectId: project.id,
      title,
      updatedAt: new Date().toISOString(),
      status: "idle",
      pendingCount: 0,
    };
    const path = manager.getSessionFile();
    if (!path) throw new UserError("无法创建新会话文件");
    this.store.setApprovalMode(summary.id, this.store.approvalMode(sessionId));
    this.#index.set(summary.id, { id: summary.id, projectId: project.id, path, summary });
    this.#clients.broadcast({ type: "session", session: summary });
    return summary;
  }

  async #open(sessionId: string): Promise<LiveSession> {
    const entry = await this.#entry(sessionId);
    if (entry.summary.readOnly) throw new UserError("子 Agent 会话是只读的，结束后可继续为新会话");
    this.#openingProjects.set(sessionId, entry.projectId);
    return this.#start(this.#project(entry.projectId), SessionManager.open(entry.path), {
      updatedAt: entry.summary.updatedAt,
    });
  }

  async #start(
    project: Project,
    sessionManager: SessionManager,
    options: { modelId?: string; thinkingLevel?: ThinkingLevel; updatedAt?: string },
  ): Promise<LiveSession> {
    const sessionId = sessionManager.getSessionId();
    if (this.#deletingSessions.has(sessionId) || this.#removingProjects.has(project.id)) {
      throw new UserError("会话或项目已被移除");
    }
    const live = new LiveSession({
      id: sessionId,
      projectId: project.id,
      store: this.store,
      clients: this.#clients,
      onReload: () => this.#invalidateCaches(project.id),
      onSubsessionsChanged: () => this.#scheduleSubsessionSync(project, sessionId),
      onProviderResponse: (providerId, _status, headers) => this.#settings.recordResponseHeaders(providerId, headers),
    });
    const services = await createServices(project.path, live);
    try {
      let model;
      if (options.modelId) {
        model = findModel(services, options.modelId);
        if (!model) throw new UserError(`找不到模型 ${options.modelId}，或它还没有配置凭据`);
      }
      const { session } = await createAgentSessionFromServices({
        services,
        sessionManager,
        ...(model ? { model } : {}),
        ...(options.thinkingLevel ? { thinkingLevel: options.thinkingLevel } : {}),
      });
      live.attach(services, session, options.updatedAt);
    } catch (error) {
      await settleServices(Promise.resolve(services));
      throw error;
    }
    if (this.#deletingSessions.has(live.id) || this.#removingProjects.has(project.id) || !this.store.project(project.id)) {
      await live.dispose();
      throw new UserError("会话或项目已被移除");
    }
    this.#live.set(live.id, live);
    return live;
  }

  async deleteSession(sessionId: string): Promise<void> {
    if (this.#deletingSessions.has(sessionId)) throw new UserError("会话正在删除");
    if (!this.#index.has(sessionId) && !this.#live.has(sessionId) && this.#live.size > 0) {
      await this.listSessions();
      if (this.#deletingSessions.has(sessionId)) throw new UserError("会话正在删除");
    }
    const indexed = this.#index.get(sessionId);
    if (indexed?.summary.readOnly && this.#subsessionStatus(indexed) === "running") {
      throw new UserError("请等子 Agent 结束后再删除会话");
    }
    this.#deletingSessions.add(sessionId);
    try {
      const pendingTimer = this.#subsessionSyncTimers.get(sessionId);
      if (pendingTimer) {
        clearTimeout(pendingTimer.timer);
        this.#subsessionSyncTimers.delete(sessionId);
      }
      const opening = this.#opening.get(sessionId);
      if (opening) await opening.catch(() => undefined);

      const live = this.#live.get(sessionId);
      if (live) await this.#dispose(live, false);
      let path = live?.session.sessionFile ?? indexed?.path;
      if (!path || !existsSync(path)) {
        path = undefined;
        for (const project of this.store.projects) {
          if (this.#removingProjects.has(project.id)) continue;
          path = SessionManager.findById(project.path, sessionId);
          if (path) break;
        }
      }
      if (!path && !live) throw new UserError("找不到要删除的会话文件");
      if (path) {
        const trash = join(this.#dataDir, "trash");
        mkdirSync(trash, { recursive: true });
        moveToTrash(path, join(trash, basename(path)));
      }
      this.#index.delete(sessionId);
      this.#watchedSubsessions.delete(sessionId);
      this.store.forgetSession(sessionId);
      this.#clients.broadcast({ type: "session.removed", sessionId });
    } finally {
      this.#deletingSessions.delete(sessionId);
    }
  }

  async #dispose(live: LiveSession, announce = true): Promise<void> {
    this.#live.delete(live.id);
    await live.dispose();
    const file = live.session.sessionFile;
    if (announce && !(file && existsSync(file))) {
      this.store.forgetSession(live.id);
      this.#clients.broadcast({ type: "session.removed", sessionId: live.id });
    }
  }

  async #sweep(): Promise<void> {
    if (this.#sweeping) return;
    this.#sweeping = true;
    try {
      const now = Date.now();
      for (const live of Array.from(this.#live.values())) {
        if (live.busy || this.#clients.isWatched(live.id) || now - live.lastActivity < IDLE_DISPOSE_MS) continue;
        await this.#dispose(live).catch((error: unknown) => console.error(`[core] disposing ${live.id} failed`, error));
      }
      for (const [projectId, entry] of Array.from(this.#projectServices.entries())) {
        if (now - entry.lastActivity < IDLE_DISPOSE_MS) continue;
        this.#projectServices.delete(projectId);
        await settleServices(entry.promise);
      }
    } finally {
      this.#sweeping = false;
    }
  }

  #scheduleSubsessionSync(project: Project, parentId: string): void {
    if (this.#subsessionSyncTimers.has(parentId) || this.#deletingSessions.has(parentId) || this.#removingProjects.has(project.id)) return;
    const timer = setTimeout(() => {
      this.#subsessionSyncTimers.delete(parentId);
      void this.#syncSubsessions(project, parentId).catch((error: unknown) => {
        console.error(`[core] syncing sub-sessions for ${parentId} failed`, error);
      });
    }, 600);
    timer.unref();
    this.#subsessionSyncTimers.set(parentId, { projectId: project.id, timer });
  }

  async #syncSubsessions(project: Project, parentId: string): Promise<void> {
    if (this.#deletingSessions.has(parentId) || this.#removingProjects.has(project.id) || !this.store.project(project.id)) return;
    const parentFile = this.#live.get(parentId)?.session.sessionFile ?? this.#index.get(parentId)?.path;
    if (!parentFile) return;
    let recorded: Map<string, string> | undefined;
    for (const info of await SessionManager.list(project.path)) {
      if (this.#deletingSessions.has(parentId) || this.#removingProjects.has(project.id) || !this.store.project(project.id)) return;
      if (this.#deletingSessions.has(info.id) || info.parentSessionPath !== parentFile) continue;
      const firstMessage = info.messageCount > 0 ? info.firstMessage : undefined;
      const summary: SessionSummary = {
        id: info.id,
        projectId: project.id,
        title: sessionTitle(info.name, firstMessage),
        updatedAt: info.modified.toISOString(),
        status: "idle",
        pendingCount: 0,
      };
      const agentId = parseAutoSubagentName(info.name)?.idPrefix ?? (recorded ??= recordedSubagentIds(parentFile)).get(info.path);
      if (!agentId) continue;
      this.#enrichSubsession(summary, parentId, info.name, firstMessage, agentId);
      const prev = this.#index.get(info.id)?.summary;
      this.#index.set(info.id, { id: info.id, projectId: project.id, path: info.path, summary });
      if (prev?.status !== summary.status) this.#clients.toWatchers(info.id, { type: "reset", sessionId: info.id });
      if (
        !prev ||
        prev.title !== summary.title ||
        prev.status !== summary.status ||
        prev.updatedAt !== summary.updatedAt ||
        prev.parentId !== summary.parentId ||
        prev.agentId !== summary.agentId
      ) {
        this.#clients.broadcast({ type: "session", session: summary });
      }
    }
  }

  #subsessionVersion(entry: IndexedSession, status = this.#subsessionStatus(entry)): string {
    const stat = statSync(entry.path, { throwIfNoEntry: false });
    return stat ? `${status}:${stat.mtimeMs}:${stat.size}` : "";
  }

  #refreshSubsessionViews(): void {
    for (const [id, last] of this.#watchedSubsessions) {
      const entry = this.#index.get(id);
      if (!entry || !this.#clients.isWatched(id)) {
        this.#watchedSubsessions.delete(id);
        continue;
      }
      try {
        const current = this.#subsessionVersion(entry);
        if (last === current) continue;
        this.#watchedSubsessions.set(id, current);
        this.#clients.toWatchers(id, { type: "reset", sessionId: id });
      } catch (error) {
        console.error(`[core] refreshing sub-session ${id} failed`, error);
      }
    }
  }

  async dispose(): Promise<void> {
    clearInterval(this.#sweeper);
    clearInterval(this.#subsessionRefresh);
    for (const entry of this.#subsessionSyncTimers.values()) clearTimeout(entry.timer);
    this.#subsessionSyncTimers.clear();
    const projectServices = Array.from(this.#projectServices.values(), (entry) => entry.promise);
    this.#projectServices.clear();
    await Promise.all([
      ...Array.from(this.#live.values()).map((live) => this.#dispose(live, false)),
      ...projectServices.map(settleServices),
      this.#settings.dispose(),
    ]);
  }

  // -------------------------------------------------------------------------
  // Files & Git

  searchFiles(projectId: string, query: string, limit: number): Promise<string[]> {
    return searchProjectFiles(this.#project(projectId).path, query, limit);
  }
  listProjectFiles(projectId: string, refresh = false): Promise<{ files: string[]; truncated: boolean }> {
    return listTreeFiles(this.#project(projectId).path, refresh);
  }
  readProjectFile(projectId: string, rawPath: string): Promise<ProjectFileContent> {
    return readPreviewFile(this.#project(projectId).path, rawPath);
  }
  gitStatus(projectId: string): Promise<GitStatus> {
    return readGitStatus(this.#project(projectId).path);
  }
  gitDiff(projectId: string, path: string, staged?: boolean): Promise<GitFileDiff> {
    return readGitDiff(this.#project(projectId).path, path, staged);
  }
}
