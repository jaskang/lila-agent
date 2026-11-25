import * as React from "react";
import { Clock, ExternalLink, KeyRound, RotateCw, TriangleAlert } from "lucide-react";
import type { ProviderQuotaItem, QuotaWindow, UsageBucket, UsageRange, UsageSummary } from "@lila/protocol";
import { IconButton } from "@/components/icon-button";
import { Button } from "@/components/ui/button";
import { Empty, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Progress } from "@/components/ui/progress";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { cn } from "cn";
import { actions, useLila } from "@/lib/store";

const RANGE_OPTIONS: Array<{ value: UsageRange; label: string }> = [
  { value: "today", label: "今天" },
  { value: "7d", label: "7 天" },
  { value: "30d", label: "30 天" },
  { value: "all", label: "全部" },
];

const COMPACT_NUM = new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 1 });

function formatTokens(n: number): string {
  return n < 1_000 ? String(n) : COMPACT_NUM.format(n);
}

function formatCost(usd: number): string {
  if (usd <= 0) return "$0.00";
  if (usd < 0.01) return `$${usd.toFixed(4)}`;
  return `$${usd.toFixed(2)}`;
}

function formatResetCountdown(iso?: string): string | undefined {
  if (!iso) return undefined;
  const target = Date.parse(iso);
  if (!Number.isFinite(target)) return undefined;
  const diffMs = target - Date.now();
  if (diffMs <= 0) return "即将重置";
  const mins = Math.ceil(diffMs / 60_000);
  if (mins < 60) return `${mins} 分后重置`;
  const hours = Math.floor(mins / 60);
  const remMins = mins % 60;
  if (hours < 24) return remMins > 0 ? `${hours} 小时 ${remMins} 分后重置` : `${hours} 小时后重置`;
  const days = Math.floor(hours / 24);
  const remHours = hours % 24;
  return remHours > 0 ? `${days} 天 ${remHours} 小时后重置` : `${days} 天后重置`;
}

function WindowBar({ window: win }: { window: QuotaWindow }) {
  const pct = win.usedPercent;
  const resetText = formatResetCountdown(win.resetsAt);
  const tone =
    win.status === "limited"
      ? "text-destructive"
      : win.status === "warning"
        ? "text-warning-foreground"
        : "text-foreground";

  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-baseline justify-between gap-2 text-xs">
        <span className="truncate text-foreground">{win.label}</span>
        <span className={cn("shrink-0 font-mono tabular-nums", tone)}>
          {pct !== undefined ? `${pct}%` : win.remainingText ?? ""}
        </span>
      </div>
      {pct !== undefined ? (
        <Progress
          value={pct}
          className={cn(
            "h-1.5",
            win.status === "limited" && "[&_[data-slot=progress-indicator]]:bg-destructive",
            win.status === "warning" && "[&_[data-slot=progress-indicator]]:bg-warning-foreground",
          )}
        />
      ) : null}
      {(pct !== undefined && win.remainingText) || resetText ? (
        <div className="flex items-center justify-between gap-2 text-[11px] text-muted-foreground">
          <span className="truncate">{pct !== undefined ? win.remainingText ?? "" : ""}</span>
          {resetText ? (
            <span className="inline-flex shrink-0 items-center gap-1">
              <Clock className="size-3" />
              {resetText}
            </span>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function QuotaCard({ item }: { item: ProviderQuotaItem }) {
  return (
    <div className="flex flex-col gap-2.5 rounded-xl border border-border/70 p-3.5">
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-baseline gap-2">
          <span className="truncate text-sm font-medium text-foreground">{item.providerName}</span>
          {item.plan ? <span className="truncate text-xs text-muted-foreground">{item.plan}</span> : null}
        </div>
        {item.billingUrl ? (
          <IconButton size="icon-xs" label="打开控制台" onClick={() => void actions.openExternalUrl(item.billingUrl!)}>
            <ExternalLink />
          </IconButton>
        ) : null}
      </div>

      {item.status === "ok" ? (
        <div className="flex flex-col gap-2.5">
          {item.balances?.map((balance, index) => (
            <div key={index} className="flex flex-col gap-0.5">
              <span className="font-mono text-xl font-semibold tracking-tight text-foreground tabular-nums">{balance.total}</span>
              {balance.details ? <span className="text-xs text-muted-foreground">{balance.details}</span> : null}
            </div>
          ))}
          {item.windows?.map((win, index) => (
            <WindowBar key={index} window={win} />
          ))}
        </div>
      ) : item.status === "waiting" ? (
        <p className="text-xs text-muted-foreground">发送消息后自动同步</p>
      ) : item.status === "error" ? (
        <div className="flex items-start gap-1.5 text-xs text-destructive">
          <TriangleAlert className="mt-0.5 size-3.5 shrink-0" />
          <span className="break-words">{item.error ?? "查询失败"}</span>
        </div>
      ) : (
        <p className="text-xs text-muted-foreground">暂不支持自动查询</p>
      )}
    </div>
  );
}

function BreakdownList({ items }: { items: UsageBucket[] }) {
  if (!items.length) return <p className="px-3 py-6 text-center text-xs text-muted-foreground">暂无记录</p>;
  const maxCost = Math.max(...items.map((item) => item.cost), 0);
  const maxTokens = Math.max(...items.map((item) => item.totalTokens), 1);

  return (
    <div className="divide-y divide-border/50">
      {items.map((item) => {
        const share = maxCost > 0 ? (item.cost / maxCost) * 100 : (item.totalTokens / maxTokens) * 100;
        const cacheBase = item.input + item.cacheRead;
        const cacheRate = cacheBase > 0 ? Math.round((item.cacheRead / cacheBase) * 100) : 0;
        return (
          <div key={item.key} className="flex flex-col gap-1.5 px-1 py-2.5">
            <div className="flex items-baseline justify-between gap-2">
              <div className="flex min-w-0 items-baseline gap-2">
                <span className="truncate text-xs font-medium text-foreground" title={item.key}>
                  {item.label}
                </span>
                {item.provider ? <span className="shrink-0 text-[11px] text-muted-foreground">{item.provider}</span> : null}
              </div>
              <div className="flex shrink-0 items-baseline gap-3 font-mono text-xs tabular-nums">
                <span className="text-muted-foreground">{formatTokens(item.totalTokens)}</span>
                <span className="font-medium text-foreground">{formatCost(item.cost)}</span>
              </div>
            </div>
            <Progress value={Math.max(2, share)} className="h-1" />
            <div className="flex flex-wrap items-center gap-x-3 gap-y-0.5 text-[11px] text-muted-foreground tabular-nums">
              <span>{item.requests} 次</span>
              <span>入 {formatTokens(item.input)}</span>
              <span>出 {formatTokens(item.output)}</span>
              {item.cacheRead > 0 ? <span>缓存 {cacheRate}%</span> : null}
            </div>
          </div>
        );
      })}
    </div>
  );
}

export function UsageSettingsTab() {
  const connected = useLila((state) => state.connection === "connected");
  const [range, setRange] = React.useState<UsageRange>("7d");
  const [quotas, setQuotas] = React.useState<ProviderQuotaItem[]>();
  const [quotasLoading, setQuotasLoading] = React.useState(false);
  const [quotasError, setQuotasError] = React.useState<string>();
  const [summary, setSummary] = React.useState<UsageSummary>();
  const [summaryLoading, setSummaryLoading] = React.useState(false);
  const [summaryError, setSummaryError] = React.useState<string>();
  const [, setTick] = React.useState(0);
  const summarySeqRef = React.useRef(0);

  React.useEffect(() => {
    const timer = window.setInterval(() => setTick((t) => t + 1), 30_000);
    return () => clearInterval(timer);
  }, []);

  const loadQuotas = React.useCallback(
    async (refresh = false) => {
      if (!connected) return;
      setQuotasLoading(true);
      setQuotasError(undefined);
      try {
        setQuotas(await actions.listQuotas(refresh));
      } catch (failure) {
        setQuotasError(failure instanceof Error ? failure.message : String(failure));
      } finally {
        setQuotasLoading(false);
      }
    },
    [connected],
  );

  const loadSummary = React.useCallback(
    async (targetRange: UsageRange) => {
      if (!connected) return;
      const seq = ++summarySeqRef.current;
      setSummaryLoading(true);
      setSummaryError(undefined);
      try {
        const next = await actions.getUsageSummary(targetRange, true);
        if (summarySeqRef.current === seq) setSummary(next);
      } catch (failure) {
        if (summarySeqRef.current === seq) setSummaryError(failure instanceof Error ? failure.message : String(failure));
      } finally {
        if (summarySeqRef.current === seq) setSummaryLoading(false);
      }
    },
    [connected],
  );

  React.useEffect(() => {
    void loadQuotas(false);
  }, [loadQuotas]);

  React.useEffect(() => {
    void loadSummary(range);
  }, [range, loadSummary]);

  const totals = summary?.totals;
  const cacheBase = totals ? totals.input + totals.cacheRead : 0;
  const cacheRate = cacheBase > 0 && totals ? Math.round((totals.cacheRead / cacheBase) * 100) : 0;
  const maxDailyTokens = React.useMemo(
    () => (summary?.byDay.length ? Math.max(...summary.byDay.map((d) => d.totalTokens), 1) : 1),
    [summary?.byDay],
  );

  return (
    <div className="flex flex-col gap-6">
      <section className="flex flex-col gap-2.5">
        <div className="flex items-center justify-between gap-2 px-1">
          <h3 className="text-xs font-medium text-muted-foreground">服务商额度</h3>
          <IconButton size="icon-xs" label="刷新额度" disabled={quotasLoading} onClick={() => void loadQuotas(true)}>
            {quotasLoading ? <Spinner /> : <RotateCw />}
          </IconButton>
        </div>

        {quotasError ? <p className="px-1 text-xs text-destructive-foreground">{quotasError}</p> : null}

        {quotasLoading && !quotas ? (
          <div className="flex items-center justify-center gap-2 py-8 text-xs text-muted-foreground">
            <Spinner />
            正在读取…
          </div>
        ) : !quotas?.length ? (
          <Empty className="py-6">
            <EmptyHeader>
              <EmptyTitle>尚未配置服务商</EmptyTitle>
            </EmptyHeader>
            <Button type="button" size="sm" variant="outline" onClick={() => actions.setSettingsTab("providers")}>
              <KeyRound />
              配置服务商
            </Button>
          </Empty>
        ) : (
          <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
            {quotas.map((item) => (
              <QuotaCard key={item.providerId} item={item} />
            ))}
          </div>
        )}
      </section>

      <section className="flex flex-col gap-3">
        <div className="flex items-center justify-between gap-2 px-1">
          <h3 className="text-xs font-medium text-muted-foreground">本地消耗</h3>
          <div className="flex items-center gap-1.5">
            <div className="inline-flex rounded-lg bg-muted/60 p-0.5">
              {RANGE_OPTIONS.map((opt) => {
                const active = range === opt.value;
                return (
                  <Button
                    key={opt.value}
                    type="button"
                    size="xs"
                    variant="ghost"
                    aria-pressed={active}
                    className={cn(
                      "px-2.5 font-normal text-muted-foreground hover:text-foreground",
                      active && "bg-background font-medium text-foreground shadow-2xs",
                    )}
                    onClick={() => setRange(opt.value)}
                  >
                    {opt.label}
                  </Button>
                );
              })}
            </div>
            <IconButton size="icon-xs" label="刷新统计" disabled={summaryLoading} onClick={() => void loadSummary(range)}>
              {summaryLoading ? <Spinner /> : <RotateCw />}
            </IconButton>
          </div>
        </div>

        {summaryError ? (
          <p className="px-1 text-xs text-destructive-foreground">{summaryError}</p>
        ) : summaryLoading && !summary ? (
          <div className="flex items-center justify-center gap-2 py-10 text-xs text-muted-foreground">
            <Spinner />
            正在统计…
          </div>
        ) : summary ? (
          <>
            <div className="grid grid-cols-2 gap-4 border-y border-border/60 py-3.5 sm:grid-cols-4">
              <div className="flex flex-col gap-0.5 px-1">
                <span className="text-xs text-muted-foreground">估算花费</span>
                <span className="font-mono text-lg font-semibold tracking-tight tabular-nums">{formatCost(summary.totals.cost)}</span>
              </div>
              <div className="flex flex-col gap-0.5 px-1">
                <span className="text-xs text-muted-foreground">Token</span>
                <span className="font-mono text-lg font-semibold tracking-tight tabular-nums">{formatTokens(summary.totals.totalTokens)}</span>
              </div>
              <div className="flex flex-col gap-0.5 px-1">
                <span className="text-xs text-muted-foreground">缓存命中</span>
                <span className="font-mono text-lg font-semibold tracking-tight tabular-nums">{cacheRate}%</span>
              </div>
              <div className="flex flex-col gap-0.5 px-1">
                <span className="text-xs text-muted-foreground">请求</span>
                <span className="font-mono text-lg font-semibold tracking-tight tabular-nums">{summary.totals.requests}</span>
              </div>
            </div>

            <Tabs defaultValue="models" className="gap-2">
              <TabsList>
                <TabsTrigger value="models">模型</TabsTrigger>
                <TabsTrigger value="providers">服务商</TabsTrigger>
                <TabsTrigger value="daily">每日</TabsTrigger>
              </TabsList>

              <TabsContent value="models" className="border-t border-border/60">
                <BreakdownList items={summary.byModel} />
              </TabsContent>

              <TabsContent value="providers" className="border-t border-border/60">
                <BreakdownList items={summary.byProvider} />
              </TabsContent>

              <TabsContent value="daily" className="border-t border-border/60">
                {summary.byDay.length ? (
                  <div className="divide-y divide-border/50">
                    {[...summary.byDay].reverse().map((day) => (
                      <div key={day.date} className="flex flex-col gap-1.5 px-1 py-2.5">
                        <div className="flex items-baseline justify-between gap-2 font-mono text-xs tabular-nums">
                          <span className="font-medium text-foreground">{day.date}</span>
                          <div className="flex items-baseline gap-3">
                            <span className="text-muted-foreground">{day.requests} 次</span>
                            <span className="text-muted-foreground">{formatTokens(day.totalTokens)}</span>
                            <span className="font-medium text-foreground">{formatCost(day.cost)}</span>
                          </div>
                        </div>
                        <Progress value={Math.max(2, (day.totalTokens / maxDailyTokens) * 100)} className="h-1" />
                      </div>
                    ))}
                  </div>
                ) : (
                  <p className="px-3 py-6 text-center text-xs text-muted-foreground">暂无记录</p>
                )}
              </TabsContent>
            </Tabs>
          </>
        ) : null}
      </section>
    </div>
  );
}
