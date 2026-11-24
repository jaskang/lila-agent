import { spawn } from "node:child_process";
import { closeSync, existsSync, mkdirSync, openSync, readFileSync, realpathSync, renameSync, statSync, unlinkSync, writeFileSync } from "node:fs";
import { readFile, stat } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { dirname, extname, isAbsolute, join, relative, resolve } from "node:path";
import { app, BrowserWindow, dialog, ipcMain, nativeTheme, protocol, shell } from "electron";
import { DEFAULT_CORE_PORT, PROTOCOL_VERSION } from "@lila/protocol";

protocol.registerSchemesAsPrivileged([
  { scheme: "lila-media", privileges: { standard: true, secure: true, supportFetchAPI: true } },
]);

const port = Number(process.env.LILA_CORE_PORT ?? DEFAULT_CORE_PORT);
const coreHost = process.env.LILA_CORE_HOST || "127.0.0.1";
const coreUrlHost = coreHost.includes(":") && !coreHost.startsWith("[") ? `[${coreHost}]` : coreHost;
const coreUrl = `http://${coreUrlHost}:${port}`;
const dataDir = process.env.LILA_DATA_DIR ?? join(app.getPath("appData"), "Lila");
const MAX_LOG_BYTES = 5 * 1024 * 1024;
const MAX_MEDIA_BYTES = 20 * 1024 * 1024;
const LOCK_STALE_MS = 30_000;
const BLOCKED_OPEN_EXTS = new Set([
  ".workflow",
  ".scpt",
  ".applescript",
  ".terminal",
  ".inetloc",
  ".webloc",
  ".jar",
  ".app",
  ".command",
  ".sh",
  ".bash",
  ".zsh",
  ".exe",
  ".bat",
  ".cmd",
  ".dmg",
  ".pkg",
]);
const MEDIA_MIME: Record<string, string> = {
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".gif": "image/gif",
  ".webp": "image/webp",
  ".svg": "image/svg+xml",
  ".avif": "image/avif",
  ".bmp": "image/bmp",
  ".ico": "image/x-icon",
};

let window: BrowserWindow | undefined;
let quitting = false;
let ensuring: Promise<void> | undefined;

type ThemeMode = "system" | "light" | "dark";

function isThemeMode(value: unknown): value is ThemeMode {
  return value === "system" || value === "light" || value === "dark";
}

function windowBackgroundColor(): string {
  return nativeTheme.shouldUseDarkColors ? "#171717" : "#ffffff";
}

function readSavedTheme(): ThemeMode {
  try {
    const raw = readFileSync(join(dataDir, "theme"), "utf8").trim();
    if (isThemeMode(raw)) return raw;
  } catch {
    // Missing or unreadable theme preference defaults to system.
  }
  return "system";
}

function applyTheme(mode: ThemeMode, persist: boolean): void {
  nativeTheme.themeSource = mode;
  if (window && !window.isDestroyed()) {
    window.setBackgroundColor(windowBackgroundColor());
  }
  if (!persist) return;
  try {
    mkdirSync(dataDir, { recursive: true });
    writeFileSync(join(dataDir, "theme"), `${mode}\n`);
  } catch {
    // Best-effort theme persistence.
  }
}

type Health = { ok: boolean; app?: string; protocolVersion?: number; pid?: number };

async function health(): Promise<Health | undefined> {
  try {
    const response = await fetch(`${coreUrl}/health`, { signal: AbortSignal.timeout(1_000) });
    return response.ok ? ((await response.json()) as Health) : undefined;
  } catch {
    return undefined;
  }
}

async function waitFor(check: () => Promise<boolean>, timeoutMs: number): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await check()) return true;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  return false;
}

function readRecordedPid(): number | undefined {
  const pidPath = join(dataDir, "core.pid");
  if (!existsSync(pidPath)) return undefined;
  const parsed = Number(readFileSync(pidPath, "utf8").trim());
  return Number.isInteger(parsed) && parsed > 0 ? parsed : undefined;
}

function isOwnedCore(current: Health): boolean {
  if (current.app === "lila-core") return true;
  const recorded = readRecordedPid();
  return recorded !== undefined && current.pid === recorded;
}

function rotateLogIfNeeded(logPath: string): void {
  try {
    if (existsSync(logPath) && statSync(logPath).size >= MAX_LOG_BYTES) {
      renameSync(logPath, `${logPath}.1`);
    }
  } catch {
    // Best-effort log rotation.
  }
}

async function withCoreLock<T>(run: () => Promise<T>): Promise<T> {
  mkdirSync(dataDir, { recursive: true });
  const lockPath = join(dataDir, "core.lock");
  const deadline = Date.now() + 15_000;
  let acquired = false;
  while (Date.now() < deadline) {
    try {
      const fd = openSync(lockPath, "wx", 0o600);
      writeFileSync(fd, `${process.pid}\n`);
      closeSync(fd);
      acquired = true;
      break;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
      try {
        if (Date.now() - statSync(lockPath).mtimeMs > LOCK_STALE_MS) unlinkSync(lockPath);
      } catch {
        // Another instance just released or updated the lock.
      }
      await new Promise((r) => setTimeout(r, 100));
    }
  }
  if (!acquired) {
    throw new Error(`获取 core 启动锁超时：${lockPath}`);
  }
  try {
    return await run();
  } finally {
    try {
      unlinkSync(lockPath);
    } catch {
      // Ignore cleanup failure.
    }
  }
}

/** Connects to a running core, or starts one that outlives this app. */
async function ensureCore(): Promise<void> {
  const initial = await health();
  if (initial?.protocolVersion === PROTOCOL_VERSION) return;

  await withCoreLock(async () => {
    const current = await health();
    if (current?.protocolVersion === PROTOCOL_VERSION) return;
    if (current?.pid) {
      if (!isOwnedCore(current)) {
        throw new Error(`端口 ${port} 已被其他进程占用（pid ${current.pid}），无法启动 Lila core`);
      }
      // A core from an older build is still running and cannot talk to this window; replace it.
      let exited = false;
      try {
        process.kill(current.pid, "SIGTERM");
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code === "ESRCH") exited = true;
        else throw error;
      }
      if (!exited) {
        exited = await waitFor(async () => !(await health()), 5_000);
      }
      if (!exited) {
        try {
          process.kill(current.pid, "SIGKILL");
        } catch (error) {
          if ((error as NodeJS.ErrnoException).code === "ESRCH") exited = true;
          else throw error;
        }
        if (!exited) {
          exited = await waitFor(async () => !(await health()), 2_000);
        }
      }
      if (!exited) {
        throw new Error(`旧的 Lila core（pid ${current.pid}）未在规定时间内退出，已中止启动`);
      }
    }

    // Core is plain TypeScript run by this app's own Node (Node strips the types), so it ships as files.
    const coreDir = app.isPackaged ? join(process.resourcesPath, "core") : join(app.getAppPath(), "../core");
    mkdirSync(dataDir, { recursive: true });
    const logPath = join(dataDir, "core.log");
    rotateLogIfNeeded(logPath);
    const log = openSync(logPath, "a");
    try {
      const child = spawn(process.execPath, [join(coreDir, "src", "main.ts")], {
        cwd: coreDir,
        detached: true,
        env: { ...process.env, ELECTRON_RUN_AS_NODE: "1", LILA_CORE_HOST: coreHost, LILA_CORE_PORT: String(port), LILA_DATA_DIR: dataDir },
        stdio: ["ignore", log, log],
      });
      child.unref();
    } finally {
      closeSync(log);
    }

    if (!(await waitFor(async () => (await health())?.protocolVersion === PROTOCOL_VERSION, 10_000))) {
      throw new Error(`core 没有在 ${coreUrl} 启动，日志见 ${logPath}`);
    }
  });
}

function agentDir(): string {
  return resolve(process.env.PI_CODING_AGENT_DIR || join(homedir(), ".pi", "agent"));
}

function registeredProjectPaths(): string[] {
  const statePath = join(dataDir, "state.json");
  if (!existsSync(statePath)) return [];
  try {
    const parsed = JSON.parse(readFileSync(statePath, "utf8")) as { projects?: Array<{ path?: unknown }> };
    return Array.isArray(parsed.projects)
      ? parsed.projects.map((project) => project.path).filter((path): path is string => typeof path === "string" && path.length > 0)
      : [];
  } catch {
    return [];
  }
}

function isWithin(root: string, target: string): boolean {
  const rel = relative(resolve(root), resolve(target));
  return rel === "" || (!rel.startsWith("..") && !isAbsolute(rel));
}

function resolveReal(target: string): string {
  try {
    return realpathSync(target);
  } catch {
    return resolve(target);
  }
}

/** Resolves the realpath of an existing target or of its nearest existing ancestor (prevents symlink escape). */
function resolveRealPath(target: string): string {
  const resolved = resolve(target);
  try {
    if (existsSync(resolved)) return realpathSync(resolved);
    const missing: string[] = [];
    let cursor = resolved;
    while (!existsSync(cursor)) {
      const parent = dirname(cursor);
      if (parent === cursor) return resolved;
      missing.unshift(relative(parent, cursor));
      cursor = parent;
    }
    return resolve(realpathSync(cursor), ...missing);
  } catch {
    return resolved;
  }
}

function allowedRoots(includeBundle: boolean): string[] {
  const roots = [agentDir(), join(homedir(), ".agents"), ...registeredProjectPaths()];
  if (includeBundle) {
    roots.push(app.getAppPath(), process.resourcesPath);
    if (!app.isPackaged) roots.push(resolve(app.getAppPath(), "../.."));
  }
  return roots.map(resolveReal);
}

function allowedLogFiles(): string[] {
  return [join(dataDir, "core.log"), join(dataDir, "core.log.1")].map(resolveRealPath);
}

function isAllowedPath(realTarget: string, includeBundle: boolean): boolean {
  if (allowedRoots(includeBundle).some((root) => isWithin(root, realTarget))) return true;
  return allowedLogFiles().includes(realTarget);
}

function isAllowedMcpConfigPath(realTarget: string): boolean {
  if (realTarget === resolveRealPath(join(agentDir(), "mcp.json"))) return true;
  return registeredProjectPaths().some((projectPath) => realTarget === resolveRealPath(join(projectPath, ".pi", "mcp.json")));
}

function showWindow(): void {
  if (!window || window.isDestroyed()) {
    createWindow();
    return;
  }
  if (window.isMinimized()) window.restore();
  window.show();
  window.focus();
}

function createWindow(): void {
  const win = new BrowserWindow({
    width: 1200,
    height: 800,
    minWidth: 760,
    minHeight: 520,
    show: false,
    backgroundColor: windowBackgroundColor(),
    title: "Lila",
    titleBarStyle: "hiddenInset",
    trafficLightPosition: { x: 18, y: 20 },
    webPreferences: {
      preload: join(import.meta.dirname, "../preload/index.cjs"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });
  window = win;

  win.once("ready-to-show", () => win.show());
  // Closing only hides the window so reopening is instant; the app keeps running in the Dock.
  win.on("close", (event) => {
    if (quitting || process.platform !== "darwin") return;
    event.preventDefault();
    win.hide();
  });
  win.webContents.setWindowOpenHandler(({ url }) => {
    try {
      const parsed = new URL(url);
      if (parsed.protocol === "https:" || parsed.protocol === "http:" || parsed.protocol === "mailto:") {
        void shell.openExternal(parsed.toString());
      }
    } catch {
      // Ignore malformed URLs.
    }
    return { action: "deny" };
  });

  const devUrl = process.env.ELECTRON_RENDERER_URL;
  if (!app.isPackaged && devUrl) void win.loadURL(devUrl);
  else void win.loadFile(join(import.meta.dirname, "../renderer/index.html"));
}

ipcMain.handle("lila:connection", async () => {
  ensuring ??= ensureCore().finally(() => {
    ensuring = undefined;
  });
  await ensuring;
  const token = readFileSync(join(dataDir, "core-token"), "utf8").trim();
  return { url: `ws://${coreUrlHost}:${port}/ws`, token };
});

ipcMain.handle("lila:pick-directory", async () => {
  const options: Electron.OpenDialogOptions = { title: "选择项目目录", properties: ["openDirectory", "createDirectory"] };
  const result = window ? await dialog.showOpenDialog(window, options) : await dialog.showOpenDialog(options);
  return result.canceled ? undefined : result.filePaths[0];
});

ipcMain.handle("lila:open-path", async (_event, path: string) => {
  const resolved = resolve(path);
  const realTarget = resolveRealPath(resolved);
  if (!isAllowedPath(realTarget, false)) {
    throw new Error(`不允许打开该路径：${resolved}`);
  }
  if (BLOCKED_OPEN_EXTS.has(extname(resolved).toLowerCase()) || BLOCKED_OPEN_EXTS.has(extname(realTarget).toLowerCase())) {
    throw new Error(`不允许直接打开可执行文件：${resolved}`);
  }
  if (existsSync(realTarget)) {
    const info = statSync(realTarget);
    if (info.isFile() && (info.mode & 0o111) !== 0) {
      throw new Error(`不允许直接打开可执行文件：${resolved}`);
    }
  } else if (isAllowedMcpConfigPath(realTarget)) {
    // Pi reads MCP servers from mcp.json; give the user an empty one to fill in.
    mkdirSync(dirname(realTarget), { recursive: true });
    writeFileSync(realTarget, '{"mcpServers":{}}\n');
  }
  const error = await shell.openPath(realTarget);
  if (error) throw new Error(error);
});

ipcMain.on("lila:reveal-path", (_event, path: string) => {
  const realTarget = resolveRealPath(path);
  if (!isAllowedPath(realTarget, true)) return;
  shell.showItemInFolder(realTarget);
});

ipcMain.on("lila:focus", () => showWindow());

ipcMain.on("lila:set-theme", (_event, mode: unknown) => {
  if (isThemeMode(mode)) applyTheme(mode, true);
});

function resolveAllowedMedia(rawPath: string, rawBase: string | null): { path: string; mime: string } | undefined {
  const candidates: string[] = [];
  if (isAbsolute(rawPath)) {
    candidates.push(resolve(rawPath));
    // Web projects often reference files under public/ with a leading slash (e.g. /logo.svg).
    if (rawBase && isAbsolute(rawBase)) {
      const trimmed = rawPath.replace(/^\/+/, "");
      if (trimmed) {
        candidates.push(resolve(rawBase, trimmed));
        candidates.push(resolve(rawBase, "public", trimmed));
      }
    }
  } else if (rawBase && isAbsolute(rawBase)) {
    candidates.push(resolve(rawBase, rawPath));
  }

  const roots = allowedRoots(false);
  const lilaTmpDir = join(tmpdir(), "lila");
  if (existsSync(lilaTmpDir)) roots.push(resolveReal(lilaTmpDir));

  for (const candidate of candidates) {
    const mime = MEDIA_MIME[extname(candidate).toLowerCase()];
    if (!mime || !existsSync(candidate)) continue;
    try {
      const realPath = realpathSync(candidate);
      if (roots.some((root) => isWithin(root, realPath))) return { path: realPath, mime };
    } catch {
      // Ignore unreadable candidate and try fallback.
    }
  }
  return undefined;
}

function registerMediaProtocol(): void {
  protocol.handle("lila-media", async (request) => {
    if (request.method !== "GET") return new Response("Method Not Allowed", { status: 405 });
    const url = new URL(request.url);
    const rawPath = url.searchParams.get("path");
    if (!rawPath) return new Response("Bad Request", { status: 400 });
    const resolved = resolveAllowedMedia(rawPath, url.searchParams.get("base"));
    if (!resolved) return new Response("Not Found", { status: 404 });
    try {
      const info = await stat(resolved.path);
      if (!info.isFile()) return new Response("Not Found", { status: 404 });
      if (info.size > MAX_MEDIA_BYTES) return new Response("Payload Too Large", { status: 413 });
      const data = await readFile(resolved.path);
      return new Response(data, {
        status: 200,
        headers: {
          "Content-Type": resolved.mime,
          "Cache-Control": "no-cache",
          "Content-Security-Policy": "sandbox",
          "X-Content-Type-Options": "nosniff",
        },
      });
    } catch {
      return new Response("Not Found", { status: 404 });
    }
  });
}

app.on("before-quit", () => {
  quitting = true;
});

void app.whenReady().then(() => {
  applyTheme(readSavedTheme(), false);
  nativeTheme.on("updated", () => {
    if (window && !window.isDestroyed()) window.setBackgroundColor(windowBackgroundColor());
  });
  registerMediaProtocol();
  createWindow();
  app.on("activate", showWindow);
});

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") app.quit();
});
