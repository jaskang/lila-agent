import { readFileSync } from "node:fs";
import { parseSessionEntries, SessionManager } from "@earendil-works/pi-coding-agent";
import { DEFAULT_SANDBOX_MODE, ThinkingLevel, type ApprovalMode, type SessionState, type SessionStatus } from "@lila/protocol";
import { UserError } from "./errors.ts";
import { aroundTranscriptItem, earlier, projectBranch, searchTranscript, snapshot, transcriptTurns } from "./transcript.ts";

/** Pi can repair or migrate files when opening them. Read into memory so only
 * pi-subagents can write to the original, even if it later resumes the child. */
function readSession(path: string, cwd: string): SessionManager {
  const entries = parseSessionEntries(readFileSync(path, "utf8"));
  if (entries[0]?.type !== "session") throw new UserError("子会话文件无法读取");
  return SessionManager.inMemory(cwd, undefined, entries);
}

export function readSubsessionView(path: string, cwd: string, status: SessionStatus, approvalMode: ApprovalMode) {
  const manager = readSession(path, cwd);
  const context = manager.buildSessionContext();
  const thinking = ThinkingLevel.safeParse(context.thinkingLevel);
  const branch = manager.getBranch();
  const items = () => projectBranch(branch, { running: status === "running" })
    .map((item) => item.kind === "user" ? { ...item, canRewind: false } : item);
  const state: SessionState = {
    status,
    access: "read-only",
    runActive: false,
    ...(context.model ? { modelId: `${context.model.provider}/${context.model.modelId}` } : {}),
    thinkingLevel: thinking.success ? thinking.data : "off",
    thinkingLevels: ["off"],
    approvalMode,
    sandboxMode: DEFAULT_SANDBOX_MODE,
    queue: { steering: [], followUp: [] },
    cost: 0,
    compacting: false,
    pending: [],
    statusLines: {},
  };
  return {
    open: (limit: number | undefined) => ({ ...snapshot(items(), limit), state }),
    earlier: (beforeId: string, limit: number) => earlier(items(), beforeId, limit),
    search: (query: string, offset: number) => searchTranscript(items(), query, offset),
    turns: (offset: number) => transcriptTurns(branch, offset),
    around: (itemId: string) => aroundTranscriptItem(items(), itemId),
  };
}

/** Older Lila versions could rename a child. Its parent still records the child file and Agent id. */
export function recordedSubagentIds(parentPath: string): Map<string, string> {
  const ids = new Map<string, string>();
  try {
    const entries = parseSessionEntries(readFileSync(parentPath, "utf8"));
    for (const entry of entries) {
      if (entry.type !== "custom" || entry.customType !== "subagents:record") continue;
      const data = entry.data as { sessionFile?: unknown; id?: unknown } | undefined;
      if (typeof data?.sessionFile === "string" && typeof data.id === "string") ids.set(data.sessionFile, data.id);
    }
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") console.warn("[core] could not read child session records", error);
  }
  return ids;
}
