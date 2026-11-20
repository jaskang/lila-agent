const COMPACT_TOKENS = new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 1 });

export function formatCompactTokens(tokens: number): string {
  return `${COMPACT_TOKENS.format(tokens)} tokens`;
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
