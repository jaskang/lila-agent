import { toast } from "sonner";
import { ApprovalMode, DEFAULT_APPROVAL_MODE, type ImageInput, type SessionSummary } from "@lila/protocol";
import {
  DEFAULT_APPROVAL_KEY,
  DRAFTS_SESSION_KEY,
  type DraftImage,
  NEW_SESSION_KEY,
  type NewSessionOptions,
  NOTIFICATIONS_KEY,
  type State,
} from "./types.ts";

export function safeLocalGet(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

export function safeLocalSet(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Ignore storage write errors.
  }
}

export function safeLocalRemove(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch {
    // Ignore storage remove errors.
  }
}

export function readSavedDrafts(): Record<string, string> {
  try {
    const raw = JSON.parse(sessionStorage.getItem(DRAFTS_SESSION_KEY) ?? "null") as unknown;
    if (raw && typeof raw === "object" && !Array.isArray(raw)) {
      const out: Record<string, string> = {};
      for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
        if (typeof v === "string" && v) out[k] = v;
      }
      return out;
    }
  } catch {
    // Ignore broken sessionStorage entry.
  }
  return {};
}

export function persistDrafts(drafts: Record<string, string>): void {
  try {
    const nonEmpty: Record<string, string> = {};
    for (const [k, v] of Object.entries(drafts)) {
      if (v) nonEmpty[k] = v;
    }
    sessionStorage.setItem(DRAFTS_SESSION_KEY, JSON.stringify(nonEmpty));
  } catch {
    // Ignore storage quota errors.
  }
}

export function readNewSession(): Pick<State, "newSession" | "defaultApprovalMode"> {
  const savedDefault = ApprovalMode.safeParse(safeLocalGet(DEFAULT_APPROVAL_KEY));
  let newSession: NewSessionOptions = {};
  let defaultApprovalMode = DEFAULT_APPROVAL_MODE;
  try {
    const saved = JSON.parse(safeLocalGet(NEW_SESSION_KEY) ?? "null") as (NewSessionOptions & { approvalMode?: unknown }) | null;
    if (saved && typeof saved === "object") {
      const { approvalMode, ...options } = saved;
      newSession = options;
      const legacyDefault = ApprovalMode.safeParse(approvalMode);
      if (legacyDefault.success && !savedDefault.success) {
        defaultApprovalMode = legacyDefault.data;
        safeLocalSet(DEFAULT_APPROVAL_KEY, defaultApprovalMode);
      }
    }
  } catch {
    // A broken entry falls back to the defaults.
  }
  return { newSession, defaultApprovalMode: savedDefault.success ? savedDefault.data : defaultApprovalMode };
}

export function readNotificationsEnabled(): boolean {
  const saved = safeLocalGet(NOTIFICATIONS_KEY);
  if (saved === "false") return false;
  if (saved === "true") return true;
  return true;
}

export function buildChildrenByParent(sessions: Record<string, SessionSummary>): Record<string, SessionSummary[]> {
  const children: Record<string, SessionSummary[]> = {};
  for (const session of Object.values(sessions)) {
    if (session.parentId) (children[session.parentId] ??= []).push(session);
  }
  for (const list of Object.values(children)) list.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
  return children;
}

function syncDockBadge(sessions: Record<string, SessionSummary>): void {
  let total = 0;
  for (const session of Object.values(sessions)) {
    if (!session.parentId && !session.readOnly) total += session.pendingCount;
  }
  try {
    window.lila.setBadge(total);
  } catch {
    // Ignore if bridge is unavailable in tests.
  }
}

export function withSessions(sessions: Record<string, SessionSummary>): Pick<State, "sessions" | "childrenByParent"> {
  syncDockBadge(sessions);
  return { sessions, childrenByParent: buildChildrenByParent(sessions) };
}

export function toDraftImages(images: ImageInput[]): DraftImage[] {
  return images.map((image, index) => ({
    ...image,
    id: crypto.randomUUID(),
    name: `图片 ${index + 1}`,
  }));
}

export function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function report(error: unknown): void {
  toast.error(message(error));
}

export async function attempt<T>(run: () => Promise<T>): Promise<T | undefined> {
  try {
    return await run();
  } catch (error) {
    report(error);
    return undefined;
  }
}
