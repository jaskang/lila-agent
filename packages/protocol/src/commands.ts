import { z } from "zod";
import {
  ApprovalMode,
  ImageInput,
  PendingAnswer,
  type Project,
  type SessionState,
  type SessionSummary,
  type SlashCommand,
  type TranscriptItem,
  type TranscriptSearchResults,
  type TranscriptTurns,
} from "./session.js";
import {
  type AuthLoginEvent,
  AuthMethod,
  type CapabilitiesSnapshot,
  type CoreSettings,
  type McpConfigSnapshot,
  McpExposure,
  McpScope,
  McpServerInput,
  type MemorySnapshot,
  MemoryTarget,
  type ModelInfo,
  PackageScope,
  type ProviderAuthInfo,
  type ProviderQuotaItem,
  ThinkingLevel,
  UsageRange,
  type UsageSummary,
} from "./settings.js";
import type { GitFileDiff, GitStatus, ProjectFileContent } from "./workspace.js";

const Id = z.string().min(1);
const RequestId = Id;
const SessionRef = { requestId: RequestId, sessionId: Id };
const WatchSlot = z.enum(["main", "preview"]);
export type WatchSlot = z.infer<typeof WatchSlot>;

export const ClientCommand = z.discriminatedUnion("type", [
  z.object({ type: z.literal("ping"), requestId: RequestId }),
  z.object({ type: z.literal("projects.list"), requestId: RequestId }),
  z.object({ type: z.literal("projects.add"), requestId: RequestId, path: z.string().min(1) }),
  z.object({ type: z.literal("projects.remove"), requestId: RequestId, projectId: Id }),
  z.object({ type: z.literal("models.list"), requestId: RequestId, refresh: z.boolean().optional() }),
  z.object({ type: z.literal("provider.models.set"), requestId: RequestId, providerId: Id, modelIds: z.array(Id).nullable() }),
  z.object({ type: z.literal("settings.get"), requestId: RequestId }),
  z.object({
    type: z.literal("settings.update"),
    requestId: RequestId,
    defaultModelId: z.string().min(1).optional(),
    defaultThinkingLevel: ThinkingLevel.optional(),
    autoCompaction: z.boolean().optional(),
    autoRetry: z.boolean().optional(),
  }),
  z.object({ type: z.literal("providers.list"), requestId: RequestId }),
  z.object({ type: z.literal("auth.login.start"), requestId: RequestId, providerId: Id, method: AuthMethod }),
  z.object({ type: z.literal("auth.login.respond"), requestId: RequestId, loginId: Id, promptId: Id, value: z.string() }),
  z.object({ type: z.literal("auth.login.cancel"), requestId: RequestId, loginId: Id }),
  z.object({ type: z.literal("auth.logout"), requestId: RequestId, providerId: Id }),
  z.object({ type: z.literal("mcp.list"), requestId: RequestId, projectId: Id.optional() }),
  z.object({
    type: z.literal("mcp.upsert"),
    requestId: RequestId,
    scope: McpScope,
    projectId: Id.optional(),
    name: z.string().trim().min(1).max(120),
    config: McpServerInput,
  }),
  z.object({
    type: z.literal("mcp.toggle"),
    requestId: RequestId,
    scope: McpScope,
    projectId: Id.optional(),
    name: Id,
    enabled: z.boolean().optional(),
    exposure: McpExposure.optional(),
  }),
  z.object({
    type: z.literal("mcp.remove"),
    requestId: RequestId,
    scope: McpScope,
    projectId: Id.optional(),
    name: Id,
  }),
  z.object({ type: z.literal("capabilities.get"), requestId: RequestId, projectId: Id.optional(), sessionId: Id.optional() }),
  z.object({ type: z.literal("memory.get"), requestId: RequestId, projectId: Id.optional(), sessionId: Id.optional() }),
  z.object({ type: z.literal("memory.add"), requestId: RequestId, target: MemoryTarget, content: z.string().trim().min(1).max(20_000) }),
  z.object({
    type: z.literal("memory.replace"),
    requestId: RequestId,
    target: MemoryTarget,
    oldText: z.string().min(1).max(20_000),
    newContent: z.string().trim().min(1).max(20_000),
  }),
  z.object({ type: z.literal("memory.remove"), requestId: RequestId, target: MemoryTarget, oldText: z.string().min(1).max(20_000) }),
  z.object({
    type: z.literal("memory.config.update"),
    requestId: RequestId,
    enabled: z.boolean().optional(),
    memoryCharLimit: z.number().int().min(500).max(50_000).optional(),
    userCharLimit: z.number().int().min(500).max(50_000).optional(),
    dreamingEnabled: z.boolean().optional(),
    dreamingMinHours: z.number().int().min(1).max(720).optional(),
    dreamingMinTurns: z.number().int().min(1).max(500).optional(),
    dreamingModelId: z.string().min(1).nullable().optional(),
  }),
  z.object({ type: z.literal("packages.install"), requestId: RequestId, source: z.string().trim().min(1), scope: PackageScope, projectId: Id.optional() }),
  z.object({ type: z.literal("packages.remove"), requestId: RequestId, source: z.string().trim().min(1), scope: PackageScope, projectId: Id.optional() }),
  z.object({ type: z.literal("packages.update"), requestId: RequestId, source: z.string().trim().min(1).optional(), projectId: Id.optional() }),
  z.object({ type: z.literal("workspace.reload"), requestId: RequestId, projectId: Id.optional(), sessionId: Id.optional() }),
  z.object({ type: z.literal("usage.summary"), requestId: RequestId, range: UsageRange, onlyConfigured: z.boolean().optional() }),
  z.object({ type: z.literal("quotas.list"), requestId: RequestId, refresh: z.boolean().optional() }),
  z.object({ type: z.literal("sessions.list"), requestId: RequestId }),
  z.object({
    type: z.literal("session.create"),
    requestId: RequestId,
    projectId: Id,
    modelId: Id.optional(),
    thinkingLevel: ThinkingLevel.optional(),
    approvalMode: ApprovalMode.optional(),
  }),
  z.object({ type: z.literal("session.open"), ...SessionRef, slot: WatchSlot.optional(), limit: z.number().int().positive().max(1000).optional() }),
  z.object({ type: z.literal("session.unwatch"), requestId: RequestId, slot: WatchSlot.optional() }),
  z.object({ type: z.literal("session.earlier"), ...SessionRef, beforeId: Id, limit: z.number().int().positive().max(1000) }),
  z.object({ type: z.literal("session.search"), ...SessionRef, query: z.string().trim().min(1).max(500), offset: z.number().int().nonnegative() }),
  z.object({ type: z.literal("session.turns"), ...SessionRef, offset: z.number().int().nonnegative() }),
  z.object({ type: z.literal("session.around"), ...SessionRef, itemId: Id }),
  z.object({
    type: z.literal("session.prompt"),
    ...SessionRef,
    text: z.string(),
    images: z.array(ImageInput).max(10).optional(),
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
  z.object({ type: z.literal("session.fork"), ...SessionRef, itemId: Id.optional() }),
  z.object({ type: z.literal("session.compact"), ...SessionRef }),
  z.object({ type: z.literal("session.rewind"), ...SessionRef, itemId: Id }),
  z.object({ type: z.literal("session.respond"), ...SessionRef, pendingId: Id, answer: PendingAnswer }),
  z.object({
    type: z.literal("session.queue.update"),
    ...SessionRef,
    action: z.enum(["remove", "promote", "clear"]),
    kind: z.enum(["steering", "followUp"]).optional(),
    index: z.number().int().nonnegative().optional(),
  }),
  z.object({ type: z.literal("session.retry"), ...SessionRef }),
  z.object({ type: z.literal("commands.list"), requestId: RequestId, projectId: Id, sessionId: Id.optional() }),
  z.object({ type: z.literal("session.reload"), ...SessionRef }),
  z.object({ type: z.literal("files.search"), requestId: RequestId, projectId: Id, query: z.string(), limit: z.number().int().positive().max(200) }),
  z.object({ type: z.literal("files.list"), requestId: RequestId, projectId: Id, refresh: z.boolean().optional() }),
  z.object({ type: z.literal("files.read"), requestId: RequestId, projectId: Id, path: z.string().min(1) }),
  z.object({ type: z.literal("git.status"), requestId: RequestId, projectId: Id }),
  z.object({ type: z.literal("git.diff"), requestId: RequestId, projectId: Id, path: z.string().min(1), staged: z.boolean().optional() }),
]);
export type ClientCommand = z.infer<typeof ClientCommand>;
export type CommandType = ClientCommand["type"];

type Empty = Record<string, never>;

export interface CommandResults {
  ping: { ok: true };
  "projects.list": { projects: Project[] };
  "projects.add": { project: Project };
  "projects.remove": Empty;
  "models.list": { models: ModelInfo[]; defaultModelId?: string };
  "provider.models.set": { models: ModelInfo[]; defaultModelId?: string };
  "settings.get": { settings: CoreSettings };
  "settings.update": { settings: CoreSettings };
  "providers.list": { providers: ProviderAuthInfo[] };
  "auth.login.start": { loginId: string };
  "auth.login.respond": Empty;
  "auth.login.cancel": Empty;
  "auth.logout": Empty;
  "mcp.list": { config: McpConfigSnapshot };
  "mcp.upsert": { config: McpConfigSnapshot };
  "mcp.toggle": { config: McpConfigSnapshot };
  "mcp.remove": { config: McpConfigSnapshot };
  "capabilities.get": { capabilities: CapabilitiesSnapshot };
  "memory.get": { memory: MemorySnapshot };
  "memory.add": { memory: MemorySnapshot };
  "memory.replace": { memory: MemorySnapshot };
  "memory.remove": { memory: MemorySnapshot };
  "memory.config.update": { memory: MemorySnapshot };
  "packages.install": { capabilities: CapabilitiesSnapshot };
  "packages.remove": { capabilities: CapabilitiesSnapshot };
  "packages.update": { capabilities: CapabilitiesSnapshot };
  "workspace.reload": Empty;
  "usage.summary": { summary: UsageSummary };
  "quotas.list": { quotas: ProviderQuotaItem[] };
  "sessions.list": { sessions: SessionSummary[] };
  "session.create": { session: SessionSummary };
  "session.open": { items: TranscriptItem[]; hasEarlier: boolean; state: SessionState };
  "session.unwatch": Empty;
  "session.earlier": { items: TranscriptItem[]; hasEarlier: boolean };
  "session.search": TranscriptSearchResults;
  "session.turns": TranscriptTurns;
  "session.around": { items: TranscriptItem[] };
  "session.prompt": Empty;
  "session.abort": Empty;
  "session.configure": { state: SessionState };
  "session.rename": { session: SessionSummary };
  "session.delete": Empty;
  "session.fork": { session: SessionSummary };
  "session.compact": Empty;
  "session.rewind": { text: string };
  "session.respond": Empty;
  "session.queue.update": { state: SessionState };
  "session.retry": Empty;
  "commands.list": { commands: SlashCommand[] };
  "session.reload": Empty;
  "files.search": { files: string[] };
  "files.list": { files: string[]; truncated: boolean };
  "files.read": { file: ProjectFileContent };
  "git.status": { status: GitStatus };
  "git.diff": { diff: GitFileDiff };
}

export type ServerMessage =
  | { type: "hello"; protocolVersion: number; coreVersion: string }
  | { type: "response"; requestId: string; ok: true; data: unknown }
  | { type: "response"; requestId: string; ok: false; error: string }
  | { type: "projects"; projects: Project[] }
  | { type: "session"; session: SessionSummary }
  | { type: "session.removed"; sessionId: string }
  | { type: "item"; sessionId: string; item: TranscriptItem }
  | { type: "delta"; sessionId: string; itemId: string; channel: "text" | "thinking"; delta: string }
  | { type: "state"; sessionId: string; state: SessionState }
  | { type: "reset"; sessionId: string }
  | { type: "auth.login"; loginId: string; providerId: string; event: AuthLoginEvent }
  | { type: "notify"; sessionId?: string; level: "info" | "warning" | "error"; message: string }
  | { type: "error"; code: string; message: string };

export type ServerMessageType = ServerMessage["type"];
