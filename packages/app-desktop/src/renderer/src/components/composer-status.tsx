import * as React from "react";
import { ArrowUp, Clock3, MessageCircleQuestion, ShieldAlert, Square, Trash2, X } from "lucide-react";
import type { SessionState, SubagentProgress } from "@lila/protocol";
import { SubSessionLink } from "@/components/agent-card";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { Trace, TraceCollapsible, TraceCollapsibleContent, TraceCollapsibleTrigger, TraceDescription, TraceIcon, TraceLine, TraceTitle } from "@/components/ui/trace";
import { formatElapsed } from "@/lib/format";
import { actions } from "@/lib/store";

function ActiveSubagentRow({ sessionId, subagent, now }: { sessionId: string; subagent: SubagentProgress; now: number }) {
  const elapsed = formatElapsed(subagent.startedAt, now);
  const [stopping, setStopping] = React.useState(false);

  return (
    <div className="flex items-center gap-2 rounded-lg px-2 py-1 text-xs hover:bg-muted/50">
      <Spinner className="size-3.5 shrink-0 text-muted-foreground" />
      <span className="min-w-0 flex-1 truncate" title={subagent.activity ? `${subagent.description} · ${subagent.activity}` : subagent.description}>
        <span className="font-medium">{subagent.description || subagent.type || "子 Agent"}</span>
        {subagent.activity ? <span className="ms-1.5 font-mono text-muted-foreground">· {subagent.activity}</span> : null}
      </span>
      {elapsed ? <span className="shrink-0 text-muted-foreground tabular-nums">{elapsed}</span> : null}
      <Button
        type="button"
        size="icon-xs"
        variant="ghost"
        disabled={stopping}
        title="停止该子 Agent"
        aria-label={`停止 ${subagent.description || "子 Agent"}`}
        className="size-6 shrink-0 text-muted-foreground hover:text-destructive"
        onClick={() => {
          setStopping(true);
          void actions.stopSubagent(sessionId, subagent.id).finally(() => setStopping(false));
        }}
      >
        <Square className="size-2.5 fill-current" />
      </Button>
      <SubSessionLink sessionId={sessionId} agentId={subagent.id} description={subagent.description} />
    </div>
  );
}

/** shadcn Trace and Queue composition for the status above the composer. */
export function ComposerStatus({ sessionId, state }: { sessionId: string; state: SessionState }) {
  const active = (state.subagents ?? []).filter((entry) => entry.status === "queued" || entry.status === "running");
  const { steering, followUp } = state.queue;
  const queued = steering.length + followUp.length;
  const pending = state.pending[0];
  const [now, setNow] = React.useState(Date.now);
  const [busyKey, setBusyKey] = React.useState<string>();
  React.useEffect(() => {
    if (!active.length) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [active.length]);

  const mutateQueue = async (key: string, action: "remove" | "promote" | "clear", kind?: "steering" | "followUp", index?: number) => {
    if (busyKey) return;
    setBusyKey(key);
    try {
      await actions.updateQueue(sessionId, action, kind, index);
    } finally {
      setBusyKey(undefined);
    }
  };

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
      {state.retry?.message ? <TraceDescription className="min-w-0 shrink truncate text-xs">{state.retry.message}</TraceDescription> : null}
      {activePreview ? <TraceDescription className="min-w-0 shrink truncate text-xs">{activePreview}</TraceDescription> : null}
      {sessionStatus && active.length > 0 ? <TraceDescription className="text-xs">{active.length} 个子 Agent</TraceDescription> : null}
      {queued > 0 ? <TraceDescription className="text-xs">{queued} 条待发送</TraceDescription> : null}
    </TraceLine>
  );
  if (!active.length && !queued) return <Trace className="min-h-7 justify-center px-2">{summary}</Trace>;

  return (
    <Trace className="min-h-7 justify-center px-2">
      <TraceCollapsible defaultOpen={queued > 0}>
        <TraceCollapsibleTrigger>{summary}</TraceCollapsibleTrigger>
        <TraceCollapsibleContent className="max-h-44 overflow-y-auto">
          <Trace className="gap-1.5">
            {active.map((agent) => <ActiveSubagentRow key={agent.id} sessionId={sessionId} subagent={agent} now={now} />)}
            {queued > 1 ? (
              <div className="flex justify-end px-1">
                <Button
                  type="button"
                  size="xs"
                  variant="ghost"
                  className="h-6 text-[11px] text-muted-foreground hover:text-destructive"
                  disabled={Boolean(busyKey)}
                  onClick={() => void mutateQueue("clear-all", "clear")}
                >
                  <Trash2 className="size-3" />
                  清空队列
                </Button>
              </div>
            ) : null}
            {([["steering", "插话", steering], ["followUp", "排队", followUp]] as const).flatMap(([kind, label, messages]) =>
              messages.map((text, index) => {
                const rowKey = `${kind}-${index}`;
                return (
                  <div key={rowKey} className="group/queue flex items-center gap-1.5 rounded-lg px-2 py-1 text-xs hover:bg-muted/50">
                    <Clock3 className="size-3.5 shrink-0 text-muted-foreground" />
                    <span className="shrink-0 text-[11px] text-muted-foreground">{label}</span>
                    <span className="min-w-0 flex-1 truncate text-foreground" title={text}>
                      {text}
                    </span>
                    <div className="flex shrink-0 items-center gap-0.5">
                      {kind === "followUp" ? (
                        <Button
                          type="button"
                          size="icon-xs"
                          variant="ghost"
                          title="提升为插话（当前工具结束后立刻送达）"
                          aria-label="提升为插话"
                          disabled={Boolean(busyKey)}
                          className="size-6 text-muted-foreground hover:text-foreground"
                          onClick={() => void mutateQueue(`promote-${rowKey}`, "promote", "followUp", index)}
                        >
                          <ArrowUp className="size-3" />
                        </Button>
                      ) : null}
                      <Button
                        type="button"
                        size="icon-xs"
                        variant="ghost"
                        title="移出队列"
                        aria-label="移出队列"
                        disabled={Boolean(busyKey)}
                        className="size-6 text-muted-foreground hover:text-destructive"
                        onClick={() => void mutateQueue(`remove-${rowKey}`, "remove", kind, index)}
                      >
                        <X className="size-3" />
                      </Button>
                    </div>
                  </div>
                );
              }),
            )}
          </Trace>
        </TraceCollapsibleContent>
      </TraceCollapsible>
    </Trace>
  );
}
