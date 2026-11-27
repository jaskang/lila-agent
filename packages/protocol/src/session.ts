import { z } from "zod";
import type { ThinkingLevel } from "./settings.js";

const Id = z.string().min(1);

/** Persisted IDs: auto is full access, ask-exec is risk-based approval, ask-all is request approval. */
export const ApprovalMode = z.enum(["auto", "ask-exec", "ask-all"]);
export type ApprovalMode = z.infer<typeof ApprovalMode>;
export const DEFAULT_APPROVAL_MODE: ApprovalMode = "ask-exec";

/** OS-level process sandbox boundary for bash execution, orthogonal to ApprovalMode. */
export const SandboxMode = z.enum(["off", "workspace-write"]);
export type SandboxMode = z.infer<typeof SandboxMode>;
export const DEFAULT_SANDBOX_MODE: SandboxMode = "off";
export const DEFAULT_SCHEDULE_SANDBOX_MODE: SandboxMode = "workspace-write";

export type SandboxStatus = {
  available: boolean;
  backend?: "seatbelt" | "bubblewrap" | "windows";
  /** True when a user-installed external sandbox extension takes precedence over Lila's built-in sandbox. */
  external?: boolean;
  reason?: string;
};

export const MAX_PROMPT_IMAGES = 10;
export const MAX_PROMPT_IMAGE_BYTES = 10 * 1024 * 1024;
export const MAX_PROMPT_IMAGE_BASE64_BYTES = 48 * 1024 * 1024;

export const ImageInput = z.object({
  mimeType: z.string().regex(/^image\//),
  data: z.string().min(1).max(Math.ceil(MAX_PROMPT_IMAGE_BYTES / 3) * 4),
});
export type ImageInput = z.infer<typeof ImageInput>;

export function promptImageBytes(images: readonly ImageInput[]): number {
  return images.reduce((total, image) => total + image.data.length, 0);
}

export type Project = { id: string; path: string; name: string };
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
  /** The original file remains owned by pi-subagents. */
  readOnly?: boolean;
};

export type ToolStatus = "running" | "done" | "error";
export type RunStatus = "completed" | "aborted" | "error" | "incomplete";
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
  | { kind: "assistant"; id: string; text: string; thinking: string; model?: string; timestamp?: number; streaming: boolean; error?: string }
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
  | {
      kind: "custom";
      id: string;
      customType: string;
      text: string;
      notifications?: SubagentNotification[];
      details?: Record<string, unknown>;
    }
  | { kind: "notice"; id: string; tone: "info" | "warning" | "error"; text: string }
  | { kind: "run"; id: string; status: RunStatus; durationMs?: number };

export const TRANSCRIPT_SEARCH_PAGE_SIZE = 50;
export type TranscriptSearchMatch = { itemId: string; kind: TranscriptItem["kind"]; excerpt: string };
export type TranscriptSearchResults = { matches: TranscriptSearchMatch[]; total: number };
export const TRANSCRIPT_TURNS_PAGE_SIZE = 30;
export type TranscriptTurn = { itemId: string; number: number; preview: string };
export type TranscriptTurns = { turns: TranscriptTurn[]; total: number };
export type ItemKind = TranscriptItem["kind"];

export type PendingQuestion = { id: string; question: string; options: string[]; multiple: boolean };

export type ApprovalRuleSuggestion = { pattern: string; label: string };

export type PendingRequest = {
  id: string;
  kind: "approval" | "select" | "confirm" | "input" | "editor" | "ask";
  title: string;
  message?: string;
  /** select: the option texts. approval: allow / deny, rendered by the client. */
  options: string[];
  placeholder?: string;
  prefill?: string;
  /** approval: the tool call that waits. */
  toolCallId?: string;
  toolName?: string;
  /** approval: reusable rule the user may approve for the session or project. */
  ruleSuggestion?: ApprovalRuleSuggestion;
  /** ask: one or more questions. */
  questions?: PendingQuestion[];
};

export const PendingAnswer = z.discriminatedUnion("type", [
  z.object({ type: z.literal("cancel") }),
  /** approval: "allow" | "deny". select: the option text. */
  z.object({ type: z.literal("option"), value: z.string(), note: z.string().optional() }),
  z.object({ type: z.literal("confirm"), confirmed: z.boolean() }),
  z.object({ type: z.literal("text"), value: z.string() }),
  z.object({
    type: z.literal("ask"),
    answers: z.array(z.object({ id: Id, selected: z.array(z.string()), custom: z.string().optional() })),
  }),
]);
export type PendingAnswer = z.infer<typeof PendingAnswer>;

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

export type ContextBreakdown = {
  /** Estimated tokens for system prompt, AGENTS.md, skills, and tool definitions. */
  system: number;
  /** Estimated tokens for user & assistant dialogue text. */
  conversation: number;
  /** Estimated tokens for tool calls and tool outputs. */
  toolOutputs: number;
  /** Estimated tokens for assistant thinking blocks. */
  thinking: number;
};

export type SessionState = {
  status: SessionStatus;
  access: "starting" | "ready" | "read-only" | "failed";
  startupError?: string;
  /** The session's own Run, independent of any detached sub-agents. */
  runActive: boolean;
  modelId?: string;
  thinkingLevel: ThinkingLevel;
  /** Levels the current model accepts; ["off"] for a model without reasoning. */
  thinkingLevels: ThinkingLevel[];
  approvalMode: ApprovalMode;
  sandboxMode: SandboxMode;
  sandbox?: SandboxStatus;
  /** Reusable approval rules granted for this session only. */
  sessionRules?: string[];
  /** Reusable approval rules persisted for this project. */
  projectRules?: string[];
  /** Messages typed while the agent ran, waiting to be delivered. */
  queue: { steering: string[]; followUp: string[] };
  context?: { tokens: number | null; window: number; percent: number | null; breakdown?: ContextBreakdown };
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
