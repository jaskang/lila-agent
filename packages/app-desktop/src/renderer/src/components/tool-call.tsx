import * as React from "react";
import {
  ArrowUpRight,
  Bot,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
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
import type { SessionSummary, SubagentProgress, TranscriptItem } from "@lila/protocol";
import { Code, DiffView, languageOf } from "@/components/code";
import { Markdown } from "@/components/markdown";
import { Badge } from "@/components/ui/badge";
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
import { cn } from "cn";
import { actions, useLila } from "@/lib/store";

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

function resolveFilePath(projectPath: string | undefined, target: string | undefined): string | undefined {
  if (!target) return undefined;
  if (target.startsWith("/")) return target;
  if (!projectPath) return undefined;
  return `${projectPath.replace(/\/+$/, "")}/${target.replace(/^\.\//, "")}`;
}

function FileActions({ sessionId, path }: { sessionId: string; path: string | undefined }) {
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

const COMPACT_TOKENS = new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 1 });

export function formatCompactTokens(tokens: number): string {
  return `${COMPACT_TOKENS.format(tokens)} tokens`;
}

export function findMatchingSubSession(
  sessions: Record<string, SessionSummary>,
  parentSessionId: string,
  match: { agentId?: string | undefined; description?: string | undefined; prompt?: string | undefined },
): string | undefined {
  const children = Object.values(sessions).filter((session) => session.parentId === parentSessionId);
  if (!children.length) return undefined;
  const agentId = match.agentId?.toLowerCase();
  if (agentId) {
    const byId = children.find((child) => {
      const childAgentId = child.agentId?.toLowerCase();
      return childAgentId && (childAgentId === agentId || agentId.startsWith(childAgentId) || childAgentId.startsWith(agentId));
    });
    if (byId) return byId.id;
  }
  const promptLine = match.prompt?.trim().split("\n")[0]?.trim();
  const candidates = [match.description?.trim(), promptLine].filter((value): value is string => Boolean(value));
  for (const candidate of candidates) {
    const normalized = candidate.toLowerCase();
    const matched = children.find((child) => {
      const title = child.title.replace(/…$/, "").trim().toLowerCase();
      return title && (normalized === title || normalized.startsWith(title) || title.startsWith(normalized));
    });
    if (matched) return matched.id;
  }
  return children.length === 1 ? children[0]?.id : undefined;
}

function SubSessionLink({
  sessionId,
  agentId,
  description,
  prompt,
}: {
  sessionId: string;
  agentId?: string | undefined;
  description?: string | undefined;
  prompt?: string | undefined;
}) {
  const subSessionId = useLila((state) => findMatchingSubSession(state.sessions, sessionId, { agentId, description, prompt }));
  if (!subSessionId) return null;
  return (
    <Button type="button" size="xs" variant="ghost" className="ms-auto h-6 text-muted-foreground" onClick={() => actions.select(subSessionId)}>
      查看子会话
      <ArrowUpRight />
    </Button>
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
        openByDefault: Boolean(tool.diff),
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

const AGENT_STATUS: Record<string, string> = {
  running: "运行中",
  queued: "排队中",
  background: "后台运行中",
  completed: "已完成",
  steered: "已完成（被引导）",
  stopped: "已停止",
  aborted: "已中止",
  error: "出错",
};

function isSubagentNoise(text: string): boolean {
  const trimmed = text.trim();
  if (!trimmed) return true;
  if (/^\d+ tool uses\.\.\.$/.test(trimmed)) return true;
  if (/^Agent started in background\.\s+Agent ID:/i.test(trimmed)) return true;
  return false;
}

function formatElapsed(startedAt: number, now: number): string {
  const seconds = Math.max(0, Math.floor((now - startedAt) / 1_000));
  if (seconds < 60) return `${seconds}s`;
  return `${Math.floor(seconds / 60)}m${seconds % 60}s`;
}

type ResolvedAgent = {
  agentId: string | undefined;
  type: string;
  description: string | undefined;
  status: string;
  active: boolean;
  isError: boolean;
  background: boolean;
  activity: string | undefined;
  modelName: string | undefined;
  turnCount: number | undefined;
  maxTurns: number | undefined;
  toolUses: number | undefined;
  tokens: number;
  tokensText: string | undefined;
  cost: number;
  durationText: string | undefined;
  startedAt: number | undefined;
  prompt: string | undefined;
  output: string | undefined;
  error: string | undefined;
};

function resolveAgent(tool: ToolItem, live: SubagentProgress | undefined, now: number): ResolvedAgent {
  const { args, details } = tool;
  const agentId = str(details?.agentId);
  const type = live?.type || str(details?.subagentType) || str(args.subagent_type) || "Agent";
  const description = live?.description || str(args.description) || str(details?.description);
  const rawDetailsStatus = str(details?.status);
  const background = Boolean(live?.background || rawDetailsStatus === "background" || args.run_in_background);
  const status = live?.status ?? rawDetailsStatus ?? (tool.status === "running" ? "running" : tool.status === "error" ? "error" : "completed");
  const active = status === "running" || status === "queued" || status === "background" || tool.status === "running";
  const isError = status === "error" || status === "aborted" || status === "stopped";
  const activity = active ? (str(details?.activity) ?? live?.activity) : undefined;
  const modelName = str(details?.modelName) ?? live?.model;
  const turnCount = live?.turns || num(details?.turnCount);
  const maxTurns = num(details?.maxTurns);
  const toolUses = live?.toolUses || num(details?.toolUses);
  const tokens = live?.tokens ?? 0;
  const tokensText = str(details?.tokens) ?? (tokens > 0 ? formatCompactTokens(tokens) : undefined);
  const cost = live?.cost || num(details?.cost) || 0;
  const realDurationMs = live?.durationMs ?? (num(details?.durationMs) && num(details?.durationMs)! > 0 ? num(details?.durationMs) : undefined);
  const durationText = active
    ? live?.startedAt
      ? formatElapsed(live.startedAt, now)
      : undefined
    : duration(realDurationMs);
  const prompt = str(args.prompt);
  const rawOutput = live?.output ?? (!isSubagentNoise(tool.output) ? tool.output : undefined);
  const error = live?.error ?? str(details?.error);
  return {
    agentId,
    type,
    description,
    status,
    active,
    isError,
    background,
    activity,
    modelName,
    turnCount,
    maxTurns,
    toolUses,
    tokens,
    tokensText,
    cost,
    durationText,
    startedAt: live?.startedAt,
    prompt,
    output: rawOutput,
    error,
  };
}

function AgentDetailsBody({ info, sessionId, showSubLink = true }: { info: ResolvedAgent; sessionId: string; showSubLink?: boolean }) {
  const stats = [
    info.modelName,
    info.turnCount ? (info.maxTurns ? `第 ${info.turnCount}/${info.maxTurns} 轮` : `${info.turnCount} 轮`) : undefined,
    info.toolUses ? `${info.toolUses} 次工具调用` : undefined,
    info.tokensText,
    info.cost > 0 ? `$${info.cost.toFixed(4)}` : undefined,
    info.durationText,
  ].filter(Boolean);

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-1.5 text-xs">
        <Badge variant={info.isError ? "destructive" : "secondary"}>
          {info.background && info.active ? "后台运行中" : (AGENT_STATUS[info.status] ?? info.status)}
        </Badge>
        {stats.map((stat) => (
          <span key={stat} className="text-muted-foreground tabular-nums">
            {stat}
          </span>
        ))}
        {showSubLink ? <SubSessionLink sessionId={sessionId} agentId={info.agentId} description={info.description} prompt={info.prompt} /> : null}
      </div>
      {info.active && info.activity ? (
        <div className="flex items-center gap-2 rounded-lg border bg-muted/40 px-2.5 py-1.5 text-xs text-muted-foreground">
          <Spinner className="size-3.5 shrink-0" />
          <span className="truncate font-mono">{info.activity}</span>
        </div>
      ) : null}
      {info.prompt ? (
        <div className="rounded-lg bg-muted/40 px-3 py-2 text-xs leading-relaxed text-muted-foreground whitespace-pre-wrap break-words">
          {info.prompt}
        </div>
      ) : null}
      {info.error ? <p className="text-sm text-destructive">{info.error}</p> : null}
      {info.output ? (
        <div className="rounded-lg border bg-card px-3 py-2 text-foreground">
          <Markdown text={info.output} streaming={info.active} />
        </div>
      ) : null}
    </div>
  );
}

function SingleAgentCard({ tool, sessionId }: { tool: ToolItem; sessionId: string }) {
  const agentId = str(tool.details?.agentId);
  const live = useLila<SubagentProgress | undefined>((state) =>
    agentId ? state.views[sessionId]?.state?.subagents?.find((entry) => entry.id === agentId) : undefined,
  );
  const [now, setNow] = React.useState(() => Date.now());
  const info = resolveAgent(tool, live, now);
  React.useEffect(() => {
    if (!info.active || !info.startedAt) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [info.active, info.startedAt]);

  const [open, setOpen] = React.useState(info.isError);
  const [touched, setTouched] = React.useState(false);
  React.useEffect(() => {
    if (!touched && info.isError) setOpen(true);
  }, [info.isError, touched]);

  const status = info.active ? "running" : info.isError ? "error" : undefined;
  const meta = [
    info.activity,
    info.turnCount ? `${info.turnCount} 轮` : undefined,
    info.toolUses ? `${info.toolUses} 次工具` : undefined,
    info.durationText,
  ].filter(Boolean);

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
            <TraceIcon>
              {info.active ? (
                <Spinner />
              ) : info.isError ? (
                <CircleAlert />
              ) : (
                <CheckCircle2 className="text-emerald-600 dark:text-emerald-400" />
              )}
            </TraceIcon>
            <TraceTitle>子 Agent · {info.type}</TraceTitle>
            {info.description ? <TraceDescription className="max-w-[22rem] shrink truncate font-mono text-xs">{info.description}</TraceDescription> : null}
            {meta.length ? <TraceDescription className="truncate text-xs tabular-nums">{meta.join(" · ")}</TraceDescription> : null}
          </TraceLine>
        </TraceCollapsibleTrigger>
        <TraceCollapsibleContent>
          <div className="flex flex-col gap-2 text-foreground">
            <AgentDetailsBody info={info} sessionId={sessionId} />
          </div>
        </TraceCollapsibleContent>
      </TraceCollapsible>
    </Trace>
  );
}

function AgentGroupRow({
  tool,
  sessionId,
  live,
  now,
  open,
  onToggle,
}: {
  tool: ToolItem;
  sessionId: string;
  live: SubagentProgress | undefined;
  now: number;
  open: boolean;
  onToggle: () => void;
}) {
  const info = resolveAgent(tool, live, now);
  const subSessionId = useLila((state) => findMatchingSubSession(state.sessions, sessionId, { agentId: info.agentId, description: info.description, prompt: info.prompt }));
  const compactMeta = [
    info.turnCount ? `${info.turnCount} 轮` : undefined,
    info.toolUses ? `${info.toolUses} 次工具` : undefined,
    info.tokensText,
    info.durationText,
  ].filter(Boolean);

  return (
    <div className="flex flex-col">
      <div
        role="button"
        tabIndex={0}
        aria-expanded={open}
        onClick={onToggle}
        onKeyDown={(event) => {
          if (event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            onToggle();
          }
        }}
        className="flex cursor-pointer items-center gap-2 px-3 py-2 text-xs transition-colors hover:bg-muted/50 focus-visible:bg-muted/50 focus-visible:outline-none"
      >
        <span className="shrink-0 text-muted-foreground">{open ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}</span>
        <span className="shrink-0">
          {info.active ? (
            <Spinner className="size-3.5 text-muted-foreground" />
          ) : info.isError ? (
            <CircleAlert className="size-3.5 text-destructive" />
          ) : (
            <CheckCircle2 className="size-3.5 text-emerald-600 dark:text-emerald-400" />
          )}
        </span>
        <Badge variant="secondary" className="h-5 shrink-0 px-1.5 font-normal">
          {info.type}
        </Badge>
        <span className="min-w-0 flex-1 truncate" title={info.activity ? `${info.description ?? ""} · ${info.activity}` : info.description}>
          <span className="font-medium text-foreground">{info.description || "子 Agent"}</span>
          {info.activity ? <span className="ms-1.5 font-mono text-muted-foreground">· {info.activity}</span> : null}
        </span>
        {compactMeta.length ? <span className="hidden shrink-0 text-muted-foreground tabular-nums sm:inline">{compactMeta.join(" · ")}</span> : null}
        {subSessionId ? (
          <Button
            type="button"
            size="xs"
            variant="ghost"
            className="h-6 shrink-0 px-1.5 text-muted-foreground"
            onClick={(event) => {
              event.stopPropagation();
              actions.select(subSessionId);
            }}
          >
            子会话
            <ArrowUpRight />
          </Button>
        ) : null}
      </div>
      {open ? (
        <div className="border-t bg-muted/20 px-4 py-3">
          <AgentDetailsBody info={info} sessionId={sessionId} showSubLink={false} />
        </div>
      ) : null}
    </div>
  );
}

/** Compact grouped view when multiple sub-agents are dispatched in parallel. */
export const AgentGroup = React.memo(function AgentGroup({ tools, sessionId }: { tools: ToolItem[]; sessionId: string }) {
  const subagents = useLila((state) => state.views[sessionId]?.state?.subagents);
  const byId = React.useMemo(() => new Map((subagents ?? []).map((entry) => [entry.id, entry])), [subagents]);
  const [expanded, setExpanded] = React.useState<Record<string, boolean>>({});
  const [now, setNow] = React.useState(() => Date.now());

  const resolved = tools.map((tool) => {
    const id = str(tool.details?.agentId);
    return resolveAgent(tool, id ? byId.get(id) : undefined, now);
  });
  const activeCount = resolved.filter((info) => info.active).length;
  const errorCount = resolved.filter((info) => info.isError).length;
  const doneCount = resolved.length - activeCount - errorCount;
  const totalTokens = resolved.reduce((sum, info) => sum + info.tokens, 0);
  const totalCost = resolved.reduce((sum, info) => sum + info.cost, 0);

  React.useEffect(() => {
    if (!activeCount) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [activeCount]);

  const allExpanded = tools.every((tool) => Boolean(expanded[tool.id]));
  const toggleAll = () => {
    const next = !allExpanded;
    setExpanded(Object.fromEntries(tools.map((tool) => [tool.id, next])));
  };

  const summaryParts = [
    activeCount ? `${activeCount} 运行中` : undefined,
    doneCount ? `${doneCount} 已完成` : undefined,
    errorCount ? `${errorCount} 出错` : undefined,
  ].filter(Boolean);

  const metricsParts = [
    totalTokens > 0 ? formatCompactTokens(totalTokens) : undefined,
    totalCost > 0 ? `$${totalCost.toFixed(4)}` : undefined,
  ].filter(Boolean);

  return (
    <div className="overflow-hidden rounded-xl border bg-card/60">
      <div className="flex flex-wrap items-center gap-2 border-b bg-muted/30 px-3 py-2 text-xs">
        {activeCount > 0 ? (
          <Spinner className="size-3.5 shrink-0 text-muted-foreground" />
        ) : errorCount > 0 ? (
          <CircleAlert className="size-3.5 shrink-0 text-destructive" />
        ) : (
          <Bot className="size-3.5 shrink-0 text-muted-foreground" />
        )}
        <span className="font-medium">{tools.length} 个子 Agent</span>
        {summaryParts.length ? (
          <span className={cn("text-muted-foreground", errorCount > 0 && !activeCount && "text-destructive")}>{summaryParts.join(" · ")}</span>
        ) : null}
        {metricsParts.length ? <span className="text-muted-foreground tabular-nums">· {metricsParts.join(" · ")}</span> : null}
        <Button type="button" size="xs" variant="ghost" className="ms-auto h-6 px-2 text-muted-foreground" onClick={toggleAll}>
          {allExpanded ? "全部收起" : "全部展开"}
        </Button>
      </div>
      <div className="divide-y">
        {tools.map((tool) => {
          const id = str(tool.details?.agentId);
          return (
            <AgentGroupRow
              key={tool.id}
              tool={tool}
              sessionId={sessionId}
              live={id ? byId.get(id) : undefined}
              now={now}
              open={Boolean(expanded[tool.id])}
              onToggle={() => setExpanded((current) => ({ ...current, [tool.id]: !current[tool.id] }))}
            />
          );
        })}
      </div>
    </div>
  );
});

export const ToolCall = React.memo(function ToolCall({ tool, sessionId }: { tool: ToolItem; sessionId: string }) {
  if (tool.name === "Agent") return <SingleAgentCard tool={tool} sessionId={sessionId} />;
  const view = viewOf(tool, sessionId);
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
