import { spawn } from "node:child_process";
import { closeSync, existsSync, mkdirSync, openSync, readFileSync, realpathSync, renameSync, statSync, unlinkSync, writeFileSync } from "node:fs";
import { readFile, stat } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { dirname, extname, isAbsolute, join, relative, resolve } from "node:path";
import { app, BrowserWindow, clipboard, dialog, ipcMain, Menu, nativeTheme, protocol, screen, shell } from "electron";
import { DEFAULT_CORE_PORT, PROTOCOL_VERSION } from "@lila/protocol";

protocol.registerSchemesAsPrivileged([
  { scheme: "lila-media", privileges: { standard: true, secure: true, supportFetchAPI: true } },
]);

const port = Number(process.env.LILA_CORE_PORT ?? DEFAULT_CORE_PORT);
const coreHost = process.env.LILA_CORE_HOST || "127.0.0.1";
const coreUrlHost = coreHost.includes(":") && !coreHost.startsWith("[") ? `[${coreHost}]` : coreHost;
const coreUrl = `http://${coreUrlHost}:${port}`;
const dataDir = process.env.LILA_DATA_DIR ?? join(app.getPath("appData"), "Lila");
const useWindowVibrancy = process.platform === "darwin";
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
  return nativeTheme.shouldUseDarkColors ? "#121212" : "#fafafa";
}

type WindowState = {
  x?: number;
  y?: number;
  width: number;
  height: number;
  maximized?: boolean;
};

const DEFAULT_WINDOW_STATE: WindowState = { width: 1200, height: 800 };

function readWindowState(): WindowState {
  try {
    const raw = JSON.parse(readFileSync(join(dataDir, "window-state.json"), "utf8")) as Partial<WindowState>;
    const width = typeof raw.width === "number" && Number.isFinite(raw.width) && raw.width >= 760 ? Math.round(raw.width) : DEFAULT_WINDOW_STATE.width;
    const height = typeof raw.height === "number" && Number.isFinite(raw.height) && raw.height >= 520 ? Math.round(raw.height) : DEFAULT_WINDOW_STATE.height;
    const state: WindowState = { width, height, maximized: Boolean(raw.maximized) };
    if (typeof raw.x === "number" && Number.isFinite(raw.x) && typeof raw.y === "number" && Number.isFinite(raw.y)) {
      const bounds = { x: Math.round(raw.x), y: Math.round(raw.y), width, height };
      const visible = screen.getAllDisplays().some((display) => {
        const area = display.workArea;
        const overlapX = Math.min(bounds.x + bounds.width, area.x + area.width) - Math.max(bounds.x, area.x);
        const overlapY = Math.min(bounds.y + bounds.height, area.y + area.height) - Math.max(bounds.y, area.y);
        return overlapX >= 120 && overlapY >= 120;
      });
      if (visible) {
        state.x = bounds.x;
        state.y = bounds.y;
      }
    }
    return state;
  } catch {
    return DEFAULT_WINDOW_STATE;
  }
}

function saveWindowState(win: BrowserWindow): void {
  if (win.isDestroyed()) return;
  try {
    const maximized = win.isMaximized();
    const bounds = maximized ? win.getNormalBounds() : win.getBounds();
    mkdirSync(dataDir, { recursive: true });
    writeFileSync(
      join(dataDir, "window-state.json"),
      JSON.stringify({ x: bounds.x, y: bounds.y, width: bounds.width, height: bounds.height, maximized }, null, 2) + "\n",
    );
  } catch {
    // Best-effort window state persistence.
  }
}

function openExternalSafe(rawUrl: string): Promise<void> {
  const parsed = new URL(rawUrl);
  if (parsed.protocol !== "https:" && parsed.protocol !== "http:" && parsed.protocol !== "mailto:") {
    throw new Error(`不支持打开该协议的链接：${parsed.protocol}`);
  }
  return shell.openExternal(parsed.toString());
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
  if (!useWindowVibrancy && window && !window.isDestroyed()) {
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
async function ensureCore(forceRestart = false): Promise<void> {
  if (!forceRestart) {
    const initial = await health();
    if (initial?.protocolVersion === PROTOCOL_VERSION) return;
  }

  await withCoreLock(async () => {
    const current = await health();
    if (!forceRestart && current?.protocolVersion === PROTOCOL_VERSION) return;
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

function isAllowedSettingsConfigPath(realTarget: string): boolean {
  return (
    realTarget === resolveRealPath(join(agentDir(), "settings.json")) ||
    realTarget === resolveRealPath(join(agentDir(), "models.json"))
  );
}

function isAllowedAppNavigation(targetUrl: string): boolean {
  const devUrl = process.env.ELECTRON_RENDERER_URL;
  if (!app.isPackaged && devUrl) {
    try {
      return new URL(targetUrl).origin === new URL(devUrl).origin;
    } catch {
      return false;
    }
  }
  const indexPath = resolve(import.meta.dirname, "../renderer/index.html");
  try {
    const parsed = new URL(targetUrl);
    return parsed.protocol === "file:" && resolve(decodeURIComponent(parsed.pathname)) === indexPath;
  } catch {
    return false;
  }
}

function installApplicationMenu(): void {
  const isMac = process.platform === "darwin";
  const sendMenu = (action: "new-session" | "settings") => {
    showWindow();
    if (window && !window.isDestroyed()) {
      window.webContents.send("lila:menu-action", action);
    }
  };
  const template: Electron.MenuItemConstructorOptions[] = [
    ...(isMac
      ? [
          {
            label: app.name,
            submenu: [
              { role: "about" as const, label: "关于 Lila" },
              { type: "separator" as const },
              { label: "设置…", accelerator: "CmdOrCtrl+,", click: () => sendMenu("settings") },
              { type: "separator" as const },
              { role: "services" as const, label: "服务" },
              { type: "separator" as const },
              { role: "hide" as const, label: "隐藏 Lila" },
              { role: "hideOthers" as const, label: "隐藏其他" },
              { role: "unhide" as const, label: "全部显示" },
              { type: "separator" as const },
              { role: "quit" as const, label: "退出 Lila" },
            ],
          },
        ]
      : []),
    {
      label: "文件",
      submenu: [
        { label: "新建会话", accelerator: "CmdOrCtrl+N", click: () => sendMenu("new-session") },
        ...(isMac ? [] : [{ label: "设置…", accelerator: "CmdOrCtrl+,", click: () => sendMenu("settings") }]),
        { type: "separator" as const },
        isMac ? { role: "close" as const, label: "关闭窗口" } : { role: "quit" as const, label: "退出" },
      ],
    },
    {
      label: "编辑",
      submenu: [
        { role: "undo" as const, label: "撤销" },
        { role: "redo" as const, label: "重做" },
        { type: "separator" as const },
        { role: "cut" as const, label: "剪切" },
        { role: "copy" as const, label: "复制" },
        { role: "paste" as const, label: "粘贴" },
        { role: "selectAll" as const, label: "全选" },
      ],
    },
    {
      label: "视图",
      submenu: [
        { role: "resetZoom" as const, label: "实际大小" },
        { role: "zoomIn" as const, label: "放大" },
        { role: "zoomOut" as const, label: "缩小" },
        { type: "separator" as const },
        { role: "togglefullscreen" as const, label: "切换全屏" },
        ...(!app.isPackaged
          ? [
              { type: "separator" as const },
              { role: "reload" as const, label: "重新加载" },
              { role: "toggleDevTools" as const, label: "开发者工具" },
            ]
          : []),
      ],
    },
    {
      label: "窗口",
      submenu: [
        { role: "minimize" as const, label: "最小化" },
        { role: "zoom" as const, label: "缩放" },
        ...(isMac ? [{ type: "separator" as const }, { role: "front" as const, label: "前置全部窗口" }] : []),
      ],
    },
  ];
  Menu.setApplicationMenu(Menu.buildFromTemplate(template));
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
  const initialState = readWindowState();
  const win = new BrowserWindow({
    ...(initialState.x !== undefined && initialState.y !== undefined ? { x: initialState.x, y: initialState.y } : {}),
    width: initialState.width,
    height: initialState.height,
    minWidth: 760,
    minHeight: 520,
    show: false,
    ...(useWindowVibrancy ? { vibrancy: "sidebar" } : { backgroundColor: windowBackgroundColor() }),
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

  let persistTimer: ReturnType<typeof setTimeout> | undefined;
  const schedulePersist = () => {
    clearTimeout(persistTimer);
    persistTimer = setTimeout(() => saveWindowState(win), 300);
  };
  win.on("resize", schedulePersist);
  win.on("move", schedulePersist);
  win.on("maximize", schedulePersist);
  win.on("unmaximize", schedulePersist);

  win.once("ready-to-show", () => {
    if (initialState.maximized) win.maximize();
    win.show();
  });
  // Closing only hides the window so reopening is instant; the app keeps running in the Dock.
  win.on("close", (event) => {
    clearTimeout(persistTimer);
    saveWindowState(win);
    if (quitting || process.platform !== "darwin") return;
    event.preventDefault();
    win.hide();
  });
  win.webContents.setWindowOpenHandler(({ url }) => {
    try {
      void openExternalSafe(url);
    } catch {
      // Ignore malformed or disallowed URLs.
    }
    return { action: "deny" };
  });
  win.webContents.on("will-navigate", (event, url) => {
    if (isAllowedAppNavigation(url)) return;
    event.preventDefault();
    try {
      void openExternalSafe(url);
    } catch {
      // Ignore malformed or disallowed URLs.
    }
  });
  win.webContents.on("context-menu", (_event, params) => {
    const items: Electron.MenuItemConstructorOptions[] = [];
    if (params.linkURL) {
      items.push(
        {
          label: "在浏览器中打开链接",
          click: () => {
            try {
              void openExternalSafe(params.linkURL);
            } catch {
              // Ignore disallowed URL.
            }
          },
        },
        { label: "复制链接地址", click: () => clipboard.writeText(params.linkURL) },
        { type: "separator" },
      );
    }
    if (params.isEditable) {
      items.push(
        { role: "cut", label: "剪切", enabled: params.editFlags.canCut },
        { role: "copy", label: "复制", enabled: params.editFlags.canCopy },
        { role: "paste", label: "粘贴", enabled: params.editFlags.canPaste },
        { type: "separator" },
        { role: "selectAll", label: "全选", enabled: params.editFlags.canSelectAll },
      );
    } else if (params.selectionText.trim()) {
      items.push({ role: "copy", label: "复制" });
    }
    if (!items.length) return;
    Menu.buildFromTemplate(items).popup({ window: win });
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

ipcMain.handle("lila:pick-directory", async (_event, purpose?: "project") => {
  const options: Electron.OpenDialogOptions = {
    title: purpose === "project" ? "选择项目目录" : "选择目录",
    properties: ["openDirectory", "createDirectory"],
  };
  const result = window ? await dialog.showOpenDialog(window, options) : await dialog.showOpenDialog(options);
  const path = result.canceled ? undefined : result.filePaths[0];
  if (!path || purpose !== "project") return path;

  const trustOptions: Electron.MessageBoxOptions = {
    type: "question",
    title: "信任项目",
    message: "信任并添加此项目？",
    detail: `${path}

Lila 会读取该目录的 .pi 配置，并加载其中的扩展和 MCP 服务器。请确认你信任此目录的来源。`,
    buttons: ["取消", "信任并添加"],
    defaultId: 0,
    cancelId: 0,
  };
  const trust = window ? await dialog.showMessageBox(window, trustOptions) : await dialog.showMessageBox(trustOptions);
  return trust.response === 1 ? path : undefined;
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
  } else if (isAllowedSettingsConfigPath(realTarget)) {
    mkdirSync(dirname(realTarget), { recursive: true });
    writeFileSync(realTarget, "{}\n");
  }
  const error = await shell.openPath(realTarget);
  if (error) throw new Error(error);
});

ipcMain.handle("lila:reveal-path", async (_event, path: string): Promise<{ ok: boolean; error?: string }> => {
  const realTarget = resolveRealPath(path);
  if (!isAllowedPath(realTarget, true)) return { ok: false, error: `不允许定位该路径：${path}` };
  if (!existsSync(realTarget)) return { ok: false, error: "文件尚未创建" };
  shell.showItemInFolder(realTarget);
  return { ok: true };
});

ipcMain.handle("lila:open-external", async (_event, url: string) => {
  await openExternalSafe(url);
});

ipcMain.handle("lila:restart-core", async () => {
  ensuring = ensureCore(true).finally(() => {
    ensuring = undefined;
  });
  await ensuring;
});

ipcMain.handle("lila:app-info", () => ({
  appVersion: app.getVersion(),
  electronVersion: process.versions.electron,
  platform: process.platform,
  arch: process.arch,
}));

ipcMain.on("lila:set-badge", (_event, count: unknown) => {
  if (process.platform !== "darwin" || !app.dock) return;
  const num = typeof count === "number" && Number.isFinite(count) && count > 0 ? Math.floor(count) : 0;
  app.dock.setBadge(num > 0 ? String(num) : "");
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

const singleInstanceLock = app.requestSingleInstanceLock();
if (!singleInstanceLock) {
  app.quit();
} else {
  app.on("second-instance", () => showWindow());

  void app.whenReady().then(() => {
    applyTheme(readSavedTheme(), false);
    if (!useWindowVibrancy) {
      nativeTheme.on("updated", () => {
        if (window && !window.isDestroyed()) window.setBackgroundColor(windowBackgroundColor());
      });
    }
    installApplicationMenu();
    registerMediaProtocol();
    createWindow();
    app.on("activate", showWindow);
  });
}

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") app.quit();
});
