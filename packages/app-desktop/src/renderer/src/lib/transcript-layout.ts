import type { TranscriptItem } from "@lila/protocol";

export type FileChange = { itemId: string; path: string } & (
  | { kind: "patch"; patch: string }
  | { kind: "write"; content: string }
);

export type AgentToolItem = Extract<TranscriptItem, { kind: "tool" }>;

export type TranscriptBlock =
  | { kind: "item"; id: string; item: TranscriptItem; showFooter?: boolean; turnChanges?: FileChange[] }
  | { kind: "activity"; id: string; items: TranscriptItem[] }
  | { kind: "agents"; id: string; tools: AgentToolItem[] }
  | { kind: "changes"; id: string; changes: FileChange[] };

function isSubagentTool(item: TranscriptItem): item is AgentToolItem {
  return item.kind === "tool" && Boolean(item.subagent);
}

function isRoutineActivity(item: TranscriptItem): boolean {
  if (item.kind === "tool") return item.status !== "error" && item.name !== "ask_user" && !item.subagent;
  if (item.kind === "assistant") return Boolean(item.thinking && !item.text && !item.error && !item.streaming);
  return false;
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
  let agents: AgentToolItem[] = [];
  let turnChanges: FileChange[] = [];
  let lastReplyBlock: Extract<TranscriptBlock, { kind: "item" }> | undefined;

  function flushActivity() {
    const first = activity[0];
    if (first) blocks.push({ kind: "activity", id: `activity:${first.id}`, items: activity });
    activity = [];
  }

  function flushAgents() {
    const first = agents[0];
    if (!first) return;
    if (agents.length === 1) blocks.push({ kind: "item", id: first.id, item: first });
    else blocks.push({ kind: "agents", id: `agents:${first.id}`, tools: agents });
    agents = [];
  }

  function finalizeTurn() {
    const completedReply = lastReplyBlock && lastReplyBlock.item.kind === "assistant" && !lastReplyBlock.item.streaming
      ? lastReplyBlock
      : undefined;
    if (completedReply) {
      completedReply.showFooter = true;
      if (turnChanges.length) completedReply.turnChanges = turnChanges;
    } else if (turnChanges.length && !(lastReplyBlock && lastReplyBlock.item.kind === "assistant" && lastReplyBlock.item.streaming)) {
      const first = turnChanges[0]!;
      blocks.push({ kind: "changes", id: `changes:${first.itemId}`, changes: turnChanges });
    }
    turnChanges = [];
    lastReplyBlock = undefined;
  }

  for (const item of items) {
    if (item.kind === "user") {
      flushActivity();
      flushAgents();
      finalizeTurn();
    } else if (item.kind === "run") {
      flushActivity();
      flushAgents();
    }
    if (item.kind === "run" && item.status === "completed") continue;
    if (isSubagentTool(item)) {
      flushActivity();
      agents.push(item);
    } else if (isRoutineActivity(item)) {
      flushAgents();
      activity.push(item);
    } else {
      flushActivity();
      flushAgents();
      const block: Extract<TranscriptBlock, { kind: "item" }> = { kind: "item", id: item.id, item };
      if (item.kind === "assistant" && item.text) {
        block.showFooter = false;
        lastReplyBlock = block;
      }
      blocks.push(block);
    }
    const change = fileChange(item, projectPrefix);
    if (change) turnChanges.push(change);
  }
  flushActivity();
  flushAgents();
  finalizeTurn();

  return blocks;
}
