import { join } from "node:path";
import { spawn } from "node:child_process";
import { app, BrowserWindow, shell } from "electron";
import { DEFAULT_CORE_PORT } from "@lila/protocol";

const coreUrl = `http://127.0.0.1:${DEFAULT_CORE_PORT}`;

async function isCoreReady(): Promise<boolean> {
  try {
    const response = await fetch(`${coreUrl}/health`);
    return response.ok;
  } catch {
    return false;
  }
}

async function ensureCore(): Promise<void> {
  if (await isCoreReady()) return;

  const coreDir = app.isPackaged
    ? join(process.resourcesPath, "core")
    : join(app.getAppPath(), "../../packages/core");
  const coreCommand = app.isPackaged ? join(coreDir, "lilad") : (process.env.LILA_BUN_BIN ?? "bun");
  const coreArgs = app.isPackaged ? [] : ["run", "src/main.ts"];

  const projectPath =
    process.env.LILA_PROJECT_PATH ?? (app.isPackaged ? app.getPath("documents") : join(app.getAppPath(), "../.."));

  const child = spawn(coreCommand, coreArgs, {
    cwd: coreDir,
    detached: true,
    env: {
      ...process.env,
      LILA_CORE_PORT: String(DEFAULT_CORE_PORT),
      LILA_PROJECT_PATH: projectPath,
    },
    stdio: "ignore",
  });
  child.unref();

  const deadline = Date.now() + 5_000;
  while (Date.now() < deadline) {
    if (await isCoreReady()) return;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`core did not become ready at ${coreUrl}`);
}

function createWindow(): void {
  const win = new BrowserWindow({
    width: 1000,
    height: 720,
    show: false,
    title: "Lila",
    webPreferences: {
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });

  win.once("ready-to-show", () => win.show());

  win.webContents.setWindowOpenHandler(({ url }) => {
    void shell.openExternal(url);
    return { action: "deny" };
  });

  const devUrl = process.env.ELECTRON_RENDERER_URL;
  if (!app.isPackaged && devUrl) {
    void win.loadURL(devUrl);
  } else {
    void win.loadFile(join(import.meta.dirname, "../renderer/index.html"));
  }
}

void app.whenReady().then(async () => {
  await ensureCore();
  createWindow();
  app.on("activate", () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow();
  });
});

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") app.quit();
});
