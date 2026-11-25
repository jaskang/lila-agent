import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { AgentSessionServices } from "@earendil-works/pi-coding-agent";
import type { ProviderQuotaItem, QuotaBalance, QuotaWindow } from "@lila/protocol";

const FETCH_TIMEOUT_MS = 8_000;
const API_CACHE_TTL_MS = 60_000;
const REFRESH_COOLDOWN_MS = 10_000;

const BILLING_URLS: Record<string, string> = {
  anthropic: "https://platform.claude.com/settings/billing",
  openai: "https://platform.openai.com/settings/organization/billing/overview",
  "openai-codex": "https://chatgpt.com/#settings/Subscription",
  google: "https://aistudio.google.com/",
  "google-vertex": "https://console.cloud.google.com/billing",
  deepseek: "https://platform.deepseek.com/usage",
  openrouter: "https://openrouter.ai/credits",
  moonshotai: "https://platform.moonshot.ai/console/account",
  "moonshotai-cn": "https://platform.moonshot.cn/console/account",
  "kimi-coding": "https://www.kimi.com/coding",
  minimax: "https://platform.minimax.io/user-center/payment/coding-plan",
  "minimax-cn": "https://platform.minimaxi.com/user-center/payment/coding-plan",
  zai: "https://z.ai/manage-apikey/billing",
  "zai-coding-cn": "https://open.bigmodel.cn/usercenter/proj-mgmt/rate-limits",
  xai: "https://console.x.ai/",
  groq: "https://console.groq.com/settings/billing",
  mistral: "https://console.mistral.ai/billing",
};

type HeaderSnapshot = {
  plan?: string;
  windows: QuotaWindow[];
  updatedAt: string;
};

type CachedApiQuota = {
  fetchedAt: number;
  item: ProviderQuotaItem;
};

function asRecord(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined;
}

function asNum(value: unknown): number | undefined {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string" && value.trim()) {
    const parsed = Number(value);
    if (Number.isFinite(parsed)) return parsed;
  }
  return undefined;
}

function asStr(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function clampPercent(value: number): number {
  return Math.max(0, Math.min(100, Math.round(value * 10) / 10));
}

function windowStatus(usedPercent?: number): QuotaWindow["status"] {
  if (usedPercent === undefined) return "ok";
  if (usedPercent >= 99.5) return "limited";
  if (usedPercent >= 80) return "warning";
  return "ok";
}

function toIsoTimestamp(raw: unknown): string | undefined {
  if (typeof raw === "number" && Number.isFinite(raw) && raw > 0) {
    // Distinguish seconds vs milliseconds.
    const ms = raw > 1e12 ? raw : raw * 1000;
    return new Date(ms).toISOString();
  }
  if (typeof raw === "string" && raw.trim()) {
    const num = Number(raw);
    if (Number.isFinite(num) && num > 0) {
      const ms = num > 1e12 ? num : num * 1000;
      return new Date(ms).toISOString();
    }
    const parsed = Date.parse(raw);
    if (Number.isFinite(parsed)) return new Date(parsed).toISOString();
  }
  return undefined;
}

function currencySymbol(currency: string): string {
  const upper = currency.toUpperCase();
  if (upper === "CNY" || upper === "RMB") return "¥";
  if (upper === "USD") return "$";
  return `${upper} `;
}

function formatMoney(currency: string, amount: number | string): string {
  const num = typeof amount === "number" ? amount : Number(amount);
  const formatted = Number.isFinite(num) ? num.toFixed(2) : String(amount);
  return `${currencySymbol(currency)}${formatted}`;
}

function normalizeHeaders(headers: Record<string, string>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(headers)) out[k.toLowerCase()] = v;
  return out;
}

export function parseAnthropicHeaders(rawHeaders: Record<string, string>): HeaderSnapshot | undefined {
  const h = normalizeHeaders(rawHeaders);
  const windows: QuotaWindow[] = [];

  const u5 = asNum(h["anthropic-ratelimit-unified-5h-utilization"]);
  if (u5 !== undefined) {
    const pct = clampPercent(u5 <= 1 ? u5 * 100 : u5);
    const resetsAt = toIsoTimestamp(h["anthropic-ratelimit-unified-5h-reset"]);
    windows.push({
      label: "5 小时窗口",
      usedPercent: pct,
      ...(resetsAt ? { resetsAt } : {}),
      status: windowStatus(pct),
    });
  }

  const u7 = asNum(h["anthropic-ratelimit-unified-7d-utilization"]);
  if (u7 !== undefined) {
    const pct = clampPercent(u7 <= 1 ? u7 * 100 : u7);
    const resetsAt = toIsoTimestamp(h["anthropic-ratelimit-unified-7d-reset"]);
    windows.push({
      label: "7 天窗口",
      usedPercent: pct,
      ...(resetsAt ? { resetsAt } : {}),
      status: windowStatus(pct),
    });
  }

  // Fallback for standard API key rate-limit headers when unified subscription headers are absent.
  if (!windows.length) {
    const tokLimit = asNum(h["anthropic-ratelimit-tokens-limit"]);
    const tokRem = asNum(h["anthropic-ratelimit-tokens-remaining"]);
    if (tokLimit && tokLimit > 0 && tokRem !== undefined) {
      const pct = clampPercent(((tokLimit - tokRem) / tokLimit) * 100);
      const resetsAt = toIsoTimestamp(h["anthropic-ratelimit-tokens-reset"]);
      windows.push({
        label: "Token 速率窗口",
        usedPercent: pct,
        remainingText: `${tokRem.toLocaleString()} / ${tokLimit.toLocaleString()}`,
        ...(resetsAt ? { resetsAt } : {}),
        status: windowStatus(pct),
      });
    }
    const reqLimit = asNum(h["anthropic-ratelimit-requests-limit"]);
    const reqRem = asNum(h["anthropic-ratelimit-requests-remaining"]);
    if (reqLimit && reqLimit > 0 && reqRem !== undefined) {
      const pct = clampPercent(((reqLimit - reqRem) / reqLimit) * 100);
      const resetsAt = toIsoTimestamp(h["anthropic-ratelimit-requests-reset"]);
      windows.push({
        label: "请求速率窗口",
        usedPercent: pct,
        remainingText: `${reqRem.toLocaleString()} / ${reqLimit.toLocaleString()}`,
        ...(resetsAt ? { resetsAt } : {}),
        status: windowStatus(pct),
      });
    }
  }

  if (!windows.length) return undefined;
  const plan = u5 !== undefined || u7 !== undefined ? "Claude 订阅" : "API Rate Limit";
  return { plan, windows, updatedAt: new Date().toISOString() };
}

export function parseCodexHeaders(rawHeaders: Record<string, string>): HeaderSnapshot | undefined {
  const h = normalizeHeaders(rawHeaders);
  const windows: QuotaWindow[] = [];

  const primary = asNum(h["x-codex-primary-used-percent"]);
  if (primary !== undefined) {
    const pct = clampPercent(primary);
    const afterSec = asNum(h["x-codex-primary-reset-after-seconds"]);
    const resetsAt =
      toIsoTimestamp(h["x-codex-primary-reset-at"]) ?? (afterSec !== undefined ? new Date(Date.now() + afterSec * 1000).toISOString() : undefined);
    windows.push({
      label: "5 小时窗口",
      usedPercent: pct,
      ...(resetsAt ? { resetsAt } : {}),
      status: windowStatus(pct),
    });
  }

  const secondary = asNum(h["x-codex-secondary-used-percent"]);
  if (secondary !== undefined) {
    const pct = clampPercent(secondary);
    const afterSec = asNum(h["x-codex-secondary-reset-after-seconds"]);
    const resetsAt =
      toIsoTimestamp(h["x-codex-secondary-reset-at"]) ?? (afterSec !== undefined ? new Date(Date.now() + afterSec * 1000).toISOString() : undefined);
    windows.push({
      label: "7 天窗口",
      usedPercent: pct,
      ...(resetsAt ? { resetsAt } : {}),
      status: windowStatus(pct),
    });
  }

  if (!windows.length) return undefined;
  const plan = asStr(h["x-codex-plan-type"]) ?? "Codex 订阅";
  return { plan, windows, updatedAt: new Date().toISOString() };
}

async function fetchJson(url: string, headers: Record<string, string>): Promise<{ ok: boolean; status: number; data: unknown; text: string }> {
  const response = await fetch(url, {
    method: "GET",
    headers: { Accept: "application/json", ...headers },
    signal: AbortSignal.timeout(FETCH_TIMEOUT_MS),
  });
  const text = await response.text();
  let data: unknown;
  try {
    data = JSON.parse(text);
  } catch {
    data = undefined;
  }
  return { ok: response.ok, status: response.status, data, text };
}

async function fetchDeepSeekQuota(providerId: string, providerName: string, apiKey: string): Promise<ProviderQuotaItem> {
  const res = await fetchJson("https://api.deepseek.com/user/balance", { Authorization: `Bearer ${apiKey}` });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  const root = asRecord(res.data);
  const infos = Array.isArray(root?.balance_infos) ? root.balance_infos : [];
  const balances: QuotaBalance[] = [];
  for (const raw of infos) {
    const item = asRecord(raw);
    const currency = asStr(item?.currency) ?? "CNY";
    const total = asStr(item?.total_balance) ?? String(asNum(item?.total_balance) ?? "0.00");
    const topped = asNum(item?.topped_up_balance);
    const granted = asNum(item?.granted_balance);
    const detailParts: string[] = [];
    if (topped !== undefined) detailParts.push(`充值 ${formatMoney(currency, topped)}`);
    if (granted !== undefined && granted > 0) detailParts.push(`赠送 ${formatMoney(currency, granted)}`);
    balances.push({
      currency,
      total: formatMoney(currency, total),
      ...(detailParts.length ? { details: detailParts.join(" · ") } : {}),
    });
  }
  return {
    providerId,
    providerName,
    source: "api",
    status: "ok",
    balances,
    updatedAt: new Date().toISOString(),
    billingUrl: BILLING_URLS[providerId],
  };
}

async function fetchOpenRouterQuota(providerId: string, providerName: string, apiKey: string): Promise<ProviderQuotaItem> {
  const res = await fetchJson("https://openrouter.ai/api/v1/key", { Authorization: `Bearer ${apiKey}` });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  const data = asRecord(asRecord(res.data)?.data);
  const usage = asNum(data?.usage) ?? 0;
  const limit = asNum(data?.limit);
  const remaining = asNum(data?.limit_remaining);
  const reset = asStr(data?.limit_reset);
  const balances: QuotaBalance[] = [];
  const windows: QuotaWindow[] = [];

  if (limit !== undefined && limit > 0 && remaining !== undefined) {
    const pct = clampPercent(((limit - remaining) / limit) * 100);
    balances.push({
      currency: "USD",
      total: formatMoney("USD", remaining),
      details: `限额 ${formatMoney("USD", limit)} · 已用 ${formatMoney("USD", usage)}${reset ? ` · ${reset} 重置` : ""}`,
    });
    windows.push({
      label: reset ? `Key 额度 (${reset})` : "Key 额度",
      usedPercent: pct,
      remainingText: `剩余 ${formatMoney("USD", remaining)}`,
      status: windowStatus(pct),
    });
  } else {
    balances.push({
      currency: "USD",
      total: formatMoney("USD", usage),
      details: "当前 Key 累计已用（未设上限）",
    });
  }
  return {
    providerId,
    providerName,
    source: "api",
    status: "ok",
    ...(data?.is_free_tier === true ? { plan: "Free Tier" } : {}),
    balances,
    ...(windows.length ? { windows } : {}),
    updatedAt: new Date().toISOString(),
    billingUrl: BILLING_URLS[providerId],
  };
}

async function fetchMoonshotBalanceQuota(providerId: string, providerName: string, apiKey: string): Promise<ProviderQuotaItem> {
  const host = providerId === "moonshotai-cn" ? "https://api.moonshot.cn" : "https://api.moonshot.ai";
  const res = await fetchJson(`${host}/v1/users/me/balance`, { Authorization: `Bearer ${apiKey}` });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  const data = asRecord(asRecord(res.data)?.data);
  const available = asNum(data?.available_balance);
  if (available === undefined) throw new Error("未返回余额字段");
  const cash = asNum(data?.cash_balance);
  const voucher = asNum(data?.voucher_balance);
  const currency = providerId === "moonshotai-cn" ? "CNY" : "USD";
  const detailParts: string[] = [];
  if (cash !== undefined) detailParts.push(`现金 ${formatMoney(currency, cash)}`);
  if (voucher !== undefined && voucher > 0) detailParts.push(`代金券 ${formatMoney(currency, voucher)}`);
  return {
    providerId,
    providerName,
    source: "api",
    status: "ok",
    balances: [{ currency, total: formatMoney(currency, available), ...(detailParts.length ? { details: detailParts.join(" · ") } : {}) }],
    updatedAt: new Date().toISOString(),
    billingUrl: BILLING_URLS[providerId],
  };
}

async function fetchKimiCodingQuota(providerId: string, providerName: string, apiKey: string): Promise<ProviderQuotaItem> {
  const res = await fetchJson("https://api.kimi.com/coding/v1/usages", { Authorization: `Bearer ${apiKey}` });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  const root = asRecord(res.data);
  const windows: QuotaWindow[] = [];

  const pushEntry = (label: string, obj: Record<string, unknown> | undefined) => {
    if (!obj) return;
    const limit = asNum(obj.limit);
    const used = asNum(obj.used);
    const remaining = asNum(obj.remaining);
    const resetsAt = toIsoTimestamp(obj.resetTime ?? obj.reset_at ?? obj.resetAt);
    let pct: number | undefined;
    if (limit && limit > 0) {
      const actualUsed = used ?? (remaining !== undefined ? limit - remaining : undefined);
      if (actualUsed !== undefined) pct = clampPercent((actualUsed / limit) * 100);
    }
    const remVal = remaining ?? (limit !== undefined && used !== undefined ? Math.max(0, limit - used) : undefined);
    if (pct !== undefined || remVal !== undefined) {
      windows.push({
        label,
        ...(pct !== undefined ? { usedPercent: pct } : {}),
        ...(remVal !== undefined && limit !== undefined ? { remainingText: `剩余 ${remVal} / ${limit}` } : {}),
        ...(resetsAt ? { resetsAt } : {}),
        status: windowStatus(pct),
      });
    }
  };

  pushEntry("周期额度", asRecord(root?.usage));
  const limits = Array.isArray(root?.limits) ? root.limits : [];
  for (const raw of limits) {
    const item = asRecord(raw);
    const win = asRecord(item?.window);
    const detail = asRecord(item?.detail) ?? item;
    const duration = asNum(win?.duration);
    const timeUnit = asStr(win?.timeUnit)?.toLowerCase() ?? "";
    const label =
      duration !== undefined
        ? timeUnit.includes("hour")
          ? `${duration} 小时窗口`
          : timeUnit.includes("minute")
            ? `${duration} 分钟窗口`
            : timeUnit.includes("day")
              ? `${duration} 天窗口`
              : `${duration} ${timeUnit}`
        : "滚动窗口";
    pushEntry(label, detail);
  }

  return {
    providerId,
    providerName,
    source: "api",
    status: windows.length ? "ok" : "error",
    plan: asStr(asRecord(root?.user)?.membership) ?? "Kimi Code",
    ...(windows.length ? { windows } : { error: "未解析到套餐额度字段" }),
    updatedAt: new Date().toISOString(),
    billingUrl: BILLING_URLS[providerId],
  };
}

async function fetchMiniMaxQuota(providerId: string, providerName: string, apiKey: string): Promise<ProviderQuotaItem> {
  const host = providerId === "minimax-cn" ? "https://api.minimaxi.com" : "https://api.minimax.io";
  const res = await fetchJson(`${host}/v1/api/openplatform/coding_plan/remains`, { Authorization: `Bearer ${apiKey}` });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  const root = asRecord(res.data);
  const baseResp = asRecord(root?.base_resp);
  const statusCode = asNum(baseResp?.status_code);
  if (statusCode !== undefined && statusCode !== 0) {
    throw new Error(asStr(baseResp?.status_msg) ?? `错误码 ${statusCode}`);
  }
  const items = Array.isArray(root?.model_remains) ? root.model_remains : [];
  const windows: QuotaWindow[] = [];

  for (const raw of items) {
    const item = asRecord(raw);
    if (!item) continue;
    const modelName = asStr(item.model_name);
    const total = asNum(item.current_interval_total_count);
    // MiniMax's `current_interval_usage_count` reports remaining count in the current window.
    const remaining = asNum(item.current_interval_usage_count);
    const remainsMs = asNum(item.remains_time);
    const endTime = toIsoTimestamp(item.end_time) ?? (remainsMs !== undefined ? new Date(Date.now() + remainsMs).toISOString() : undefined);
    if (total && total > 0 && remaining !== undefined) {
      const used = Math.max(0, total - remaining);
      const pct = clampPercent((used / total) * 100);
      windows.push({
        label: modelName ? `${modelName} (5h)` : "5 小时窗口",
        usedPercent: pct,
        remainingText: `剩余 ${remaining} / ${total}`,
        ...(endTime ? { resetsAt: endTime } : {}),
        status: windowStatus(pct),
      });
    }
    const weeklyTotal = asNum(item.current_weekly_total_count);
    const weeklyRemaining = asNum(item.current_weekly_usage_count);
    const weeklyRemainsMs = asNum(item.weekly_remains_time);
    const weeklyReset =
      toIsoTimestamp(item.weekly_end_time) ??
      (weeklyRemainsMs !== undefined ? new Date(Date.now() + weeklyRemainsMs).toISOString() : undefined);
    if (weeklyTotal && weeklyTotal > 0 && weeklyRemaining !== undefined) {
      const used = Math.max(0, weeklyTotal - weeklyRemaining);
      const pct = clampPercent((used / weeklyTotal) * 100);
      windows.push({
        label: modelName ? `${modelName} (7d)` : "7 天窗口",
        usedPercent: pct,
        remainingText: `剩余 ${weeklyRemaining} / ${weeklyTotal}`,
        ...(weeklyReset ? { resetsAt: weeklyReset } : {}),
        status: windowStatus(pct),
      });
    }
    // Keep card concise when multiple models share identical window counts.
    if (windows.length >= 2) break;
  }

  return {
    providerId,
    providerName,
    source: "api",
    status: windows.length ? "ok" : "error",
    plan: "MiniMax Coding Plan",
    ...(windows.length ? { windows } : { error: "当前 Key 可能不是 Coding Plan，未返回套餐余量" }),
    updatedAt: new Date().toISOString(),
    billingUrl: BILLING_URLS[providerId],
  };
}

async function fetchZaiQuota(providerId: string, providerName: string, apiKey: string): Promise<ProviderQuotaItem> {
  const host = providerId === "zai-coding-cn" ? "https://open.bigmodel.cn" : "https://api.z.ai";
  const url = `${host}/api/monitor/usage/quota/limit`;
  let res = await fetchJson(url, { Authorization: `Bearer ${apiKey}` });
  if (res.status === 401 || res.status === 403) {
    res = await fetchJson(url, { Authorization: apiKey });
  }
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  const root = asRecord(res.data);
  const data = asRecord(root?.data) ?? root;
  const limits = Array.isArray(data?.limits) ? data.limits : [];
  const windows: QuotaWindow[] = [];

  for (const raw of limits) {
    const item = asRecord(raw);
    if (!item) continue;
    const type = asStr(item.type) ?? "QUOTA";
    const label =
      type === "TOKENS_LIMIT"
        ? "5 小时 Token 配额"
        : type === "TIME_LIMIT"
          ? "月度工具/MCP 配额"
          : type;
    const percentage = asNum(item.percentage);
    const usage = asNum(item.usage);
    const current = asNum(item.currentValue ?? item.current_value);
    const remaining = asNum(item.remaining);
    const resetsAt = toIsoTimestamp(item.nextResetTime ?? item.next_reset_time);

    let pct = percentage !== undefined ? clampPercent(percentage) : undefined;
    if (pct === undefined && usage && usage > 0 && current !== undefined) {
      pct = clampPercent((current / usage) * 100);
    }
    const remText =
      remaining !== undefined && usage !== undefined
        ? `剩余 ${remaining.toLocaleString()} / ${usage.toLocaleString()}`
        : current !== undefined && usage !== undefined
          ? `已用 ${current.toLocaleString()} / ${usage.toLocaleString()}`
          : undefined;
    if (pct !== undefined || remText) {
      windows.push({
        label,
        ...(pct !== undefined ? { usedPercent: pct } : {}),
        ...(remText ? { remainingText: remText } : {}),
        ...(resetsAt ? { resetsAt } : {}),
        status: windowStatus(pct),
      });
    }
  }

  return {
    providerId,
    providerName,
    source: "api",
    status: windows.length ? "ok" : "error",
    plan: asStr(data?.level) ?? "GLM Coding Plan",
    ...(windows.length ? { windows } : { error: "未解析到套餐额度字段" }),
    updatedAt: new Date().toISOString(),
    billingUrl: BILLING_URLS[providerId],
  };
}

async function fetchCodexQuota(
  providerId: string,
  providerName: string,
  accessToken: string,
  extraHeaders?: Record<string, string | null>,
): Promise<ProviderQuotaItem> {
  const headers: Record<string, string> = { Authorization: `Bearer ${accessToken}` };
  for (const [k, v] of Object.entries(extraHeaders ?? {})) {
    if (typeof v === "string" && k.toLowerCase() === "chatgpt-account-id") headers["ChatGPT-Account-Id"] = v;
  }
  const res = await fetchJson("https://chatgpt.com/backend-api/wham/usage", headers);
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  const root = asRecord(res.data);
  const rateLimit = asRecord(root?.rate_limit);
  const windows: QuotaWindow[] = [];

  const readWindow = (label: string, raw: unknown) => {
    const win = asRecord(raw);
    if (!win) return;
    const used = asNum(win.used_percent);
    if (used === undefined) return;
    const pct = clampPercent(used);
    const resetAfter = asNum(win.reset_after_seconds);
    const resetsAt =
      toIsoTimestamp(win.reset_at) ?? (resetAfter !== undefined ? new Date(Date.now() + resetAfter * 1000).toISOString() : undefined);
    windows.push({
      label,
      usedPercent: pct,
      ...(resetsAt ? { resetsAt } : {}),
      status: windowStatus(pct),
    });
  };

  readWindow("5 小时窗口", rateLimit?.primary_window);
  readWindow("7 天窗口", rateLimit?.secondary_window);

  const credits = asRecord(root?.credits);
  const balances: QuotaBalance[] = [];
  if (credits?.has_credits && credits.balance !== undefined) {
    balances.push({ currency: "USD", total: formatMoney("USD", String(credits.balance)), details: "额外额度余额" });
  }

  return {
    providerId,
    providerName,
    source: "api",
    status: windows.length || balances.length ? "ok" : "error",
    plan: asStr(root?.plan_type) ? `ChatGPT ${asStr(root?.plan_type)}` : "Codex 订阅",
    ...(balances.length ? { balances } : {}),
    ...(windows.length ? { windows } : {}),
    updatedAt: new Date().toISOString(),
    billingUrl: BILLING_URLS[providerId],
  };
}

/** Stores passive quota snapshots captured from provider response headers and fetches live balances for configured providers. */
export class QuotaTracker {
  readonly #path: string;
  #headerSnapshots: Record<string, HeaderSnapshot> = {};
  readonly #apiCache = new Map<string, CachedApiQuota>();

  constructor(dataDir: string) {
    mkdirSync(dataDir, { recursive: true, mode: 0o700 });
    this.#path = join(dataDir, "quotas.json");
    this.#load();
  }

  #load(): void {
    if (!existsSync(this.#path)) return;
    try {
      const parsed = asRecord(JSON.parse(readFileSync(this.#path, "utf8")));
      if (!parsed) return;
      for (const [providerId, raw] of Object.entries(parsed)) {
        const entry = asRecord(raw);
        if (!entry || !Array.isArray(entry.windows) || typeof entry.updatedAt !== "string") continue;
        this.#headerSnapshots[providerId] = {
          ...(typeof entry.plan === "string" ? { plan: entry.plan } : {}),
          windows: entry.windows as QuotaWindow[],
          updatedAt: entry.updatedAt,
        };
      }
    } catch {
      // Ignore unreadable cache; fresh headers will overwrite it.
    }
  }

  #save(): void {
    try {
      const tmp = `${this.#path}.${process.pid}.tmp`;
      writeFileSync(tmp, `${JSON.stringify(this.#headerSnapshots, null, 2)}\n`, { mode: 0o600 });
      renameSync(tmp, this.#path);
    } catch {
      // Best-effort persistence.
    }
  }

  recordResponseHeaders(providerId: string | undefined, headers: Record<string, string>): void {
    if (!providerId) return;
    let snapshot: HeaderSnapshot | undefined;
    if (providerId === "anthropic") snapshot = parseAnthropicHeaders(headers);
    else if (providerId === "openai-codex") snapshot = parseCodexHeaders(headers);
    if (!snapshot) return;
    this.#headerSnapshots[providerId] = snapshot;
    this.#save();
  }

  clearProvider(providerId: string): void {
    this.#apiCache.delete(providerId);
    if (providerId in this.#headerSnapshots) {
      delete this.#headerSnapshots[providerId];
      this.#save();
    }
  }

  async listForConfiguredProviders(services: AgentSessionServices, refresh = false): Promise<ProviderQuotaItem[]> {
    await services.modelRuntime.refresh({ allowNetwork: false });
    const configured = services.modelRuntime
      .getProviders()
      .filter((provider) => services.modelRuntime.getProviderAuthStatus(provider.id).configured);

    const results = await Promise.all(
      configured.map((provider) => this.#resolveOne(services, provider.id, provider.name || provider.id, refresh)),
    );

    const rank = (item: ProviderQuotaItem) => (item.status === "ok" ? 0 : item.status === "waiting" ? 1 : item.status === "error" ? 2 : 3);
    return results.sort((a, b) => rank(a) - rank(b) || a.providerName.localeCompare(b.providerName));
  }

  async #resolveOne(services: AgentSessionServices, providerId: string, providerName: string, refresh: boolean): Promise<ProviderQuotaItem> {
    if (providerId === "anthropic") {
      const snap = this.#headerSnapshots.anthropic;
      if (snap) {
        return {
          providerId,
          providerName,
          source: "headers",
          status: "ok",
          ...(snap.plan ? { plan: snap.plan } : {}),
          windows: snap.windows,
          updatedAt: snap.updatedAt,
          billingUrl: BILLING_URLS.anthropic,
        };
      }
      return {
        providerId,
        providerName,
        source: "headers",
        status: "waiting",
        billingUrl: BILLING_URLS.anthropic,
      };
    }

    const supportedActive = new Set([
      "deepseek",
      "openrouter",
      "moonshotai",
      "moonshotai-cn",
      "kimi-coding",
      "minimax",
      "minimax-cn",
      "zai",
      "zai-coding-cn",
      "openai-codex",
    ]);

    if (!supportedActive.has(providerId)) {
      return {
        providerId,
        providerName,
        source: "unsupported",
        status: "unsupported",
        ...(BILLING_URLS[providerId] ? { billingUrl: BILLING_URLS[providerId] } : {}),
      };
    }

    const now = Date.now();
    const cached = this.#apiCache.get(providerId);
    const minAge = refresh ? REFRESH_COOLDOWN_MS : API_CACHE_TTL_MS;
    if (cached && now - cached.fetchedAt < minAge) {
      return cached.item;
    }

    try {
      const auth = await services.modelRuntime.getAuth(providerId);
      const token = auth?.auth.apiKey;
      if (!token) throw new Error("未读取到有效凭据");

      let item: ProviderQuotaItem;
      switch (providerId) {
        case "deepseek":
          item = await fetchDeepSeekQuota(providerId, providerName, token);
          break;
        case "openrouter":
          item = await fetchOpenRouterQuota(providerId, providerName, token);
          break;
        case "moonshotai":
        case "moonshotai-cn":
          item = await fetchMoonshotBalanceQuota(providerId, providerName, token);
          break;
        case "kimi-coding":
          item = await fetchKimiCodingQuota(providerId, providerName, token);
          break;
        case "minimax":
        case "minimax-cn":
          item = await fetchMiniMaxQuota(providerId, providerName, token);
          break;
        case "zai":
        case "zai-coding-cn":
          item = await fetchZaiQuota(providerId, providerName, token);
          break;
        case "openai-codex":
          item = await fetchCodexQuota(providerId, providerName, token, auth?.auth.headers);
          break;
        default:
          item = { providerId, providerName, source: "unsupported", status: "unsupported", ...(BILLING_URLS[providerId] ? { billingUrl: BILLING_URLS[providerId] } : {}) };
      }

      this.#apiCache.set(providerId, { fetchedAt: now, item });
      return item;
    } catch (error) {
      // For Codex, gracefully fall back to passive response-header quota snapshot if active query hits 429/error.
      if (providerId === "openai-codex" && this.#headerSnapshots["openai-codex"]) {
        const snap = this.#headerSnapshots["openai-codex"];
        return {
          providerId,
          providerName,
          source: "headers",
          status: "ok",
          ...(snap.plan ? { plan: snap.plan } : {}),
          windows: snap.windows,
          updatedAt: snap.updatedAt,
          billingUrl: BILLING_URLS["openai-codex"],
        };
      }
      const message = error instanceof Error ? error.message : String(error);
      const failed: ProviderQuotaItem = {
        providerId,
        providerName,
        source: "api",
        status: "error",
        error: message,
        updatedAt: new Date().toISOString(),
        ...(BILLING_URLS[providerId] ? { billingUrl: BILLING_URLS[providerId] } : {}),
      };
      this.#apiCache.set(providerId, { fetchedAt: now, item: failed });
      return failed;
    }
  }
}
