import { createRequire } from "node:module";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import {
  type AgentSessionServices,
  createAgentSessionServices,
  createCodemodeExtension,
  createMcpExtension,
  createToolSearchExtension,
  DefaultPackageManager,
  getAgentDir,
  type LoadExtensionsResult,
} from "@earendil-works/pi-coding-agent";
import type {
  CapabilitiesSnapshot,
  McpConfigSnapshot,
  McpExposure,
  McpImportEntry,
  McpScope,
  McpServerInput,
  MemorySnapshot,
  MemoryTarget,
  PackageScope,
  Project,
} from "@lila/protocol";
import { UserError } from "./errors.ts";
import { createLilaExtension } from "./lila-extension.ts";
import type { LiveSession } from "./live-session.ts";
import { loadMcpSnapshot, patchMcpServer, removeMcpServer, upsertMcpServers } from "./mcp-config.ts";
import {
  addMemoryEntry,
  createBuiltinMemoryExtension,
  type MemoryConfigPatch,
  readMemorySettings,
  readMemorySnapshot,
  removeMemoryEntry,
  replaceMemoryEntry,
  updateMemoryConfig,
} from "./memory.ts";

/** pi-subagents' extension entry, loaded unless the user installed the package into Pi themselves. */
const SUBAGENTS_ENTRY = join(dirname(createRequire(import.meta.url).resolve("@tintinweb/pi-subagents/package.json")), "src", "index.ts");
const BUILTIN_MEMORY_NAME = "pi-memory";

export function isExternalMemoryExtension(extension: { path: string; tools: ReadonlyMap<string, unknown> }): boolean {
  if (!extension.path.includes("/") && !extension.path.includes("\\")) return false;
  return (
    /pi-(?:hermes-|observational-)?memory/i.test(extension.path) ||
    extension.tools.has("memory_add") ||
    extension.tools.has("memory_write")
  );
}

function isBuiltinMemoryExtension(extension: { path: string }): boolean {
  return extension.path === `builtin:${BUILTIN_MEMORY_NAME}` || extension.path === BUILTIN_MEMORY_NAME;
}

function withoutDuplicateBuiltins(result: LoadExtensionsResult): LoadExtensionsResult {
  let { extensions, errors } = result;
  const userSubagents = extensions.some((extension) => extension.path !== SUBAGENTS_ENTRY && extension.path.includes("pi-subagents"));
  if (userSubagents) {
    extensions = extensions.filter((extension) => extension.path !== SUBAGENTS_ENTRY);
    errors = errors.filter((error) => error.path !== SUBAGENTS_ENTRY && !error.error.includes(SUBAGENTS_ENTRY));
  }
  const userMemory = extensions.some((extension) => isExternalMemoryExtension(extension));
  if (userMemory) {
    extensions = extensions.filter((extension) => !isBuiltinMemoryExtension(extension));
    errors = errors.filter((error) => error.path !== BUILTIN_MEMORY_NAME);
  }
  if (extensions === result.extensions && errors === result.errors) return result;
  return { ...result, extensions, errors };
}

/** Pi services for a project: its skills, prompts, AGENTS.md, the user's extensions, and Lila's built-ins. */
export function createServices(cwd: string, live?: LiveSession): Promise<AgentSessionServices> {
  const agentDir = getAgentDir();
  const memoryEnabled = readMemorySettings(agentDir).enabled;
  return createAgentSessionServices({
    cwd,
    agentDir,
    resourceLoaderOptions: {
      additionalExtensionPaths: [SUBAGENTS_ENTRY],
      extensionFactories: [
        { name: "codemode", factory: createCodemodeExtension(), replaceable: true, builtin: true },
        { name: "tool-search", factory: createToolSearchExtension(), replaceable: true, builtin: true },
        { name: "mcp", factory: createMcpExtension(), replaceable: true, builtin: true },
        ...(memoryEnabled
          ? [{ name: BUILTIN_MEMORY_NAME, factory: createBuiltinMemoryExtension(agentDir), replaceable: true, builtin: true }]
          : []),
        ...(live ? [{ name: "lila", factory: createLilaExtension(live, cwd), hidden: true }] : []),
      ],
      extensionsOverride: withoutDuplicateBuiltins,
    },
    resourceLoaderReloadOptions: { resolveProjectTrust: async () => true },
  });
}

type ExtensionsHostDeps = {
  project: (projectId: string) => Project;
  servicesForCapabilities: (projectId?: string, sessionId?: string) => Promise<{ services: AgentSessionServices; cwd?: string }>;
  invalidateProjectServices: (projectId?: string) => void;
  reloadIdleSessions: (projectId?: string, sessionId?: string) => Promise<void>;
};

export class ExtensionsService {
  readonly #deps: ExtensionsHostDeps;

  constructor(deps: ExtensionsHostDeps) {
    this.#deps = deps;
  }

  #resolveMcpTargetPath(scope: McpScope, projectId?: string): { targetPath: string; projectDir?: string } {
    const project = projectId ? this.#deps.project(projectId) : undefined;
    if (scope === "project") {
      if (!project) throw new UserError("请先选择一个项目后再配置项目级 MCP");
      return { targetPath: join(project.path, ".pi", "mcp.json"), projectDir: project.path };
    }
    return { targetPath: join(getAgentDir(), "mcp.json"), ...(project ? { projectDir: project.path } : {}) };
  }

  listMcp(projectId?: string): McpConfigSnapshot {
    const project = projectId ? this.#deps.project(projectId) : undefined;
    return loadMcpSnapshot(getAgentDir(), project?.path);
  }

  upsertMcp(scope: McpScope, name: string, config: McpServerInput, projectId?: string): Promise<McpConfigSnapshot> {
    return this.importMcp(scope, [{ name, config }], projectId);
  }

  async importMcp(scope: McpScope, entries: readonly McpImportEntry[], projectId?: string): Promise<McpConfigSnapshot> {
    const { targetPath, projectDir } = this.#resolveMcpTargetPath(scope, projectId);
    upsertMcpServers(targetPath, entries);
    this.#deps.invalidateProjectServices(scope === "project" ? projectId : undefined);
    return loadMcpSnapshot(getAgentDir(), projectDir);
  }

  async toggleMcp(
    scope: McpScope,
    name: string,
    patch: { enabled?: boolean | undefined; exposure?: McpExposure | undefined },
    projectId?: string,
  ): Promise<McpConfigSnapshot> {
    const { targetPath, projectDir } = this.#resolveMcpTargetPath(scope, projectId);
    patchMcpServer(targetPath, name, patch);
    this.#deps.invalidateProjectServices(scope === "project" ? projectId : undefined);
    return loadMcpSnapshot(getAgentDir(), projectDir);
  }

  async removeMcp(scope: McpScope, name: string, projectId?: string): Promise<McpConfigSnapshot> {
    const { targetPath, projectDir } = this.#resolveMcpTargetPath(scope, projectId);
    removeMcpServer(targetPath, name);
    this.#deps.invalidateProjectServices(scope === "project" ? projectId : undefined);
    return loadMcpSnapshot(getAgentDir(), projectDir);
  }

  async getCapabilities(projectId?: string, sessionId?: string): Promise<CapabilitiesSnapshot> {
    const { services, cwd } = await this.#deps.servicesForCapabilities(projectId, sessionId);
    const loader = services.resourceLoader;
    const extensions = loader.getExtensions();
    const skills = loader.getSkills();
    const prompts = loader.getPrompts();
    const agentDir = services.agentDir;
    const packageManager = new DefaultPackageManager({
      cwd: cwd ?? homedir(),
      agentDir,
      settingsManager: services.settingsManager,
    });
    const packages = packageManager.listConfiguredPackages().map((item) => ({
      source: item.source,
      scope: item.scope,
      ...(item.installedPath ? { installedPath: item.installedPath } : {}),
    }));

    return {
      agentDir,
      ...(cwd ? { cwd } : {}),
      packages,
      skills: skills.skills.map((skill) => ({ name: skill.name, description: skill.description, path: skill.filePath, scope: skill.sourceInfo.scope })),
      prompts: prompts.prompts.map((prompt) => ({ name: prompt.name, description: prompt.description, path: prompt.filePath })),
      extensions: extensions.extensions
        .filter((extension) => !extension.hidden)
        .map((extension) => ({ path: extension.path, tools: [...extension.tools.keys()], commands: [...extension.commands.keys()] })),
      contextFiles: loader.getAgentsFiles().agentsFiles.map((file) => file.path),
      diagnostics: [
        ...services.diagnostics.map((diagnostic) => diagnostic.message),
        ...extensions.errors.map((error) => `${error.path}: ${error.error}`),
        ...skills.diagnostics.map((diagnostic) => diagnostic.message),
        ...prompts.diagnostics.map((diagnostic) => diagnostic.message),
      ],
    };
  }

  async #packageManagerFor(scope: PackageScope, projectId?: string): Promise<{ manager: DefaultPackageManager; cwd: string }> {
    const project = projectId ? this.#deps.project(projectId) : undefined;
    if (scope === "project" && !project) throw new UserError("请先选择一个项目后再安装或移除项目级扩展包");
    const cwd = project?.path ?? homedir();
    const { services } = await this.#deps.servicesForCapabilities(project?.id);
    await services.settingsManager.reload();
    return {
      cwd,
      manager: new DefaultPackageManager({ cwd, agentDir: getAgentDir(), settingsManager: services.settingsManager }),
    };
  }

  async installPackage(source: string, scope: PackageScope, projectId?: string): Promise<CapabilitiesSnapshot> {
    const { manager } = await this.#packageManagerFor(scope, projectId);
    await manager.installAndPersist(source, { local: scope === "project" });
    await this.#deps.reloadIdleSessions(scope === "project" ? projectId : undefined);
    return this.getCapabilities(projectId);
  }

  async removePackage(source: string, scope: PackageScope, projectId?: string): Promise<CapabilitiesSnapshot> {
    const { manager } = await this.#packageManagerFor(scope, projectId);
    await manager.removeAndPersist(source, { local: scope === "project" });
    await this.#deps.reloadIdleSessions(scope === "project" ? projectId : undefined);
    return this.getCapabilities(projectId);
  }

  async updatePackages(source?: string, projectId?: string): Promise<CapabilitiesSnapshot> {
    const { manager } = await this.#packageManagerFor("user", projectId);
    await manager.update(source);
    await this.#deps.reloadIdleSessions(projectId);
    return this.getCapabilities(projectId);
  }

  async #detectExternalMemoryExtension(projectId?: string, sessionId?: string): Promise<string | undefined> {
    try {
      const { services } = await this.#deps.servicesForCapabilities(projectId, sessionId);
      const found = services.resourceLoader.getExtensions().extensions.find((extension) => isExternalMemoryExtension(extension));
      return found?.path;
    } catch {
      return undefined;
    }
  }

  async getMemory(projectId?: string, sessionId?: string): Promise<MemorySnapshot> {
    const external = await this.#detectExternalMemoryExtension(projectId, sessionId);
    return readMemorySnapshot(getAgentDir(), external);
  }

  async addMemory(target: MemoryTarget, content: string): Promise<MemorySnapshot> {
    const external = await this.#detectExternalMemoryExtension();
    return addMemoryEntry(getAgentDir(), target, content, external);
  }

  async replaceMemory(target: MemoryTarget, oldText: string, newContent: string): Promise<MemorySnapshot> {
    const external = await this.#detectExternalMemoryExtension();
    return replaceMemoryEntry(getAgentDir(), target, oldText, newContent, external);
  }

  async removeMemory(target: MemoryTarget, oldText: string): Promise<MemorySnapshot> {
    const external = await this.#detectExternalMemoryExtension();
    return removeMemoryEntry(getAgentDir(), target, oldText, external);
  }

  async updateMemorySettings(patch: MemoryConfigPatch): Promise<MemorySnapshot> {
    const external = await this.#detectExternalMemoryExtension();
    const snapshot = await updateMemoryConfig(getAgentDir(), patch, external);
    this.#deps.invalidateProjectServices();
    return snapshot;
  }
}
