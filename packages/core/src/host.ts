import { clampThinkingLevel, getSupportedThinkingLevels } from "@earendil-works/pi-ai/compat";
import { execFile } from "node:child_process";
import { randomUUID } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, realpathSync, renameSync, statSync, unlinkSync } from "node:fs";
import { open, readdir } from "node:fs/promises";
import { createRequire } from "node:module";
import { homedir } from "node:os";
import { basename, dirname, extname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { promisify } from "node:util";
import {
  type AgentSessionServices,
  createAgentSessionFromServices,
  createAgentSessionServices,
  createCodemodeExtension,
  createMcpExtension,
  createToolSearchExtension,
  getAgentDir,
  type LoadExtensionsResult,
  SessionManager,
} from "@earendil-works/pi-coding-agent";
import type { ApprovalMode, GitFileDiff, GitStatus, ModelInfo, Project, ProjectFileContent, SessionSummary, ThinkingLevel } from "@lila/protocol";
import fuzzysort from "fuzzysort";
import { UserError } from "./errors.ts";
import { gitDiff as readGitDiff, gitStatus as readGitStatus } from "./git.ts";
import { createLilaExtension } from "./lila-extension.ts";
import { type Clients, findModel, LiveSession, parseAutoSubagentName, sessionTitle, slashCommands } from "./live-session.ts";
import { Store } from "./store.ts";
import { readSubsessionView, recordedSubagentIds } from "./subsession-view.ts";

const IDLE_DISPOSE_MS = 15 * 60_000;
const SWEEP_INTERVAL_MS = 60_000;
const SUBSESSION_REFRESH_MS = 750;
const WALK_LIMIT = 20_000;
const TREE_FILE_LIMIT = 3_000;
const FILE_PREVIEW_MAX_BYTES = 512 * 1024;
const IMAGE_EXTS = new Set([".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".avif", ".bmp", ".ico"]);
const WALK_SKIP: Record<string, true> = { node_modules: true, ".git": true };
const FILE_CACHE_TTL_MS = 8_000;
const MAX_FILE_CACHE_ENTRIES = 32;
const execFileAsync = promisify(execFile);
const fileListCache = new Map<string, { files: string[]; expiresAt: number }>();

/** pi-subagents' extension entry, loaded unless the user installed the package into Pi themselves. */
const SUBAGENTS_ENTRY = join(dirname(createRequire(import.meta.url).resolve("@tintinweb/pi-subagents/package.json")), "src", "index.ts");

function withoutDuplicateSubagents(result: LoadExtensionsResult): LoadExtensionsResult {
  const userCopy = result.extensions.some((extension) => extension.path !== SUBAGENTS_ENTRY && extension.path.includes("pi-subagents"));
  if (!userCopy) return result;
  return {
    ...result,
    extensions: result.extensions.filter((extension) => extension.path !== SUBAGENTS_ENTRY),
    errors: result.errors.filter((error) => error.path !== SUBAGENTS_ENTRY && !error.error.includes(SUBAGENTS_ENTRY)),
  };
}

/** Pi services for a project: its skills, prompts, AGENTS.md, the user's extensions, and Lila's built-ins. */
function createServices(cwd: string, live?: LiveSession): Promise<AgentSessionServices> {
  return createAgentSessionServices({
    cwd,
    agentDir: getAgentDir(),
    resourceLoaderOptions: {
      additionalExtensionPaths: [SUBAGENTS_ENTRY],
      // The pi CLI's built-in extensions; SDK sessions do not load them on their own.
      extensionFactories: [
        { name: "codemode", factory: createCodemodeExtension(), replaceable: true, builtin: true },
        { name: "tool-search", factory: createToolSearchExtension(), replaceable: true, builtin: true },
        { name: "mcp", factory: createMcpExtension(), replaceable: true, builtin: true },
        ...(live ? [{ name: "lila", factory: createLilaExtension(live), hidden: true }] : []),
      ],
      extensionsOverride: withoutDuplicateSubagents,
    },
    // A project the user added to Lila is trusted.
    resourceLoaderReloadOptions: { resolveProjectTrust: async () => true },
  });
}

type IndexedSession = { id: string; projectId: string; path: string; summary: SessionSummary };
type CachedServices = { promise: Promise<AgentSessionServices>; lastActivity: number };

async function settleServices(promise: Promise<AgentSessionServices>): Promise<void> {
  try {
    const services = await promise;
    await services.modelRuntime.settleCredentialWrites();
  } catch {
    // Creation already failed or credentials could not be flushed during teardown.
  }
}

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
  #modelServices: Promise<AgentSessionServices> | undefined;
  /** Services of projects without a live session, for the slash menu of a session not created yet. */
  readonly #projectServices = new Map<string, CachedServices>();
  readonly #subsessionSyncTimers = new Map<string, { projectId: string; timer: NodeJS.Timeout }>();
  readonly #sweeper: NodeJS.Timeout;
  readonly #subsessionRefresh: NodeJS.Timeout;
  readonly #watchedSubsessions = new Map<string, string>();
  #sweeping = false;

  constructor(dataDir: string, clients: Clients) {
    this.#dataDir = dataDir;
    this.#clients = clients;
    this.store = new Store(dataDir);
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
        if (!openProjectId || openProjectId === projectId) {
          pendingOpens.push(promise.catch(() => undefined));
        }
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
      if (project) fileListCache.delete(project.path);
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
  // Models

  /** One services instance for the user's agentDir, so models registered by user extensions are listed too. */
  #models(): { promise: Promise<AgentSessionServices>; cached: boolean } {
    const cached = this.#modelServices !== undefined;
    this.#modelServices ??= createAgentSessionServices({ cwd: homedir(), agentDir: getAgentDir() }).catch((error: unknown) => {
      this.#modelServices = undefined;
      throw error;
    });
    return { promise: this.#modelServices, cached };
  }

  #invalidateCaches(projectId?: string): void {
    const models = this.#modelServices;
    this.#modelServices = undefined;
    if (models) void settleServices(models);
    if (projectId) {
      const cached = this.#projectServices.get(projectId);
      if (cached) {
        this.#projectServices.delete(projectId);
        void settleServices(cached.promise);
      }
    }
  }

  async listModels(): Promise<{ models: ModelInfo[]; defaultModelId?: string }> {
    const { promise, cached } = this.#models();
    const services = await promise;
    if (cached) {
      // Pick up credentials or default model changes made via the `pi` CLI without restarting core.
      await services.settingsManager.reload();
      await services.modelRuntime.refresh({ allowNetwork: false });
    }
    const models = services.modelRuntime.getAvailableSnapshot().map(
      (model): ModelInfo => ({
        id: `${model.provider}/${model.id}`,
        provider: model.provider,
        label: model.name,
        reasoning: model.reasoning,
        thinkingLevels: getSupportedThinkingLevels(model),
        defaultThinkingLevel: clampThinkingLevel(model, services.settingsManager.getModelThinkingLevel(model.provider, model.id) ?? services.settingsManager.getDefaultThinkingLevel() ?? "medium"),
        images: model.input.includes("image"),
        contextWindow: model.contextWindow,
      }),
    );
    const provider = services.settingsManager.getDefaultProvider();
    const modelId = services.settingsManager.getDefaultModel();
    return { models, ...(provider && modelId ? { defaultModelId: `${provider}/${modelId}` } : {}) };
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
    // Merge conservatively so concurrent sub-session syncs are not dropped and deleted sessions/projects do not resurrect.
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
    // Pi records who spawned a session (pi-subagents does for every sub-agent); nest those under their parent.
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
    // A live session knows its status, pending requests and newest activity better than its file.
    for (const live of this.#live.values()) {
      if (this.#deletingSessions.has(live.id) || this.#removingProjects.has(live.projectId)) continue;
      summaries.set(live.id, live.summary());
    }
    return Array.from(summaries.values()).sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
  }

  /** Slash commands of a session, or of its project before the session exists. */
  async commands(projectId: string, sessionId?: string) {
    if (sessionId) return (await this.live(sessionId)).commands();
    const project = this.#project(projectId);
    const existingLive = Array.from(this.#live.values()).find((live) => live.projectId === project.id);
    if (existingLive) return existingLive.commands();
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
    const loader = (await entry.promise).resourceLoader;
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

  /** The live session, opening the Pi session file when needed. */
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

  async forkSubsession(sessionId: string): Promise<SessionSummary> {
    const entry = await this.#entry(sessionId);
    if (!entry.summary.readOnly) throw new UserError("只有子 Agent 会话可以用此方式继续");
    if (this.#subsessionStatus(entry) === "running") throw new UserError("请等子 Agent 结束后再继续");
    const project = this.#project(entry.projectId);
    const manager = SessionManager.forkFrom(entry.path, project.path);
    // The fork has its own id and name. The agent keeps ownership of the source.
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
    });
    const services = await createServices(project.path, live);
    try {
      let model;
      if (options.modelId) {
        model = findModel(services, options.modelId);
        if (!model) throw new UserError(`找不到模型 ${options.modelId}，或它还没有配置凭据`);
      }
      // Pi clamps a thinking level the model does not support to the nearest one it does.
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
    // An agent can start before the scheduled index sync discovers its child file.
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
    // A session that never got a message has no file, so it leaves the list with its live session.
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
    const modelServices = this.#modelServices;
    this.#modelServices = undefined;
    const projectServices = Array.from(this.#projectServices.values(), (entry) => entry.promise);
    this.#projectServices.clear();
    await Promise.all([
      ...Array.from(this.#live.values()).map((live) => this.#dispose(live, false)),
      ...projectServices.map(settleServices),
      ...(modelServices ? [settleServices(modelServices)] : []),
    ]);
  }

  // -------------------------------------------------------------------------
  // Files & Git

  async searchFiles(projectId: string, query: string, limit: number): Promise<string[]> {
    const project = this.#project(projectId);
    const files = await listFiles(project.path);
    if (!query.trim()) return files.slice(0, limit);
    return fuzzysort.go(query.trim(), files, { limit }).map((result) => result.target);
  }

  async listProjectFiles(projectId: string, refresh = false): Promise<{ files: string[]; truncated: boolean }> {
    const project = this.#project(projectId);
    const files = await listFiles(project.path, refresh);
    return {
      files: files.slice(0, TREE_FILE_LIMIT),
      truncated: files.length > TREE_FILE_LIMIT,
    };
  }

  async readProjectFile(projectId: string, rawPath: string): Promise<ProjectFileContent> {
    const project = this.#project(projectId);
    return await readPreviewFile(project.path, rawPath);
  }

  async gitStatus(projectId: string): Promise<GitStatus> {
    return readGitStatus(this.#project(projectId).path);
  }

  async gitDiff(projectId: string, path: string, staged?: boolean): Promise<GitFileDiff> {
    return readGitDiff(this.#project(projectId).path, path, staged);
  }
}

function resolveRealOrAbs(target: string): string {
  try {
    return realpathSync(target);
  } catch {
    return resolve(target);
  }
}

function isWithinDir(root: string, target: string): boolean {
  const rel = relative(root, target);
  return rel === "" || (!rel.startsWith("..") && !isAbsolute(rel));
}

async function readPreviewFile(projectRoot: string, rawPath: string): Promise<ProjectFileContent> {
  const candidate = isAbsolute(rawPath) ? resolve(rawPath) : resolve(projectRoot, rawPath);
  if (!existsSync(candidate)) throw new UserError(`找不到文件：${rawPath}`);

  let realTarget: string;
  try {
    realTarget = realpathSync(candidate);
  } catch {
    throw new UserError(`无法访问文件：${rawPath}`);
  }

  const realProject = resolveRealOrAbs(projectRoot);
  const realAgent = resolveRealOrAbs(getAgentDir());
  const inProject = isWithinDir(realProject, realTarget);
  const inAgent = isWithinDir(realAgent, realTarget);
  if (!inProject && !inAgent) {
    throw new UserError(`不允许读取项目目录之外的文件：${rawPath}`);
  }

  const info = statSync(realTarget);
  if (!info.isFile()) throw new UserError(`只能预览文件，无法打开目录：${rawPath}`);

  const displayPath = inProject ? relative(realProject, realTarget).split(sep).join("/") : realTarget;
  const ext = extname(realTarget).toLowerCase();
  if (IMAGE_EXTS.has(ext)) {
    return {
      path: displayPath,
      fullPath: realTarget,
      size: info.size,
      isBinary: true,
      isImage: true,
      truncated: false,
      content: "",
    };
  }

  const handle = await open(realTarget, "r");
  try {
    const readSize = Math.min(info.size, FILE_PREVIEW_MAX_BYTES);
    const buffer = Buffer.allocUnsafe(readSize);
    const { bytesRead } = await handle.read(buffer, 0, readSize, 0);
    const slice = buffer.subarray(0, bytesRead);
    const probe = slice.subarray(0, Math.min(slice.length, 8_192));
    const isBinary = probe.includes(0);
    return {
      path: displayPath,
      fullPath: realTarget,
      size: info.size,
      isBinary,
      isImage: false,
      truncated: info.size > FILE_PREVIEW_MAX_BYTES,
      content: isBinary ? "" : slice.toString("utf8"),
    };
  } finally {
    await handle.close();
  }
}

/** Project files relative to its root: git's view when it is a repository, otherwise a bounded walk. */
async function listFiles(root: string, refresh = false): Promise<string[]> {
  const now = Date.now();
  const cached = !refresh ? fileListCache.get(root) : undefined;
  if (cached && now < cached.expiresAt) return cached.files;

  let files: string[];
  try {
    const { stdout } = await execFileAsync(
      "git",
      ["-c", "core.quotepath=false", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
      { cwd: root, maxBuffer: 64 * 1024 * 1024 },
    );
    files = stdout.split("\0").filter(Boolean);
  } catch {
    files = [];
    const queue = [root];
    while (queue.length > 0 && files.length < WALK_LIMIT) {
      const dir = queue.shift() as string;
      const entries = await readdir(dir, { withFileTypes: true }).catch(() => []);
      for (const entry of entries) {
        if (WALK_SKIP[entry.name]) continue;
        const path = join(dir, entry.name);
        if (entry.isDirectory()) queue.push(path);
        else if (entry.isFile()) files.push(relative(root, path));
        if (files.length >= WALK_LIMIT) break;
      }
    }
  }

  if (fileListCache.size >= MAX_FILE_CACHE_ENTRIES) {
    for (const [key, entry] of fileListCache) {
      if (now >= entry.expiresAt) fileListCache.delete(key);
    }
    if (fileListCache.size >= MAX_FILE_CACHE_ENTRIES) {
      const oldest = fileListCache.keys().next().value;
      if (oldest !== undefined) fileListCache.delete(oldest);
    }
  }
  fileListCache.set(root, { files, expiresAt: now + FILE_CACHE_TTL_MS });
  return files;
}
