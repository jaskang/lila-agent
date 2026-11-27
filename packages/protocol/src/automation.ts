import { z } from "zod";
import { ApprovalMode } from "./session.js";
import { ThinkingLevel } from "./settings.js";

export const ScheduleKind = z.enum(["cron", "interval", "once"]);
export type ScheduleKind = z.infer<typeof ScheduleKind>;

export const ScheduleSessionMode = z.enum(["new", "reuse"]);
export type ScheduleSessionMode = z.infer<typeof ScheduleSessionMode>;

export const ScheduleRunStatus = z.enum(["idle", "running", "success", "error", "skipped"]);
export type ScheduleRunStatus = z.infer<typeof ScheduleRunStatus>;

export interface ScheduleRunRecord {
  id: string;
  startedAt: string;
  finishedAt?: string;
  status: Exclude<ScheduleRunStatus, "idle">;
  sessionId?: string;
  error?: string;
  trigger: "schedule" | "manual";
}

export interface ScheduledJob {
  id: string;
  name: string;
  projectId: string;
  /** Standard 5/6-field cron expression, interval ("15m", "1h", "1d"), or one-shot ("+30m" / ISO timestamp). */
  schedule: string;
  scheduleKind: ScheduleKind;
  intervalMs?: number;
  prompt: string;
  enabled: boolean;
  sessionMode: ScheduleSessionMode;
  /** Bound session when `sessionMode === "reuse"`. */
  sessionId?: string;
  modelId?: string;
  thinkingLevel?: ThinkingLevel;
  approvalMode: ApprovalMode;
  createdAt: string;
  updatedAt: string;
  lastRunAt?: string;
  nextRunAt?: string;
  lastStatus: ScheduleRunStatus;
  lastError?: string;
  lastSessionId?: string;
  runCount: number;
  recentRuns: ScheduleRunRecord[];
}

export const ScheduleCreateInput = z.object({
  name: z.string().trim().min(1).max(120),
  projectId: z.string().min(1),
  schedule: z.string().trim().min(1).max(120),
  prompt: z.string().trim().min(1).max(20_000),
  enabled: z.boolean().optional(),
  sessionMode: ScheduleSessionMode.optional(),
  sessionId: z.string().min(1).optional(),
  modelId: z.string().min(1).optional(),
  thinkingLevel: ThinkingLevel.optional(),
  approvalMode: ApprovalMode.optional(),
});
export type ScheduleCreateInput = z.infer<typeof ScheduleCreateInput>;

export const ScheduleUpdatePatch = z.object({
  name: z.string().trim().min(1).max(120).optional(),
  projectId: z.string().min(1).optional(),
  schedule: z.string().trim().min(1).max(120).optional(),
  prompt: z.string().trim().min(1).max(20_000).optional(),
  enabled: z.boolean().optional(),
  sessionMode: ScheduleSessionMode.optional(),
  sessionId: z.string().min(1).nullable().optional(),
  modelId: z.string().min(1).nullable().optional(),
  thinkingLevel: ThinkingLevel.nullable().optional(),
  approvalMode: ApprovalMode.optional(),
});
export type ScheduleUpdatePatch = z.infer<typeof ScheduleUpdatePatch>;

export const TaskStatus = z.enum(["inbox", "clarifying", "running", "review", "done"]);
export type TaskStatus = z.infer<typeof TaskStatus>;

export const TaskPriority = z.enum(["low", "normal", "high"]);
export type TaskPriority = z.infer<typeof TaskPriority>;

export const TaskSource = z.enum(["manual", "wechat", "whatsapp", "github", "asana", "schedule"]);
export type TaskSource = z.infer<typeof TaskSource>;

export const TaskLinkKind = z.enum(["design", "issue", "pr", "doc", "other"]);
export type TaskLinkKind = z.infer<typeof TaskLinkKind>;

export const TaskLink = z.object({
  url: z.string().trim().min(1).max(2_000),
  label: z.string().trim().max(160).optional(),
  kind: TaskLinkKind.optional(),
});
export type TaskLink = z.infer<typeof TaskLink>;

export interface AssistantTask {
  id: string;
  title: string;
  projectId?: string;
  status: TaskStatus;
  priority: TaskPriority;
  source: TaskSource;
  summary: string;
  links: TaskLink[];
  blockingQuestions: string[];
  draftReply?: string;
  sessionId?: string;
  createdAt: string;
  updatedAt: string;
}

export const TaskCreateInput = z.object({
  title: z.string().trim().min(1).max(200),
  projectId: z.string().min(1).optional(),
  status: TaskStatus.optional(),
  priority: TaskPriority.optional(),
  source: TaskSource.optional(),
  summary: z.string().max(40_000).optional(),
  links: z.array(TaskLink).max(30).optional(),
  blockingQuestions: z.array(z.string().trim().min(1).max(1_000)).max(20).optional(),
  draftReply: z.string().max(10_000).optional(),
});
export type TaskCreateInput = z.infer<typeof TaskCreateInput>;

export const TaskUpdatePatch = z.object({
  title: z.string().trim().min(1).max(200).optional(),
  projectId: z.string().min(1).nullable().optional(),
  status: TaskStatus.optional(),
  priority: TaskPriority.optional(),
  source: TaskSource.optional(),
  summary: z.string().max(40_000).optional(),
  links: z.array(TaskLink).max(30).optional(),
  blockingQuestions: z.array(z.string().trim().min(1).max(1_000)).max(20).optional(),
  draftReply: z.string().max(10_000).nullable().optional(),
  sessionId: z.string().min(1).nullable().optional(),
});
export type TaskUpdatePatch = z.infer<typeof TaskUpdatePatch>;
