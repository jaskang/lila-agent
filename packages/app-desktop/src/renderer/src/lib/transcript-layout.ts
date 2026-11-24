import type { TranscriptItem } from "@lila/protocol";

export type FileChange = { itemId: string; path: string } & (
  | { kind: "patch"; patch: string }
  | { kind: "write"; content: string }
);

export type TranscriptBlock =
  | { kind: "item"; id: string; item: TranscriptItem }
  | { kind: "activity"; id: string; items: TranscriptItem[] }
  | { kind: "changes"; id: string; changes: FileChange[] };

function isRoutineActivity(item: TranscriptItem): boolean {
  if (item.kind === "tool") return item.status !== "error" && item.name !== "ask_user";
  if (item.kind === "assistant") return Boolean(item.thinking && !item.text && !item.error && !item.streaming);
  if (item.kind !== "custom" || !item.notifications?.length) return false;
  return item.notifications.every((entry) => entry.status === "completed" || entry.status === "steered");
}

function fileChange(item: TranscriptItem, projectPrefix: string | undefined): FileChange | undefined {
  if (item.kind !== "tool" || item.status !== "done" || (item.name !== "write" && item.name !== "edit")) return;
  const rawPath = item.args.path ?? item.args.file_path;
  if (typeof rawPath !== "string" || !rawPath.trim()) return;
  const path = projectPrefix && rawPath.startsWith(projectPrefix) ? rawPath.slice(projectPrefix.length) : rawPath.replace(/^\.\//, "");
  if (item.diff) return { itemId: item.id, path, kind: "patch", patch: item.diff };
  if (item.name === "write" && typeof item.args.content === "string") {
    return { itemId: item.id, path, kind: "write", content: item.args.content };
  }
}

/** Keep messages, failures and user requests in order while folding contiguous routine steps. */
export function layoutTranscript(items: readonly TranscriptItem[], projectPath?: string): TranscriptBlock[] {
  const projectPrefix = projectPath ? `${projectPath.replace(/\/+$/, "")}/` : undefined;
  const blocks: TranscriptBlock[] = [];
  let activity: TranscriptItem[] = [];
  let changes: FileChange[] = [];
  let hasReply = false;

  function flushActivity() {
    const first = activity[0];
    if (first) blocks.push({ kind: "activity", id: `activity:${first.id}`, items: activity });
    activity = [];
  }

  function flushChanges() {
    const first = changes[0];
    if (first && hasReply) blocks.push({ kind: "changes", id: `changes:${first.itemId}`, changes });
    changes = [];
    hasReply = false;
  }

  for (const item of items) {
    if (item.kind === "user" || item.kind === "run") {
      flushActivity();
      flushChanges();
    }
    if (isRoutineActivity(item)) {
      activity.push(item);
    } else {
      flushActivity();
      blocks.push({ kind: "item", id: item.id, item });
    }
    const change = fileChange(item, projectPrefix);
    if (change) changes.push(change);
    if (item.kind === "assistant" && item.text && !item.streaming) hasReply = true;
  }
  flushActivity();
  flushChanges();
  return blocks;
}
