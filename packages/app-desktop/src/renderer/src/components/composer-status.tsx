import * as React from "react";
import { Bot, Clock3, MessageCircleQuestion, ShieldAlert } from "lucide-react";
import type { SessionState, SubagentProgress } from "@lila/protocol";
import { SubSessionLink } from "@/components/agent-card";
import { Badge } from "@/components/ui/badge";
import { Spinner } from "@/components/ui/spinner";
import { Trace, TraceCollapsible, TraceCollapsibleContent, TraceCollapsibleTrigger, TraceDescription, TraceIcon, TraceLine, TraceTitle } from "@/components/ui/trace";
import { formatCompactTokens, formatElapsed } from "@/lib/format";

function ActiveSubagentRow({ sessionId, subagent, now }: { sessionId: string; subagent: SubagentProgress; now: number }) {
  const stats = [
    subagent.turns ? `${subagent.turns} 轮` : undefined,
    subagent.toolUses ? `${subagent.toolUses} 次工具` : undefined,
    subagent.tokens ? formatCompactTokens(subagent.tokens) : undefined,
    subagent.cost ? `$${subagent.cost.toFixed(4)}` : undefined,
    formatElapsed(subagent.startedAt, now),
  ].filter(Boolean);

  return (
    <div className="flex items-center gap-2 rounded-lg px-2 py-1 text-xs hover:bg-muted/50">
      <Spinner className="size-3.5 shrink-0 text-muted-foreground" />
      <Badge variant="secondary" className="h-5 shrink-0 gap-1 font-normal">
        <Bot className="size-3" />
        {subagent.type}
      </Badge>
      {subagent.background ? (
        <Badge variant="outline" className="h-5 shrink-0 font-normal text-muted-foreground">
          后台
        </Badge>
      ) : null}
      <span className="min-w-0 flex-1 truncate" title={subagent.activity ? `${subagent.description} · ${subagent.activity}` : subagent.description}>
        <span className="font-medium">{subagent.description || "子 Agent"}</span>
        {subagent.activity ? <span className="ms-1.5 font-mono text-muted-foreground">· {subagent.activity}</span> : null}
      </span>
      <span className="hidden shrink-0 text-muted-foreground tabular-nums sm:inline">{stats.join(" · ")}</span>
      <SubSessionLink sessionId={sessionId} agentId={subagent.id} description={subagent.description} />
    </div>
  );
}

/** shadcn Trace and Queue composition for the status above the composer. */
export function ComposerStatus({ sessionId, state }: { sessionId: string; state: SessionState }) {
  const active = (state.subagents ?? []).filter((entry) =>
    (entry.status === "queued" || entry.status === "running") && (!state.runActive || entry.background),
  );
  const { steering, followUp } = state.queue;
  const queued = steering.length + followUp.length;
  const pending = state.pending[0];
  const [now, setNow] = React.useState(Date.now);
  React.useEffect(() => {
    if (!active.length) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [active.length]);

  const sessionStatus = pending ? (pending.kind === "approval" ? "等待你批准" : "等待你回答")
    : state.access === "starting" ? "正在初始化会话"
    : state.retry ? `正在重试（${state.retry.attempt}/${state.retry.max}）`
    : state.compacting ? "正在压缩上下文"
    : undefined;
  const title = sessionStatus ?? (active.length
    ? `${active.length} 个子 Agent ${active.every((entry) => entry.background) ? "在后台运行" : "正在运行"}`
    : queued ? "发送队列" : undefined);
  if (!title) return null;
  const Icon = pending ? (pending.kind === "approval" ? ShieldAlert : MessageCircleQuestion)
    : sessionStatus || active.length ? Spinner : Clock3;

  const activePreview = !sessionStatus && active.length > 0
    ? active.map((entry) => entry.description?.trim() || entry.type).filter(Boolean).join("、")
    : undefined;

  const summary = (
    <TraceLine className="py-0.5" {...(pending ? { status: "pending" as const } : {})}>
      <TraceIcon><Icon aria-hidden /></TraceIcon>
      <TraceTitle role="status" className="shrink-0 font-medium">{title}</TraceTitle>
      {activePreview ? <TraceDescription className="min-w-0 shrink truncate text-xs">{activePreview}</TraceDescription> : null}
      {sessionStatus && active.length > 0 ? <TraceDescription className="text-xs">{active.length} 个子 Agent</TraceDescription> : null}
      {queued > 0 ? <TraceDescription className="text-xs">{queued} 条待发送</TraceDescription> : null}
    </TraceLine>
  );
  if (!active.length && !queued) return <Trace className="min-h-7 justify-center px-2">{summary}</Trace>;

  return (
    <Trace className="min-h-7 justify-center px-2">
      <TraceCollapsible>
        <TraceCollapsibleTrigger>{summary}</TraceCollapsibleTrigger>
        <TraceCollapsibleContent className="max-h-44 overflow-y-auto">
          <Trace className="gap-1.5">
            {active.map((agent) => <ActiveSubagentRow key={agent.id} sessionId={sessionId} subagent={agent} now={now} />)}
            {([["插话", steering], ["排队", followUp]] as const).flatMap(([label, messages]) => messages.map((text, index) => (
              <TraceLine key={`${label}-${index}`} status="pending" className="w-full">
                <TraceIcon><Clock3 /></TraceIcon>
                <TraceDescription>{label}</TraceDescription>
                <TraceTitle title={text}>{text}</TraceTitle>
              </TraceLine>
            )))}
          </Trace>
        </TraceCollapsibleContent>
      </TraceCollapsible>
    </Trace>
  );
}
