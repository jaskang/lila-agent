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

type SubagentNotificationSummary = {
  id?: string;
  description: string;
  status: string;
  toolUses?: number;
  turnCount?: number;
  totalTokens?: number;
  totalCost?: number;
  durationMs?: number;
  error?: string;
  preview?: string;
};

function unescapeXml(text: string): string {
  return text
    .replaceAll("&lt;", "<")
    .replaceAll("&gt;", ">")
    .replaceAll("&quot;", '"')
    .replaceAll("&apos;", "'")
    .replaceAll("&amp;", "&");
}

function cleanNotificationPreview(raw: string | undefined): string | undefined {
  if (!raw) return undefined;
  const cleaned = raw
    .replace(/\n?\.\.\.\(truncated,\s*use get_subagent_result for full output\)\s*$/i, "…")
    .replace(/\n?Full transcript available at:[^\n]*$/gim, "")
    .trim();
  if (!cleaned || cleaned === "No output.") return undefined;
  return cleaned;
}

function normalizeNotificationDetail(raw: unknown): SubagentNotificationSummary | undefined {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return undefined;
  const d = raw as Record<string, unknown>;
  if (typeof d.description !== "string" && typeof d.id !== "string") return undefined;
  const preview = cleanNotificationPreview(typeof d.resultPreview === "string" ? d.resultPreview : undefined);
  return {
    ...(typeof d.id === "string" && d.id ? { id: d.id } : {}),
    description: typeof d.description === "string" && d.description ? d.description : "子 Agent",
    status: typeof d.status === "string" && d.status ? d.status : "completed",
    ...(typeof d.toolUses === "number" && d.toolUses > 0 ? { toolUses: d.toolUses } : {}),
    ...(typeof d.turnCount === "number" && d.turnCount > 0 ? { turnCount: d.turnCount } : {}),
    ...(typeof d.totalTokens === "number" && d.totalTokens > 0 ? { totalTokens: d.totalTokens } : {}),
    ...(typeof d.totalCost === "number" && d.totalCost > 0 ? { totalCost: d.totalCost } : {}),
    ...(typeof d.durationMs === "number" && d.durationMs > 0 ? { durationMs: d.durationMs } : {}),
    ...(typeof d.error === "string" && d.error ? { error: d.error } : {}),
    ...(preview ? { preview } : {}),
  };
}

function xmlTag(block: string, tag: string): string | undefined {
  const match = new RegExp(`<${tag}>([\\s\\S]*?)</${tag}>`).exec(block);
  return match?.[1] !== undefined ? unescapeXml(match[1]) : undefined;
}

function parseTaskNotificationXml(rawText: string): SubagentNotificationSummary[] {
  const blocks = [...rawText.matchAll(/<task-notification>([\s\S]*?)<\/task-notification>/g)];
  const parsed: SubagentNotificationSummary[] = [];
  for (const match of blocks) {
    const body = match[1] ?? "";
    const id = xmlTag(body, "task-id")?.trim();
    const statusRaw = xmlTag(body, "status")?.trim() ?? "completed";
    const summary = xmlTag(body, "summary")?.trim() ?? "";
    const descMatch = /^Agent\s+"([^"]+)"/i.exec(summary);
    const description = descMatch?.[1] || summary || "子 Agent";
    const statusLower = statusRaw.toLowerCase();
    const status =
      statusLower === "done" || statusLower === "completed"
        ? "completed"
        : statusLower.includes("abort")
          ? "aborted"
          : statusLower.includes("stop")
            ? "stopped"
            : statusLower.includes("err") || statusLower.includes("fail")
              ? "error"
              : statusRaw;
    const totalTokens = Number(xmlTag(body, "total_tokens"));
    const toolUses = Number(xmlTag(body, "tool_uses"));
    const totalCost = Number(xmlTag(body, "estimated_cost_usd"));
    const durationMs = Number(xmlTag(body, "duration_ms"));
    const preview = cleanNotificationPreview(xmlTag(body, "result"));
    parsed.push({
      ...(id ? { id } : {}),
      description,
      status,
      ...(Number.isFinite(toolUses) && toolUses > 0 ? { toolUses } : {}),
      ...(Number.isFinite(totalTokens) && totalTokens > 0 ? { totalTokens } : {}),
      ...(Number.isFinite(totalCost) && totalCost > 0 ? { totalCost } : {}),
      ...(Number.isFinite(durationMs) && durationMs > 0 ? { durationMs } : {}),
      ...(preview ? { preview } : {}),
    });
  }
  return parsed;
}

function projectSubagentNotification(rawText: string, details: unknown): { text: string; details?: Record<string, unknown> } {
  const notifications: SubagentNotificationSummary[] = [];
  if (details && typeof details === "object" && !Array.isArray(details)) {
    const record = details as Record<string, unknown>;
    const first = normalizeNotificationDetail(record);
    if (first) notifications.push(first);
    if (Array.isArray(record.others)) {
      for (const other of record.others) {
        const normalized = normalizeNotificationDetail(other);
        if (normalized) notifications.push(normalized);
      }
    }
  }
  if (!notifications.length) {
    notifications.push(...parseTaskNotificationXml(rawText));
  }
  if (!notifications.length) {
    return { text: cleanNotificationPreview(rawText) ?? "" };
  }
  // Clean markdown fallback for any client that only renders `item.text`.
  const markdown = notifications
    .map((entry) => {
      const header = notifications.length > 1 ? `**${entry.description}**` : "";
      return [header, entry.error ?? entry.preview ?? ""].filter(Boolean).join("\n\n");
    })
    .filter(Boolean)
    .join("\n\n---\n\n");
  return {
    text: markdown,
    details: { notifications },
  };
}

export function customItem(message: { customType: string; content: CustomMessage["content"]; details?: unknown }, id: string): TranscriptItem {
  const rawText = contentText(message.content);
  if (message.customType === "subagent-notification") {
    const projected = projectSubagentNotification(rawText, message.details);
    return {
      kind: "custom",
      id,
      customType: message.customType,
      text: projected.text,
      ...(projected.details ? { details: projected.details } : {}),
    };
  }
  return { kind: "custom", id, customType: message.customType, text: rawText };
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
  /** Maps pi-subagents `agentId` → index of its `Agent` tool item so persisted `subagents:record` updates it. */
  const agentIndex = new Map<string, number>();
  const idCounts = new Map<string, number>();
  let lastAssistantTools: string[] = [];
  let sawMessage = false;

  const trackAgent = (item: ToolItem, index: number) => {
    if (item.name === "Agent" && typeof item.details?.agentId === "string" && item.details.agentId) {
      agentIndex.set(item.details.agentId, index);
    }
  };

  const addTool = (item: ToolItem) => {
    const index = items.length;
    toolIndex.set(item.id, index);
    items.push(item);
    trackAgent(item, index);
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
            if (index !== undefined) {
              items[index] = next;
              trackAgent(next, index);
            } else {
              addTool(next);
            }
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
      case "custom": {
        if (entry.customType === "subagents:record" && entry.data && typeof entry.data === "object") {
          const record = entry.data as Record<string, unknown>;
          const id = typeof record.id === "string" ? record.id : undefined;
          const index = id ? agentIndex.get(id) : undefined;
          if (index !== undefined) {
            const existing = items[index] as ToolItem;
            const status = typeof record.status === "string" ? record.status : undefined;
            const startedAt = typeof record.startedAt === "number" ? record.startedAt : undefined;
            const completedAt = typeof record.completedAt === "number" ? record.completedAt : undefined;
            const durationMs = startedAt !== undefined && completedAt !== undefined && completedAt >= startedAt ? completedAt - startedAt : undefined;
            const isErr = status === "error" || status === "aborted" || status === "stopped";
            const nextDetails: Record<string, unknown> = {
              ...existing.details,
              ...(status ? { status } : {}),
              ...(durationMs !== undefined ? { durationMs } : {}),
              ...(typeof record.error === "string" && record.error ? { error: record.error } : {}),
            };
            const output = typeof record.result === "string" && record.result ? clipOutput(record.result) : existing.output;
            items[index] = {
              ...existing,
              status: isErr ? "error" : "done",
              output,
              details: nextDetails,
              ...(durationMs !== undefined ? { durationMs } : {}),
            };
          }
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
