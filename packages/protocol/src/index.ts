import { z } from "zod";

/**
 * Lila's wire protocol between core and its clients (desktop now, mobile later).
 *
 * Commands from clients are validated with Zod at the core boundary. Messages from core are plain
 * TypeScript types: core is trusted once the socket is authenticated, and streaming frames are too
 * frequent to validate one by one.
 *
 * The transcript is not stored by Lila. Pi's session file is the only record; core projects its
 * entries into `TranscriptItem`s when a client opens a session, then pushes `item` / `delta` frames.
 * A client that reconnects simply opens the session again.
 */

export const PROTOCOL_VERSION = 4;
export const DEFAULT_CORE_PORT = 7457;

const Id = z.string().min(1);

/** auto: never ask. ask-exec: ask before commands, MCP, subagents and scripts; file edits run. ask-all: ask before every tool that is not read-only. */
export const ApprovalMode = z.enum(["auto", "ask-exec", "ask-all"]);
export type ApprovalMode = z.infer<typeof ApprovalMode>;

export const ThinkingLevel = z.enum(["off", "minimal", "low", "medium", "high", "xhigh", "max"]);
export type ThinkingLevel = z.infer<typeof ThinkingLevel>;

export const ImageInput = z.object({ mimeType: z.string().regex(/^image\//), data: z.string().min(1).max(14_000_000) });
export type ImageInput = z.infer<typeof ImageInput>;

export type Project = { id: string; path: string; name: string };

export type ModelInfo = {
  /** `provider/modelId`, as Pi writes it. */
  id: string;
  provider: string;
  label: string;
  reasoning: boolean;
  images: boolean;
  contextWindow?: number;
};

export type SessionStatus = "idle" | "running" | "waiting" | "error";

export type SessionSummary = {
  id: string;
  projectId: string;
  title: string;
  updatedAt: string;
  status: SessionStatus;
  pendingCount: number;
  /** Set for a sub-agent's own session: the session that spawned it. */
  parentId?: string;
  /** pi-subagents agent id (or its 8-char prefix) linking this sub-session to its `Agent` tool card. */
  agentId?: string;
};

// ---------------------------------------------------------------------------
// Transcript projection

export type ToolStatus = "running" | "done" | "error";

export type SubagentStatus = "queued" | "running" | "completed" | "steered" | "stopped" | "aborted" | "error";

/** Normalized metadata for a sub-agent tool call, produced by core so clients never parse pi-subagents internals. */
export type SubagentToolInfo = {
  id?: string;
  type?: string;
  description: string;
  prompt?: string;
  background: boolean;
  status: SubagentStatus;
  activity?: string;
  model?: string;
  turns?: number;
  maxTurns?: number;
  toolUses?: number;
  tokens?: number;
  cost?: number;
  durationMs?: number;
  output?: string;
  error?: string;
};

/** Normalized background sub-agent completion notice, stripped of raw XML and temp paths by core. */
export type SubagentNotification = {
  id?: string;
  description: string;
  status: SubagentStatus;
  toolUses?: number;
  turnCount?: number;
  totalTokens?: number;
  totalCost?: number;
  durationMs?: number;
  error?: string;
  preview?: string;
};

export type TranscriptItem =
  | { kind: "user"; id: string; text: string; images: ImageInput[]; canRewind: boolean }
  | { kind: "assistant"; id: string; text: string; thinking: string; model?: string; streaming: boolean; error?: string }
  | {
      kind: "tool";
      /** The tool call id. */
      id: string;
      name: string;
      args: Record<string, unknown>;
      status: ToolStatus;
      /** Text output, clipped by core. Replaced (not appended) on every update. */
      output: string;
      /** Unified patch for edit/write tools. */
      diff?: string;
      /** Normalized sub-agent view when this tool call spawned a sub-agent. */
      subagent?: SubagentToolInfo;
      /** Tool-specific details for rich rendering, dropped when large. */
      details?: Record<string, unknown>;
      durationMs?: number;
    }
  /** Context an extension injected, for example a background subagent's report. Markdown. */
  | {
      kind: "custom";
      id: string;
      customType: string;
      text: string;
      notifications?: SubagentNotification[];
      details?: Record<string, unknown>;
    }
  | { kind: "notice"; id: string; tone: "info" | "warning" | "error"; text: string };

export type ItemKind = TranscriptItem["kind"];

// ---------------------------------------------------------------------------
// Requests that wait for the user (tool approvals, extension dialogs, ask_user)

export type PendingQuestion = { id: string; question: string; options: string[]; multiple: boolean };

export type PendingRequest = {
  id: string;
  kind: "approval" | "select" | "confirm" | "input" | "editor" | "ask";
  title: string;
  message?: string;
  /** select: the option texts. approval: always allow / always / deny, rendered by the client. */
  options: string[];
  placeholder?: string;
  prefill?: string;
  /** approval: the tool call that waits. */
  toolCallId?: string;
  toolName?: string;
  /** ask: one or more questions. */
  questions?: PendingQuestion[];
};

export const PendingAnswer = z.discriminatedUnion("type", [
  z.object({ type: z.literal("cancel") }),
  /** approval: "allow" | "always" | "deny". select: the option text. */
  z.object({ type: z.literal("option"), value: z.string() }),
  z.object({ type: z.literal("confirm"), confirmed: z.boolean() }),
  z.object({ type: z.literal("text"), value: z.string() }),
  z.object({
    type: z.literal("ask"),
    answers: z.array(z.object({ id: Id, selected: z.array(z.string()), custom: z.string().optional() })),
  }),
]);
export type PendingAnswer = z.infer<typeof PendingAnswer>;

// ---------------------------------------------------------------------------
// Live session state

export type SubagentProgress = {
  id: string;
  type: string;
  description: string;
  status: SubagentStatus;
  background: boolean;
  startedAt: number;
  activity?: string;
  model?: string;
  toolUses: number;
  turns: number;
  tokens: number;
  cost: number;
  durationMs?: number;
  output?: string;
  error?: string;
};

export type GitFileStatus = "modified" | "added" | "deleted" | "renamed" | "copied" | "untracked" | "unmerged";

export type GitFileChange = {
  path: string;
  from?: string;
  status: GitFileStatus;
  staged: boolean;
  unstaged: boolean;
  added?: number;
  removed?: number;
};

export type GitStatus = {
  isRepo: boolean;
  branch?: string;
  ahead?: number;
  behind?: number;
  files: GitFileChange[];
};

export type GitFileDiff = {
  path: string;
  patch: string;
  binary?: boolean;
  truncated?: boolean;
};

export type SessionState = {
  status: SessionStatus;
  modelId?: string;
  thinkingLevel: ThinkingLevel;
  /** Levels the current model accepts; ["off"] for a model without reasoning. */
  thinkingLevels: ThinkingLevel[];
  approvalMode: ApprovalMode;
  /** Messages typed while the agent ran, waiting to be delivered. */
  queue: { steering: string[]; followUp: string[] };
  context?: { tokens: number | null; window: number; percent: number | null };
  /** Total cost of the session in USD. */
  cost: number;
  compacting: boolean;
  retry?: { attempt: number; max: number; message: string };
  pending: PendingRequest[];
  /** Status lines set by extensions (`ctx.ui.setStatus`). */
  statusLines: Record<string, string>;
  /** Live progress of top-level sub-agents (foreground and background) in this session. */
  subagents?: SubagentProgress[];
};

export type SlashCommand = { name: string; description: string; source: "builtin" | "extension" | "prompt" | "skill" };

export type SessionResources = {
  cwd: string;
  agentDir: string;
  /** Pi's MCP configuration files: the user one, then the project one. */
  mcpConfigPaths: string[];
  skills: Array<{ name: string; description: string; path: string; scope: string }>;
  prompts: Array<{ name: string; description: string; path: string }>;
  extensions: Array<{ path: string; tools: string[]; commands: string[] }>;
  tools: Array<{ name: string; description: string; source: string; active: boolean }>;
  contextFiles: string[];
  diagnostics: string[];
};

// ---------------------------------------------------------------------------
// Client commands and their results

const RequestId = Id;
const SessionRef = { requestId: RequestId, sessionId: Id };

export const ClientCommand = z.discriminatedUnion("type", [
  z.object({ type: z.literal("projects.list"), requestId: RequestId }),
  z.object({ type: z.literal("projects.add"), requestId: RequestId, path: z.string().min(1) }),
  z.object({ type: z.literal("projects.remove"), requestId: RequestId, projectId: Id }),
  z.object({ type: z.literal("models.list"), requestId: RequestId }),
  z.object({ type: z.literal("sessions.list"), requestId: RequestId }),
  z.object({
    type: z.literal("session.create"),
    requestId: RequestId,
    projectId: Id,
    modelId: Id.optional(),
    thinkingLevel: ThinkingLevel.optional(),
    approvalMode: ApprovalMode.optional(),
  }),
  /** Returns the newest items and makes this socket watch the session; replaces any previous watch. */
  z.object({ type: z.literal("session.open"), ...SessionRef, limit: z.number().int().positive().max(1000).optional() }),
  /** Stops streaming frames for the previously watched session on this socket. */
  z.object({ type: z.literal("session.unwatch"), requestId: RequestId }),
  z.object({ type: z.literal("session.earlier"), ...SessionRef, beforeId: Id, limit: z.number().int().positive().max(1000) }),
  z.object({
    type: z.literal("session.prompt"),
    ...SessionRef,
    text: z.string(),
    images: z.array(ImageInput).max(10).optional(),
    /** While the agent runs: steer joins after the current tool round, followUp waits for the run to end. */
    mode: z.enum(["steer", "followUp"]).optional(),
  }),
  z.object({ type: z.literal("session.abort"), ...SessionRef }),
  z.object({
    type: z.literal("session.configure"),
    ...SessionRef,
    modelId: Id.optional(),
    thinkingLevel: ThinkingLevel.optional(),
    approvalMode: ApprovalMode.optional(),
  }),
  z.object({ type: z.literal("session.rename"), ...SessionRef, title: z.string().trim().min(1).max(120) }),
  z.object({ type: z.literal("session.delete"), ...SessionRef }),
  z.object({ type: z.literal("session.compact"), ...SessionRef }),
  /** Moves the session back to before a user message and returns its text for editing. */
  z.object({ type: z.literal("session.rewind"), ...SessionRef, itemId: Id }),
  z.object({ type: z.literal("session.respond"), ...SessionRef, pendingId: Id, answer: PendingAnswer }),
  /** Slash commands: the session's when it is given, else the project's (for a session not created yet). */
  z.object({ type: z.literal("commands.list"), requestId: RequestId, projectId: Id, sessionId: Id.optional() }),
  z.object({ type: z.literal("session.resources"), ...SessionRef }),
  /** Reloads skills, prompts, extensions and MCP servers, for example after editing mcp.json. */
  z.object({ type: z.literal("session.reload"), ...SessionRef }),
  z.object({ type: z.literal("files.search"), requestId: RequestId, projectId: Id, query: z.string(), limit: z.number().int().positive().max(200) }),
  z.object({ type: z.literal("git.status"), requestId: RequestId, projectId: Id }),
  z.object({ type: z.literal("git.diff"), requestId: RequestId, projectId: Id, path: z.string().min(1), staged: z.boolean().optional() }),
]);
export type ClientCommand = z.infer<typeof ClientCommand>;
export type CommandType = ClientCommand["type"];

type Empty = Record<string, never>;

export interface CommandResults {
  "projects.list": { projects: Project[] };
  "projects.add": { project: Project };
  "projects.remove": Empty;
  "models.list": { models: ModelInfo[]; defaultModelId?: string };
  "sessions.list": { sessions: SessionSummary[] };
  "session.create": { session: SessionSummary };
  "session.open": { items: TranscriptItem[]; hasEarlier: boolean; state: SessionState };
  "session.unwatch": Empty;
  "session.earlier": { items: TranscriptItem[]; hasEarlier: boolean };
  "session.prompt": Empty;
  "session.abort": Empty;
  "session.configure": { state: SessionState };
  "session.rename": { session: SessionSummary };
  "session.delete": Empty;
  "session.compact": Empty;
  "session.rewind": { text: string };
  "session.respond": Empty;
  "commands.list": { commands: SlashCommand[] };
  "session.resources": { resources: SessionResources };
  "session.reload": Empty;
  "files.search": { files: string[] };
  "git.status": { status: GitStatus };
  "git.diff": { diff: GitFileDiff };
}

// ---------------------------------------------------------------------------
// Messages from core

export type ServerMessage =
  | { type: "hello"; protocolVersion: number; coreVersion: string }
  | { type: "response"; requestId: string; ok: true; data: unknown }
  | { type: "response"; requestId: string; ok: false; error: string }
  /** Broadcast to every client when the project list changes. */
  | { type: "projects"; projects: Project[] }
  /** Broadcast to every client when a session's list entry changes. */
  | { type: "session"; session: SessionSummary }
  | { type: "session.removed"; sessionId: string }
  /** To watchers: insert or replace one transcript item. */
  | { type: "item"; sessionId: string; item: TranscriptItem }
  /** To watchers: append streamed text to an assistant item. */
  | { type: "delta"; sessionId: string; itemId: string; channel: "text" | "thinking"; delta: string }
  | { type: "state"; sessionId: string; state: SessionState }
  /** To watchers: the transcript changed shape (rewind, compaction); open the session again. */
  | { type: "reset"; sessionId: string }
  | { type: "notify"; sessionId?: string; level: "info" | "warning" | "error"; message: string }
  | { type: "error"; code: string; message: string };

export type ServerMessageType = ServerMessage["type"];
