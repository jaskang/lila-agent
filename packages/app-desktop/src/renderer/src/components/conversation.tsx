import * as React from "react";
import { ArrowLeft, ArrowUpRight, Blocks, Bot, ChevronDown, ChevronRight, ChevronUp, GitBranch, RotateCw } from "lucide-react";
import type { SessionState, SubagentProgress, TranscriptItem } from "@lila/protocol";
import { Composer } from "@/components/composer";
import { GitSheet } from "@/components/git-sheet";
import { MarkdownBasePathProvider } from "@/components/markdown";
import { PendingPanel } from "@/components/pending-panel";
import { ResourcesSheet } from "@/components/resources-sheet";
import { ApprovalSelect, ContextUsage, ModelPicker, ThinkingSelect } from "@/components/session-controls";
import { AgentGroup, findMatchingSubSession, formatCompactTokens } from "@/components/tool-call";
import { TranscriptEntry } from "@/components/transcript-item";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { Marker, MarkerContent, MarkerIcon } from "@/components/ui/marker";
import {
  MessageScroller,
  MessageScrollerButton,
  MessageScrollerContent,
  MessageScrollerItem,
  MessageScrollerProvider,
  MessageScrollerViewport,
} from "@/components/ui/message-scroller";
import { SidebarTrigger, useSidebar } from "@/components/ui/sidebar";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "cn";
import { actions, emptyView, useLila } from "@/lib/store";

function SessionToolbar({
  sessionId,
  gitChanges,
  onGit,
  onResources,
}: {
  sessionId: string;
  gitChanges?: number | undefined;
  onGit: () => void;
  onResources: () => void;
}) {
  const session = useLila((store) => store.sessions[sessionId]);
  const parentSession = useLila((store) => (session?.parentId ? store.sessions[session.parentId] : undefined));
  const project = useLila((store) => store.projects.find((item) => item.id === session?.projectId));
  const { open, isMobile } = useSidebar();
  const [editing, setEditing] = React.useState(false);
  const [title, setTitle] = React.useState(session?.title ?? "");
  const inputRef = React.useRef<HTMLInputElement>(null);

  React.useEffect(() => {
    if (!editing) return;
    setTitle(session?.title ?? "");
    requestAnimationFrame(() => {
      inputRef.current?.focus();
      inputRef.current?.select();
    });
  }, [editing, session?.title]);

  async function saveTitle(): Promise<void> {
    const next = title.trim();
    if (!next || next === session?.title || (await actions.rename(sessionId, next))) setEditing(false);
  }

  const showTrigger = !open || isMobile;

  return (
    <header className={cn("flex h-10 shrink-0 items-center gap-2 border-b border-border/60 px-4 [-webkit-app-region:drag]", showTrigger && "pl-[74px]")}>
      {showTrigger ? <SidebarTrigger className="[-webkit-app-region:no-drag]" title="切换侧边栏（⌘B）" /> : null}
      {parentSession ? (
        <Button
          variant="ghost"
          size="xs"
          className="shrink-0 text-muted-foreground [-webkit-app-region:no-drag]"
          title={`返回父会话：${parentSession.title || "新会话"}`}
          onClick={() => actions.select(parentSession.id)}
        >
          <ArrowLeft />
          父会话
        </Button>
      ) : null}
      <div className="flex min-w-0 flex-1 items-baseline gap-2">
        {editing ? (
          <Input
            ref={inputRef}
            value={title}
            maxLength={120}
            aria-label="会话名称"
            className="h-7 max-w-sm bg-background text-sm [-webkit-app-region:no-drag]"
            onChange={(event) => setTitle(event.target.value)}
            onBlur={() => void saveTitle()}
            onKeyDown={(event) => {
              if (event.nativeEvent.isComposing) return;
              if (event.key === "Enter") void saveTitle();
              if (event.key === "Escape") setEditing(false);
            }}
          />
        ) : (
          <h1
            className="cursor-text truncate text-sm font-medium [-webkit-app-region:no-drag]"
            title="双击重命名"
            onDoubleClick={() => setEditing(true)}
          >
            {session?.title || "新会话"}
          </h1>
        )}
        <span className="truncate text-xs text-muted-foreground" title={project?.path}>
          {project?.name}
        </span>
      </div>
      {project ? (
        <Button variant="ghost" size="sm" className="font-normal text-muted-foreground [-webkit-app-region:no-drag]" onClick={onGit}>
          <GitBranch />
          改动
          {gitChanges ? (
            <Badge variant="secondary" className="h-4 min-w-4 px-1 font-mono text-[11px] tabular-nums">
              {gitChanges}
            </Badge>
          ) : null}
        </Button>
      ) : null}
      <Button variant="ghost" size="sm" className="font-normal text-muted-foreground [-webkit-app-region:no-drag]" onClick={onResources}>
        <Blocks />
        能力
      </Button>
    </header>
  );
}

function RunStatus({ state }: { state: SessionState }) {
  if (state.status !== "running" && !state.compacting) return null;
  const text = state.retry
    ? `重试中（${state.retry.attempt}/${state.retry.max}）：${state.retry.message}`
    : state.compacting
      ? "正在压缩上下文…"
      : "正在工作…";
  return (
    <Marker>
      <MarkerIcon>
        <Spinner />
      </MarkerIcon>
      <MarkerContent className={state.retry ? "text-amber-700 dark:text-amber-400" : "shimmer"}>{text}</MarkerContent>
    </Marker>
  );
}

function StatusLines({ lines }: { lines: Record<string, string> }) {
  const entries = Object.entries(lines).filter(([, text]) => text);
  if (!entries.length) return null;
  return (
    <div className="flex flex-wrap gap-x-3 gap-y-0.5 px-1 text-xs text-muted-foreground">
      {entries.map(([key, text]) => (
        <span key={key} className="truncate" title={`${key}: ${text}`}>
          {text}
        </span>
      ))}
    </div>
  );
}

function elapsedLabel(startedAt: number, now: number): string {
  const seconds = Math.max(0, Math.floor((now - startedAt) / 1_000));
  if (seconds < 60) return `${seconds}s`;
  return `${Math.floor(seconds / 60)}m${seconds % 60}s`;
}

function ActiveSubagentRow({ sessionId, subagent, now }: { sessionId: string; subagent: SubagentProgress; now: number }) {
  const targetSessionId = useLila((store) => findMatchingSubSession(store.sessions, sessionId, { agentId: subagent.id, description: subagent.description }));
  const stats = [
    subagent.turns ? `${subagent.turns} 轮` : undefined,
    subagent.toolUses ? `${subagent.toolUses} 次工具` : undefined,
    subagent.tokens ? formatCompactTokens(subagent.tokens) : undefined,
    subagent.cost ? `$${subagent.cost.toFixed(4)}` : undefined,
    elapsedLabel(subagent.startedAt, now),
  ].filter(Boolean);

  return (
    <div className="flex items-center gap-2 rounded-lg px-2 py-1 text-xs hover:bg-muted/50">
      <Spinner className="size-3.5 shrink-0 text-muted-foreground" />
      <Badge variant="secondary" className="h-5 shrink-0 gap-1 font-normal">
        <Bot className="size-3" />
        {subagent.type}
      </Badge>
      {subagent.background ? (
        <Badge variant="outline" className="h-5 shrink-0 font-normal text-muted-foreground">
          后台
        </Badge>
      ) : null}
      <span className="min-w-0 flex-1 truncate" title={subagent.activity ? `${subagent.description} · ${subagent.activity}` : subagent.description}>
        <span className="font-medium">{subagent.description || "子 Agent"}</span>
        {subagent.activity ? <span className="ms-1.5 font-mono text-muted-foreground">· {subagent.activity}</span> : null}
      </span>
      <span className="hidden shrink-0 text-muted-foreground tabular-nums sm:inline">{stats.join(" · ")}</span>
      {targetSessionId ? (
        <Button type="button" size="xs" variant="ghost" className="h-6 shrink-0 px-1.5 text-muted-foreground" onClick={() => actions.select(targetSessionId)}>
          查看
          <ArrowUpRight />
        </Button>
      ) : null}
    </div>
  );
}

type AgentToolItem = Extract<TranscriptItem, { kind: "tool" }>;
type RenderBlock =
  | { kind: "item"; item: TranscriptItem }
  | { kind: "agent-group"; id: string; tools: AgentToolItem[] };

/** Groups consecutive parallel `Agent` tool calls into one compact summary block. */
function groupTranscriptItems(items: readonly TranscriptItem[]): RenderBlock[] {
  const blocks: RenderBlock[] = [];
  for (let i = 0; i < items.length; ) {
    const item = items[i]!;
    if (item.kind === "tool" && item.name === "Agent") {
      const tools: AgentToolItem[] = [item];
      let j = i + 1;
      while (j < items.length) {
        const next = items[j]!;
        if (next.kind === "tool" && next.name === "Agent") {
          tools.push(next);
          j += 1;
        } else {
          break;
        }
      }
      if (tools.length >= 2) {
        blocks.push({ kind: "agent-group", id: `ag:${tools[0]!.id}`, tools });
      } else {
        blocks.push({ kind: "item", item });
      }
      i = j;
      continue;
    }
    blocks.push({ kind: "item", item });
    i += 1;
  }
  return blocks;
}

function ActiveSubagentsBar({ sessionId, subagents }: { sessionId: string; subagents?: SubagentProgress[] | undefined }) {
  const active = React.useMemo(
    () => (subagents ?? []).filter((entry) => entry.status === "queued" || entry.status === "running"),
    [subagents],
  );
  const [expanded, setExpanded] = React.useState(false);
  const [now, setNow] = React.useState(() => Date.now());
  React.useEffect(() => {
    if (!active.length) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [active.length]);
  if (!active.length) return null;

  // Keep the bar to one compact line by default so it doesn't duplicate the transcript's AgentGroup or crowd the viewport.
  const collapsible = active.length >= 2;
  const showRows = !collapsible || expanded;
  const totalTokens = active.reduce((sum, entry) => sum + entry.tokens, 0);
  const totalCost = active.reduce((sum, entry) => sum + entry.cost, 0);
  const summaryStats = [
    totalTokens > 0 ? formatCompactTokens(totalTokens) : undefined,
    totalCost > 0 ? `$${totalCost.toFixed(4)}` : undefined,
  ].filter(Boolean);

  return (
    <div className="flex flex-col gap-0.5 rounded-xl border bg-muted/30 p-1.5">
      {collapsible ? (
        <button
          type="button"
          aria-expanded={expanded}
          onClick={() => setExpanded((current) => !current)}
          className="flex w-full items-center gap-2 rounded-lg px-2 py-1 text-left text-xs hover:bg-muted/50"
        >
          <Spinner className="size-3.5 shrink-0 text-muted-foreground" />
          <span className="font-medium">{active.length} 个子 Agent 运行中</span>
          {summaryStats.length ? <span className="text-muted-foreground tabular-nums">· {summaryStats.join(" · ")}</span> : null}
          <span className="ms-auto flex items-center gap-0.5 text-muted-foreground">
            {expanded ? "收起" : "展开"}
            {expanded ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
          </span>
        </button>
      ) : null}
      {showRows
        ? active.map((subagent) => <ActiveSubagentRow key={subagent.id} sessionId={sessionId} subagent={subagent} now={now} />)
        : null}
    </div>
  );
}

function QueueChips({ queue }: { queue: SessionState["queue"] }) {
  if (!queue.steering.length && !queue.followUp.length) return null;
  return (
    <div className="flex flex-wrap gap-1.5 px-1">
      {queue.steering.map((text, index) => (
        <Badge key={`s-${index}`} variant="secondary" className="max-w-full" title={text}>
          <span className="text-muted-foreground">插话</span>
          <span className="truncate">{text}</span>
        </Badge>
      ))}
      {queue.followUp.map((text, index) => (
        <Badge key={`f-${index}`} variant="outline" className="max-w-full" title={text}>
          <span className="text-muted-foreground">排队</span>
          <span className="truncate">{text}</span>
        </Badge>
      ))}
    </div>
  );
}

function SessionControls({ sessionId, state }: { sessionId: string; state: SessionState }) {
  const models = useLila((store) => store.models);
  return (
    <>
      <ModelPicker models={models} value={state.modelId} onChange={(modelId) => void actions.configure(sessionId, { modelId })} />
      <ThinkingSelect levels={state.thinkingLevels} value={state.thinkingLevel} onChange={(thinkingLevel) => void actions.configure(sessionId, { thinkingLevel })} />
      <ApprovalSelect value={state.approvalMode} onChange={(approvalMode) => void actions.configure(sessionId, { approvalMode })} />
    </>
  );
}

export function Conversation({ sessionId }: { sessionId: string }) {
  const view = useLila((store) => store.views[sessionId]) ?? emptyView();
  const projectId = useLila((store) => store.sessions[sessionId]?.projectId);
  const projectPath = useLila((store) => store.projects.find((item) => item.id === projectId)?.path);
  const connected = useLila((store) => store.connection === "connected");
  const resourcesOpen = useLila((store) => store.resourcesOpen);
  const gitOpen = useLila((store) => store.gitOpen);
  const [gitChanges, setGitChanges] = React.useState<number | undefined>();
  const state = view.state;
  const running = state ? state.status === "running" || state.status === "waiting" || state.compacting : false;
  const loaded = state !== undefined;
  const prevRunning = React.useRef(running);

  const refreshGitCount = React.useCallback(() => {
    if (!projectId || !connected) return;
    void actions
      .gitStatus(projectId)
      .then((status) => setGitChanges(status.isRepo ? status.files.length : undefined))
      .catch(() => {});
  }, [projectId, connected]);

  React.useEffect(() => {
    setGitChanges(undefined);
    refreshGitCount();
  }, [refreshGitCount]);

  React.useEffect(() => {
    // Refresh git badge right after an agent run settles.
    if (prevRunning.current && !running) refreshGitCount();
    prevRunning.current = running;
  }, [running, refreshGitCount]);

  const renderBlocks = React.useMemo(() => groupTranscriptItems(view.items), [view.items]);

  return (
    <MarkdownBasePathProvider value={projectPath}>
    <div className="flex min-h-0 flex-1 flex-col">
      <SessionToolbar
        sessionId={sessionId}
        gitChanges={gitChanges}
        onGit={() => actions.setGitOpen(true)}
        onResources={() => actions.setResourcesOpen(true)}
      />
      <MessageScrollerProvider key={sessionId} autoScroll defaultScrollPosition="last-anchor" scrollPreviousItemPeek={48}>
        <MessageScroller className="min-h-0 flex-1">
          <MessageScrollerViewport>
            <MessageScrollerContent className="mx-auto w-full max-w-3xl gap-4 px-6 pt-6 pb-10">
              {view.loading && !loaded ? (
                <MessageScrollerItem messageId="loading">
                  <Empty className="py-24">
                    <EmptyHeader>
                      <EmptyTitle>正在打开会话</EmptyTitle>
                      <EmptyDescription>正在读取消息和任务状态。</EmptyDescription>
                    </EmptyHeader>
                    <Spinner />
                  </Empty>
                </MessageScrollerItem>
              ) : null}
              {!view.loading && view.error ? (
                <MessageScrollerItem messageId="load-error">
                  <Empty className="py-24">
                    <EmptyHeader>
                      <EmptyTitle>会话打开失败</EmptyTitle>
                      <EmptyDescription className="max-w-md break-words">{view.error}</EmptyDescription>
                    </EmptyHeader>
                    <Button variant="outline" onClick={() => actions.retrySession(sessionId)}>
                      <RotateCw />
                      重试
                    </Button>
                  </Empty>
                </MessageScrollerItem>
              ) : null}
              {loaded && !view.error && view.items.length === 0 ? (
                <MessageScrollerItem messageId="empty">
                  <Empty className="py-24">
                    <EmptyHeader>
                      <EmptyTitle>要做点什么？</EmptyTitle>
                      <EmptyDescription>Agent 会在项目目录里读代码、改文件、跑命令，过程都会显示在这里。</EmptyDescription>
                    </EmptyHeader>
                  </Empty>
                </MessageScrollerItem>
              ) : null}
              {view.hasEarlier ? (
                <MessageScrollerItem messageId="load-earlier">
                  <div className="flex justify-center py-1">
                    <Button variant="ghost" size="sm" disabled={view.loadingEarlier} onClick={() => void actions.loadEarlier(sessionId)}>
                      {view.loadingEarlier ? <Spinner /> : <ChevronUp />}
                      {view.loadingEarlier ? "正在加载更早消息…" : "加载更早消息"}
                    </Button>
                  </div>
                </MessageScrollerItem>
              ) : null}
              {renderBlocks.map((block) =>
                block.kind === "agent-group" ? (
                  <MessageScrollerItem key={block.id} messageId={block.id}>
                    <AgentGroup tools={block.tools} sessionId={sessionId} />
                  </MessageScrollerItem>
                ) : (
                  <MessageScrollerItem key={block.item.id} messageId={block.item.id} scrollAnchor={block.item.kind === "user"}>
                    <TranscriptEntry item={block.item} sessionId={sessionId} running={block.item.kind === "user" ? running : false} />
                  </MessageScrollerItem>
                ),
              )}
              {state && !state.pending.length ? (
                <MessageScrollerItem messageId="status">
                  <RunStatus state={state} />
                </MessageScrollerItem>
              ) : null}
            </MessageScrollerContent>
          </MessageScrollerViewport>
          <MessageScrollerButton />
        </MessageScroller>
      </MessageScrollerProvider>
      <div className="mx-auto flex w-full max-w-3xl shrink-0 flex-col gap-2 px-6 pb-5">
        {state ? <PendingPanel sessionId={sessionId} pending={state.pending} /> : null}
        {state ? <ActiveSubagentsBar sessionId={sessionId} subagents={state.subagents} /> : null}
        {state ? <QueueChips queue={state.queue} /> : null}
        <Composer
          draftKey={sessionId}
          sessionId={sessionId}
          projectId={projectId}
          running={running}
          disabled={!loaded}
          placeholder={running ? "补充说明会在当前工具结束后送达… 双击 Esc 停止" : "继续对话，/ 使用命令和技能，@ 引用文件"}
          onSubmit={(text, images, mode) => actions.prompt(sessionId, text, images, mode)}
          onAbort={() => void actions.abort(sessionId)}
          footer={state ? <SessionControls sessionId={sessionId} state={state} /> : null}
          trailing={state ? <ContextUsage sessionId={sessionId} state={state} /> : null}
        />
        {state ? <StatusLines lines={state.statusLines} /> : null}
      </div>
      <ResourcesSheet sessionId={sessionId} open={resourcesOpen} onOpenChange={actions.setResourcesOpen} />
      <GitSheet
        projectId={projectId}
        open={gitOpen}
        onOpenChange={actions.setGitOpen}
        onStatusLoaded={(status) => setGitChanges(status.isRepo ? status.files.length : undefined)}
      />
    </div>
    </MarkdownBasePathProvider>
  );
}
