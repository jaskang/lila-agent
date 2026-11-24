import * as React from "react";
import { Bot, CheckCircle2, ChevronDown, ChevronRight, CircleAlert, PanelRightOpen } from "lucide-react";
import type { SessionSummary, SubagentProgress, TranscriptItem } from "@lila/protocol";
import { Markdown } from "@/components/markdown";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import {
  Trace,
  TraceCollapsible,
  TraceCollapsibleContent,
  TraceCollapsibleTrigger,
  TraceDescription,
  TraceIcon,
  TraceLine,
  TraceTitle,
} from "@/components/ui/trace";
import { cn } from "cn";
import { formatCompactTokens, formatDuration, formatElapsed, isSubagentErrorStatus } from "@/lib/format";
import { actions, useLila } from "@/lib/store";

export type AgentToolItem = Extract<TranscriptItem, { kind: "tool" }>;

const AGENT_STATUS: Record<string, string> = {
  running: "运行中",
  queued: "排队中",
  completed: "已完成",
  steered: "已完成（被引导）",
  stopped: "已停止",
  aborted: "已中止",
  error: "出错",
};

export function findMatchingSubSession(
  children: readonly SessionSummary[] | undefined,
  match: { agentId?: string | undefined; description?: string | undefined; prompt?: string | undefined },
): string | undefined {
  if (!children?.length) return undefined;
  const agentId = match.agentId?.toLowerCase();
  if (agentId) {
    const byId = children.find((child) => {
      const childAgentId = child.agentId?.toLowerCase();
      return childAgentId && (childAgentId === agentId || agentId.startsWith(childAgentId) || childAgentId.startsWith(agentId));
    });
    return byId?.id;
  }
  const promptLine = match.prompt?.trim().split("\n")[0]?.trim();
  const candidates = [match.description?.trim(), promptLine].filter((value): value is string => Boolean(value));
  for (const candidate of candidates) {
    const normalized = candidate.toLowerCase();
    const matching = children.filter((child) => {
      const title = child.title.replace(/…$/, "").trim().toLowerCase();
      return title && (normalized === title || normalized.startsWith(title) || title.startsWith(normalized));
    });
    if (matching.length === 1) return matching[0]?.id;
  }
  return undefined;
}

type ResolvedAgent = {
  agentId: string | undefined;
  type: string;
  description: string | undefined;
  status: string;
  active: boolean;
  isError: boolean;
  background: boolean;
  activity: string | undefined;
  modelName: string | undefined;
  turnCount: number | undefined;
  maxTurns: number | undefined;
  toolUses: number | undefined;
  tokens: number;
  tokensText: string | undefined;
  cost: number;
  durationText: string | undefined;
  startedAt: number | undefined;
  prompt: string | undefined;
  output: string | undefined;
  error: string | undefined;
};

function resolveAgent(tool: AgentToolItem, live: SubagentProgress | undefined, now: number): ResolvedAgent {
  const sub = tool.subagent;
  const agentId = sub?.id;
  const type = live?.type || sub?.type || "Agent";
  const description = live?.description || sub?.description;
  const background = Boolean(live?.background ?? sub?.background);
  const status = live?.status ?? sub?.status ?? (tool.status === "running" ? "running" : tool.status === "error" ? "error" : "completed");
  const active = status === "running" || status === "queued" || tool.status === "running";
  const isError = isSubagentErrorStatus(status);
  const activity = active ? (live?.activity ?? sub?.activity) : undefined;
  const modelName = live?.model ?? sub?.model;
  const turnCount = live?.turns || sub?.turns;
  const maxTurns = sub?.maxTurns;
  const toolUses = live?.toolUses || sub?.toolUses;
  const tokens = live?.tokens ?? sub?.tokens ?? 0;
  const tokensText = tokens > 0 ? formatCompactTokens(tokens) : undefined;
  const cost = live?.cost ?? sub?.cost ?? 0;
  const durationMs = live?.durationMs ?? sub?.durationMs ?? tool.durationMs;
  const durationText = active
    ? live?.startedAt
      ? formatElapsed(live.startedAt, now)
      : undefined
    : formatDuration(durationMs);
  const prompt = sub?.prompt;
  const output = live?.output ?? (sub ? sub.output : tool.output || undefined);
  const error = live?.error ?? sub?.error;

  return {
    agentId,
    type,
    description,
    status,
    active,
    isError,
    background,
    activity,
    modelName,
    turnCount,
    maxTurns,
    toolUses,
    tokens,
    tokensText,
    cost,
    durationText,
    startedAt: live?.startedAt,
    prompt,
    output,
    error,
  };
}

export function SubSessionLink({
  sessionId,
  agentId,
  description,
  prompt,
  className,
}: {
  sessionId: string;
  agentId?: string | undefined;
  description?: string | undefined;
  prompt?: string | undefined;
  className?: string | undefined;
}) {
  const subSessionId = useLila((state) => state.selectedId === sessionId
    ? findMatchingSubSession(state.childrenByParent[sessionId], { agentId, description, prompt })
    : undefined);
  const previewed = useLila((state) => Boolean(subSessionId && state.previewId === subSessionId));
  if (!subSessionId) return null;
  return (
    <Button
      type="button"
      size="xs"
      variant="ghost"
      aria-label={description ? `侧边查看：${description}` : "侧边查看"}
      aria-pressed={previewed}
      className={cn("h-6 shrink-0 px-1.5 text-muted-foreground aria-pressed:bg-accent", className)}
      onClick={(event) => {
        event.stopPropagation();
        if (previewed) actions.closePreview();
        else actions.previewSubsession(subSessionId);
      }}
    >
      侧边查看
      <PanelRightOpen />
    </Button>
  );
}

function AgentDetailsBody({ info, sessionId, showSubLink = true }: { info: ResolvedAgent; sessionId: string; showSubLink?: boolean }) {
  const stats = [
    info.modelName,
    info.turnCount ? (info.maxTurns ? `第 ${info.turnCount}/${info.maxTurns} 轮` : `${info.turnCount} 轮`) : undefined,
    info.toolUses ? `${info.toolUses} 次工具调用` : undefined,
    info.tokensText,
    info.cost > 0 ? `$${info.cost.toFixed(4)}` : undefined,
    info.durationText,
  ].filter(Boolean);

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-1.5 text-xs">
        <Badge variant={info.isError ? "destructive" : "secondary"}>
          {info.background && info.active ? "后台运行中" : (AGENT_STATUS[info.status] ?? info.status)}
        </Badge>
        {stats.map((stat) => (
          <span key={stat} className="text-muted-foreground tabular-nums">
            {stat}
          </span>
        ))}
        {showSubLink ? <SubSessionLink sessionId={sessionId} agentId={info.agentId} description={info.description} prompt={info.prompt} className="ms-auto" /> : null}
      </div>
      {info.active && info.activity ? (
        <div className="flex items-center gap-2 rounded-lg border bg-muted/40 px-2.5 py-1.5 text-xs text-muted-foreground">
          <Spinner className="size-3.5 shrink-0" />
          <span className="truncate font-mono">{info.activity}</span>
        </div>
      ) : null}
      {info.prompt ? (
        <div className="rounded-lg bg-muted/40 px-3 py-2 text-xs leading-relaxed text-muted-foreground whitespace-pre-wrap break-words">
          {info.prompt}
        </div>
      ) : null}
      {info.error ? <p className="text-sm text-destructive">{info.error}</p> : null}
      {info.output ? (
        <div className="rounded-lg border bg-card px-3 py-2 text-foreground">
          <Markdown text={info.output} streaming={info.active} />
        </div>
      ) : null}
    </div>
  );
}

export function SingleAgentCard({ tool, sessionId, reveal = false }: { tool: AgentToolItem; sessionId: string; reveal?: boolean }) {
  const agentId = tool.subagent?.id;
  const live = useLila<SubagentProgress | undefined>((state) =>
    agentId ? state.views[sessionId]?.state?.subagents?.find((entry) => entry.id === agentId) : undefined,
  );
  const [now, setNow] = React.useState(() => Date.now());
  const info = resolveAgent(tool, live, now);
  React.useEffect(() => {
    if (!info.active || !info.startedAt) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [info.active, info.startedAt]);

  const [open, setOpen] = React.useState(info.isError);
  const [touched, setTouched] = React.useState(false);
  React.useEffect(() => {
    if (!touched && info.isError) setOpen(true);
  }, [info.isError, touched]);

  const status = info.active ? "running" : info.isError ? "error" : undefined;
  const meta = [
    info.activity,
    info.turnCount ? `${info.turnCount} 轮` : undefined,
    info.toolUses ? `${info.toolUses} 次工具` : undefined,
    info.durationText,
  ].filter(Boolean);

  return (
    <Trace>
      <TraceCollapsible
        open={open || reveal}
        onOpenChange={(next) => {
          setTouched(true);
          setOpen(next);
        }}
      >
        <div className="flex min-w-0 items-center gap-1.5">
          <TraceCollapsibleTrigger>
            <TraceLine {...(status ? { status } : {})}>
              <TraceIcon>
                {info.active ? (
                  <Spinner />
                ) : info.isError ? (
                  <CircleAlert />
                ) : (
                  <CheckCircle2 className="text-success-foreground" />
                )}
              </TraceIcon>
              <TraceTitle>子 Agent · {info.type}</TraceTitle>
              {info.description ? <TraceDescription className="max-w-[22rem] shrink truncate font-mono text-xs">{info.description}</TraceDescription> : null}
              {meta.length ? <TraceDescription className="truncate text-xs tabular-nums">{meta.join(" · ")}</TraceDescription> : null}
            </TraceLine>
          </TraceCollapsibleTrigger>
          <SubSessionLink sessionId={sessionId} agentId={info.agentId} description={info.description} prompt={info.prompt} />
        </div>
        <TraceCollapsibleContent>
          <div className="flex flex-col gap-2 text-foreground">
            <AgentDetailsBody info={info} sessionId={sessionId} showSubLink={false} />
          </div>
        </TraceCollapsibleContent>
      </TraceCollapsible>
    </Trace>
  );
}

function AgentGroupRow({
  tool,
  sessionId,
  live,
  now,
  open,
  onToggle,
}: {
  tool: AgentToolItem;
  sessionId: string;
  live: SubagentProgress | undefined;
  now: number;
  open: boolean;
  onToggle: () => void;
}) {
  const info = resolveAgent(tool, live, now);
  const compactMeta = [
    info.turnCount ? `${info.turnCount} 轮` : undefined,
    info.toolUses ? `${info.toolUses} 次工具` : undefined,
    info.tokensText,
    info.durationText,
  ].filter(Boolean);

  return (
    <div className="flex flex-col">
      <div className="flex min-w-0 items-center gap-1 pe-3 hover:bg-muted/50">
        <button
          type="button"
          aria-expanded={open}
          onClick={onToggle}
          className="flex min-w-0 flex-1 cursor-pointer items-center gap-2 ps-3 py-2 text-start text-xs focus-visible:bg-muted/50 focus-visible:outline-none"
        >
          <span className="shrink-0 text-muted-foreground">{open ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}</span>
          <span className="shrink-0">
            {info.active ? (
              <Spinner className="size-3.5 text-muted-foreground" />
            ) : info.isError ? (
              <CircleAlert className="size-3.5 text-destructive" />
            ) : (
              <CheckCircle2 className="size-3.5 text-success-foreground" />
            )}
          </span>
          <Badge variant="secondary" className="h-5 shrink-0 px-1.5 font-normal">
            {info.type}
          </Badge>
          <span className="min-w-0 flex-1 truncate" title={info.activity ? `${info.description ?? ""} · ${info.activity}` : info.description}>
            <span className="font-medium text-foreground">{info.description || "子 Agent"}</span>
            {info.activity ? <span className="ms-1.5 font-mono text-muted-foreground">· {info.activity}</span> : null}
          </span>
          {compactMeta.length ? <span className="hidden shrink-0 text-muted-foreground tabular-nums sm:inline">{compactMeta.join(" · ")}</span> : null}
        </button>
        <SubSessionLink sessionId={sessionId} agentId={info.agentId} description={info.description} prompt={info.prompt} />
      </div>
      {open ? (
        <div className="border-t bg-muted/20 px-4 py-3">
          <AgentDetailsBody info={info} sessionId={sessionId} showSubLink={false} />
        </div>
      ) : null}
    </div>
  );
}

function areAgentGroupPropsEqual(
  prev: { tools: AgentToolItem[]; sessionId: string },
  next: { tools: AgentToolItem[]; sessionId: string },
): boolean {
  if (prev.sessionId !== next.sessionId) return false;
  if (prev.tools.length !== next.tools.length) return false;
  for (let i = 0; i < prev.tools.length; i++) {
    if (prev.tools[i] !== next.tools[i]) return false;
  }
  return true;
}

/** Compact grouped view when multiple sub-agents are dispatched in parallel. */
export const AgentGroup = React.memo(function AgentGroup({ tools, sessionId }: { tools: AgentToolItem[]; sessionId: string }) {
  const subagents = useLila((state) => state.views[sessionId]?.state?.subagents);
  const byId = React.useMemo(() => new Map((subagents ?? []).map((entry) => [entry.id, entry])), [subagents]);
  const [expanded, setExpanded] = React.useState<Record<string, boolean>>({});
  const [now, setNow] = React.useState(() => Date.now());

  const resolved = tools.map((tool) => {
    const id = tool.subagent?.id;
    return resolveAgent(tool, id ? byId.get(id) : undefined, now);
  });
  const activeCount = resolved.filter((info) => info.active).length;
  const errorCount = resolved.filter((info) => info.isError).length;
  const doneCount = resolved.length - activeCount - errorCount;
  const totalTokens = resolved.reduce((sum, info) => sum + info.tokens, 0);
  const totalCost = resolved.reduce((sum, info) => sum + info.cost, 0);

  React.useEffect(() => {
    if (!activeCount) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [activeCount]);

  const allExpanded = tools.every((tool) => Boolean(expanded[tool.id]));
  const toggleAll = () => {
    const next = !allExpanded;
    setExpanded(Object.fromEntries(tools.map((tool) => [tool.id, next])));
  };

  const summaryParts = [
    activeCount ? `${activeCount} 运行中` : undefined,
    doneCount ? `${doneCount} 已完成` : undefined,
    errorCount ? `${errorCount} 出错` : undefined,
  ].filter(Boolean);

  const metricsParts = [
    totalTokens > 0 ? formatCompactTokens(totalTokens) : undefined,
    totalCost > 0 ? `$${totalCost.toFixed(4)}` : undefined,
  ].filter(Boolean);

  return (
    <div className="overflow-hidden rounded-xl border bg-card/60">
      <div className="flex flex-wrap items-center gap-2 border-b bg-muted/30 px-3 py-2 text-xs">
        {activeCount > 0 ? (
          <Spinner className="size-3.5 shrink-0 text-muted-foreground" />
        ) : errorCount > 0 ? (
          <CircleAlert className="size-3.5 shrink-0 text-destructive" />
        ) : (
          <Bot className="size-3.5 shrink-0 text-muted-foreground" />
        )}
        <span className="font-medium">{tools.length} 个子 Agent</span>
        {summaryParts.length ? (
          <span className={cn("text-muted-foreground", errorCount > 0 && !activeCount && "text-destructive")}>{summaryParts.join(" · ")}</span>
        ) : null}
        {metricsParts.length ? <span className="text-muted-foreground tabular-nums">· {metricsParts.join(" · ")}</span> : null}
        <Button type="button" size="xs" variant="ghost" className="ms-auto h-6 px-2 text-muted-foreground" onClick={toggleAll}>
          {allExpanded ? "全部收起" : "全部展开"}
        </Button>
      </div>
      <div className="divide-y">
        {tools.map((tool) => {
          const id = tool.subagent?.id;
          return (
            <AgentGroupRow
              key={tool.id}
              tool={tool}
              sessionId={sessionId}
              live={id ? byId.get(id) : undefined}
              now={now}
              open={Boolean(expanded[tool.id])}
              onToggle={() => setExpanded((current) => ({ ...current, [tool.id]: !current[tool.id] }))}
            />
          );
        })}
      </div>
    </div>
  );
}, areAgentGroupPropsEqual);
