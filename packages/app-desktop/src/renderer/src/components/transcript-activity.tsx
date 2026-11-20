import * as React from "react";
import { ListTree } from "lucide-react";
import type { SubagentProgress, TranscriptItem } from "@lila/protocol";
import { AgentGroup } from "@/components/agent-card";
import { Spinner } from "@/components/ui/spinner";
import {
  Trace,
  TraceCollapsible,
  TraceCollapsibleContent,
  TraceCollapsibleTrigger,
  TraceIcon,
  TraceLine,
  TraceTitle,
} from "@/components/ui/trace";
import { TranscriptEntry } from "@/components/transcript-item";
import { useLila } from "@/lib/store";

function summarize(items: TranscriptItem[], live: SubagentProgress[] | undefined) {
  const readPaths = new Set<string>();
  const changedPaths = new Set<string>();
  const agentIds = new Set<string>();
  let commands = 0;
  let agents = 0;
  let completed = 0;
  let other = 0;
  let active = false;
  for (const item of items) {
    if (item.kind === "tool") {
      active ||= item.status === "running";
      if (item.subagent) {
        if (item.subagent.id) agentIds.add(item.subagent.id);
        const status = live?.find((agent) => agent.id === item.subagent?.id)?.status ?? item.subagent.status;
        agents++;
        active ||= status === "running" || status === "queued";
        if (status === "completed" || status === "steered") completed++;
      } else if (item.name === "read" && typeof item.args.path === "string") {
        readPaths.add(item.args.path);
      } else if ((item.name === "edit" || item.name === "write") && item.status === "done" && typeof item.args.path === "string") {
        changedPaths.add(item.args.path);
      } else if (item.name === "bash") {
        commands++;
      } else {
        other++;
      }
    } else if (item.kind === "custom") {
      for (const notification of item.notifications ?? []) {
        if (notification.id && agentIds.has(notification.id)) continue;
        if (notification.id) agentIds.add(notification.id);
        agents++;
        completed++;
      }
    }
  }
  const details = [
    readPaths.size ? `读取 ${readPaths.size} 个文件` : undefined,
    commands ? `运行 ${commands} 条命令` : undefined,
    changedPaths.size ? `修改 ${changedPaths.size} 个文件` : undefined,
    agents ? (completed === agents ? `${agents} 个子 Agent 已完成` : `${agents} 个子 Agent`) : undefined,
    other ? `${other} 次工具调用` : undefined,
  ].filter(Boolean);
  return { title: details.join(" · ") || "思考过程", active };
}

/** shadcn Chain of Thought composition. Details reuse the actual Tool and Subagent views. */
export const TranscriptActivity = React.memo(function TranscriptActivity({
  items,
  sessionId,
  revealId,
}: {
  items: TranscriptItem[];
  sessionId: string;
  revealId: string | undefined;
}) {
  const live = useLila((state) => state.views[sessionId]?.state?.subagents);
  const [open, setOpen] = React.useState(false);
  const [viewed, setViewed] = React.useState(false);
  const reveal = items.some((item) => item.id === revealId);
  const { title, active } = summarize(items, live);

  const rows: React.ReactNode[] = [];
  for (let index = 0; (viewed || open || reveal) && index < items.length;) {
    const item = items[index]!;
    if (item.kind === "tool" && item.subagent && !reveal) {
      const agents = [item];
      while (items[index + 1]?.kind === "tool") {
        const next = items[index + 1];
        if (next?.kind !== "tool" || !next.subagent) break;
        agents.push(next);
        index++;
      }
      if (agents.length > 1) {
        rows.push(<AgentGroup key={item.id} tools={agents} sessionId={sessionId} />);
        index++;
        continue;
      }
    }
    const matched = item.id === revealId;
    rows.push(
      <div key={item.id} data-transcript-item={item.id} className={matched ? "rounded-md bg-accent/60 p-1 ring-1 ring-ring/25" : undefined}>
        <TranscriptEntry item={item} sessionId={sessionId} running={false} reveal={matched} />
      </div>,
    );
    index++;
  }

  return (
    <Trace>
      <TraceCollapsible open={open || reveal} onOpenChange={(next) => {
        if (next) setViewed(true);
        setOpen(next);
      }}>
        <TraceCollapsibleTrigger className="py-0.5">
          <TraceLine {...(active ? { status: "running" as const } : {})}>
            <TraceIcon>{active ? <Spinner /> : <ListTree />}</TraceIcon>
            <TraceTitle>{title}</TraceTitle>
          </TraceLine>
        </TraceCollapsibleTrigger>
        <TraceCollapsibleContent keepMounted>
          <Trace className="gap-1.5">{rows}</Trace>
        </TraceCollapsibleContent>
      </TraceCollapsible>
    </Trace>
  );
}, (previous, next) => (
  previous.sessionId === next.sessionId &&
  previous.revealId === next.revealId &&
  previous.items.length === next.items.length &&
  previous.items.every((item, index) => item === next.items[index])
));
