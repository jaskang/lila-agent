import { parseSkillBlock, type SessionEntry } from "@earendil-works/pi-coding-agent";
import type { ImageInput, TranscriptItem } from "@lila/protocol";

/**
 * Pure projection from Pi session entries (and live agent messages) to Lila transcript items.
 * Pi's session file is the only record; these items are rebuilt whenever a client opens a session.
 */

export type AgentMessage = Extract<SessionEntry, { type: "message" }>["message"];
export type AssistantMessage = Extract<AgentMessage, { role: "assistant" }>;
export type UserMessage = Extract<AgentMessage, { role: "user" }>;
type ToolResultMessage = Extract<AgentMessage, { role: "toolResult" }>;
type CustomMessage = Extract<AgentMessage, { role: "custom" }>;
type ContentPart = { type: string; text?: string; data?: string; mimeType?: string };
export type ToolItem = Extract<TranscriptItem, { kind: "tool" }>;
export type AssistantItem = Extract<TranscriptItem, { kind: "assistant" }>;

export const DEFAULT_SNAPSHOT_LIMIT = 300;
const OUTPUT_LIMIT = 20_000;
const OUTPUT_HEAD = 4_000;
const OUTPUT_TAIL = 16_000;
const DETAILS_LIMIT = 50_000;
const USER_IMAGES_LIMIT = 4 * 1024 * 1024;

/** Keeps the head and the (usually more useful) tail of long tool output. */
export function clipOutput(text: string): string {
  if (text.length <= OUTPUT_LIMIT) return text;
  const omitted = text.length - OUTPUT_HEAD - OUTPUT_TAIL;
  return `${text.slice(0, OUTPUT_HEAD)}\n\n…（省略 ${omitted} 个字符）…\n\n${text.slice(-OUTPUT_TAIL)}`;
}

/** Text of a message content: plain string or text parts; images become a short marker. */
export function contentText(content: string | readonly ContentPart[] | undefined): string {
  if (content === undefined) return "";
  if (typeof content === "string") return content;
  return content
    .map((part) => (part.type === "text" ? (part.text ?? "") : part.type === "image" ? "[图片]" : ""))
    .filter(Boolean)
    .join("\n");
}

/** A skill invocation is stored expanded; show it the way the user typed it. */
export function displayUserText(text: string): string {
  const skill = parseSkillBlock(text);
  if (!skill) return text;
  return skill.userMessage ? `/skill:${skill.name} ${skill.userMessage}` : `/skill:${skill.name}`;
}

export function userItem(message: UserMessage): TranscriptItem {
  const parts: readonly ContentPart[] = typeof message.content === "string" ? [] : message.content;
  const text = typeof message.content === "string" ? message.content : parts.filter((part) => part.type === "text").map((part) => part.text ?? "").join("\n");
  let images: ImageInput[] = parts.flatMap((part) => (part.type === "image" && part.data && part.mimeType ? [{ mimeType: part.mimeType, data: part.data }] : []));
  if (images.reduce((total, image) => total + image.data.length, 0) >= USER_IMAGES_LIMIT) images = [];
  return { kind: "user", id: `u:${message.timestamp}`, text: displayUserText(text), images, canRewind: true };
}

export function assistantItem(message: AssistantMessage, streaming: boolean): AssistantItem {
  const text = message.content.flatMap((part) => (part.type === "text" && part.text ? [part.text] : [])).join("\n\n");
  const thinking = message.content.flatMap((part) => (part.type === "thinking" && part.thinking ? [part.thinking] : [])).join("\n\n");
  const error = message.stopReason === "error" ? (message.errorMessage ?? "请求失败") : message.stopReason === "aborted" ? "已中断" : undefined;
  return {
    kind: "assistant",
    id: `a:${message.timestamp}`,
    text,
    thinking,
    model: `${message.provider}/${message.model}`,
    streaming,
    ...(error ? { error } : {}),
  };
}

export function hasAssistantContent(item: AssistantItem): boolean {
  return Boolean(item.text || item.thinking || item.error);
}

function clipArgs(args: Record<string, unknown>): Record<string, unknown> {
  let clipped: Record<string, unknown> | undefined;
  for (const [key, value] of Object.entries(args)) {
    if (typeof value === "string" && value.length > OUTPUT_LIMIT) (clipped ??= { ...args })[key] = clipOutput(value);
  }
  return clipped ?? args;
}

export function toolCallItems(message: AssistantMessage): ToolItem[] {
  return message.content.flatMap((part) =>
    part.type === "toolCall" ? [{ kind: "tool" as const, id: part.id, name: part.name, args: clipArgs(part.arguments as Record<string, unknown>), status: "running" as const, output: "" }] : [],
  );
}

/** Result payload of a tool run: from a toolResult message or a live tool_execution event. */
export type ToolResultLike = { content?: readonly ContentPart[]; details?: unknown };

/** Applies a (partial or final) tool result to its tool item. */
export function withToolResult(item: ToolItem, result: ToolResultLike | undefined, status: ToolItem["status"], durationMs?: number): ToolItem {
  const next: ToolItem = { ...item, status, output: clipOutput(contentText(result?.content)) };
  delete next.details;
  delete next.diff;
  const details = result?.details;
  if (details && typeof details === "object" && !Array.isArray(details)) {
    const record = details as Record<string, unknown>;
    if (item.name === "edit" && typeof record.patch === "string") next.diff = record.patch;
    if (JSON.stringify(details).length <= DETAILS_LIMIT) next.details = record;
  }
  if (durationMs !== undefined) next.durationMs = durationMs;
  return next;
}

function customItem(message: { customType: string; content: CustomMessage["content"] }, id: string): TranscriptItem {
  return { kind: "custom", id, customType: message.customType, text: contentText(message.content) };
}

export type ProjectOptions = {
  /** The agent is running: tool calls of the newest assistant message that have no result yet are still running. */
  running?: boolean;
};

/** Parses `u:<timestamp>` or `u:<timestamp>#<occurrence>` produced by `projectBranch`. */
export function parseUserItemId(itemId: string): { timestamp: number; occurrence: number } | undefined {
  const match = /^u:(\d+)(?:#(\d+))?$/.exec(itemId);
  if (!match) return undefined;
  const timestamp = Number(match[1]);
  const occurrence = match[2] ? Number(match[2]) : 1;
  if (!Number.isFinite(timestamp) || !Number.isInteger(occurrence) || occurrence < 1) return undefined;
  return { timestamp, occurrence };
}

function uniqueId(counts: Map<string, number>, base: string): string {
  const seen = (counts.get(base) ?? 0) + 1;
  counts.set(base, seen);
  return seen === 1 ? base : `${base}#${seen}`;
}

/** Projects the entries of one branch (root to leaf) into transcript items. */
export function projectBranch(entries: readonly SessionEntry[], options: ProjectOptions = {}): TranscriptItem[] {
  const items: TranscriptItem[] = [];
  const toolIndex = new Map<string, number>();
  const idCounts = new Map<string, number>();
  let lastAssistantTools: string[] = [];
  let sawMessage = false;

  const addTool = (item: ToolItem) => {
    toolIndex.set(item.id, items.length);
    items.push(item);
  };

  for (const entry of entries) {
    switch (entry.type) {
      case "message": {
        const message = entry.message;
        switch (message.role) {
          case "user": {
            sawMessage = true;
            const item = userItem(message);
            items.push({ ...item, id: uniqueId(idCounts, item.id) });
            break;
          }
          case "assistant": {
            sawMessage = true;
            const item = assistantItem(message, false);
            if (hasAssistantContent(item)) items.push({ ...item, id: uniqueId(idCounts, item.id) });
            const tools = toolCallItems(message);
            for (const tool of tools) addTool(tool);
            lastAssistantTools = tools.map((tool) => tool.id);
            break;
          }
          case "toolResult": {
            const result = message as ToolResultMessage;
            const index = toolIndex.get(result.toolCallId);
            const base: ToolItem = index !== undefined ? (items[index] as ToolItem) : { kind: "tool", id: result.toolCallId, name: result.toolName, args: {}, status: "running", output: "" };
            const next = withToolResult(base, result as ToolResultLike, result.isError ? "error" : "done", result.durationMs);
            if (index !== undefined) items[index] = next;
            else addTool(next);
            break;
          }
          case "bashExecution":
            sawMessage = true;
            items.push({
              kind: "tool",
              id: uniqueId(idCounts, `b:${message.timestamp}`),
              name: "bash",
              args: { command: message.command },
              status: message.exitCode === 0 && !message.cancelled ? "done" : "error",
              output: clipOutput(message.output),
            });
            break;
          case "custom":
            if (message.display) items.push(customItem(message, uniqueId(idCounts, `c:${message.timestamp}`)));
            break;
          default:
            break;
        }
        break;
      }
      case "custom_message":
        if (entry.display) items.push(customItem(entry, uniqueId(idCounts, `c:${Date.parse(entry.timestamp)}`)));
        break;
      case "compaction":
        items.push({ kind: "notice", id: `n:${entry.id}`, tone: "info", text: `已压缩上下文（压缩前 ${entry.tokensBefore} tokens）` });
        break;
      case "branch_summary":
        items.push({ kind: "notice", id: `n:${entry.id}`, tone: "info", text: `已回到之前的位置，离开的分支摘要：\n\n${entry.summary}` });
        break;
      case "model_change":
        // The initial model is recorded at session start; only later switches are worth showing.
        if (sawMessage) items.push({ kind: "notice", id: `n:${entry.id}`, tone: "info", text: `切换到 ${entry.provider}/${entry.modelId}` });
        break;
      default:
        break;
    }
  }

  // A tool call without a result either still runs or was cut off (crash, abort before the result was written).
  const live = new Set(options.running ? lastAssistantTools : []);
  for (const [id, index] of toolIndex) {
    const item = items[index] as ToolItem;
    if (item.status === "running" && !live.has(id)) items[index] = { ...item, status: "error", output: item.output || "未完成" };
  }
  return items;
}

/** The newest `limit` items. */
export function snapshot(items: readonly TranscriptItem[], limit = DEFAULT_SNAPSHOT_LIMIT): { items: TranscriptItem[]; hasEarlier: boolean } {
  const start = Math.max(0, items.length - limit);
  return { items: items.slice(start), hasEarlier: start > 0 };
}

/** Up to `limit` items right before the item `beforeId`. */
export function earlier(items: readonly TranscriptItem[], beforeId: string, limit: number): { items: TranscriptItem[]; hasEarlier: boolean } {
  const end = items.findIndex((item) => item.id === beforeId);
  if (end < 0) return { items: [], hasEarlier: false };
  const start = Math.max(0, end - limit);
  return { items: items.slice(start, end), hasEarlier: start > 0 };
}
