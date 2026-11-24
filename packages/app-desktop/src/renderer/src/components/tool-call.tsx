import * as React from "react";
import {
  CircleAlert,
  Code2,
  ExternalLink,
  Eye,
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
import { parseStackTrace, StackTraceView } from "@/components/stack-trace";
import { Button } from "@/components/ui/button";
import { CodeBlock, CodeBlockActions, CodeBlockContent, CodeBlockCopyButton, CodeBlockHeader, CodeBlockTitle } from "@/components/ui/code-block";
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
import { actions, useLila } from "@/lib/store";

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

function Output({ tool, sessionId, title = "输出", language }: { tool: ToolItem; sessionId?: string | undefined; title?: string; language?: string }) {
  if (!tool.output) return null;
  if (tool.status === "error" && sessionId) {
    const parsed = parseStackTrace(tool.output);
    if (parsed) return <StackTraceView parsed={parsed} rawOutput={tool.output} sessionId={sessionId} />;
  }
  return (
    <Code
      code={tool.output}
      title={tool.status === "running" ? `${title}（运行中）` : title}
      streaming={tool.status === "running"}
      {...(language ? { language } : {})}
    />
  );
}

/** shadcn Terminal pattern: auto-expands while streaming output, collapses on completion, single compact surface. */
function TerminalCall({ tool, sessionId, reveal }: { tool: ToolItem; sessionId: string; reveal: boolean }) {
  const command = str(tool.args.command) ?? "";
  const firstLine = command.split("\n")[0] ?? "";
  const multiLine = command.includes("\n");
  const running = tool.status === "running";
  const failed = tool.status === "error";
  const [open, setOpen] = React.useState<boolean | undefined>(undefined);

  const autoOpen = failed || (running && Boolean(tool.output));
  const parsedStack = React.useMemo(
    () => (failed && tool.output ? parseStackTrace(tool.output) : undefined),
    [failed, tool.output],
  );
  const status = running ? "running" : failed ? "error" : undefined;
  const took = formatDuration(tool.durationMs);
  const label = running ? "正在运行" : failed ? "运行失败" : "已运行";
  const combinedBody = multiLine
    ? `$ ${command}${tool.output ? `\n\n${tool.output}` : ""}`
    : tool.output || `$ ${command}`;

  return (
    <Trace>
      <TraceCollapsible open={reveal || (open ?? autoOpen)} onOpenChange={setOpen}>
        <TraceCollapsibleTrigger>
          <TraceLine className="text-[13px]" {...(status ? { status } : {})}>
            <TraceIcon>{running ? <Spinner /> : failed ? <CircleAlert /> : <SquareTerminal />}</TraceIcon>
            <TraceTitle>{label}</TraceTitle>
            {firstLine ? <TraceDescription className="max-w-[28rem] shrink truncate font-mono text-xs">{firstLine}</TraceDescription> : null}
            {took && !running ? <TraceDescription className="text-xs">{took}</TraceDescription> : null}
          </TraceLine>
        </TraceCollapsibleTrigger>
        <TraceCollapsibleContent>
          <div className="flex flex-col gap-2 text-foreground">
            {parsedStack ? (
              <>
                {command ? <Code code={`$ ${command}`} language="bash" maxHeight="8rem" /> : null}
                <StackTraceView parsed={parsedStack} rawOutput={tool.output} sessionId={sessionId} />
              </>
            ) : (
              <CodeBlock size="sm" className="not-typeset">
                <CodeBlockHeader>
                  <CodeBlockTitle className="font-mono text-xs text-muted-foreground">
                    $ {firstLine || "bash"}
                  </CodeBlockTitle>
                  <CodeBlockActions>
                    <CodeBlockCopyButton value={command || tool.output} title="复制命令" />
                  </CodeBlockActions>
                </CodeBlockHeader>
                <CodeBlockContent wrap className="max-h-64 whitespace-pre-wrap">
                  {combinedBody}
                </CodeBlockContent>
              </CodeBlock>
            )}
          </div>
        </TraceCollapsibleContent>
      </TraceCollapsible>
    </Trace>
  );
}

function resolveFilePath(projectPath: string | undefined, target: string | undefined): string | undefined {
  if (!target) return undefined;
  if (target.startsWith("/")) return target;
  if (!projectPath) return undefined;
  return `${projectPath.replace(/\/+$/, "")}/${target.replace(/^\.\//, "")}`;
}

function relativeDisplayPath(projectPath: string | undefined, target: string | undefined): string | undefined {
  if (!target) return undefined;
  if (projectPath) {
    const prefix = `${projectPath.replace(/\/+$/, "")}/`;
    if (target.startsWith(prefix)) return target.slice(prefix.length);
  }
  return target.replace(/^\.\//, "");
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
        className="h-6 text-muted-foreground hover:text-foreground"
        onClick={() => actions.openFilePreview(fullPath)}
      >
        <Eye />
        右侧预览
      </Button>
      <Button
        type="button"
        size="xs"
        variant="ghost"
        className="h-6 text-muted-foreground hover:text-foreground"
        onClick={() => void window.lila.openPath(fullPath).catch((error: unknown) => toast.error(error instanceof Error ? error.message : String(error)))}
      >
        <ExternalLink />
        外部打开
      </Button>
      <Button
        type="button"
        size="xs"
        variant="ghost"
        className="h-6 text-muted-foreground hover:text-foreground"
        onClick={() => window.lila.revealPath(fullPath)}
      >
        <FolderOpen />
        访达
      </Button>
    </div>
  );
}

function viewOf(tool: ToolItem, sessionId: string, projectPath: string | undefined): View {
  const { args } = tool;
  const rawPath = str(args.path) ?? str(args.file_path);
  const shortPath = relativeDisplayPath(projectPath, rawPath);
  switch (tool.name) {
    case "read":
      return {
        icon: FileSearch,
        title: "读取",
        target: shortPath,
        body: (
          <>
            <FileActions sessionId={sessionId} path={rawPath} />
            <Output tool={tool} title={shortPath ?? "内容"} {...(rawPath && languageOf(rawPath) ? { language: languageOf(rawPath) } : {})} />
          </>
        ),
      };
    case "grep":
    case "find":
      return {
        icon: Search,
        title: tool.name === "grep" ? "搜索内容" : "查找文件",
        target: [str(args.pattern), shortPath].filter(Boolean).join(" · "),
        body: <Output tool={tool} title="结果" />,
      };
    case "ls":
      return { icon: FolderTree, title: "列出目录", target: shortPath ?? ".", body: <Output tool={tool} title="结果" /> };
    case "edit":
    case "write": {
      const content = tool.name === "write" ? str(args.content) : undefined;
      return {
        icon: tool.name === "edit" ? FilePen : FilePlus,
        title: tool.name === "edit" ? "修改文件" : "写入文件",
        target: shortPath,
        body: (
          <>
            <FileActions sessionId={sessionId} path={rawPath} />
            {tool.diff ? <DiffView patch={tool.diff} path={shortPath ?? "文件"} /> : null}
            {!tool.diff && content !== undefined ? (
              <Code code={content} title={shortPath} streaming={tool.status === "running"} {...(rawPath && languageOf(rawPath) ? { language: languageOf(rawPath) } : {})} />
            ) : null}
            {!tool.diff && content === undefined && tool.name === "edit" ? <Code code={json(args)} language="json" title="参数" maxHeight="12rem" /> : null}
            {tool.status === "error" ? <Output tool={tool} sessionId={sessionId} title="错误" /> : null}
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
            <Output tool={tool} sessionId={sessionId} />
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
          <Output tool={tool} sessionId={sessionId} />
        </>
      ),
    };
  }
  const target = str(args.query) ?? str(args.url) ?? shortPath ?? str(args.pattern) ?? str(args.command);
  return {
    icon: tool.name.startsWith("web") ? Globe : Wrench,
    title: tool.name,
    target,
    body: (
      <>
        {Object.keys(args).length ? <Code code={json(args)} language="json" title="参数" maxHeight="10rem" /> : null}
        <Output tool={tool} sessionId={sessionId} />
      </>
    ),
  };
}

function StandardToolCall({ tool, sessionId, reveal }: { tool: ToolItem; sessionId: string; reveal: boolean }) {
  const projectPath = useLila((state) => {
    const projectId = state.sessions[sessionId]?.projectId;
    return state.projects.find((project) => project.id === projectId)?.path;
  });
  const view = viewOf(tool, sessionId, projectPath);
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
          <TraceLine className="text-[13px]" {...(status ? { status } : {})}>
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
}

export const ToolCall = React.memo(function ToolCall({ tool, sessionId, reveal = false }: { tool: ToolItem; sessionId: string; reveal?: boolean }) {
  if (tool.subagent) return <SingleAgentCard tool={tool} sessionId={sessionId} reveal={reveal} />;
  if (tool.name === "bash") return <TerminalCall tool={tool} sessionId={sessionId} reveal={reveal} />;
  return <StandardToolCall tool={tool} sessionId={sessionId} reveal={reveal} />;
});
