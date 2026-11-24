const COMPACT_TOKENS = new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 1 });
const TIME_HM = new Intl.DateTimeFormat("zh-CN", { hour: "2-digit", minute: "2-digit", hour12: false });
const TIME_MD_HM = new Intl.DateTimeFormat("zh-CN", { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false });
const TIME_YMD_HM = new Intl.DateTimeFormat("zh-CN", { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false });
const TIME_FULL = new Intl.DateTimeFormat("zh-CN", { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false });

export function formatCompactTokens(tokens: number): string {
  return `${COMPACT_TOKENS.format(tokens)} tokens`;
}

export function formatMessageTime(timestamp: number | undefined): { label: string; title: string; iso: string } | undefined {
  if (timestamp === undefined || !Number.isFinite(timestamp) || timestamp <= 0) return undefined;
  const date = new Date(timestamp);
  if (Number.isNaN(date.getTime())) return undefined;
  const now = new Date();
  const sameYear = date.getFullYear() === now.getFullYear();
  const sameDay = sameYear && date.getMonth() === now.getMonth() && date.getDate() === now.getDate();
  const label = sameDay ? TIME_HM.format(date) : sameYear ? TIME_MD_HM.format(date) : TIME_YMD_HM.format(date);
  return { label, title: TIME_FULL.format(date), iso: date.toISOString() };
}

export function formatDuration(ms: number | undefined): string | undefined {
  if (ms === undefined || !Number.isFinite(ms) || ms <= 0) return undefined;
  return ms < 1_000
    ? `${Math.round(ms)}ms`
    : ms < 60_000
      ? `${(ms / 1_000).toFixed(1)}s`
      : `${Math.floor(ms / 60_000)}m${Math.round((ms % 60_000) / 1_000)}s`;
}

export function formatElapsed(startedAt: number, now: number): string {
  const seconds = Math.max(0, Math.floor((now - startedAt) / 1_000));
  if (seconds < 60) return `${seconds}s`;
  return `${Math.floor(seconds / 60)}m${seconds % 60}s`;
}

export function isSubagentErrorStatus(status: string): boolean {
  return status === "error" || status === "aborted" || status === "stopped";
}
