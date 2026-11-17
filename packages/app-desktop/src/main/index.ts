import { spawn } from "node:child_process";
import { mkdirSync, openSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { app, BrowserWindow, dialog, ipcMain, shell } from "electron";
import { DEFAULT_CORE_PORT, PROTOCOL_VERSION } from "@lila/protocol";

const port = Number(process.env.LILA_CORE_PORT ?? DEFAULT_CORE_PORT);
const coreUrl = `http://127.0.0.1:${port}`;
const dataDir = process.env.LILA_DATA_DIR ?? join(app.getPath("appData"), "Lila");

let window: BrowserWindow | undefined;
let quitting = false;
let ensuring: Promise<void> | undefined;

type Health = { ok: boolean; protocolVersion?: number; pid?: number };

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

/** Connects to a running core, or starts one that outlives this app. */
async function ensureCore(): Promise<void> {
  const current = await health();
  if (current?.protocolVersion === PROTOCOL_VERSION) return;
  if (current?.pid) {
    // A core from an older build is still running and cannot talk to this window; replace it.
    process.kill(current.pid, "SIGTERM");
    await waitFor(async () => !(await health()), 5_000);
  }

  const coreDir = app.isPackaged ? join(process.resourcesPath, "core") : join(app.getAppPath(), "../core");
  const command = app.isPackaged ? join(coreDir, "lilad") : (process.env.LILA_BUN_BIN ?? "bun");
  const args = app.isPackaged ? [] : ["run", "src/main.ts"];
  mkdirSync(dataDir, { recursive: true });
  const log = openSync(join(dataDir, "core.log"), "a");
  const child = spawn(command, args, {
    cwd: coreDir,
    detached: true,
    env: { ...process.env, LILA_CORE_PORT: String(port), LILA_DATA_DIR: dataDir },
    stdio: ["ignore", log, log],
  });
  child.unref();

  if (!(await waitFor(async () => (await health())?.protocolVersion === PROTOCOL_VERSION, 10_000))) {
    throw new Error(`core 没有在 ${coreUrl} 启动，日志见 ${join(dataDir, "core.log")}`);
  }
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
    title: "Lila",
    titleBarStyle: "hiddenInset",
    trafficLightPosition: { x: 14, y: 14 },
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
    void shell.openExternal(url);
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
  return { url: `ws://127.0.0.1:${port}/ws`, token };
});

ipcMain.handle("lila:pick-directory", async () => {
  const options: Electron.OpenDialogOptions = { title: "选择项目目录", properties: ["openDirectory", "createDirectory"] };
  const result = window ? await dialog.showOpenDialog(window, options) : await dialog.showOpenDialog(options);
  return result.canceled ? undefined : result.filePaths[0];
});

ipcMain.on("lila:focus", () => showWindow());

app.on("before-quit", () => {
  quitting = true;
});

void app.whenReady().then(() => {
  createWindow();
  app.on("activate", showWindow);
});

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") app.quit();
});
