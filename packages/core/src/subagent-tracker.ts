import type { SubagentProgress, SubagentStatus, SubagentToolInfo } from "@lila/protocol";
import type { SubagentBusEvent } from "./lila-extension.ts";
import { buildSubagentToolInfo, isSubagentNoise, normalizeSubagentStatus, type ToolItem } from "./transcript.ts";

const SUBAGENT_PUSH_INTERVAL_MS = 200;
const MAX_SUBAGENT_ENTRIES = 50;
/** Safety cap so an orphaned background subagent whose finished event was lost cannot lock session idle disposal forever. */
const STALE_ACTIVE_MS = 30 * 60_000;

export function isActiveSubagent(status: SubagentStatus): boolean {
  return status === "queued" || status === "running";
}

type TrackedSubagent = {
  id: string;
  type: string;
  description: string;
  status: SubagentStatus;
  background: boolean;
  startedAt: number;
  lastUpdatedAt: number;
  activity?: string;
  model?: string;
  durationMs?: number;
  output?: string;
  error?: string;
  // Independent metric sources so incremental `usage` events and cumulative tool `details` snapshots never double-count.
  usageTurns: number;
  usageTokens: number;
  usageCost: number;
  detailsTurns: number;
  detailsToolUses: number;
  detailsCost: number;
  finalToolUses: number;
  finalTokens: number;
  finalCost: number;
};

function toProgress(entry: TrackedSubagent): SubagentProgress {
  return {
    id: entry.id,
    type: entry.type,
    description: entry.description,
    status: entry.status,
    background: entry.background,
    startedAt: entry.startedAt,
    toolUses: Math.max(entry.detailsToolUses, entry.finalToolUses),
    turns: Math.max(entry.usageTurns, entry.detailsTurns),
    tokens: Math.max(entry.usageTokens, entry.finalTokens),
    cost: Math.max(entry.usageCost, entry.detailsCost, entry.finalCost),
    ...(entry.model ? { model: entry.model } : {}),
    ...(entry.activity && isActiveSubagent(entry.status) ? { activity: entry.activity } : {}),
    ...(entry.durationMs !== undefined ? { durationMs: entry.durationMs } : {}),
    ...(entry.output ? { output: entry.output } : {}),
    ...(entry.error ? { error: entry.error } : {}),
  };
}

function createTracked(init: {
  id: string;
  type: string;
  description: string;
  status: SubagentStatus;
  background: boolean;
  now: number;
}): TrackedSubagent {
  return {
    id: init.id,
    type: init.type,
    description: init.description,
    status: init.status,
    background: init.background,
    startedAt: init.now,
    lastUpdatedAt: init.now,
    usageTurns: 0,
    usageTokens: 0,
    usageCost: 0,
    detailsTurns: 0,
    detailsToolUses: 0,
    detailsCost: 0,
    finalToolUses: 0,
    finalTokens: 0,
    finalCost: 0,
  };
}

export interface SubagentTrackerCallbacks {
  onStateChanged(activeChanged: boolean): void;
  onSubsessionsChanged(): void;
}

/** Tracks live top-level sub-agent progress from `pi-subagents` bus events and `Agent` tool updates. */
export class SubagentTracker {
  readonly #entries = new Map<string, TrackedSubagent>();
  readonly #callbacks: SubagentTrackerCallbacks;
  #pushTimer: NodeJS.Timeout | undefined;
  #pendingActiveChange = false;
  #lastHasActive = false;

  constructor(callbacks: SubagentTrackerCallbacks) {
    this.#callbacks = callbacks;
  }

  get size(): number {
    return this.#entries.size;
  }

  /** Reaps orphaned active entries past the staleness cap and reports whether any sub-agent is still active. */
  hasActive(now = Date.now()): boolean {
    let reaped = false;
    let active = false;
    for (const entry of this.#entries.values()) {
      if (!isActiveSubagent(entry.status)) continue;
      if (now - entry.lastUpdatedAt > STALE_ACTIVE_MS) {
        entry.status = "stopped";
        delete entry.activity;
        entry.error ??= "子 Agent 长时间无响应，已停止跟踪";
        reaped = true;
      } else {
        active = true;
      }
    }
    if (reaped) this.#schedulePush(true);
    return active;
  }

  snapshot(): SubagentProgress[] | undefined {
    if (!this.#entries.size) return undefined;
    return Array.from(this.#entries.values(), toProgress);
  }

  byPrefix(idPrefix: string): SubagentProgress | undefined {
    const lower = idPrefix.toLowerCase();
    for (const entry of this.#entries.values()) {
      if (entry.id.toLowerCase().startsWith(lower)) return toProgress(entry);
    }
    return undefined;
  }

  /** Called when the parent agent run settles: closes any orphaned foreground sub-agents that never emitted a finished event. */
  onAgentSettled(now = Date.now()): void {
    let changed = false;
    for (const entry of this.#entries.values()) {
      if (!isActiveSubagent(entry.status)) continue;
      if (!entry.background || now - entry.lastUpdatedAt > STALE_ACTIVE_MS) {
        entry.status = "stopped";
        delete entry.activity;
        entry.durationMs ??= Math.max(0, now - entry.startedAt);
        changed = true;
      }
    }
    if (changed) this.#schedulePush(true);
  }

  onEvent(event: SubagentBusEvent): void {
    const now = Date.now();
    switch (event.type) {
      case "created": {
        const existing = this.#entries.get(event.id);
        if (existing) {
          existing.type = event.agentType || existing.type;
          existing.description = event.description || existing.description;
          existing.background = event.isBackground;
          if (!isActiveSubagent(existing.status)) existing.status = "queued";
          existing.lastUpdatedAt = now;
        } else {
          this.#entries.set(
            event.id,
            createTracked({
              id: event.id,
              type: event.agentType || "Agent",
              description: event.description || "",
              status: "queued",
              background: event.isBackground,
              now,
            }),
          );
        }
        this.#schedulePush(true);
        break;
      }
      case "started": {
        const existing = this.#entries.get(event.id);
        if (existing) {
          existing.type = event.agentType || existing.type;
          existing.description = event.description || existing.description;
          existing.status = "running";
          existing.startedAt = now;
          existing.lastUpdatedAt = now;
        } else {
          this.#entries.set(
            event.id,
            createTracked({
              id: event.id,
              type: event.agentType || "Agent",
              description: event.description || "",
              status: "running",
              background: false,
              now,
            }),
          );
        }
        this.#trim();
        this.#schedulePush(true);
        this.#callbacks.onSubsessionsChanged();
        break;
      }
      case "usage": {
        if (event.depth !== undefined && event.depth > 1) return;
        let existing = this.#entries.get(event.id);
        if (!existing) {
          existing = createTracked({
            id: event.id,
            type: event.agentType || "Agent",
            description: event.description || "",
            status: "running",
            background: false,
            now,
          });
          this.#entries.set(event.id, existing);
        }
        const firstTurn = existing.usageTurns === 0;
        existing.usageTurns += 1;
        existing.usageTokens += event.tokens;
        existing.usageCost += event.cost;
        if (event.model) existing.model = event.model;
        if (existing.status === "queued") existing.status = "running";
        existing.lastUpdatedAt = now;
        this.#schedulePush(false);
        // Sub-agent session JSONL is flushed once its first message is written.
        if (firstTurn) this.#callbacks.onSubsessionsChanged();
        break;
      }
      case "compacted": {
        const existing = this.#entries.get(event.id);
        if (!existing) return;
        existing.activity = event.compactionCount ? `已压缩上下文（第 ${event.compactionCount} 次）` : "已压缩上下文";
        existing.lastUpdatedAt = now;
        this.#schedulePush(false);
        break;
      }
      case "steered": {
        const existing = this.#entries.get(event.id);
        if (!existing) return;
        existing.activity = "已收到引导消息";
        existing.lastUpdatedAt = now;
        this.#schedulePush(false);
        break;
      }
      case "finished": {
        const existing =
          this.#entries.get(event.id) ??
          createTracked({
            id: event.id,
            type: event.agentType || "Agent",
            description: event.description || "",
            status: normalizeSubagentStatus(event.status),
            background: false,
            now,
          });
        existing.type = event.agentType || existing.type;
        existing.description = event.description || existing.description;
        existing.status = normalizeSubagentStatus(event.status);
        delete existing.activity;
        existing.lastUpdatedAt = now;
        if (event.toolUses !== undefined) existing.finalToolUses = Math.max(existing.finalToolUses, event.toolUses);
        if (event.tokens !== undefined) existing.finalTokens = Math.max(existing.finalTokens, event.tokens);
        if (event.cost !== undefined) existing.finalCost = Math.max(existing.finalCost, event.cost);
        if (event.durationMs !== undefined) existing.durationMs = event.durationMs;
        if (event.result && !isSubagentNoise(event.result)) existing.output = event.result;
        if (event.error) existing.error = event.error;
        this.#entries.set(event.id, existing);
        this.#trim();
        this.#schedulePush(true);
        this.#callbacks.onSubsessionsChanged();
        break;
      }
      case "disposed":
        if (!this.#entries.size) return;
        this.#entries.clear();
        this.#schedulePush(true);
        break;
    }
  }

  /** Syncs progress from an `Agent` tool item and returns the item enriched with the latest typed `subagent` info. */
  syncToolItem(item: ToolItem): ToolItem {
    if (item.name !== "Agent") return item;
    const now = Date.now();
    const details = item.details;
    const id = item.subagent?.id ?? (typeof details?.agentId === "string" && details.agentId ? details.agentId : undefined);
    if (!id) {
      const subagent = item.subagent ?? buildSubagentToolInfo(item.args, details, item.status, item.output, item.durationMs);
      return { ...item, subagent };
    }

    const existing = this.#entries.get(id);
    const rawStatus = typeof details?.status === "string" ? details.status : undefined;
    const incoming = rawStatus
      ? normalizeSubagentStatus(rawStatus)
      : existing?.status ?? (item.status === "running" ? "running" : item.status === "error" ? "error" : "completed");
    // Do not regress a sub-agent that has already finished back to running when its spawn tool result lands.
    const status = existing && !isActiveSubagent(existing.status) && isActiveSubagent(incoming) ? existing.status : incoming;

    const entry =
      existing ??
      createTracked({
        id,
        type:
          (typeof details?.subagentType === "string" && details.subagentType) ||
          (typeof item.args.subagent_type === "string" && item.args.subagent_type) ||
          "Agent",
        description:
          (typeof details?.description === "string" && details.description) ||
          (typeof item.args.description === "string" && item.args.description) ||
          "",
        status,
        background: rawStatus === "background" || Boolean(item.args.run_in_background),
        now,
      });

    entry.type =
      (typeof details?.subagentType === "string" && details.subagentType) ||
      (typeof item.args.subagent_type === "string" && item.args.subagent_type) ||
      entry.type;
    entry.description =
      (typeof details?.description === "string" && details.description) ||
      (typeof item.args.description === "string" && item.args.description) ||
      entry.description;
    entry.status = status;
    entry.background = rawStatus === "background" || Boolean(item.args.run_in_background) || entry.background;
    entry.lastUpdatedAt = now;

    if (typeof details?.toolUses === "number" && details.toolUses > entry.detailsToolUses) entry.detailsToolUses = details.toolUses;
    if (typeof details?.turnCount === "number" && details.turnCount > entry.detailsTurns) entry.detailsTurns = details.turnCount;
    if (typeof details?.cost === "number" && details.cost > entry.detailsCost) entry.detailsCost = details.cost;
    if (typeof details?.modelName === "string" && details.modelName) entry.model = details.modelName;
    if (isActiveSubagent(status)) {
      if (typeof details?.activity === "string" && details.activity) entry.activity = details.activity;
    } else {
      delete entry.activity;
      if (typeof details?.durationMs === "number" && details.durationMs > 0) entry.durationMs = details.durationMs;
    }
    if (!isSubagentNoise(item.output)) entry.output = item.output;
    if (typeof details?.error === "string" && details.error) entry.error = details.error;

    this.#entries.set(id, entry);
    this.#trim();
    this.#schedulePush(!isActiveSubagent(status));

    const progress = toProgress(entry);
    const base = item.subagent ?? buildSubagentToolInfo(item.args, details, item.status, item.output, item.durationMs);
    const subagent: SubagentToolInfo = {
      ...base,
      id: progress.id,
      type: progress.type,
      description: progress.description || base.description,
      background: progress.background,
      status: progress.status,
      ...(progress.activity ? { activity: progress.activity } : {}),
      ...(progress.model ? { model: progress.model } : {}),
      ...(progress.turns > 0 ? { turns: progress.turns } : {}),
      ...(progress.toolUses > 0 ? { toolUses: progress.toolUses } : {}),
      ...(progress.tokens > 0 ? { tokens: progress.tokens } : {}),
      ...(progress.cost > 0 ? { cost: progress.cost } : {}),
      ...(progress.durationMs !== undefined ? { durationMs: progress.durationMs } : {}),
      ...(progress.output ? { output: progress.output } : {}),
      ...(progress.error ? { error: progress.error } : {}),
    };
    if (!isActiveSubagent(subagent.status)) delete subagent.activity;
    return { ...item, subagent };
  }

  dispose(): void {
    clearTimeout(this.#pushTimer);
    this.#pushTimer = undefined;
    this.#entries.clear();
    this.#lastHasActive = false;
  }

  #trim(): void {
    if (this.#entries.size <= MAX_SUBAGENT_ENTRIES) return;
    for (const [id, entry] of this.#entries) {
      if (this.#entries.size <= MAX_SUBAGENT_ENTRIES) break;
      if (!isActiveSubagent(entry.status)) this.#entries.delete(id);
    }
  }

  #computeHasActive(): boolean {
    for (const entry of this.#entries.values()) if (isActiveSubagent(entry.status)) return true;
    return false;
  }

  #schedulePush(immediate: boolean): void {
    const active = this.#computeHasActive();
    if (active !== this.#lastHasActive) {
      this.#lastHasActive = active;
      this.#pendingActiveChange = true;
      immediate = true;
    }
    if (immediate) {
      clearTimeout(this.#pushTimer);
      this.#pushTimer = undefined;
      const activeChanged = this.#pendingActiveChange;
      this.#pendingActiveChange = false;
      this.#callbacks.onStateChanged(activeChanged);
      return;
    }
    if (!this.#pushTimer) {
      this.#pushTimer = setTimeout(() => {
        this.#pushTimer = undefined;
        const activeChanged = this.#pendingActiveChange;
        this.#pendingActiveChange = false;
        this.#callbacks.onStateChanged(activeChanged);
      }, SUBAGENT_PUSH_INTERVAL_MS);
      this.#pushTimer.unref();
    }
  }
}
