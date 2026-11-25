import * as React from "react";
import { Bot, CheckCircle2, CircleAlert, PanelRightOpen } from "lucide-react";
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
  showLabel = false,
  className,
}: {
  sessionId: string;
  agentId?: string | undefined;
  description?: string | undefined;
  prompt?: string | undefined;
  showLabel?: boolean;
  className?: string | undefined;
}) {
  const subSessionId = useLila((state) =>
    state.selectedId === sessionId || state.previewId === sessionId
      ? findMatchingSubSession(state.childrenByParent[sessionId], { agentId, description, prompt })
      : undefined,
  );
  const previewed = useLila((state) => Boolean(subSessionId && state.previewId === subSessionId));
  if (!subSessionId) return null;
  const label = previewed ? "关闭侧栏预览" : description ? `在侧栏打开：${description}` : "在侧栏打开";
  return (
    <Button
      type="button"
      size={showLabel ? "xs" : "icon-xs"}
      variant={showLabel ? "outline" : "ghost"}
      title={label}
      aria-label={label}
      aria-pressed={previewed}
      className={cn(
        showLabel
          ? "h-6 shrink-0 gap-1 px-2 text-[11px] font-normal text-muted-foreground hover:text-foreground aria-pressed:bg-accent aria-pressed:text-foreground"
          : "size-6 shrink-0 text-muted-foreground/70 hover:text-foreground aria-pressed:bg-accent aria-pressed:text-foreground",
        className,
      )}
      onClick={(event) => {
        event.stopPropagation();
        if (previewed) actions.closePreview();
        else actions.previewSubsession(subSessionId);
      }}
    >
      <PanelRightOpen className="size-3.5" />
      {showLabel ? (previewed ? "关闭侧栏" : "侧栏打开") : null}
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
        {showSubLink ? (
          <SubSessionLink
            sessionId={sessionId}
            agentId={info.agentId}
            description={info.description}
            prompt={info.prompt}
            showLabel
            className="ms-auto"
          />
        ) : null}
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
      {info.error ? <p className="text-sm text-destructive-foreground">{info.error}</p> : null}
      {info.output ? (
        <div className="rounded-lg border bg-card px-3 py-2 text-foreground">
          <Markdown text={info.output} streaming={info.active} />
        </div>
      ) : null}
    </div>
  );
}

export function SingleAgentCard({
  tool,
  sessionId,
  reveal = false,
  nested = false,
}: {
  tool: AgentToolItem;
  sessionId: string;
  reveal?: boolean;
  nested?: boolean;
}) {
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

  // Top-level sub-agent tasks expand by default while running; nested ones inside AgentGroup stay collapsed as the 2nd level.
  const shouldAutoOpen = info.isError || (!nested && info.active);
  const [open, setOpen] = React.useState(() => shouldAutoOpen || reveal);
  const [touched, setTouched] = React.useState(false);
  React.useEffect(() => {
    if (reveal) setOpen(true);
  }, [reveal]);
  React.useEffect(() => {
    if (!touched && shouldAutoOpen) setOpen(true);
  }, [shouldAutoOpen, touched]);

  const status = info.active ? "running" : info.isError ? "error" : undefined;
  const meta = [
    info.type,
    info.active ? info.activity : info.isError ? "出错" : undefined,
    info.durationText,
  ].filter(Boolean);

  return (
    <Trace>
      <TraceCollapsible
        open={open}
        onOpenChange={(next) => {
          setTouched(true);
          setOpen(next);
        }}
      >
        <TraceCollapsibleTrigger className="py-0.5">
          <TraceLine className="text-[13px]" {...(status ? { status } : {})}>
            <TraceIcon>
              {info.active ? (
                <Spinner />
              ) : info.isError ? (
                <CircleAlert />
              ) : (
                <CheckCircle2 className="text-success-foreground" />
              )}
            </TraceIcon>
            <TraceTitle className="max-w-[26rem] shrink truncate font-medium text-foreground">
              {info.description || "子 Agent"}
            </TraceTitle>
            {meta.length ? (
              <TraceDescription className="min-w-0 shrink truncate text-xs tabular-nums">
                {meta.join(" · ")}
              </TraceDescription>
            ) : null}
          </TraceLine>
        </TraceCollapsibleTrigger>
        <TraceCollapsibleContent>
          <div className="flex flex-col gap-2 text-foreground">
            <AgentDetailsBody info={info} sessionId={sessionId} showSubLink />
          </div>
        </TraceCollapsibleContent>
      </TraceCollapsible>
    </Trace>
  );
}

function areAgentGroupPropsEqual(
  prev: { tools: AgentToolItem[]; sessionId: string; revealId?: string | undefined },
  next: { tools: AgentToolItem[]; sessionId: string; revealId?: string | undefined },
): boolean {
  if (prev.sessionId !== next.sessionId) return false;
  if (prev.revealId !== next.revealId) return false;
  if (prev.tools.length !== next.tools.length) return false;
  for (let i = 0; i < prev.tools.length; i++) {
    if (prev.tools[i] !== next.tools[i]) return false;
  }
  return true;
}

/** Lightweight Trace timeline group when multiple sub-agents are dispatched in parallel. */
export const AgentGroup = React.memo(function AgentGroup({
  tools,
  sessionId,
  revealId,
}: {
  tools: AgentToolItem[];
  sessionId: string;
  revealId?: string | undefined;
}) {
  const subagents = useLila((state) => state.views[sessionId]?.state?.subagents);
  const byId = React.useMemo(() => new Map((subagents ?? []).map((entry) => [entry.id, entry])), [subagents]);
  const hasReveal = Boolean(revealId && tools.some((tool) => tool.id === revealId));

  let activeCount = 0;
  let errorCount = 0;
  let doneCount = 0;
  let totalTokens = 0;
  let totalCost = 0;
  for (const tool of tools) {
    const id = tool.subagent?.id;
    const live = id ? byId.get(id) : undefined;
    const status = live?.status ?? tool.subagent?.status ?? (tool.status === "running" ? "running" : tool.status === "error" ? "error" : "completed");
    if (status === "running" || status === "queued" || tool.status === "running") activeCount++;
    else if (isSubagentErrorStatus(status)) errorCount++;
    else doneCount++;
    totalTokens += live?.tokens ?? tool.subagent?.tokens ?? 0;
    totalCost += live?.cost ?? tool.subagent?.cost ?? 0;
  }

  const shouldAutoOpen = activeCount > 0 || errorCount > 0;
  const [open, setOpen] = React.useState(() => shouldAutoOpen || hasReveal);
  const [touched, setTouched] = React.useState(false);
  React.useEffect(() => {
    if (hasReveal) setOpen(true);
  }, [hasReveal]);
  React.useEffect(() => {
    if (!touched && shouldAutoOpen) setOpen(true);
  }, [shouldAutoOpen, touched]);

  const summaryParts = [
    activeCount ? `${activeCount} 运行中` : undefined,
    doneCount ? `${doneCount} 已完成` : undefined,
    errorCount ? `${errorCount} 出错` : undefined,
    totalTokens > 0 ? formatCompactTokens(totalTokens) : undefined,
    totalCost > 0 ? `$${totalCost.toFixed(4)}` : undefined,
  ].filter(Boolean);

  const lineStatus = activeCount > 0 ? "running" : errorCount > 0 ? "error" : undefined;

  return (
    <Trace>
      <TraceCollapsible
        open={open}
        onOpenChange={(next) => {
          setTouched(true);
          setOpen(next);
        }}
      >
        <TraceCollapsibleTrigger className="py-0.5">
          <TraceLine className="text-[13px]" {...(lineStatus ? { status: lineStatus } : {})}>
            <TraceIcon>
              {activeCount > 0 ? (
                <Spinner />
              ) : errorCount > 0 ? (
                <CircleAlert />
              ) : (
                <Bot />
              )}
            </TraceIcon>
            <TraceTitle>{tools.length} 个子 Agent</TraceTitle>
            {summaryParts.length ? (
              <TraceDescription className={cn("text-xs tabular-nums", errorCount > 0 && !activeCount && "text-destructive-foreground")}>
                {summaryParts.join(" · ")}
              </TraceDescription>
            ) : null}
          </TraceLine>
        </TraceCollapsibleTrigger>
        <TraceCollapsibleContent>
          <Trace className="gap-1">
            {tools.map((tool) => {
              const matched = tool.id === revealId;
              return (
                <div
                  key={tool.id}
                  data-transcript-item={tool.id}
                  className={matched ? "rounded-md bg-accent/60 p-1 ring-1 ring-ring/25" : undefined}
                >
                  <SingleAgentCard tool={tool} sessionId={sessionId} reveal={matched} nested />
                </div>
              );
            })}
          </Trace>
        </TraceCollapsibleContent>
      </TraceCollapsible>
    </Trace>
  );
}, areAgentGroupPropsEqual);
