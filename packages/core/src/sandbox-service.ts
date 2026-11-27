import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, isAbsolute, join, resolve } from "node:path";
import {
  getDefaultWritePaths,
  realPathOf,
  SandboxManager,
  type SandboxRuntimeConfig,
} from "@anthropic-ai/sandbox-runtime";
import {
  type BashOperations,
  createLocalBashOperations,
  getAgentDir,
} from "@earendil-works/pi-coding-agent";
import type { SandboxMode, SandboxStatus } from "@lila/protocol";
import { errorMessage, UserError } from "./errors.ts";

const SANDBOX_TMP_DIR = "/tmp/claude";
const VIOLATION_OUTPUT_LIMIT = 16_000;

export function isExternalSandboxExtension(extension: { path: string; tools: ReadonlyMap<string, unknown> }): boolean {
  if (!extension.path.includes("/") && !extension.path.includes("\\")) return false;
  if (/pi-sandbox/i.test(extension.path)) return true;
  for (const name of extension.tools.keys()) {
    if (/^sandbox_/i.test(name)) return true;
  }
  return false;
}

/** Resolves the shared `.git` common directory when `projectRoot` is a git worktree. */
export function resolveWorktreeGitCommonDir(projectRoot: string): string | undefined {
  const dotGit = join(projectRoot, ".git");
  try {
    if (!statSync(dotGit).isFile()) return undefined;
    const raw = readFileSync(dotGit, "utf8");
    const match = /^gitdir:\s*(.+)$/im.exec(raw);
    const rawGitDir = match?.[1]?.trim();
    if (!rawGitDir) return undefined;
    const gitDir = isAbsolute(rawGitDir) ? rawGitDir : resolve(projectRoot, rawGitDir);
    const commonDirFile = join(gitDir, "commondir");
    if (existsSync(commonDirFile)) {
      const commonRel = readFileSync(commonDirFile, "utf8").trim();
      if (commonRel) return isAbsolute(commonRel) ? commonRel : resolve(gitDir, commonRel);
    }
    // Standard worktree layout: <common>/.git/worktrees/<name>
    const parent = dirname(gitDir);
    if (parent.endsWith(`${join("", "worktrees")}`)) return dirname(parent);
    return gitDir;
  } catch {
    return undefined;
  }
}

function uniquePaths(paths: Iterable<string | undefined>): string[] {
  const out = new Set<string>();
  for (const candidate of paths) {
    if (!candidate) continue;
    out.add(candidate);
    try {
      const resolved = realPathOf(candidate);
      if (resolved) out.add(resolved);
    } catch {
      // Non-existent path: keep the original literal path so deny rules still apply.
    }
  }
  return Array.from(out);
}

/** High-value credential paths that sandboxed processes must never read. */
export function defaultDenyReadPaths(dataDir?: string): string[] {
  const home = homedir();
  const agentDir = getAgentDir();
  return uniquePaths([
    join(home, ".ssh"),
    join(home, ".aws"),
    join(home, ".gnupg"),
    join(home, ".kube"),
    join(home, ".npmrc"),
    join(home, ".netrc"),
    join(home, ".docker", "config.json"),
    join(agentDir, "auth.json"),
    ...(dataDir ? [join(dataDir, "core-token")] : []),
  ]);
}

/** Builds a per-project filesystem-only Seatbelt/bubblewrap config (P0.1: network unrestricted). */
export function buildWorkspaceWriteConfig(projectRoot: string, dataDir?: string): Partial<SandboxRuntimeConfig> {
  const worktreeGitCommonDir = resolveWorktreeGitCommonDir(projectRoot);
  const allowWrite = uniquePaths([
    projectRoot,
    ...(worktreeGitCommonDir ? [worktreeGitCommonDir] : []),
    ...getDefaultWritePaths(),
  ]);
  const denyWrite = uniquePaths([
    join(projectRoot, ".env"),
    join(projectRoot, ".env.*"),
    join(projectRoot, ".git", "hooks"),
    join(projectRoot, ".git", "config"),
    ...(worktreeGitCommonDir
      ? [join(worktreeGitCommonDir, "hooks"), join(worktreeGitCommonDir, "config")]
      : []),
  ]);
  return {
    filesystem: {
      denyRead: defaultDenyReadPaths(dataDir),
      allowWrite,
      denyWrite,
    },
  };
}

function resolveRipgrepConfig(): { command: string } | undefined {
  const bundled = join(getAgentDir(), "bin", "rg");
  return existsSync(bundled) ? { command: bundled } : undefined;
}

function platformBackend(): SandboxStatus["backend"] {
  if (process.platform === "darwin") return "seatbelt";
  if (process.platform === "linux") return "bubblewrap";
  if (process.platform === "win32") return "windows";
  return undefined;
}

/**
 * Process-wide coordinator for `@anthropic-ai/sandbox-runtime`.
 *
 * `SandboxManager` is a module singleton, so Lila initializes it lazily once with a
 * filesystem-only base config (`network: {}` -> no proxy interception) and supplies
 * per-project filesystem boundaries via `customConfig` on every `wrapWithSandbox` call.
 */
export class SandboxService {
  readonly #dataDir?: string;
  #initPromise?: Promise<void>;
  #initialized = false;
  #disposed = false;

  constructor(dataDir?: string) {
    this.#dataDir = dataDir;
  }

  status(externalSandbox = false): SandboxStatus {
    const backend = platformBackend();
    if (externalSandbox) {
      return {
        available: true,
        ...(backend ? { backend } : {}),
        external: true,
        reason: "已检测到用户安装的外部 Pi 沙箱扩展，由外部扩展接管",
      };
    }
    if (!SandboxManager.isSupportedPlatform()) {
      return {
        available: false,
        ...(backend ? { backend } : {}),
        reason: `当前平台（${process.platform}）暂不支持内置 OS 沙箱`,
      };
    }
    const deps = SandboxManager.checkDependencies(resolveRipgrepConfig());
    if (deps.errors.length > 0) {
      return {
        available: false,
        ...(backend ? { backend } : {}),
        reason: deps.errors.join("；"),
      };
    }
    return {
      available: true,
      ...(backend ? { backend } : {}),
    };
  }

  async ensureInitialized(): Promise<void> {
    if (this.#disposed) throw new UserError("沙箱服务已关闭");
    if (this.#initialized) return;
    if (this.#initPromise) return this.#initPromise;

    const currentStatus = this.status(false);
    if (!currentStatus.available) {
      throw new UserError(`沙箱不可用（已拒绝降级裸跑）：${currentStatus.reason ?? "未知原因"}`);
    }

    this.#initPromise = (async () => {
      if (process.platform === "darwin" || process.platform === "linux") {
        // sandbox-runtime redirects TMPDIR to /tmp/claude inside Seatbelt/bwrap; ensure it exists.
        mkdirSync(SANDBOX_TMP_DIR, { recursive: true });
      }
      const rg = resolveRipgrepConfig();
      const baseConfig: SandboxRuntimeConfig = {
        // `network: {}` with omitted `allowedDomains` keeps network unrestricted (no proxy hijack).
        network: {} as SandboxRuntimeConfig["network"],
        filesystem: {
          denyRead: defaultDenyReadPaths(this.#dataDir),
          allowWrite: [],
          denyWrite: [],
        },
        ...(rg ? { ripgrep: rg } : {}),
      };
      await SandboxManager.initialize(baseConfig, undefined, true);
      this.#initialized = true;
    })().catch((error) => {
      this.#initPromise = undefined;
      throw new UserError(`初始化 OS 沙箱失败（已拒绝降级裸跑）：${errorMessage(error)}`);
    });

    return this.#initPromise;
  }

  async wrapCommand(
    command: string,
    projectRoot: string,
    options?: { binShell?: string; commandId?: string; signal?: AbortSignal },
  ): Promise<{ wrapped: string; commandId: string }> {
    await this.ensureInitialized();
    const commandId = options?.commandId ?? `lila-${randomBytes(12).toString("base64url")}`;
    const customConfig = buildWorkspaceWriteConfig(projectRoot, this.#dataDir);
    const wrapped = await SandboxManager.wrapWithSandbox(
      command,
      options?.binShell,
      customConfig,
      options?.signal,
      { commandId, commandText: command },
    );
    return { wrapped, commandId };
  }

  annotateFailure(commandId: string, output: string): string | undefined {
    if (!this.#initialized) return undefined;
    try {
      const annotated = SandboxManager.annotateStderrWithSandboxFailures(commandId, output);
      if (annotated === output) return undefined;
      return annotated.slice(output.length).trim();
    } catch {
      return undefined;
    }
  }

  cleanupAfterCommand(): void {
    if (!this.#initialized) return;
    try {
      SandboxManager.cleanupAfterCommand();
    } catch {
      // Best-effort cleanup of temp state.
    }
  }

  async dispose(): Promise<void> {
    this.#disposed = true;
    const pending = this.#initPromise;
    this.#initPromise = undefined;
    if (pending) await pending.catch(() => {});
    if (!this.#initialized) return;
    this.#initialized = false;
    await SandboxManager.reset().catch((error) => {
      console.warn("[core] SandboxManager.reset failed", error);
    });
  }
}

/**
 * Wraps Pi's `BashOperations` with Lila's per-session OS sandbox policy.
 * When `sandboxMode === "off"`, calls pass straight through to local execution.
 * When `sandboxMode === "workspace-write"`, failures to initialize/wrap fail closed.
 */
export function createSandboxedBashOperations(options: {
  sandbox: SandboxService;
  getSandboxMode: () => SandboxMode;
  projectRoot: string;
  base?: BashOperations;
  binShell?: string;
}): BashOperations {
  const base = options.base ?? createLocalBashOperations();
  return {
    exec: async (command, cwd, execOptions) => {
      if (options.getSandboxMode() === "off") {
        return base.exec(command, cwd, execOptions);
      }

      const { wrapped, commandId } = await options.sandbox.wrapCommand(command, options.projectRoot, {
        ...(options.binShell ? { binShell: options.binShell } : {}),
        ...(execOptions.signal ? { signal: execOptions.signal } : {}),
      });

      let captured = "";
      try {
        const result = await base.exec(wrapped, cwd, {
          ...execOptions,
          onData: (chunk) => {
            if (captured.length < VIOLATION_OUTPUT_LIMIT) {
              captured += chunk.toString("utf8").slice(0, VIOLATION_OUTPUT_LIMIT - captured.length);
            }
            execOptions.onData(chunk);
          },
        });

        if (result.exitCode !== 0 && !execOptions.signal?.aborted) {
          const violation = options.sandbox.annotateFailure(commandId, captured);
          if (violation) {
            execOptions.onData(Buffer.from(`\n[Lila 沙箱拦截详情]\n${violation}\n`, "utf8"));
          } else if (/operation not permitted|permission denied/i.test(captured)) {
            execOptions.onData(
              Buffer.from(
                `\n[提示] 当前会话已开启 OS 沙箱（workspace-write）：仅允许写入当前项目目录及临时目录，且禁止读取敏感凭据。如需临时放开，可在底部切换沙箱模式。\n`,
                "utf8",
              ),
            );
          }
        }
        return result;
      } finally {
        options.sandbox.cleanupAfterCommand();
      }
    },
  };
}
