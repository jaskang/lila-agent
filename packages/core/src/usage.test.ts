import { expect, test } from "vitest";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parseAnthropicHeaders, parseCodexHeaders } from "./quotas.ts";
import { scanLocalUsage } from "./usage.ts";

test("scans Pi session logs and filters to configured providers", () => {
  const agentDir = mkdtempSync(join(tmpdir(), "lila-usage-"));
  const dir = join(agentDir, "sessions", "proj-a");
  mkdirSync(dir, { recursive: true });
  const now = Date.now();
  const lines = [
    JSON.stringify({
      type: "message",
      timestamp: new Date(now).toISOString(),
      message: {
        role: "assistant",
        provider: "anthropic",
        model: "claude-sonnet-4-6",
        timestamp: now,
        usage: { input: 1000, output: 200, cacheRead: 3000, cacheWrite: 500, totalTokens: 4700, cost: { total: 0.05 } },
      },
    }),
    JSON.stringify({
      type: "message",
      timestamp: new Date(now).toISOString(),
      message: {
        role: "assistant",
        provider: "unconfigured-provider",
        model: "some-model",
        timestamp: now,
        usage: { input: 9000, output: 1000, cacheRead: 0, cacheWrite: 0, totalTokens: 10000, cost: { total: 0.99 } },
      },
    }),
  ];
  writeFileSync(join(dir, "session-1.jsonl"), `${lines.join("\n")}\n`);

  const summary = scanLocalUsage({
    agentDir,
    range: "7d",
    configuredProviders: new Map([["anthropic", "Anthropic"]]),
    modelLabels: new Map([["anthropic/claude-sonnet-4-6", "Claude Sonnet 4.6"]]),
    onlyConfigured: true,
  });

  expect(summary.totals).toMatchObject({ requests: 1, input: 1000, output: 200, cacheRead: 3000, totalTokens: 4700, cost: 0.05 });
  expect(summary.byProvider).toHaveLength(1);
  expect(summary.byProvider[0]).toMatchObject({ key: "anthropic", label: "Anthropic" });
  expect(summary.byModel[0]).toMatchObject({ key: "anthropic/claude-sonnet-4-6", label: "Claude Sonnet 4.6" });
});

test("parses Anthropic and Codex rate-limit headers into quota windows", () => {
  const anthropic = parseAnthropicHeaders({
    "Anthropic-Ratelimit-Unified-5h-Utilization": "0.42",
    "anthropic-ratelimit-unified-5h-reset": "1795600000",
    "anthropic-ratelimit-unified-7d-utilization": "0.85",
  });
  expect(anthropic?.plan).toBe("Claude 订阅");
  expect(anthropic?.windows).toHaveLength(2);
  expect(anthropic?.windows[0]).toMatchObject({ label: "5 小时窗口", usedPercent: 42, status: "ok" });
  expect(anthropic?.windows[1]).toMatchObject({ label: "7 天窗口", usedPercent: 85, status: "warning" });

  const codex = parseCodexHeaders({
    "x-codex-primary-used-percent": "25",
    "x-codex-primary-reset-after-seconds": "3600",
    "x-codex-secondary-used-percent": "100",
    "x-codex-plan-type": "plus",
  });
  expect(codex?.plan).toBe("plus");
  expect(codex?.windows[0]).toMatchObject({ label: "5 小时窗口", usedPercent: 25, status: "ok" });
  expect(codex?.windows[1]).toMatchObject({ label: "7 天窗口", usedPercent: 100, status: "limited" });
});
