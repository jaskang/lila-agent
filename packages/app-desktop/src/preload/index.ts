import { contextBridge, ipcRenderer } from "electron";

/** The only desktop-shell capabilities the window gets; Lila's protocol itself goes over WebSocket. */
const shell = {
  connection: (): Promise<{ url: string; token: string }> => ipcRenderer.invoke("lila:connection"),
  pickDirectory: (): Promise<string | undefined> => ipcRenderer.invoke("lila:pick-directory"),
  pickFiles: (): Promise<Array<{ name: string; mimeType: string; data: string }>> => ipcRenderer.invoke("lila:pick-files"),
  focus: (): void => ipcRenderer.send("lila:focus"),
};

export type LilaShell = typeof shell;

contextBridge.exposeInMainWorld("lila", shell);
