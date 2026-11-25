import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import memoryExtension, { ENTRY_DELIMITER, MemoryStore } from "@amaster.ai/pi-memory";
import { readDreamingState } from "@amaster.ai/pi-memory/dreaming-state";
import type { ExtensionFactory } from "@earendil-works/pi-coding-agent";
import type { MemorySnapshot, MemoryStoreSnapshot, MemoryTarget } from "@lila/protocol";
import { UserError } from "./errors.ts";

const SETTINGS_KEY = "pi-memory";
const DEFAULT_MEMORY_CHAR_LIMIT = 2_200;
const DEFAULT_USER_CHAR_LIMIT = 1_375;
const DEFAULT_DREAM_MIN_HOURS = 24;
const DEFAULT_DREAM_MIN_TURNS = 5;

export interface MemoryConfigPatch {
  enabled?: boolean | undefined;
  memoryCharLimit?: number | undefined;
  userCharLimit?: number | undefined;
  dreamingEnabled?: boolean | undefined;
  dreamingMinHours?: number | undefined;
  dreamingMinTurns?: number | undefined;
  dreamingModelId?: string | null | undefined;
}

export interface ParsedMemorySettings {
  enabled: boolean;
  customDataDir?: string;
  dir: string;
  memoryCharLimit: number;
  userCharLimit: number;
  dreamingEnabled: boolean;
  dreamingMinHours: number;
  dreamingMinTurns: number;
  dreamingModelId?: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function stripBom(text: string): string {
  return text.charCodeAt(0) === 0xfeff ? text.slice(1) : text;
}

function readSettingsJson(settingsPath: string): Record<string, unknown> {
  if (!existsSync(settingsPath)) return {};
  try {
    const parsed: unknown = JSON.parse(stripBom(readFileSync(settingsPath, "utf8")));
    if (!isRecord(parsed)) throw new Error("settings.json 根节点不是对象");
    return parsed;
  } catch (error) {
    throw new UserError(`读取 settings.json 失败：${error instanceof Error ? error.message : String(error)}`);
  }
}

function writeSettingsJson(settingsPath: string, data: Record<string, unknown>): void {
  mkdirSync(dirname(settingsPath), { recursive: true });
  const tempPath = `${settingsPath}.tmp-${process.pid}-${Date.now()}`;
  writeFileSync(tempPath, `${JSON.stringify(data, null, 2)}\n`, "utf8");
  renameSync(tempPath, settingsPath);
}

function positiveInt(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) && value >= 1 ? Math.floor(value) : fallback;
}

export function readMemorySettings(agentDir: string): ParsedMemorySettings {
  const settingsPath = join(agentDir, "settings.json");
  let rawSection: Record<string, unknown> = {};
  if (existsSync(settingsPath)) {
    try {
      const parsed: unknown = JSON.parse(stripBom(readFileSync(settingsPath, "utf8")));
      if (isRecord(parsed) && isRecord(parsed[SETTINGS_KEY])) {
        rawSection = parsed[SETTINGS_KEY];
      }
    } catch {
      // Malformed settings fall back to defaults for read-only inspection; mutations will report it cleanly.
    }
  }

  const enabled = rawSection.enabled === false ? false : true;
  const customDataDir = typeof rawSection.dataDir === "string" && rawSection.dataDir.trim() ? resolve(rawSection.dataDir.trim()) : undefined;
  const dir = customDataDir ?? join(agentDir, "memories");
  const memoryCharLimit = positiveInt(rawSection.memoryCharLimit, DEFAULT_MEMORY_CHAR_LIMIT);
  const userCharLimit = positiveInt(rawSection.userCharLimit, DEFAULT_USER_CHAR_LIMIT);

  const rawDreaming = isRecord(rawSection.dreaming) ? rawSection.dreaming : {};
  const dreamingEnabled = rawDreaming.enabled === false ? false : true;
  const dreamingMinHours = positiveInt(rawDreaming.minHoursSinceLastRun, DEFAULT_DREAM_MIN_HOURS);
  const dreamingMinTurns = positiveInt(
    rawDreaming.minTurnsSinceLastRun ?? rawDreaming.minSessionsSinceLastRun,
    DEFAULT_DREAM_MIN_TURNS,
  );
  let dreamingModelId: string | undefined;
  if (isRecord(rawDreaming.model)) {
    const provider = typeof rawDreaming.model.provider === "string" ? rawDreaming.model.provider.trim() : "";
    const model = typeof rawDreaming.model.model === "string" ? rawDreaming.model.model.trim() : "";
    if (provider && model) dreamingModelId = `${provider}/${model}`;
  }

  return {
    enabled,
    ...(customDataDir ? { customDataDir } : {}),
    dir,
    memoryCharLimit,
    userCharLimit,
    dreamingEnabled,
    dreamingMinHours,
    dreamingMinTurns,
    ...(dreamingModelId ? { dreamingModelId } : {}),
  };
}

/**
 * Built-in Pi extension factory for `@amaster.ai/pi-memory`.
 * Injects `agentDir/memories` unless the user explicitly configured `pi-memory.dataDir`,
 * keeping test runs (`PI_CODING_AGENT_DIR`) isolated from `~/.pi/agent/memories`.
 */
export function createBuiltinMemoryExtension(agentDir: string): ExtensionFactory {
  return (pi) => {
    const cfg = readMemorySettings(agentDir);
    if (!cfg.enabled) return;
    const injected: Record<string, unknown> = {};
    if (!cfg.customDataDir) injected.dataDir = cfg.dir;
    if (process.env.PI_OFFLINE !== undefined) {
      injected.dreaming = { enabled: false };
    }
    memoryExtension(pi, Object.keys(injected).length ? injected : undefined);
  };
}

function storeFor(cfg: ParsedMemorySettings): MemoryStore {
  return new MemoryStore({
    dir: cfg.dir,
    memoryCharLimit: cfg.memoryCharLimit,
    userCharLimit: cfg.userCharLimit,
  });
}

function measureStore(entries: string[], limit: number, target: MemoryTarget, dir: string): MemoryStoreSnapshot {
  const charCount = entries.length ? entries.join(ENTRY_DELIMITER).length : 0;
  const percent = limit > 0 ? Math.min(100, Math.round((charCount / limit) * 100)) : 0;
  return {
    target,
    filePath: join(dir, target === "user" ? "USER.md" : "MEMORY.md"),
    entries,
    charCount,
    charLimit: limit,
    percent,
  };
}

export async function readMemorySnapshot(agentDir: string, externalExtension?: string): Promise<MemorySnapshot> {
  const cfg = readMemorySettings(agentDir);
  const store = storeFor(cfg);
  await store.loadFromDisk();
  const [memResult, userResult, dreamState] = await Promise.all([
    store.read("memory"),
    store.read("user"),
    readDreamingState(cfg.dir).catch(() => ({ lastConsolidatedAt: null, lastSessionCount: 0 })),
  ]);
  return {
    enabled: cfg.enabled,
    ...(externalExtension ? { externalExtension } : {}),
    dir: cfg.dir,
    memory: measureStore(memResult.entries, cfg.memoryCharLimit, "memory", cfg.dir),
    user: measureStore(userResult.entries, cfg.userCharLimit, "user", cfg.dir),
    dreaming: {
      enabled: cfg.dreamingEnabled,
      minHoursSinceLastRun: cfg.dreamingMinHours,
      minTurnsSinceLastRun: cfg.dreamingMinTurns,
      ...(cfg.dreamingModelId ? { modelId: cfg.dreamingModelId } : {}),
      ...(dreamState.lastConsolidatedAt ? { lastConsolidatedAt: dreamState.lastConsolidatedAt } : {}),
      lastSessionCount: dreamState.lastSessionCount,
    },
  };
}

function assertMemoryOk(result: { success: boolean; error?: string; driftBackup?: string; remediation?: string }): void {
  if (result.success) return;
  const detail = [result.error, result.remediation].filter(Boolean).join(" ");
  throw new UserError(detail || "记忆操作失败");
}

export async function addMemoryEntry(agentDir: string, target: MemoryTarget, content: string, externalExtension?: string): Promise<MemorySnapshot> {
  const cfg = readMemorySettings(agentDir);
  const store = storeFor(cfg);
  await store.loadFromDisk();
  assertMemoryOk(await store.add(target, content));
  return readMemorySnapshot(agentDir, externalExtension);
}

export async function replaceMemoryEntry(
  agentDir: string,
  target: MemoryTarget,
  oldText: string,
  newContent: string,
  externalExtension?: string,
): Promise<MemorySnapshot> {
  const cfg = readMemorySettings(agentDir);
  const store = storeFor(cfg);
  await store.loadFromDisk();
  assertMemoryOk(await store.replace(target, oldText, newContent));
  return readMemorySnapshot(agentDir, externalExtension);
}

export async function removeMemoryEntry(agentDir: string, target: MemoryTarget, oldText: string, externalExtension?: string): Promise<MemorySnapshot> {
  const cfg = readMemorySettings(agentDir);
  const store = storeFor(cfg);
  await store.loadFromDisk();
  assertMemoryOk(await store.remove(target, oldText));
  return readMemorySnapshot(agentDir, externalExtension);
}

export async function updateMemoryConfig(agentDir: string, patch: MemoryConfigPatch, externalExtension?: string): Promise<MemorySnapshot> {
  const settingsPath = join(agentDir, "settings.json");
  const root = readSettingsJson(settingsPath);
  const existing = isRecord(root[SETTINGS_KEY]) ? { ...root[SETTINGS_KEY] } : {};
  const dreaming = isRecord(existing.dreaming) ? { ...existing.dreaming } : {};

  if (patch.enabled !== undefined) existing.enabled = patch.enabled;
  if (patch.memoryCharLimit !== undefined) existing.memoryCharLimit = patch.memoryCharLimit;
  if (patch.userCharLimit !== undefined) existing.userCharLimit = patch.userCharLimit;
  if (patch.dreamingEnabled !== undefined) dreaming.enabled = patch.dreamingEnabled;
  if (patch.dreamingMinHours !== undefined) dreaming.minHoursSinceLastRun = patch.dreamingMinHours;
  if (patch.dreamingMinTurns !== undefined) dreaming.minTurnsSinceLastRun = patch.dreamingMinTurns;
  if (patch.dreamingModelId !== undefined) {
    if (patch.dreamingModelId === null) {
      delete dreaming.model;
    } else {
      const slash = patch.dreamingModelId.indexOf("/");
      if (slash <= 0 || slash >= patch.dreamingModelId.length - 1) {
        throw new UserError(`无效的模型标识：${patch.dreamingModelId}`);
      }
      dreaming.model = {
        provider: patch.dreamingModelId.slice(0, slash),
        model: patch.dreamingModelId.slice(slash + 1),
      };
    }
  }

  if (Object.keys(dreaming).length > 0) existing.dreaming = dreaming;
  else delete existing.dreaming;

  root[SETTINGS_KEY] = existing;
  writeSettingsJson(settingsPath, root);
  return readMemorySnapshot(agentDir, externalExtension);
}
