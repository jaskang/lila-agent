import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { Cron } from "croner";
import {
  DEFAULT_APPROVAL_MODE,
  DEFAULT_SCHEDULE_SANDBOX_MODE,
  SandboxMode,
  type ScheduleCreateInput,
  type ScheduledJob,
  type ScheduleKind,
  type ScheduleRunRecord,
  type ScheduleUpdatePatch,
} from "@lila/protocol";
import { errorMessage, UserError } from "./errors.ts";

const MAX_RECENT_RUNS = 10;
const MIN_INTERVAL_MS = 10_000;
const MAX_TIMEOUT_MS = 2_147_483_647;
const UNIT_MS: Record<string, number> = {
  s: 1_000,
  m: 60_000,
  h: 3_600_000,
  d: 86_400_000,
};

export interface ParsedSchedule {
  kind: ScheduleKind;
  normalized: string;
  intervalMs?: number;
  nextRunAt?: string;
}

/** Parses and validates a cron expression (5 or 6 fields), fixed interval ("15m"/"1h"), or one-shot ("+30m"/ISO). */
export function parseScheduleExpression(raw: string, now = Date.now()): ParsedSchedule {
  const trimmed = raw.trim();
  if (!trimmed) throw new UserError("调度表达式不能为空");

  const relMatch = trimmed.match(/^\+(\d+)(s|m|h|d)$/i);
  if (relMatch) {
    const amount = Number(relMatch[1]);
    const unit = UNIT_MS[relMatch[2]!.toLowerCase()]!;
    const ms = amount * unit;
    if (!Number.isFinite(ms) || ms < MIN_INTERVAL_MS) {
      throw new UserError("单次相对定时至少需要 10 秒（例如 +10m、+1h）");
    }
    const iso = new Date(now + ms).toISOString();
    return { kind: "once", normalized: iso, nextRunAt: iso };
  }

  const intervalMatch = trimmed.match(/^(\d+)(s|m|h|d)$/i);
  if (intervalMatch) {
    const amount = Number(intervalMatch[1]);
    const unit = UNIT_MS[intervalMatch[2]!.toLowerCase()]!;
    const ms = amount * unit;
    if (!Number.isFinite(ms) || ms < MIN_INTERVAL_MS) {
      throw new UserError("周期定时间隔至少需要 10 秒（例如 15m、1h、1d）");
    }
    return {
      kind: "interval",
      normalized: `${amount}${intervalMatch[2]!.toLowerCase()}`,
      intervalMs: ms,
      nextRunAt: new Date(now + ms).toISOString(),
    };
  }

  if (/^\d{4}-\d{2}-\d{2}T/.test(trimmed)) {
    const date = new Date(trimmed);
    if (Number.isNaN(date.getTime())) throw new UserError(`无效的时间格式：${trimmed}`);
    if (date.getTime() <= now) throw new UserError(`单次定时时间已过：${date.toISOString()}`);
    const iso = date.toISOString();
    return { kind: "once", normalized: iso, nextRunAt: iso };
  }

  const fields = trimmed.split(/\s+/);
  if (fields.length !== 5 && fields.length !== 6) {
    throw new UserError(
      `无法识别的调度表达式「${trimmed}」。支持 5/6 段 Cron（如 "0 9 * * 1-5"）、周期（如 "15m"/"1h"）或单次（如 "+30m" 或 ISO 时间）。`,
    );
  }
  try {
    const probe = new Cron(trimmed, { paused: true });
    const next = probe.nextRun(new Date(now));
    probe.stop();
    if (!next) throw new UserError(`Cron 表达式「${trimmed}」未来不会再触发`);
    return { kind: "cron", normalized: trimmed, nextRunAt: next.toISOString() };
  } catch (error) {
    if (error instanceof UserError) throw error;
    throw new UserError(`Cron 表达式无效：${errorMessage(error)}`);
  }
}

export interface SchedulerHost {
  hasProject(projectId: string): boolean;
  executeJob(job: ScheduledJob, trigger: "schedule" | "manual"): Promise<{ sessionId?: string; skipped?: string }>;
  onChange(schedules: ScheduledJob[]): void;
}

type PersistedSchedules = { version: 1; jobs: ScheduledJob[] };

/** Persistent, core-level background scheduler powered by `croner`. Survives idle session disposal and core restarts. */
export class SchedulerService {
  readonly #path: string;
  readonly #host: SchedulerHost;
  readonly #jobs = new Map<string, ScheduledJob>();
  readonly #crons = new Map<string, Cron>();
  readonly #timers = new Map<string, NodeJS.Timeout>();
  readonly #running = new Set<string>();
  #disposed = false;

  constructor(dataDir: string, host: SchedulerHost) {
    mkdirSync(dataDir, { recursive: true, mode: 0o700 });
    this.#path = join(dataDir, "schedules.json");
    this.#host = host;
    this.#load();
    this.#armAll();
  }

  list(): ScheduledJob[] {
    return Array.from(this.#jobs.values(), (job) => this.#withComputedNextRun(job)).sort(
      (a, b) => Date.parse(b.updatedAt) - Date.parse(a.updatedAt),
    );
  }

  get(id: string): ScheduledJob | undefined {
    const job = this.#jobs.get(id);
    return job ? this.#withComputedNextRun(job) : undefined;
  }

  create(input: ScheduleCreateInput): ScheduledJob {
    if (!this.#host.hasProject(input.projectId)) throw new UserError("所选项目不存在，可能已被移除");
    const parsed = parseScheduleExpression(input.schedule);
    const now = new Date().toISOString();
    const enabled = input.enabled ?? true;
    const job: ScheduledJob = {
      id: randomUUID(),
      name: input.name.trim(),
      projectId: input.projectId,
      schedule: parsed.normalized,
      scheduleKind: parsed.kind,
      ...(parsed.intervalMs !== undefined ? { intervalMs: parsed.intervalMs } : {}),
      prompt: input.prompt.trim(),
      enabled,
      sessionMode: input.sessionMode ?? "new",
      ...(input.sessionId ? { sessionId: input.sessionId } : {}),
      ...(input.modelId ? { modelId: input.modelId } : {}),
      ...(input.thinkingLevel ? { thinkingLevel: input.thinkingLevel } : {}),
      approvalMode: input.approvalMode ?? DEFAULT_APPROVAL_MODE,
      sandboxMode: input.sandboxMode ?? DEFAULT_SCHEDULE_SANDBOX_MODE,
      createdAt: now,
      updatedAt: now,
      ...(enabled && parsed.nextRunAt ? { nextRunAt: parsed.nextRunAt } : {}),
      lastStatus: "idle",
      runCount: 0,
      recentRuns: [],
    };
    this.#jobs.set(job.id, job);
    if (job.enabled) this.#arm(job);
    this.#saveAndBroadcast();
    return this.#withComputedNextRun(job);
  }

  update(id: string, patch: ScheduleUpdatePatch): ScheduledJob {
    const existing = this.#jobs.get(id);
    if (!existing) throw new UserError("定时任务不存在");

    const nextProjectId = patch.projectId ?? existing.projectId;
    if (!this.#host.hasProject(nextProjectId)) throw new UserError("所选项目不存在，可能已被移除");

    let schedule = existing.schedule;
    let scheduleKind = existing.scheduleKind;
    let intervalMs = existing.intervalMs;
    if (patch.schedule !== undefined) {
      const parsed = parseScheduleExpression(patch.schedule);
      schedule = parsed.normalized;
      scheduleKind = parsed.kind;
      intervalMs = parsed.intervalMs;
    } else if (patch.enabled === true && !existing.enabled && existing.scheduleKind === "once") {
      // Re-enabling a one-shot job requires a future timestamp.
      parseScheduleExpression(existing.schedule);
    }

    const updated: ScheduledJob = {
      ...existing,
      name: patch.name !== undefined ? patch.name.trim() : existing.name,
      projectId: nextProjectId,
      schedule,
      scheduleKind,
      prompt: patch.prompt !== undefined ? patch.prompt.trim() : existing.prompt,
      enabled: patch.enabled ?? existing.enabled,
      sessionMode: patch.sessionMode ?? existing.sessionMode,
      approvalMode: patch.approvalMode ?? existing.approvalMode,
      sandboxMode: patch.sandboxMode ?? existing.sandboxMode,
      updatedAt: new Date().toISOString(),
    };

    if (intervalMs !== undefined) updated.intervalMs = intervalMs;
    else delete updated.intervalMs;

    if (patch.sessionId === null || updated.sessionMode === "new") delete updated.sessionId;
    else if (patch.sessionId !== undefined) updated.sessionId = patch.sessionId;

    if (patch.modelId === null) delete updated.modelId;
    else if (patch.modelId !== undefined) updated.modelId = patch.modelId;

    if (patch.thinkingLevel === null) delete updated.thinkingLevel;
    else if (patch.thinkingLevel !== undefined) updated.thinkingLevel = patch.thinkingLevel;

    this.#disarm(id);
    this.#jobs.set(id, updated);
    if (updated.enabled) this.#arm(updated);
    else delete updated.nextRunAt;

    this.#saveAndBroadcast();
    return this.#withComputedNextRun(updated);
  }

  delete(id: string): void {
    if (!this.#jobs.has(id)) throw new UserError("定时任务不存在");
    this.#disarm(id);
    this.#jobs.delete(id);
    this.#saveAndBroadcast();
  }

  /** Removes jobs bound to a deleted project so timers never fire against missing directories. */
  removeByProject(projectId: string): void {
    let changed = false;
    for (const [id, job] of Array.from(this.#jobs.entries())) {
      if (job.projectId !== projectId) continue;
      this.#disarm(id);
      this.#jobs.delete(id);
      changed = true;
    }
    if (changed) this.#saveAndBroadcast();
  }

  /** Clears a bound reusable session when that session is deleted; the next run will create a fresh one. */
  forgetSession(sessionId: string): void {
    let changed = false;
    for (const job of this.#jobs.values()) {
      if (job.sessionId !== sessionId) continue;
      delete job.sessionId;
      changed = true;
    }
    if (changed) this.#saveAndBroadcast();
  }

  /** Updates the bound session id for reuse-mode jobs after the host creates or recreates it. */
  bindSession(jobId: string, sessionId: string): void {
    const job = this.#jobs.get(jobId);
    if (!job || job.sessionMode !== "reuse" || job.sessionId === sessionId) return;
    job.sessionId = sessionId;
    job.updatedAt = new Date().toISOString();
    this.#saveAndBroadcast();
  }

  async runNow(id: string): Promise<{ schedule: ScheduledJob; sessionId?: string }> {
    const job = this.#jobs.get(id);
    if (!job) throw new UserError("定时任务不存在");
    const sessionId = await this.#trigger(id, "manual");
    return { schedule: this.get(id) ?? job, ...(sessionId ? { sessionId } : {}) };
  }

  dispose(): void {
    this.#disposed = true;
    for (const id of Array.from(this.#crons.keys())) this.#disarm(id);
    for (const id of Array.from(this.#timers.keys())) this.#disarm(id);
  }

  #armAll(): void {
    let dirty = false;
    for (const job of this.#jobs.values()) {
      if (!job.enabled) continue;
      if (!this.#host.hasProject(job.projectId)) {
        job.enabled = false;
        job.lastStatus = "error";
        job.lastError = "关联的项目已不存在，已自动停用";
        delete job.nextRunAt;
        dirty = true;
        continue;
      }
      if (!this.#arm(job)) dirty = true;
    }
    if (dirty) this.#save();
  }

  #arm(job: ScheduledJob): boolean {
    if (this.#disposed) return true;
    this.#disarm(job.id);
    try {
      if (job.scheduleKind === "interval" && job.intervalMs) {
        const ms = Math.max(MIN_INTERVAL_MS, job.intervalMs);
        const timer = setInterval(() => void this.#trigger(job.id, "schedule"), ms);
        timer.unref();
        this.#timers.set(job.id, timer);
        job.nextRunAt = new Date(Date.now() + ms).toISOString();
        return true;
      }
      if (job.scheduleKind === "once") {
        const targetMs = Date.parse(job.schedule);
        const delay = targetMs - Date.now();
        if (!Number.isFinite(targetMs) || delay <= 0) {
          job.enabled = false;
          job.lastStatus = "error";
          job.lastError = "单次定时时间已过期，未补跑";
          delete job.nextRunAt;
          return false;
        }
        job.nextRunAt = new Date(targetMs).toISOString();
        this.#armOnce(job.id, targetMs);
        return true;
      }
      const cron = new Cron(job.schedule, { protect: true }, () => void this.#trigger(job.id, "schedule"));
      this.#crons.set(job.id, cron);
      const next = cron.nextRun();
      if (next) job.nextRunAt = next.toISOString();
      else delete job.nextRunAt;
      return true;
    } catch (error) {
      job.enabled = false;
      job.lastStatus = "error";
      job.lastError = errorMessage(error);
      delete job.nextRunAt;
      return false;
    }
  }

  #armOnce(jobId: string, targetMs: number): void {
    const remaining = targetMs - Date.now();
    if (remaining <= 0) {
      void this.#trigger(jobId, "schedule");
      return;
    }
    const step = Math.min(remaining, MAX_TIMEOUT_MS);
    const timer = setTimeout(() => {
      this.#timers.delete(jobId);
      if (this.#disposed) return;
      const job = this.#jobs.get(jobId);
      if (!job?.enabled) return;
      if (Date.now() + 50 < targetMs) {
        this.#armOnce(jobId, targetMs);
        return;
      }
      job.enabled = false;
      delete job.nextRunAt;
      this.#saveAndBroadcast();
      void this.#trigger(jobId, "schedule");
    }, step);
    timer.unref();
    this.#timers.set(jobId, timer);
  }

  #disarm(id: string): void {
    const cron = this.#crons.get(id);
    if (cron) {
      cron.stop();
      this.#crons.delete(id);
    }
    const timer = this.#timers.get(id);
    if (timer) {
      clearTimeout(timer);
      clearInterval(timer);
      this.#timers.delete(id);
    }
  }

  async #trigger(id: string, trigger: "schedule" | "manual"): Promise<string | undefined> {
    if (this.#disposed) return undefined;
    const job = this.#jobs.get(id);
    if (!job) return undefined;
    if (trigger === "schedule" && !job.enabled && job.scheduleKind !== "once") return undefined;

    const startedAt = new Date().toISOString();
    const record: ScheduleRunRecord = {
      id: randomUUID(),
      startedAt,
      status: "running",
      trigger,
    };

    if (this.#running.has(id)) {
      record.status = "skipped";
      record.finishedAt = startedAt;
      record.error = "上一次执行尚未结束，已跳过本次触发";
      job.lastStatus = "skipped";
      job.lastError = record.error;
      job.recentRuns = [record, ...job.recentRuns].slice(0, MAX_RECENT_RUNS);
      this.#saveAndBroadcast();
      return undefined;
    }

    this.#running.add(id);
    job.lastRunAt = startedAt;
    job.lastStatus = "running";
    delete job.lastError;
    job.recentRuns = [record, ...job.recentRuns].slice(0, MAX_RECENT_RUNS);
    this.#saveAndBroadcast();

    try {
      const result = await this.#host.executeJob(this.#withComputedNextRun(job), trigger);
      const current = this.#jobs.get(id);
      if (!current) return result.sessionId;
      const finishedAt = new Date().toISOString();
      record.finishedAt = finishedAt;
      if (result.sessionId) {
        record.sessionId = result.sessionId;
        current.lastSessionId = result.sessionId;
      }
      if (result.skipped) {
        record.status = "skipped";
        record.error = result.skipped;
        current.lastStatus = "skipped";
        current.lastError = result.skipped;
      } else {
        record.status = "success";
        current.lastStatus = "success";
        delete current.lastError;
        current.runCount += 1;
      }
      if (current.enabled && current.scheduleKind === "interval" && current.intervalMs) {
        current.nextRunAt = new Date(Date.now() + current.intervalMs).toISOString();
      }
      this.#saveAndBroadcast();
      return result.sessionId;
    } catch (error) {
      const current = this.#jobs.get(id);
      const message = errorMessage(error);
      record.finishedAt = new Date().toISOString();
      record.status = "error";
      record.error = message;
      if (current) {
        current.lastStatus = "error";
        current.lastError = message;
        this.#saveAndBroadcast();
      }
      if (trigger === "manual") throw error;
      return undefined;
    } finally {
      this.#running.delete(id);
    }
  }

  #withComputedNextRun(job: ScheduledJob): ScheduledJob {
    let nextRunAt = job.nextRunAt;
    if (!job.enabled) {
      nextRunAt = undefined;
    } else if (job.scheduleKind === "cron") {
      nextRunAt = this.#crons.get(job.id)?.nextRun()?.toISOString() ?? nextRunAt;
    } else if (job.scheduleKind === "once") {
      nextRunAt = job.schedule;
    }
    return {
      ...job,
      recentRuns: job.recentRuns.map((run) => ({ ...run })),
      ...(nextRunAt ? { nextRunAt } : { nextRunAt: undefined }),
    };
  }

  #load(): void {
    if (!existsSync(this.#path)) return;
    try {
      const parsed = JSON.parse(readFileSync(this.#path, "utf8")) as Partial<PersistedSchedules>;
      if (!parsed || !Array.isArray(parsed.jobs)) throw new Error("schedules.json root is invalid");
      for (const job of parsed.jobs) {
        if (!job || typeof job.id !== "string" || typeof job.projectId !== "string" || typeof job.schedule !== "string") continue;
        // Clear stale running state from a crashed core process.
        const parsedSandbox = SandboxMode.safeParse((job as { sandboxMode?: unknown }).sandboxMode);
        const normalized: ScheduledJob = {
          ...job,
          sandboxMode: parsedSandbox.success ? parsedSandbox.data : DEFAULT_SCHEDULE_SANDBOX_MODE,
          lastStatus: job.lastStatus === "running" ? "error" : (job.lastStatus ?? "idle"),
          ...(job.lastStatus === "running" ? { lastError: "上次运行期间 core 中断" } : {}),
          recentRuns: Array.isArray(job.recentRuns) ? job.recentRuns.slice(0, MAX_RECENT_RUNS) : [],
          runCount: typeof job.runCount === "number" && Number.isFinite(job.runCount) ? job.runCount : 0,
        };
        this.#jobs.set(normalized.id, normalized);
      }
    } catch (error) {
      const backup = `${this.#path}.corrupt.${Date.now()}`;
      try {
        renameSync(this.#path, backup);
        console.warn(`[core] corrupted schedules.json backed up to ${backup}`, error);
      } catch (backupError) {
        console.warn("[core] failed to back up corrupted schedules.json", backupError);
      }
    }
  }

  #save(): void {
    const payload: PersistedSchedules = { version: 1, jobs: Array.from(this.#jobs.values()) };
    const tmp = `${this.#path}.${process.pid}.tmp`;
    writeFileSync(tmp, `${JSON.stringify(payload, null, 2)}\n`, { mode: 0o600 });
    renameSync(tmp, this.#path);
  }

  #saveAndBroadcast(): void {
    this.#save();
    this.#host.onChange(this.list());
  }
}
