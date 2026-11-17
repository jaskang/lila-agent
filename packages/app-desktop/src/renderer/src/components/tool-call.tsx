import * as React from "react";
import { CircleAlert, FilePen, FileSearch, Globe, ListTodo, SquareTerminal, Wrench } from "lucide-react";
import { Code, DiffView, languageOf } from "@/components/code";
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
import type { TranscriptItem } from "@/lib/store";

type ToolItem = Extract<TranscriptItem, { kind: "tool" }>;

const ICONS: Record<string, React.ComponentType> = {
  bash: SquareTerminal,
  eval: SquareTerminal,
  read: FileSearch,
  grep: FileSearch,
  glob: FileSearch,
  edit: FilePen,
  write: FilePen,
  ast_edit: FilePen,
  web_search: Globe,
  todo: ListTodo,
};

function str(value: unknown): string | undefined {
  return typeof value === "string" && value ? value : undefined;
}

/** A short, human-readable target for the tool call: the command, path or query. */
function subject(tool: ToolItem): string | undefined {
  const args = tool.args;
  return str(args.command) ?? str(args.path) ?? str(args.pattern) ?? str(args.query) ?? str(args.url) ?? tool.diffs[0]?.path;
}

function Details({ tool }: { tool: ToolItem }) {
  const command = tool.toolName === "bash" ? str(tool.args.command) : undefined;
  const writeContent = tool.toolName === "write" && !tool.diffs.length ? str(tool.args.content) : undefined;
  const path = str(tool.args.path);
  const otherArgs = command || writeContent ? undefined : Object.keys(tool.args).length ? JSON.stringify(tool.args, null, 2) : undefined;
  return (
    <div className="flex flex-col gap-2 text-foreground">
      {command ? <Code code={command} language="bash" maxHeight="10rem" /> : null}
      {writeContent ? <Code code={writeContent} title={path} {...(path && languageOf(path) ? { language: languageOf(path) } : {})} /> : null}
      {tool.diffs.map((file) => (
        <DiffView key={file.path} file={file} />
      ))}
      {otherArgs && !tool.diffs.length ? <Code code={otherArgs} language="json" title="参数" maxHeight="10rem" /> : null}
      {tool.output ? <Code code={tool.output} title={tool.status === "running" ? "输出（运行中）" : "输出"} streaming={tool.status === "running"} /> : null}
    </div>
  );
}

export const ToolCall = React.memo(function ToolCall({ tool }: { tool: ToolItem }) {
  const Icon = ICONS[tool.toolName] ?? Wrench;
  const hasDiff = tool.diffs.length > 0;
  // Diffs are the point of an edit, so they start open; other details stay folded.
  const [open, setOpen] = React.useState(hasDiff || tool.status === "error");
  const [touched, setTouched] = React.useState(false);
  React.useEffect(() => {
    if (!touched && (hasDiff || tool.status === "error")) setOpen(true);
  }, [hasDiff, tool.status, touched]);
  const status = tool.status === "running" ? "running" : tool.status === "error" ? "error" : undefined;
  const target = subject(tool);
  return (
    <Trace>
      <TraceCollapsible
        open={open}
        onOpenChange={(next) => {
          setTouched(true);
          setOpen(next);
        }}
      >
        <TraceCollapsibleTrigger>
          <TraceLine {...(status ? { status } : {})}>
            <TraceIcon>{tool.status === "running" ? <Spinner /> : tool.status === "error" ? <CircleAlert /> : <Icon />}</TraceIcon>
            <TraceTitle>{tool.intent ?? tool.toolName}</TraceTitle>
            {target ? <TraceDescription className="max-w-[28rem] truncate font-mono text-xs">{target}</TraceDescription> : null}
          </TraceLine>
        </TraceCollapsibleTrigger>
        <TraceCollapsibleContent>
          <Details tool={tool} />
        </TraceCollapsibleContent>
      </TraceCollapsible>
    </Trace>
  );
});
