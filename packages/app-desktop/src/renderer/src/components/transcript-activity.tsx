import * as React from "react";
import { ListTree } from "lucide-react";
import type { TranscriptItem } from "@lila/protocol";
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

function summarize(items: TranscriptItem[]) {
  const readPaths = new Set<string>();
  const changedPaths = new Set<string>();
  let commands = 0;
  let other = 0;
  let active = false;
  for (const item of items) {
    if (item.kind !== "tool") continue;
    active ||= item.status === "running";
    if (item.name === "read" && typeof item.args.path === "string") {
      readPaths.add(item.args.path);
    } else if ((item.name === "edit" || item.name === "write") && item.status === "done" && typeof item.args.path === "string") {
      changedPaths.add(item.args.path);
    } else if (item.name === "bash") {
      commands++;
    } else {
      other++;
    }
  }
  const details = [
    readPaths.size ? `读取 ${readPaths.size} 个文件` : undefined,
    commands ? `运行 ${commands} 条命令` : undefined,
    changedPaths.size ? `修改 ${changedPaths.size} 个文件` : undefined,
    other ? `${other} 次工具调用` : undefined,
  ].filter(Boolean);
  return { title: details.join(" · ") || "思考过程", active };
}

/** shadcn Chain of Thought composition. Details reuse the actual Tool views. */
export const TranscriptActivity = React.memo(function TranscriptActivity({
  items,
  sessionId,
  revealId,
}: {
  items: TranscriptItem[];
  sessionId: string;
  revealId: string | undefined;
}) {
  const [open, setOpen] = React.useState(false);
  const [viewed, setViewed] = React.useState(false);
  const reveal = items.some((item) => item.id === revealId);
  const { title, active } = summarize(items);

  const rows: React.ReactNode[] = [];
  for (let index = 0; (viewed || open || reveal) && index < items.length; index++) {
    const item = items[index]!;
    const matched = item.id === revealId;
    rows.push(
      <div key={item.id} data-transcript-item={item.id} className={matched ? "rounded-md bg-accent/60 p-1 ring-1 ring-ring/25" : undefined}>
        <TranscriptEntry item={item} sessionId={sessionId} running={false} reveal={matched} />
      </div>,
    );
  }

  return (
    <Trace>
      <TraceCollapsible open={open || reveal} onOpenChange={(next) => {
        if (next) setViewed(true);
        setOpen(next);
      }}>
        <TraceCollapsibleTrigger className="py-0.5">
          <TraceLine className="text-[13px]" {...(active ? { status: "running" as const } : {})}>
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
