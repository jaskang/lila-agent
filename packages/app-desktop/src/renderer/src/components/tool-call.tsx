import * as React from "react";
import {
  Brain,
  Check,
  CircleAlert,
  Code2,
  Copy,
  ExternalLink,
  Eye,
  FilePen,
  FilePlus,
  FileSearch,
  FolderOpen,
  FolderTree,
  Globe,
  Maximize2,
  MessageCircleQuestion,
  Minimize2,
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

/** shadcn Terminal pattern: auto-expands on output/failure, stays open after completion, supports copying output & auto-tailing. */
function TerminalCall({ tool, sessionId, reveal }: { tool: ToolItem; sessionId: string; reveal: boolean }) {
  const command = str(tool.args.command) ?? "";
  const firstLine = command.split("\n")[0] ?? "";
  const multiLine = command.includes("\n");
  const running = tool.status === "running";
  const failed = tool.status === "error";
  const [open, setOpen] = React.useState<boolean>(() => failed || (running && Boolean(tool.output)) || reveal);
  const [touched, setTouched] = React.useState(false);
  const [tall, setTall] = React.useState(false);
  const [copiedCmd, setCopiedCmd] = React.useState(false);
  const contentRef = React.useRef<HTMLPreElement>(null);
  const stickBottomRef = React.useRef(true);

  React.useEffect(() => {
    if (reveal) setOpen(true);
  }, [reveal]);

  React.useEffect(() => {
    if (!touched && (failed || (running && Boolean(tool.output)))) {
      setOpen(true);
    }
  }, [failed, running, tool.output, touched]);

  // Auto-tail streaming terminal output unless the user scrolled up.
  React.useEffect(() => {
    if (!running || !open || !stickBottomRef.current) return;
    const node = contentRef.current;
    if (node) node.scrollTop = node.scrollHeight;
  }, [running, open, tool.output]);

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
  const isLongOutput = combinedBody.length > 500 || combinedBody.split("\n").length > 12;

  const copyCommand = () => {
    if (!command) return;
    void navigator.clipboard
      .writeText(command)
      .then(() => {
        setCopiedCmd(true);
        window.setTimeout(() => setCopiedCmd(false), 1_500);
      })
      .catch(() => toast.error("复制失败"));
  };

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
                  <CodeBlockActions className="gap-1">
                    {command && tool.output ? (
                      <Button
                        type="button"
                        size="xs"
                        variant="ghost"
                        className="h-6 px-1.5 text-[11px] font-normal text-muted-foreground hover:text-foreground"
                        title="复制执行的命令"
                        onClick={copyCommand}
                      >
                        {copiedCmd ? <Check className="size-3" /> : <Copy className="size-3" />}
                        命令
                      </Button>
                    ) : null}
                    <CodeBlockCopyButton
                      value={tool.output || command}
                      title={tool.output ? "复制输出" : "复制命令"}
                    />
                    {isLongOutput ? (
                      <Button
                        type="button"
                        size="icon-xs"
                        variant="ghost"
                        className="size-6 text-muted-foreground hover:text-foreground"
                        title={tall ? "收起高度" : "展开高度"}
                        aria-label={tall ? "收起高度" : "展开高度"}
                        onClick={() => setTall((v) => !v)}
                      >
                        {tall ? <Minimize2 className="size-3" /> : <Maximize2 className="size-3" />}
                      </Button>
                    ) : null}
                  </CodeBlockActions>
                </CodeBlockHeader>
                <CodeBlockContent
                  ref={contentRef}
                  wrap
                  onScroll={(event) => {
                    const node = event.currentTarget;
                    stickBottomRef.current = node.scrollHeight - node.scrollTop - node.clientHeight <= 24;
                  }}
                  className={tall ? "max-h-[36rem] whitespace-pre-wrap" : "max-h-64 whitespace-pre-wrap"}
                >
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
        onClick={() => void actions.revealPath(fullPath)}
      >
        <FolderOpen />
        定位
      </Button>
    </div>
  );
}

function viewOf(tool: ToolItem, sessionId: string, projectPath: string | undefined): View {
  const { args } = tool;
  const rawPath = str(args.path) ?? str(args.file_path);
  const shortPath = relativeDisplayPath(projectPath, rawPath);
  switch (tool.name) {
    case "read": {
      const offset = typeof args.offset === "number" && Number.isFinite(args.offset) ? args.offset : undefined;
      const limit = typeof args.limit === "number" && Number.isFinite(args.limit) ? args.limit : undefined;
      const rangeSuffix =
        offset !== undefined
          ? limit !== undefined
            ? `L${offset}-${offset + limit - 1}`
            : `L${offset}+`
          : limit !== undefined
            ? `前 ${limit} 行`
            : undefined;
      return {
        icon: FileSearch,
        title: "读取",
        target: [shortPath, rangeSuffix].filter(Boolean).join(" · ") || undefined,
        body: (
          <>
            <FileActions sessionId={sessionId} path={rawPath} />
            <Output tool={tool} title={shortPath ?? "内容"} {...(rawPath && languageOf(rawPath) ? { language: languageOf(rawPath) } : {})} />
          </>
        ),
      };
    }
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
    case "memory_read":
    case "memory_add":
    case "memory_replace":
    case "memory_remove": {
      const targetLabel = args.target === "user" ? "用户画像" : "Agent 笔记";
      const titleMap: Record<string, string> = {
        memory_read: `读取${targetLabel}`,
        memory_add: `写入${targetLabel}`,
        memory_replace: `更新${targetLabel}`,
        memory_remove: `删除${targetLabel}`,
      };
      const detailEntries = Array.isArray(tool.details?.entries)
        ? tool.details.entries.filter((item): item is string => typeof item === "string")
        : undefined;
      const usageText = str(tool.details?.usage);
      const summaryTarget =
        str(args.content) ??
        str(args.newContent) ??
        str(args.oldText) ??
        usageText;
      return {
        icon: Brain,
        title: titleMap[tool.name] ?? "记忆操作",
        target: summaryTarget,
        body: (
          <div className="flex flex-col gap-2 text-xs">
            {tool.name === "memory_add" && str(args.content) ? (
              <div className="rounded-lg border border-border/70 bg-card/50 p-2.5 whitespace-pre-wrap break-words text-foreground">
                {str(args.content)}
              </div>
            ) : null}
            {tool.name === "memory_replace" ? (
              <div className="flex flex-col gap-1.5 rounded-lg border border-border/70 bg-card/50 p-2.5">
                {str(args.oldText) ? (
                  <div className="text-muted-foreground line-through whitespace-pre-wrap break-words">{str(args.oldText)}</div>
                ) : null}
                {str(args.newContent) ? (
                  <div className="text-foreground whitespace-pre-wrap break-words">{str(args.newContent)}</div>
                ) : null}
              </div>
            ) : null}
            {tool.name === "memory_remove" && str(args.oldText) ? (
              <div className="rounded-lg border border-border/70 bg-card/50 p-2.5 text-muted-foreground line-through whitespace-pre-wrap break-words">
                {str(args.oldText)}
              </div>
            ) : null}
            {tool.status === "error" ? (
              <Output tool={tool} sessionId={sessionId} title="错误" />
            ) : detailEntries ? (
              <div className="flex flex-col gap-1 rounded-lg border border-border/60 bg-muted/30 p-2.5">
                <div className="flex items-center justify-between text-[11px] text-muted-foreground">
                  <span>{args.target === "user" ? "USER.md" : "MEMORY.md"}（{detailEntries.length} 条）</span>
                  {usageText ? <span className="tabular-nums">{usageText}</span> : null}
                </div>
                {detailEntries.length ? (
                  <ul className="list-disc space-y-1 ps-4 text-foreground/90">
                    {detailEntries.map((entry, idx) => (
                      <li key={idx} className="whitespace-pre-wrap break-words">
                        {entry}
                      </li>
                    ))}
                  </ul>
                ) : (
                  <span className="text-muted-foreground">（空）</span>
                )}
              </div>
            ) : (
              <Output tool={tool} sessionId={sessionId} title="结果" />
            )}
          </div>
        ),
      };
    }
    case "ask_user": {
      const questions = Array.isArray(args.questions) ? (args.questions as Array<{ id?: unknown; question?: unknown }>) : [];
      const rawAnswers = Array.isArray(tool.details?.answers)
        ? (tool.details.answers as Array<{ id?: unknown; selected?: unknown; custom?: unknown }>)
        : undefined;
      return {
        icon: MessageCircleQuestion,
        title: "向你提问",
        target: questions.map((item) => str(item.question)).filter(Boolean).join(" · "),
        body: rawAnswers?.length ? (
          <div className="flex flex-col gap-2 rounded-lg border border-border/70 bg-card/50 p-3 text-xs">
            {questions.map((q, idx) => {
              const qid = str(q.id);
              const ans = rawAnswers.find((a) => (qid ? str(a.id) === qid : false)) ?? rawAnswers[idx];
              const parts = [
                ...(Array.isArray(ans?.selected) ? ans.selected.filter((s): s is string => typeof s === "string") : []),
                ...(typeof ans?.custom === "string" && ans.custom.trim() ? [ans.custom.trim()] : []),
              ];
              return (
                <div key={qid ?? idx} className="flex flex-col gap-0.5">
                  <span className="font-medium text-foreground">{str(q.question) ?? `问题 ${idx + 1}`}</span>
                  <span className="text-muted-foreground">{parts.length ? parts.join("、") : "（未作答）"}</span>
                </div>
              );
            })}
          </div>
        ) : (
          <Output tool={tool} title="回答" />
        ),
      };
    }
  }
  const mcp = parseMcpName(tool.name);
  if (mcp) {
    const mcpTarget =
      str(args.query) ??
      str(args.url) ??
      str(args.path) ??
      str(args.name) ??
      str(args.id) ??
      Object.values(args).find((v): v is string => typeof v === "string" && v.trim().length > 0 && v.length <= 120);
    return {
      icon: Plug,
      title: `MCP · ${mcp.server} · ${mcp.tool}`,
      target: mcpTarget,
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
  const [open, setOpen] = React.useState(wantsOpen || reveal);
  const [touched, setTouched] = React.useState(false);
  React.useEffect(() => {
    if (reveal) setOpen(true);
  }, [reveal]);
  React.useEffect(() => {
    if (!touched && wantsOpen) setOpen(true);
  }, [wantsOpen, touched]);
  const status = tool.status === "running" ? "running" : tool.status === "error" ? "error" : undefined;
  const took = formatDuration(tool.durationMs);
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
