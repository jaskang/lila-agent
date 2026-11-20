import { execFile } from "node:child_process";
import { randomUUID } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, renameSync, statSync, unlinkSync } from "node:fs";
import { readdir } from "node:fs/promises";
import { createRequire } from "node:module";
import { homedir } from "node:os";
import { basename, dirname, join, relative, resolve } from "node:path";
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
import type { ApprovalMode, GitFileDiff, GitStatus, ModelInfo, Project, SessionSummary, ThinkingLevel } from "@lila/protocol";
import fuzzysort from "fuzzysort";
import { UserError } from "./errors.ts";
import { gitDiff as readGitDiff, gitStatus as readGitStatus } from "./git.ts";
import { createLilaExtension } from "./lila-extension.ts";
import { type Clients, findModel, LiveSession, parseAutoSubagentName, sessionTitle, slashCommands } from "./live-session.ts";
import { Store } from "./store.ts";

const IDLE_DISPOSE_MS = 15 * 60_000;
const SWEEP_INTERVAL_MS = 60_000;
const WALK_LIMIT = 20_000;
const WALK_SKIP: Record<string, true> = { node_modules: true, ".git": true };
const execFileAsync = promisify(execFile);

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
  #index = new Map<string, IndexedSession>();
  #modelServices: Promise<AgentSessionServices> | undefined;
  /** Services of projects without a live session, for the slash menu of a session not created yet. */
  readonly #projectServices = new Map<string, CachedServices>();
  readonly #subsessionSyncTimers = new Map<string, NodeJS.Timeout>();
  readonly #sweeper: NodeJS.Timeout;

  constructor(dataDir: string, clients: Clients) {
    this.#dataDir = dataDir;
    this.#clients = clients;
    this.store = new Store(dataDir);
    this.#sweeper = setInterval(() => void this.#sweep(), SWEEP_INTERVAL_MS);
    this.#sweeper.unref();
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
    for (const live of Array.from(this.#live.values())) if (live.projectId === projectId) await this.#dispose(live);
    const cached = this.#projectServices.get(projectId);
    if (cached) {
      this.#projectServices.delete(projectId);
      await settleServices(cached.promise);
    }
    this.store.removeProject(projectId);
    this.#clients.broadcast({ type: "projects", projects: [...this.store.projects] });
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
    parentId: string,
    rawName: string | undefined,
    firstMessage: string | undefined,
  ): void {
    summary.parentId = parentId;
    const auto = parseAutoSubagentName(rawName);
    const parentLive = this.#live.get(parentId);
    const liveSub = auto && parentLive ? parentLive.subagentByPrefix(auto.idPrefix) : undefined;
    const agentId = liveSub?.id ?? auto?.idPrefix;
    if (agentId) summary.agentId = agentId;
    summary.title = sessionTitle(rawName, firstMessage, { isSubsession: true, description: liveSub?.description });
    if (liveSub && !this.#live.has(summary.id)) {
      if (liveSub.status === "running" || liveSub.status === "queued") summary.status = "running";
      else if (liveSub.status === "error" || liveSub.status === "aborted" || liveSub.status === "stopped") summary.status = "error";
      else summary.status = "idle";
    }
  }

  async listSessions(): Promise<SessionSummary[]> {
    const index = new Map<string, IndexedSession>();
    const parents = new Map<string, { parentPath: string; name?: string | undefined; firstMessage?: string | undefined }>();
    await Promise.all(
      this.store.projects.map(async (project) => {
        for (const info of await SessionManager.list(project.path)) {
          const firstMessage = info.messageCount > 0 ? info.firstMessage : undefined;
          if (info.parentSessionPath) parents.set(info.id, { parentPath: info.parentSessionPath, name: info.name, firstMessage });
          index.set(info.id, {
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
    // Pi records who spawned a session (pi-subagents does for every sub-agent); nest those under their parent.
    const idByPath = new Map(Array.from(index.values(), (entry) => [entry.path, entry.id]));
    for (const [id, meta] of parents) {
      const parentId = idByPath.get(meta.parentPath);
      const entry = index.get(id);
      if (parentId && entry) this.#enrichSubsession(entry.summary, parentId, meta.name, meta.firstMessage);
    }
    this.#index = index;
    const summaries = new Map(Array.from(index.values(), (entry) => [entry.id, entry.summary]));
    // A live session knows its status, pending requests and newest activity better than its file.
    for (const live of this.#live.values()) summaries.set(live.id, live.summary());
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
    const live = this.#live.get(sessionId);
    if (live) {
      live.touch();
      return live;
    }
    let opening = this.#opening.get(sessionId);
    if (!opening) {
      opening = this.#open(sessionId).finally(() => this.#opening.delete(sessionId));
      this.#opening.set(sessionId, opening);
    }
    return opening;
  }

  async #open(sessionId: string): Promise<LiveSession> {
    let entry = this.#index.get(sessionId);
    if (!entry || !existsSync(entry.path)) {
      await this.listSessions();
      entry = this.#index.get(sessionId);
    }
    if (!entry) throw new UserError("会话不存在，可能已经被删除");
    const project = this.#project(entry.projectId);
    const parentId = entry.summary.parentId;
    const agentId = entry.summary.agentId;
    const parentLive = parentId ? this.#live.get(parentId) : undefined;
    const subagentDescription = agentId && parentLive ? parentLive.subagentByPrefix(agentId)?.description : undefined;
    return this.#start(project, SessionManager.open(entry.path), {
      updatedAt: entry.summary.updatedAt,
      ...(parentId ? { parentId } : {}),
      ...(agentId ? { agentId } : {}),
      ...(subagentDescription ? { subagentDescription } : {}),
    });
  }

  async #start(
    project: Project,
    sessionManager: SessionManager,
    options: { modelId?: string; thinkingLevel?: ThinkingLevel; updatedAt?: string; parentId?: string; agentId?: string; subagentDescription?: string },
  ): Promise<LiveSession> {
    const live = new LiveSession({
      id: sessionManager.getSessionId(),
      projectId: project.id,
      parentId: options.parentId,
      agentId: options.agentId,
      subagentDescription: options.subagentDescription,
      store: this.store,
      clients: this.#clients,
      onReload: () => this.#invalidateCaches(project.id),
      onSubsessionsChanged: () => this.#scheduleSubsessionSync(project, sessionManager.getSessionId()),
    });
    const services = await createServices(project.path, live);
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
    await live.attach(services, session, options.updatedAt);
    this.#live.set(live.id, live);
    return live;
  }

  async deleteSession(sessionId: string): Promise<void> {
    const live = this.#live.get(sessionId);
    let path = live?.session.sessionFile ?? this.#index.get(sessionId)?.path;
    if (live) await this.#dispose(live, false);
    if (!path) {
      await this.listSessions();
      path = this.#index.get(sessionId)?.path;
    }
    if (path && existsSync(path)) {
      const trash = join(this.#dataDir, "trash");
      mkdirSync(trash, { recursive: true });
      moveToTrash(path, join(trash, basename(path)));
    }
    this.#index.delete(sessionId);
    this.store.forgetSession(sessionId);
    this.#clients.broadcast({ type: "session.removed", sessionId });
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
  }

  #scheduleSubsessionSync(project: Project, parentId: string): void {
    if (this.#subsessionSyncTimers.has(parentId)) return;
    const timer = setTimeout(() => {
      this.#subsessionSyncTimers.delete(parentId);
      void this.#syncSubsessions(project, parentId).catch((error: unknown) => {
        console.error(`[core] syncing sub-sessions for ${parentId} failed`, error);
      });
    }, 600);
    timer.unref();
    this.#subsessionSyncTimers.set(parentId, timer);
  }

  async #syncSubsessions(project: Project, parentId: string): Promise<void> {
    const parentFile = this.#live.get(parentId)?.session.sessionFile ?? this.#index.get(parentId)?.path;
    if (!parentFile) return;
    for (const info of await SessionManager.list(project.path)) {
      if (info.parentSessionPath !== parentFile) continue;
      const firstMessage = info.messageCount > 0 ? info.firstMessage : undefined;
      const liveChild = this.#live.get(info.id);
      const summary: SessionSummary = {
        id: info.id,
        projectId: project.id,
        title: sessionTitle(info.name, firstMessage),
        updatedAt: info.modified.toISOString(),
        status: liveChild?.status ?? "idle",
        pendingCount: liveChild?.summary().pendingCount ?? 0,
      };
      this.#enrichSubsession(summary, parentId, info.name, firstMessage);
      const prev = this.#index.get(info.id)?.summary;
      this.#index.set(info.id, { id: info.id, projectId: project.id, path: info.path, summary });
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

  async dispose(): Promise<void> {
    clearInterval(this.#sweeper);
    for (const timer of this.#subsessionSyncTimers.values()) clearTimeout(timer);
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

  async gitStatus(projectId: string): Promise<GitStatus> {
    return readGitStatus(this.#project(projectId).path);
  }

  async gitDiff(projectId: string, path: string, staged?: boolean): Promise<GitFileDiff> {
    return readGitDiff(this.#project(projectId).path, path, staged);
  }
}

/** Project files relative to its root: git's view when it is a repository, otherwise a bounded walk. */
async function listFiles(root: string): Promise<string[]> {
  try {
    const { stdout } = await execFileAsync("git", ["ls-files", "--cached", "--others", "--exclude-standard"], { cwd: root, maxBuffer: 64 * 1024 * 1024 });
    return stdout.split("\n").filter(Boolean);
  } catch {
    const files: string[] = [];
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
    return files;
  }
}
