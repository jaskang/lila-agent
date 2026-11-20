import * as React from "react";
import {
  CircleAlert,
  Code2,
  ExternalLink,
  FilePen,
  FilePlus,
  FileSearch,
  FolderOpen,
  FolderTree,
  Globe,
  MessageCircleQuestion,
  Plug,
  Search,
  SquareTerminal,
  Wrench,
} from "lucide-react";
import { toast } from "sonner";
import type { TranscriptItem } from "@lila/protocol";
import { SingleAgentCard } from "@/components/agent-card";
import { Code, DiffView, languageOf } from "@/components/code";
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
import { formatDuration } from "@/lib/format";
import { useLila } from "@/lib/store";

type ToolItem = Extract<TranscriptItem, { kind: "tool" }>;

function str(value: unknown): string | undefined {
  return typeof value === "string" && value ? value : undefined;
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

type View = { icon: React.ComponentType; title: string; target?: string | undefined; body: React.ReactNode };

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

function resolveFilePath(projectPath: string | undefined, target: string | undefined): string | undefined {
  if (!target) return undefined;
  if (target.startsWith("/")) return target;
  if (!projectPath) return undefined;
  return `${projectPath.replace(/\/+$/, "")}/${target.replace(/^\.\//, "")}`;
}

export function FileActions({ sessionId, path }: { sessionId: string; path: string | undefined }) {
  const projectPath = useLila((state) => {
    const projectId = state.sessions[sessionId]?.projectId;
    return state.projects.find((project) => project.id === projectId)?.path;
  });
  const fullPath = resolveFilePath(projectPath, path);
  if (!fullPath) return null;
  return (
    <div className="flex items-center gap-1">
      <Button
        type="button"
        size="xs"
        variant="ghost"
        className="h-6 text-muted-foreground"
        onClick={() => void window.lila.openPath(fullPath).catch((error: unknown) => toast.error(error instanceof Error ? error.message : String(error)))}
      >
        <ExternalLink />
        打开文件
      </Button>
      <Button type="button" size="xs" variant="ghost" className="h-6 text-muted-foreground" onClick={() => window.lila.revealPath(fullPath)}>
        <FolderOpen />
        在访达中显示
      </Button>
    </div>
  );
}

function viewOf(tool: ToolItem, sessionId: string): View {
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
        body: (
          <>
            <FileActions sessionId={sessionId} path={path} />
            <Output tool={tool} title={path ?? "内容"} {...(path && languageOf(path) ? { language: languageOf(path) } : {})} />
          </>
        ),
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
        body: (
          <>
            <FileActions sessionId={sessionId} path={path} />
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

export const ToolCall = React.memo(function ToolCall({ tool, sessionId, reveal = false }: { tool: ToolItem; sessionId: string; reveal?: boolean }) {
  if (tool.subagent) return <SingleAgentCard tool={tool} sessionId={sessionId} reveal={reveal} />;
  const view = viewOf(tool, sessionId);
  const Icon = view.icon;
  const wantsOpen = tool.status === "error";
  const [open, setOpen] = React.useState(wantsOpen);
  const [touched, setTouched] = React.useState(false);
  React.useEffect(() => {
    if (!touched && wantsOpen) setOpen(true);
  }, [wantsOpen, touched]);
  const status = tool.status === "running" ? "running" : tool.status === "error" ? "error" : undefined;
  const took = formatDuration(tool.durationMs);
  return (
    <Trace>
      <TraceCollapsible
        open={open || reveal}
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
