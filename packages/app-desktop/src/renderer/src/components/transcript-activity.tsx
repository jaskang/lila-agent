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
  let searches = 0;
  let mcpCalls = 0;
  let other = 0;
  let active = false;
  for (const item of items) {
    if (item.kind !== "tool") continue;
    active ||= item.status === "running";
    const filePath =
      typeof item.args.path === "string"
        ? item.args.path
        : typeof item.args.file_path === "string"
          ? item.args.file_path
          : undefined;
    if (item.name === "read" && filePath) {
      readPaths.add(filePath);
    } else if ((item.name === "edit" || item.name === "write") && item.status === "done" && filePath) {
      changedPaths.add(filePath);
    } else if (item.name === "bash") {
      commands++;
    } else if (item.name === "grep" || item.name === "find" || item.name === "ls") {
      searches++;
    } else if (item.name.startsWith("mcp__") || item.name === "codemode") {
      mcpCalls++;
    } else {
      other++;
    }
  }
  const details = [
    readPaths.size ? `读取 ${readPaths.size} 个文件` : undefined,
    searches ? `检索 ${searches} 次` : undefined,
    commands ? `运行 ${commands} 条命令` : undefined,
    changedPaths.size ? `修改 ${changedPaths.size} 个文件` : undefined,
    mcpCalls ? `调用 ${mcpCalls} 次工具/MCP` : undefined,
    other ? `${other} 次其他操作` : undefined,
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
  const reveal = items.some((item) => item.id === revealId);
  const [open, setOpen] = React.useState(reveal);
  const [viewed, setViewed] = React.useState(reveal);
  React.useEffect(() => {
    if (reveal) {
      setViewed(true);
      setOpen(true);
    }
  }, [reveal]);
  const { title, active } = summarize(items);

  const rows: React.ReactNode[] = [];
  for (let index = 0; (viewed || open) && index < items.length; index++) {
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
      <TraceCollapsible open={open} onOpenChange={(next) => {
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
