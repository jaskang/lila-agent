import { contextBridge, ipcRenderer } from "electron";

export type ThemeMode = "system" | "light" | "dark";
export type MenuAction = "new-session" | "settings";
export type AppInfo = { appVersion: string; electronVersion: string; platform: string; arch: string };

/** The only desktop-shell capabilities the window gets; Lila's protocol itself goes over WebSocket. */
const shell = {
  windowMaterial: process.platform === "darwin" ? "vibrant" : "solid",
  connection: (): Promise<{ url: string; token: string }> => ipcRenderer.invoke("lila:connection"),
  pickDirectory: (purpose?: "project"): Promise<string | undefined> => ipcRenderer.invoke("lila:pick-directory", purpose),
  /** Opens a file (mcp.json, SKILL.md) in its default app; a missing mcp.json is created empty first. */
  openPath: (path: string): Promise<void> => ipcRenderer.invoke("lila:open-path", path),
  revealPath: (path: string): Promise<{ ok: boolean; error?: string }> => ipcRenderer.invoke("lila:reveal-path", path),
  openExternal: (url: string): Promise<void> => ipcRenderer.invoke("lila:open-external", url),
  restartCore: (): Promise<void> => ipcRenderer.invoke("lila:restart-core"),
  appInfo: (): Promise<AppInfo> => ipcRenderer.invoke("lila:app-info"),
  setBadge: (count: number): void => ipcRenderer.send("lila:set-badge", count),
  focus: (): void => ipcRenderer.send("lila:focus"),
  setTheme: (theme: ThemeMode): void => ipcRenderer.send("lila:set-theme", theme),
  onMenuAction: (listener: (action: MenuAction) => void): (() => void) => {
    const handler = (_event: Electron.IpcRendererEvent, action: MenuAction) => listener(action);
    ipcRenderer.on("lila:menu-action", handler);
    return () => ipcRenderer.off("lila:menu-action", handler);
  },
};

export type LilaShell = typeof shell;

contextBridge.exposeInMainWorld("lila", shell);
