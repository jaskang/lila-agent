import { contextBridge, ipcRenderer } from "electron";

export type ThemeMode = "system" | "light" | "dark";

/** The only desktop-shell capabilities the window gets; Lila's protocol itself goes over WebSocket. */
const shell = {
  connection: (): Promise<{ url: string; token: string }> => ipcRenderer.invoke("lila:connection"),
  pickDirectory: (): Promise<string | undefined> => ipcRenderer.invoke("lila:pick-directory"),
  /** Opens a file (mcp.json, SKILL.md) in its default app; a missing mcp.json is created empty first. */
  openPath: (path: string): Promise<void> => ipcRenderer.invoke("lila:open-path", path),
  revealPath: (path: string): void => ipcRenderer.send("lila:reveal-path", path),
  focus: (): void => ipcRenderer.send("lila:focus"),
  setTheme: (theme: ThemeMode): void => ipcRenderer.send("lila:set-theme", theme),
};

export type LilaShell = typeof shell;

contextBridge.exposeInMainWorld("lila", shell);
