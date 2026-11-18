import * as React from "react";
import { Bot, CircleAlert, Code2, FilePen, FilePlus, FileSearch, FolderTree, Globe, MessageCircleQuestion, Plug, Search, SquareTerminal, Wrench } from "lucide-react";
import type { TranscriptItem } from "@lila/protocol";
import { Code, DiffView, languageOf } from "@/components/code";
import { Markdown } from "@/components/markdown";
import { Badge } from "@/components/ui/badge";
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

type ToolItem = Extract<TranscriptItem, { kind: "tool" }>;

function str(value: unknown): string | undefined {
  return typeof value === "string" && value ? value : undefined;
}

function num(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function duration(ms: number | undefined): string | undefined {
  if (ms === undefined) return undefined;
  return ms < 1_000 ? `${ms}ms` : ms < 60_000 ? `${(ms / 1_000).toFixed(1)}s` : `${Math.floor(ms / 60_000)}m${Math.round((ms % 60_000) / 1_000)}s`;
}

function json(value: unknown): string {
  return JSON.stringify(value, null, 2);
}

/** `mcp__server__tool` → { server, tool }. */
export function parseMcpName(name: string): { server: string; tool: string } | undefined {
  if (!name.startsWith("mcp__")) return undefined;
  const [server, ...rest] = name.slice(5).split("__");
  if (!server || !rest.length) return undefined;
  return { server, tool: rest.join("__") };
}

type View = { icon: React.ComponentType; title: string; target?: string | undefined; body: React.ReactNode; openByDefault?: boolean };

function Output({ tool, title = "输出", language }: { tool: ToolItem; title?: string; language?: string }) {
  if (!tool.output) return null;
  return (
    <Code
      code={tool.output}
      title={tool.status === "running" ? `${title}（运行中）` : title}
      streaming={tool.status === "running"}
      {...(language ? { language } : {})}
    />
  );
}

function viewOf(tool: ToolItem): View {
  const { args } = tool;
  const path = str(args.path) ?? str(args.file_path);
  switch (tool.name) {
    case "bash": {
      const command = str(args.command) ?? "";
      return {
        icon: SquareTerminal,
        title: "运行命令",
        target: command.split("\n")[0],
        body: (
          <>
            <Code code={command} language="bash" maxHeight="10rem" />
            <Output tool={tool} />
          </>
        ),
      };
    }
    case "read":
      return {
        icon: FileSearch,
        title: "读取",
        target: path,
        body: <Output tool={tool} title={path ?? "内容"} {...(path && languageOf(path) ? { language: languageOf(path) } : {})} />,
      };
    case "grep":
    case "find":
      return {
        icon: Search,
        title: tool.name === "grep" ? "搜索内容" : "查找文件",
        target: [str(args.pattern), path].filter(Boolean).join(" · "),
        body: <Output tool={tool} title="结果" />,
      };
    case "ls":
      return { icon: FolderTree, title: "列出目录", target: path ?? ".", body: <Output tool={tool} title="结果" /> };
    case "edit":
    case "write": {
      const content = tool.name === "write" ? str(args.content) : undefined;
      return {
        icon: tool.name === "edit" ? FilePen : FilePlus,
        title: tool.name === "edit" ? "修改文件" : "写入文件",
        target: path,
        openByDefault: Boolean(tool.diff),
        body: (
          <>
            {tool.diff ? <DiffView patch={tool.diff} path={path ?? "文件"} /> : null}
            {!tool.diff && content !== undefined ? (
              <Code code={content} title={path} streaming={tool.status === "running"} {...(path && languageOf(path) ? { language: languageOf(path) } : {})} />
            ) : null}
            {!tool.diff && content === undefined && tool.name === "edit" ? <Code code={json(args)} language="json" title="参数" maxHeight="12rem" /> : null}
            {tool.status === "error" ? <Output tool={tool} title="错误" /> : null}
          </>
        ),
      };
    }
    case "Agent":
      return agentView(tool);
    case "codemode": {
      const code = str(args.code) ?? json(args);
      return {
        icon: Code2,
        title: "运行脚本",
        target: str(args.intent) ?? str(args.description),
        body: (
          <>
            <Code code={code} language="javascript" title="脚本" maxHeight="14rem" />
            <Output tool={tool} />
          </>
        ),
      };
    }
    case "ask_user": {
      const questions = Array.isArray(args.questions) ? (args.questions as Array<{ question?: unknown }>) : [];
      return {
        icon: MessageCircleQuestion,
        title: "向你提问",
        target: questions.map((item) => str(item.question)).filter(Boolean).join(" · "),
        body: <Output tool={tool} title="回答" />,
      };
    }
  }
  const mcp = parseMcpName(tool.name);
  if (mcp) {
    return {
      icon: Plug,
      title: `MCP · ${mcp.server} · ${mcp.tool}`,
      body: (
        <>
          {Object.keys(args).length ? <Code code={json(args)} language="json" title="参数" maxHeight="10rem" /> : null}
          <Output tool={tool} />
        </>
      ),
    };
  }
  const target = str(args.query) ?? str(args.url) ?? path ?? str(args.pattern) ?? str(args.command);
  return {
    icon: tool.name.startsWith("web") ? Globe : Wrench,
    title: tool.name,
    target,
    body: (
      <>
        {Object.keys(args).length ? <Code code={json(args)} language="json" title="参数" maxHeight="10rem" /> : null}
        <Output tool={tool} />
      </>
    ),
  };
}

const AGENT_STATUS: Record<string, string> = {
  running: "运行中",
  queued: "排队中",
  background: "后台运行",
  completed: "已完成",
  steered: "已完成（被引导）",
  stopped: "已停止",
  aborted: "已中止",
  error: "出错",
};

function agentView(tool: ToolItem): View {
  const { args, details } = tool;
  const type = str(args.subagent_type) ?? "Agent";
  const description = str(args.description) ?? str(details?.description);
  const status = str(details?.status);
  const stats = [
    str(details?.modelName),
    num(details?.toolUses) ? `${num(details?.toolUses)} 次工具调用` : undefined,
    str(details?.tokens),
    num(details?.cost) ? `$${num(details?.cost)?.toFixed(4)}` : undefined,
    duration(num(details?.durationMs)),
  ].filter(Boolean);
  const error = str(details?.error);
  const prompt = str(args.prompt);
  return {
    icon: Bot,
    title: `子 Agent · ${type}`,
    target: description,
    body: (
      <div className="flex flex-col gap-2">
        <div className="flex flex-wrap items-center gap-1.5 text-xs">
          {status ? <Badge variant={status === "error" || status === "aborted" ? "destructive" : "secondary"}>{AGENT_STATUS[status] ?? status}</Badge> : null}
          {stats.map((stat) => (
            <span key={stat} className="text-muted-foreground">
              {stat}
            </span>
          ))}
        </div>
        {prompt ? <Code code={prompt} title="任务" maxHeight="8rem" /> : null}
        {error ? <p className="text-sm text-destructive">{error}</p> : null}
        {tool.output ? (
          <div className="rounded-lg border bg-card px-3 py-2 text-foreground">
            <Markdown text={tool.output} streaming={tool.status === "running"} />
          </div>
        ) : null}
      </div>
    ),
  };
}

export const ToolCall = React.memo(function ToolCall({ tool }: { tool: ToolItem }) {
  const view = viewOf(tool);
  const Icon = view.icon;
  // Diffs are the point of an edit, so they start open; errors open too. Other details stay folded.
  const wantsOpen = Boolean(view.openByDefault) || tool.status === "error";
  const [open, setOpen] = React.useState(wantsOpen);
  const [touched, setTouched] = React.useState(false);
  React.useEffect(() => {
    if (!touched && wantsOpen) setOpen(true);
  }, [wantsOpen, touched]);
  const status = tool.status === "running" ? "running" : tool.status === "error" ? "error" : undefined;
  const took = duration(tool.durationMs);
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
            <TraceTitle>{view.title}</TraceTitle>
            {view.target ? <TraceDescription className="max-w-[28rem] shrink truncate font-mono text-xs">{view.target}</TraceDescription> : null}
            {took && tool.status !== "running" ? <TraceDescription className="text-xs">{took}</TraceDescription> : null}
          </TraceLine>
        </TraceCollapsibleTrigger>
        <TraceCollapsibleContent>
          <div className="flex flex-col gap-2 text-foreground">{view.body}</div>
        </TraceCollapsibleContent>
      </TraceCollapsible>
    </Trace>
  );
});
