import { z } from "zod";

export const PROTOCOL_VERSION = 1;

export const DEFAULT_CORE_PORT = 7457;

export const ApprovalMode = z.enum(["yolo", "write", "always-ask"]);
export type ApprovalMode = z.infer<typeof ApprovalMode>;

export const ServerHello = z.object({
  type: z.literal("hello"),
  protocolVersion: z.literal(PROTOCOL_VERSION),
  coreVersion: z.string(),
});
export type ServerHello = z.infer<typeof ServerHello>;

const RequestId = z.string().min(1);
const SessionId = z.string().min(1);

export const ClientCommand = z.discriminatedUnion("type", [
  z.object({ type: z.literal("projects.list"), requestId: RequestId }),
  z.object({ type: z.literal("projects.add"), requestId: RequestId, path: z.string().min(1) }),
  z.object({ type: z.literal("models.list"), requestId: RequestId }),
  z.object({ type: z.literal("sessions.list"), requestId: RequestId, projectId: z.string().min(1).optional() }),
  z.object({
    type: z.literal("session.create"),
    requestId: RequestId,
    projectId: z.string().min(1),
    modelId: z.string().min(1).optional(),
    approvalMode: ApprovalMode.optional(),
  }),
  z.object({
    type: z.literal("session.configure"),
    requestId: RequestId,
    sessionId: SessionId,
    modelId: z.string().min(1).nullable().optional(),
    approvalMode: ApprovalMode.optional(),
  }),
  z.object({
    type: z.literal("message.send"),
    requestId: RequestId,
    sessionId: SessionId,
    text: z.string().min(1),
  }),
  z.object({
    type: z.literal("run.abort"),
    requestId: RequestId,
    sessionId: SessionId,
    runId: z.string().min(1),
  }),
  z.object({
    type: z.literal("pending.respond"),
    requestId: RequestId,
    sessionId: SessionId,
    pendingId: z.string().min(1),
    answer: z.unknown(),
  }),
  z.object({
    type: z.literal("events.resume"),
    sessionId: SessionId,
    lastEventSeq: z.number().int().nonnegative(),
  }),
]);
export type ClientCommand = z.infer<typeof ClientCommand>;

export const SessionEventType = z.enum([
  "session.created",
  "session.updated",
  "message.user",
  "run.started",
  "assistant.delta",
  "assistant.completed",
  "tool.started",
  "tool.output",
  "tool.finished",
  "pending.created",
  "pending.resolved",
  "run.finished",
  "run.failed",
  "run.aborted",
  "run.interrupted",
]);
export type SessionEventType = z.infer<typeof SessionEventType>;

export const SessionEvent = z.object({
  type: z.literal("event"),
  sessionId: SessionId,
  seq: z.number().int().positive(),
  eventType: SessionEventType,
  payload: z.record(z.string(), z.unknown()),
  requestId: RequestId.optional(),
  runId: z.string().min(1).optional(),
  pendingId: z.string().min(1).optional(),
});
export type SessionEvent = z.infer<typeof SessionEvent>;

export const CoreResponse = z.object({
  type: z.literal("response"),
  requestId: RequestId,
  command: z.string().min(1),
  success: z.boolean(),
  data: z.record(z.string(), z.unknown()).optional(),
  error: z.string().min(1).optional(),
});
export type CoreResponse = z.infer<typeof CoreResponse>;

export const CoreError = z.object({
  type: z.literal("error"),
  code: z.string().min(1),
  message: z.string().min(1),
  requestId: RequestId.optional(),
});
export type CoreError = z.infer<typeof CoreError>;

export const ProtocolMessage = z.union([ServerHello, SessionEvent, CoreResponse, CoreError]);
export type ProtocolMessage = z.infer<typeof ProtocolMessage>;
