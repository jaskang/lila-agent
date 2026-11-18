import { z } from "zod";

export const PROTOCOL_VERSION = 2;

export const DEFAULT_CORE_PORT = 7457;

const Id = z.string().min(1);

export const ApprovalMode = z.enum(["yolo", "write", "always-ask"]);
export type ApprovalMode = z.infer<typeof ApprovalMode>;

export const SessionStatus = z.enum(["idle", "running", "waiting", "finished", "failed", "aborted", "interrupted"]);
export type SessionStatus = z.infer<typeof SessionStatus>;

export const Project = z.object({ id: Id, path: z.string().min(1), name: z.string().min(1) });
export type Project = z.infer<typeof Project>;

export const Model = z.object({ id: Id, label: z.string().min(1) });
export type Model = z.infer<typeof Model>;

export const AttachmentMeta = z.object({ name: z.string().min(1), mimeType: z.string().min(1) });
export type AttachmentMeta = z.infer<typeof AttachmentMeta>;
export const Attachment = AttachmentMeta.extend({ data: z.string().min(1).max(7_000_000) });
export type Attachment = z.infer<typeof Attachment>;

export const SessionSummary = z.object({
  id: Id,
  projectId: Id,
  title: z.string(),
  modelId: Id.optional(),
  approvalMode: ApprovalMode,
  status: SessionStatus,
  updatedAt: z.string(),
  pendingCount: z.number().int().nonnegative(),
});
export type SessionSummary = z.infer<typeof SessionSummary>;

export const FileDiff = z.object({ path: z.string(), diff: z.string(), op: z.string().optional() });
export type FileDiff = z.infer<typeof FileDiff>;

export const PendingKind = z.enum(["approval", "confirm", "select", "input", "ask"]);
export type PendingKind = z.infer<typeof PendingKind>;

export const PendingQuestion = z.object({
  id: Id,
  question: z.string(),
  options: z.array(z.string()),
  multiple: z.boolean(),
});
export type PendingQuestion = z.infer<typeof PendingQuestion>;

/** The user's answer to a pending item. Option values are the upstream option text, unchanged. */
export const PendingAnswer = z.discriminatedUnion("type", [
  z.object({ type: z.literal("cancel") }),
  z.object({ type: z.literal("option"), value: z.string() }),
  z.object({ type: z.literal("confirm"), confirmed: z.boolean() }),
  z.object({ type: z.literal("text"), value: z.string() }),
  z.object({
    type: z.literal("ask"),
    answers: z.array(
      z.object({ id: Id, selectedOptions: z.array(z.string()), customInput: z.string().optional() }),
    ),
  }),
]);
export type PendingAnswer = z.infer<typeof PendingAnswer>;

// ---------------------------------------------------------------------------
// Persisted session events. Only finished facts are stored; streaming deltas
// travel as transient StreamMessage frames and are never replayed.

function event<T extends string, P extends z.ZodRawShape>(eventType: T, payload: P) {
  return z.object({
    type: z.literal("event"),
    sessionId: Id,
    seq: z.number().int().positive(),
    runId: Id.optional(),
    eventType: z.literal(eventType),
    payload: z.object(payload),
  });
}

export const SessionEvent = z.discriminatedUnion("eventType", [
  event("session.created", { projectId: Id, modelId: Id.optional(), approvalMode: ApprovalMode }),
  event("session.updated", { modelId: Id.optional(), approvalMode: ApprovalMode }),
  event("message.user", { text: z.string(), attachments: z.array(AttachmentMeta).optional() }),
  event("run.started", {}),
  event("assistant.message", { messageId: Id, text: z.string(), thinking: z.string(), model: z.string().optional() }),
  event("tool.started", {
    toolCallId: Id,
    toolName: z.string(),
    intent: z.string().optional(),
    args: z.record(z.string(), z.unknown()),
  }),
  event("tool.finished", { toolCallId: Id, isError: z.boolean(), output: z.string(), diffs: z.array(FileDiff) }),
  event("pending.created", {
    pendingId: Id,
    kind: PendingKind,
    title: z.string(),
    message: z.string().optional(),
    options: z.array(z.string()),
    questions: z.array(PendingQuestion),
    toolCallId: Id.optional(),
    placeholder: z.string().optional(),
    defaultValue: z.string().optional(),
  }),
  event("pending.resolved", {
    pendingId: Id,
    status: z.enum(["answered", "cancelled", "interrupted"]),
    answer: z.string().optional(),
  }),
  event("run.finished", {}),
  event("run.aborted", {}),
  event("run.failed", { error: z.string() }),
  event("run.interrupted", { reason: z.string() }),
]);
export type SessionEvent = z.infer<typeof SessionEvent>;
export type SessionEventType = SessionEvent["eventType"];
export type SessionEventBody = {
  [K in SessionEvent as K["eventType"]]: { eventType: K["eventType"]; payload: K["payload"] };
}[SessionEventType];

// ---------------------------------------------------------------------------
// Transient stream frames: sent only to clients watching the session.

const streamBase = { type: z.literal("stream"), sessionId: Id, runId: Id };

export const StreamMessage = z.discriminatedUnion("kind", [
  z.object({
    ...streamBase,
    kind: z.literal("assistant.delta"),
    messageId: Id,
    channel: z.enum(["text", "thinking"]),
    delta: z.string(),
  }),
  /** Latest output and diffs of a running tool; each frame replaces the previous one. */
  z.object({
    ...streamBase,
    kind: z.literal("tool.progress"),
    toolCallId: Id,
    toolName: z.string(),
    output: z.string().optional(),
    diffs: z.array(FileDiff).optional(),
  }),
]);
export type StreamMessage = z.infer<typeof StreamMessage>;

/** Unfinished output of the active run, sent when a client opens a running session. */
export type LiveSnapshot = {
  runId: string;
  messages: Array<{ messageId: string; text: string; thinking: string }>;
  tools: Array<{ toolCallId: string; toolName: string; output: string; diffs: FileDiff[] }>;
};

// ---------------------------------------------------------------------------
// Client commands and their results.

const RequestId = Id;

export const ClientCommand = z.discriminatedUnion("type", [
  z.object({ type: z.literal("projects.list"), requestId: RequestId }),
  z.object({ type: z.literal("projects.add"), requestId: RequestId, path: z.string().min(1) }),
  z.object({ type: z.literal("models.list"), requestId: RequestId }),
  z.object({ type: z.literal("sessions.list"), requestId: RequestId }),
  z.object({
    type: z.literal("session.create"),
    requestId: RequestId,
    projectId: Id,
    modelId: Id.optional(),
    approvalMode: ApprovalMode.optional(),
  }),
  z.object({
    type: z.literal("session.configure"),
    requestId: RequestId,
    sessionId: Id,
    modelId: Id.nullable().optional(),
    approvalMode: ApprovalMode.optional(),
  }),
  z.object({ type: z.literal("session.rename"), requestId: RequestId, sessionId: Id, title: z.string().trim().min(1).max(120) }),
  z.object({ type: z.literal("session.delete"), requestId: RequestId, sessionId: Id }),
  z.object({ type: z.literal("message.send"), requestId: RequestId, sessionId: Id, text: z.string().trim().min(1), attachments: z.array(Attachment).max(10).optional() }),
  z.object({ type: z.literal("run.abort"), requestId: RequestId, sessionId: Id }),
  z.object({
    type: z.literal("pending.respond"),
    requestId: RequestId,
    sessionId: Id,
    pendingId: Id,
    answer: PendingAnswer,
  }),
  /** Replays events after `afterSeq` and makes this socket watch the session; replaces any previous watch. */
  z.object({
    type: z.literal("events.resume"),
    requestId: RequestId,
    sessionId: Id,
    afterSeq: z.number().int().nonnegative(),
  }),
  z.object({
    type: z.literal("events.before"),
    requestId: RequestId,
    sessionId: Id,
    beforeSeq: z.number().int().positive(),
    limit: z.number().int().positive().max(500),
  }),
]);
export type ClientCommand = z.infer<typeof ClientCommand>;
export type CommandType = ClientCommand["type"];

export interface CommandResults {
  "projects.list": { projects: Project[] };
  "projects.add": { project: Project };
  "models.list": { models: Model[]; defaultModelId?: string };
  "sessions.list": { sessions: SessionSummary[] };
  "session.create": { session: SessionSummary };
  "session.configure": { session: SessionSummary };
  "session.rename": { session: SessionSummary };
  "session.delete": Record<string, never>;
  "message.send": { runId: string };
  "run.abort": Record<string, never>;
  "pending.respond": Record<string, never>;
  "events.resume": { events: SessionEvent[]; live?: LiveSnapshot; hasEarlier: boolean };
  "events.before": { events: SessionEvent[]; hasEarlier: boolean };
}

// ---------------------------------------------------------------------------
// Server messages.

export const ServerHello = z.object({
  type: z.literal("hello"),
  protocolVersion: z.number().int(),
  coreVersion: z.string(),
});
export type ServerHello = z.infer<typeof ServerHello>;

export const CoreResponse = z.discriminatedUnion("ok", [
  z.object({ type: z.literal("response"), requestId: RequestId, command: z.string(), ok: z.literal(true), data: z.unknown() }),
  z.object({ type: z.literal("response"), requestId: RequestId, command: z.string(), ok: z.literal(false), error: z.string() }),
]);
export type CoreResponse = z.infer<typeof CoreResponse>;

/** Broadcast to every client whenever a session's list entry changes. */
export const SessionChanged = z.object({ type: z.literal("session"), session: SessionSummary });
export type SessionChanged = z.infer<typeof SessionChanged>;

export const CoreError = z.object({ type: z.literal("error"), code: z.string().min(1), message: z.string().min(1) });
export type CoreError = z.infer<typeof CoreError>;

export const ProtocolMessage = z.union([ServerHello, SessionEvent, StreamMessage, CoreResponse, SessionChanged, CoreError]);
export type ProtocolMessage = z.infer<typeof ProtocolMessage>;
