import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import type { DailyUsagePoint, UsageBucket, UsageRange, UsageSummary, UsageTotals } from "@lila/protocol";

type UsageRecord = {
  timestamp: number;
  provider: string;
  model: string;
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
  totalTokens: number;
  cost: number;
};

type CachedFile = {
  mtimeMs: number;
  size: number;
  records: UsageRecord[];
};

const fileCache = new Map<string, CachedFile>();

function asRecord(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined;
}

function asNum(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : 0;
}

function asStr(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function parseTimestamp(raw: unknown): number {
  if (typeof raw === "number" && Number.isFinite(raw) && raw > 0) return raw;
  if (typeof raw === "string") {
    const parsed = Date.parse(raw);
    if (Number.isFinite(parsed) && parsed > 0) return parsed;
  }
  return 0;
}

function extractUsageRecord(entry: Record<string, unknown>): UsageRecord | undefined {
  let provider: string | undefined;
  let model: string | undefined;
  let usage: Record<string, unknown> | undefined;
  let timestamp = 0;

  if (entry.type === "message") {
    const message = asRecord(entry.message);
    if (!message || message.role !== "assistant") return undefined;
    provider = asStr(message.provider);
    model = asStr(message.responseModel) ?? asStr(message.model);
    usage = asRecord(message.usage);
    timestamp = parseTimestamp(message.timestamp) || parseTimestamp(entry.timestamp);
  } else if (entry.type === "usage") {
    provider = asStr(entry.provider);
    model = asStr(entry.model);
    usage = asRecord(entry.usage);
    timestamp = parseTimestamp(entry.timestamp);
  } else {
    return undefined;
  }

  if (!provider || !model || !usage) return undefined;
  const input = asNum(usage.input);
  const output = asNum(usage.output);
  const cacheRead = asNum(usage.cacheRead);
  const cacheWrite = asNum(usage.cacheWrite);
  const totalTokens = asNum(usage.totalTokens) || input + output + cacheRead + cacheWrite;
  const costObj = asRecord(usage.cost);
  const cost = asNum(costObj?.total);
  if (totalTokens === 0 && cost === 0) return undefined;

  return { timestamp, provider, model, input, output, cacheRead, cacheWrite, totalTokens, cost };
}

function parseSessionFile(filePath: string): UsageRecord[] {
  let text: string;
  try {
    text = readFileSync(filePath, "utf8");
  } catch {
    return [];
  }
  const records: UsageRecord[] = [];
  for (const line of text.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed || !trimmed.includes('"usage"')) continue;
    try {
      const parsed = asRecord(JSON.parse(trimmed));
      if (!parsed) continue;
      const record = extractUsageRecord(parsed);
      if (record) records.push(record);
    } catch {
      // Ignore partial or malformed lines (e.g. a session currently being appended).
    }
  }
  return records;
}

function collectJsonlFiles(dir: string, out: string[]): void {
  if (!existsSync(dir)) return;
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return;
  }
  for (const entry of entries) {
    const fullPath = join(dir, entry.name);
    if (entry.isDirectory()) collectJsonlFiles(fullPath, out);
    else if (entry.isFile() && entry.name.endsWith(".jsonl")) out.push(fullPath);
  }
}

function emptyTotals(): UsageTotals {
  return { requests: 0, input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: 0 };
}

function addRecord(target: UsageTotals, record: UsageRecord): void {
  target.requests += 1;
  target.input += record.input;
  target.output += record.output;
  target.cacheRead += record.cacheRead;
  target.cacheWrite += record.cacheWrite;
  target.totalTokens += record.totalTokens;
  target.cost += record.cost;
}

function localDateKey(timestamp: number): string {
  const d = new Date(timestamp);
  const year = d.getFullYear();
  const month = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function rangeStartMs(range: UsageRange, now = Date.now()): number {
  if (range === "all") return 0;
  const d = new Date(now);
  d.setHours(0, 0, 0, 0);
  if (range === "today") return d.getTime();
  const days = range === "7d" ? 6 : 29;
  d.setDate(d.getDate() - days);
  return d.getTime();
}

export function scanLocalUsage(options: {
  agentDir: string;
  range: UsageRange;
  configuredProviders: ReadonlyMap<string, string>;
  modelLabels: ReadonlyMap<string, string>;
  onlyConfigured?: boolean;
}): UsageSummary {
  const sessionsRoot = join(options.agentDir, "sessions");
  const files: string[] = [];
  collectJsonlFiles(sessionsRoot, files);
  const seen = new Set(files);
  for (const cachedPath of fileCache.keys()) {
    if (!seen.has(cachedPath)) fileCache.delete(cachedPath);
  }

  const cutoff = rangeStartMs(options.range);
  const onlyConfigured = options.onlyConfigured !== false;
  const totals = emptyTotals();
  const byProviderMap = new Map<string, UsageBucket>();
  const byModelMap = new Map<string, UsageBucket>();
  const byDayMap = new Map<string, DailyUsagePoint>();

  for (const filePath of files) {
    let info;
    try {
      info = statSync(filePath);
    } catch {
      continue;
    }
    let cached = fileCache.get(filePath);
    if (!cached || cached.mtimeMs !== info.mtimeMs || cached.size !== info.size) {
      cached = { mtimeMs: info.mtimeMs, size: info.size, records: parseSessionFile(filePath) };
      fileCache.set(filePath, cached);
    }

    for (const record of cached.records) {
      if (cutoff > 0 && record.timestamp < cutoff) continue;
      if (onlyConfigured && !options.configuredProviders.has(record.provider)) continue;

      addRecord(totals, record);

      const providerLabel = options.configuredProviders.get(record.provider) ?? record.provider;
      let providerBucket = byProviderMap.get(record.provider);
      if (!providerBucket) {
        providerBucket = { key: record.provider, label: providerLabel, ...emptyTotals() };
        byProviderMap.set(record.provider, providerBucket);
      }
      addRecord(providerBucket, record);

      const modelKey = `${record.provider}/${record.model}`;
      const modelLabel = options.modelLabels.get(modelKey) ?? record.model;
      let modelBucket = byModelMap.get(modelKey);
      if (!modelBucket) {
        modelBucket = { key: modelKey, label: modelLabel, provider: providerLabel, ...emptyTotals() };
        byModelMap.set(modelKey, modelBucket);
      }
      addRecord(modelBucket, record);

      if (record.timestamp > 0) {
        const day = localDateKey(record.timestamp);
        let dayBucket = byDayMap.get(day);
        if (!dayBucket) {
          dayBucket = { date: day, ...emptyTotals() };
          byDayMap.set(day, dayBucket);
        }
        addRecord(dayBucket, record);
      }
    }
  }

  const byCostThenTokens = (a: UsageTotals, b: UsageTotals) => b.cost - a.cost || b.totalTokens - a.totalTokens;

  return {
    range: options.range,
    configuredProviders: [...options.configuredProviders.keys()],
    totals,
    byProvider: [...byProviderMap.values()].sort(byCostThenTokens),
    byModel: [...byModelMap.values()].sort(byCostThenTokens),
    byDay: [...byDayMap.values()].sort((a, b) => a.date.localeCompare(b.date)),
    scannedFiles: files.length,
  };
}
