import * as React from "react";
import { ArrowDown, ArrowLeft, Blocks, ChevronUp, FolderTree, GitBranch, RotateCw, Search } from "lucide-react";
import type { SessionState, TranscriptItem, TranscriptSearchMatch } from "@lila/protocol";
import { ComposerStatus } from "@/components/composer-status";
import { ConversationSearch } from "@/components/conversation-search";
import type { ChangeSelection } from "@/components/reply-changes";
import { Composer } from "@/components/composer";
import { MarkdownBasePathProvider } from "@/components/markdown";
import { PendingPanel } from "@/components/pending-panel";
import { ResourcesSheet } from "@/components/resources-sheet";
import { ApprovalSelect, ContextUsage, ModelPicker, ModelSetup } from "@/components/session-controls";
import { SubsessionActions } from "@/components/subsession-preview";
import { TranscriptBlocks } from "@/components/transcript-blocks";
import { TurnRail } from "@/components/turn-directory";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import {
  MessageScroller,
  MessageScrollerButton,
  MessageScrollerContent,
  MessageScrollerItem,
  MessageScrollerProvider,
  MessageScrollerViewport,
  useMessageScroller,
} from "@/components/ui/message-scroller";
import { SidebarTrigger, useSidebar } from "@/components/ui/sidebar";
import { Spinner } from "@/components/ui/spinner";
import { Trace, TraceIcon, TraceLine, TraceTitle } from "@/components/ui/trace";
import { cn } from "cn";
import { layoutTranscript } from "@/lib/transcript-layout";
import { actions, EMPTY_VIEW, useLila } from "@/lib/store";

const ABORT_ARM_MS = 1_500;
const BLOCKING_OVERLAY_SELECTOR = [
  '[data-slot="dialog-content"]',
  '[data-slot="alert-dialog-content"]',
  '[data-slot="sheet-content"]',
  '[data-slot="popover-content"]',
  '[data-slot="select-content"]',
  '[data-slot="combobox-content"]',
  '[data-slot="dropdown-menu-content"]',
  "[data-composer-suggest]",
].join(", ");

function SessionToolbar({
  sessionId,
  gitChanges,
  onResources,
  onSearch,
  readOnly,
}: {
  sessionId: string;
  gitChanges?: number | undefined;
  onResources: () => void;
  onSearch: () => void;
  readOnly: boolean;
}) {
  const session = useLila((store) => store.sessions[sessionId]);
  const parentSession = useLila((store) => (session?.parentId ? store.sessions[session.parentId] : undefined));
  const project = useLila((store) => store.projects.find((item) => item.id === session?.projectId));
  const rightPanel = useLila((store) => store.rightPanel);
  const { open, isMobile } = useSidebar();
  const [editing, setEditing] = React.useState(false);
  const [title, setTitle] = React.useState(session?.title ?? "");
  const inputRef = React.useRef<HTMLInputElement>(null);
  const savingRef = React.useRef(false);

  const startEditing = () => {
    setTitle(session?.title ?? "");
    setEditing(true);
  };

  React.useEffect(() => {
    if (!editing) return;
    requestAnimationFrame(() => {
      inputRef.current?.focus();
      inputRef.current?.select();
    });
  }, [editing]);

  async function saveTitle(): Promise<void> {
    if (savingRef.current) return;
    savingRef.current = true;
    const currentTitle = session?.title ?? "";
    const next = title.trim();
    try {
      if (!next || next === currentTitle) {
        setTitle(currentTitle);
        setEditing(false);
        return;
      }
      const ok = await actions.rename(sessionId, next);
      if (!ok) setTitle(currentTitle);
      setEditing(false);
    } finally {
      savingRef.current = false;
    }
  }

  const showTrigger = !open || isMobile;

  return (
    <header className={cn("flex h-11 shrink-0 items-center gap-2 border-b border-border/60 px-4 [-webkit-app-region:drag]", showTrigger && "pl-[74px]")}>
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
              if (event.key === "Enter") {
                event.preventDefault();
                void saveTitle();
              } else if (event.key === "Escape") {
                event.preventDefault();
                setTitle(session?.title ?? "");
                setEditing(false);
              }
            }}
          />
        ) : (
          <h1
            className={cn("truncate text-sm font-medium", !readOnly && "cursor-text [-webkit-app-region:no-drag]")}
            title={readOnly ? undefined : "双击重命名"}
            onDoubleClick={readOnly ? undefined : startEditing}
          >
            {session?.title || "新会话"}
          </h1>
        )}
        <span className="truncate text-xs text-muted-foreground" title={project?.path}>
          {project?.name}
        </span>
      </div>
      <Button type="button" size="icon-sm" variant="ghost" aria-label="查找会话内容" title="查找会话内容（⌘F）" onClick={onSearch} className="text-muted-foreground [-webkit-app-region:no-drag]">
        <Search />
      </Button>
      {project ? (
        <>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            aria-label="项目文件"
            title="项目文件（⌘E）"
            aria-pressed={rightPanel === "files"}
            className={cn("font-normal text-muted-foreground [-webkit-app-region:no-drag]", rightPanel === "files" && "bg-accent text-foreground")}
            onClick={() => actions.toggleRightPanel("files")}
          >
            <FolderTree />
            <span className="hidden @min-[30rem]:inline">文件</span>
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            aria-label="工作区改动"
            title="工作区改动（⇧⌘G）"
            aria-pressed={rightPanel === "changes"}
            className={cn("font-normal text-muted-foreground [-webkit-app-region:no-drag]", rightPanel === "changes" && "bg-accent text-foreground")}
            onClick={() => actions.toggleRightPanel("changes")}
          >
            <GitBranch />
            <span className="hidden @min-[30rem]:inline">改动</span>
            {gitChanges ? (
              <Badge variant="secondary" className="h-4 min-w-4 px-1 font-mono text-[11px] tabular-nums">
                {gitChanges}
              </Badge>
            ) : null}
          </Button>
        </>
      ) : null}
      {!readOnly ? (
        <Button variant="ghost" size="sm" aria-label="能力面板" title="能力面板（技能 / MCP / 扩展）" className="font-normal text-muted-foreground [-webkit-app-region:no-drag]" onClick={onResources}>
          <Blocks />
          <span className="hidden @min-[30rem]:inline">能力</span>
        </Button>
      ) : null}
    </header>
  );
}

function StatusLines({ lines }: { lines: Record<string, string> }) {
  const entries = Object.entries(lines).filter(([, text]) => text);
  if (!entries.length) return null;
  return (
    <div className="flex flex-wrap gap-x-3 gap-y-0.5 px-1 text-xs text-muted-foreground">
      {entries.map(([key, text]) => (
        <span key={key} className="min-w-0 max-w-full truncate" title={`${key}: ${text}`}>
          {text}
        </span>
      ))}
    </div>
  );
}

function SessionControls({ sessionId, state }: { sessionId: string; state: SessionState }) {
  const models = useLila((store) => store.models);
  return (
    <>
      <ModelPicker
        models={models}
        modelId={state.modelId}
        thinkingLevel={state.thinkingLevel}
        currentLevels={state.modelId ? state.thinkingLevels : undefined}
        onApply={(modelId, thinkingLevel) => actions.configure(sessionId, {
          ...(modelId !== state.modelId ? { modelId } : {}),
          thinkingLevel,
        })}
      />
      <ApprovalSelect value={state.approvalMode} onChange={(approvalMode) => void actions.configure(sessionId, { approvalMode })} />
    </>
  );
}

function revealSearchMatch(target: HTMLElement, query: string): void {
  const needle = query.toLowerCase();
  const nodes = document.createTreeWalker(target, NodeFilter.SHOW_TEXT);
  for (let node = nodes.nextNode(); node; node = nodes.nextNode()) {
    const text = node.textContent ?? "";
    const start = text.toLowerCase().indexOf(needle);
    if (start < 0 || start >= text.length || !node.parentElement?.getClientRects().length) continue;
    const range = document.createRange();
    range.setStart(node, start);
    range.setEnd(node, Math.min(text.length, start + query.length));
    CSS.highlights?.set("lila-transcript-search", new Highlight(range));
    node.parentElement.scrollIntoView({ block: "center", inline: "nearest" });
    return;
  }
  target.scrollIntoView({ block: "center", inline: "nearest" });
}

type TranscriptTarget = { id: string; query: string; version: number };

function TranscriptNavigation({ viewportRef, target, scrollToLatest, scrollEndSeq }: {
  viewportRef: React.RefObject<HTMLDivElement | null>;
  target: TranscriptTarget | undefined;
  scrollToLatest: React.RefObject<boolean>;
  scrollEndSeq: number;
}) {
  const { scrollToEnd, scrollToMessage } = useMessageScroller();

  React.useEffect(() => {
    if (!target && !scrollToLatest.current) return;
    const frame = requestAnimationFrame(() => {
      if (scrollToLatest.current) {
        scrollToLatest.current = false;
        scrollToEnd();
      } else if (target) {
        const row = viewportRef.current?.querySelector<HTMLElement>(`[data-transcript-item="${CSS.escape(target.id)}"]`);
        if (target.query) {
          if (row) revealSearchMatch(row, target.query);
        } else {
          scrollToMessage(target.id);
          row?.focus({ preventScroll: true });
        }
      }
    });
    return () => {
      cancelAnimationFrame(frame);
      CSS.highlights?.delete("lila-transcript-search");
    };
  }, [scrollToEnd, scrollToLatest, scrollToMessage, target, viewportRef]);

  React.useEffect(() => {
    if (!scrollEndSeq) return;
    const frame = requestAnimationFrame(() => {
      scrollToEnd({ behavior: "auto" });
    });
    return () => cancelAnimationFrame(frame);
  }, [scrollEndSeq, scrollToEnd]);

  return null;
}

export function Conversation({ sessionId }: { sessionId: string }) {
  const view = useLila((store) => store.views[sessionId]) ?? EMPTY_VIEW;
  const projectId = useLila((store) => store.sessions[sessionId]?.projectId);
  const projectPath = useLila((store) => store.projects.find((item) => item.id === projectId)?.path);
  const connected = useLila((store) => store.connection === "connected");
  const resourcesOpen = useLila((store) => store.resourcesOpen);
  const gitChanges = useLila((store) => (projectId ? store.gitChangeCounts[projectId] : undefined));
  const [abortArmed, setAbortArmed] = React.useState(false);
  const state = view.state;
  const child = useLila((store) => store.sessions[sessionId]?.readOnly === true);
  const readOnly = child || state?.access === "read-only";
  const ready = state?.access === "ready";
  const running = state?.runActive === true;
  const canStop = connected && ready && (running || state.compacting);
  const loaded = state !== undefined;
  const gitWorkActive = state?.status === "running" || state?.status === "waiting" || state?.compacting === true;
  const prevGitWorkActive = React.useRef(gitWorkActive);
  const [searchOpen, setSearchOpen] = React.useState(false);
  const [searchFocus, setSearchFocus] = React.useState(0);
  const [scrollEndSeq, setScrollEndSeq] = React.useState(0);
  const [transcriptTarget, setTranscriptTarget] = React.useState<TranscriptTarget>();
  const [history, setHistory] = React.useState<TranscriptItem[]>();
  const handleReview = React.useCallback(
    (selection: ChangeSelection) => actions.openReplyReview(sessionId, selection.changes, selection.path),
    [sessionId],
  );
  const latestItem = view.items.at(-1);
  const replyVisible = latestItem?.kind === "assistant" && latestItem.streaming && Boolean(latestItem.text || latestItem.thinking);
  const toolRunning = latestItem?.kind === "tool" && latestItem.status === "running";
  const showResponseStatus = running && connected && !readOnly && !history
    && !state.pending.length && !state.retry && !state.compacting && !replyVisible && !toolRunning;
  const searchVersion = React.useRef(0);
  const scrollToLatest = React.useRef(false);
  const abortTimerRef = React.useRef<number | undefined>(undefined);
  const abortArmedRef = React.useRef(false);
  const viewportRef = React.useRef<HTMLDivElement>(null);
  const scrollTopRef = React.useRef<number | undefined>(undefined);
  const initialScrollTopRef = React.useRef<number | undefined>(actions.getScrollTop(sessionId));
  const restoredScrollRef = React.useRef(false);

  const refreshGitCount = React.useCallback(() => {
    if (!projectId || !connected) return;
    void actions
      .gitStatus(projectId)
      .then((status) => actions.setGitChangeCount(projectId, status.isRepo ? status.files.length : undefined))
      .catch(() => {});
  }, [projectId, connected]);

  React.useEffect(() => {
    refreshGitCount();
  }, [refreshGitCount]);

  React.useEffect(() => {
    // The background agents can keep changing files after the parent Run.
    if (prevGitWorkActive.current && !gitWorkActive) {
      refreshGitCount();
      actions.bumpWorkspaceRefresh();
    }
    prevGitWorkActive.current = gitWorkActive;
  }, [gitWorkActive, refreshGitCount]);

  const disarmAbort = React.useCallback(() => {
    window.clearTimeout(abortTimerRef.current);
    abortTimerRef.current = undefined;
    abortArmedRef.current = false;
    setAbortArmed(false);
  }, []);

  const handleAbort = React.useCallback(() => {
    disarmAbort();
    void actions.abort(sessionId);
  }, [disarmAbort, sessionId]);

  React.useEffect(() => {
    if (!canStop) disarmAbort();
    return () => window.clearTimeout(abortTimerRef.current);
  }, [canStop, disarmAbort]);

  // Global double-Escape to abort when no overlay, command palette, or completion menu is open.
  React.useEffect(() => {
    if (!canStop || searchOpen) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.isComposing || event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey) return;
      const store = useLila.getState();
      if (store.paletteOpen || store.resourcesOpen) return;
      if (document.querySelector(BLOCKING_OVERLAY_SELECTOR)) return;
      event.preventDefault();
      if (abortArmedRef.current) {
        handleAbort();
      } else {
        abortArmedRef.current = true;
        setAbortArmed(true);
        window.clearTimeout(abortTimerRef.current);
        abortTimerRef.current = window.setTimeout(() => {
          abortArmedRef.current = false;
          setAbortArmed(false);
        }, ABORT_ARM_MS);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [canStop, handleAbort, searchOpen]);

  // Restore saved scrollTop synchronously after MessageScrollerProvider's layout effects and before paint.
  React.useLayoutEffect(() => {
    if (!loaded || restoredScrollRef.current) return;
    restoredScrollRef.current = true;
    const saved = initialScrollTopRef.current;
    if (saved !== undefined && viewportRef.current) {
      viewportRef.current.scrollTop = saved;
      scrollTopRef.current = saved;
    }
  }, [loaded]);

  // Save current scrollTop before leaving this session; clear it when already at the bottom so revisiting resumes auto-scroll.
  React.useEffect(() => {
    const viewport = viewportRef;
    return () => {
      const node = viewport.current;
      if (node) {
        const atBottom = node.scrollHeight - node.scrollTop - node.clientHeight <= 24;
        actions.setScrollTop(sessionId, atBottom ? undefined : node.scrollTop);
      } else if (scrollTopRef.current !== undefined) {
        actions.setScrollTop(sessionId, scrollTopRef.current);
      }
    };
  }, [sessionId]);

  const openSearch = React.useCallback(() => {
    setSearchOpen(true);
    setSearchFocus((current) => current + 1);
  }, []);

  const clearSearchHit = React.useCallback(() => {
    searchVersion.current++;
    setTranscriptTarget(undefined);
  }, []);

  const closeSearch = React.useCallback(() => {
    clearSearchHit();
    scrollToLatest.current = Boolean(history);
    setHistory(undefined);
    setSearchOpen(false);
  }, [clearSearchHit, history]);

  const jumpToItem = React.useCallback(async (itemId: string, query = ""): Promise<boolean> => {
    const version = ++searchVersion.current;
    const index = useLila.getState().views[sessionId]?.index;
    if (index?.[itemId] !== undefined) {
      setHistory(undefined);
    } else {
      try {
        const items = await actions.transcriptAround(sessionId, itemId);
        if (version !== searchVersion.current || !items.some((item) => item.id === itemId)) return false;
        setHistory(items);
      } catch {
        return false;
      }
    }
    if (!query) setSearchOpen(false);
    setTranscriptTarget({ id: itemId, query, version });
    return true;
  }, [sessionId]);
  const showSearchHit = React.useCallback((hit: TranscriptSearchMatch, query: string) => jumpToItem(hit.itemId, query), [jumpToItem]);

  React.useEffect(() => () => { searchVersion.current++; }, []);

  React.useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.isComposing || event.defaultPrevented || document.querySelector(BLOCKING_OVERLAY_SELECTOR)) return;
      if ((event.metaKey || event.ctrlKey) && !event.shiftKey && !event.altKey && event.key.toLowerCase() === "f") {
        event.preventDefault();
        openSearch();
      } else if (searchOpen && event.key === "Escape") {
        event.preventDefault();
        closeSearch();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [openSearch, closeSearch, searchOpen]);

  const visibleItems = history ?? view.items;
  const hasTurnRail = visibleItems.some((item) => item.kind === "user");
  const renderBlocks = React.useMemo(() => layoutTranscript(visibleItems, projectPath), [visibleItems, projectPath]);

  return (
    <MarkdownBasePathProvider value={projectPath}>
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        <SessionToolbar
          sessionId={sessionId}
          gitChanges={gitChanges}
          onResources={() => actions.setResourcesOpen(true)}
          onSearch={openSearch}
          readOnly={readOnly}
        />
        {searchOpen ? (
          <ConversationSearch sessionId={sessionId} connected={connected} onHit={showSearchHit} onClear={clearSearchHit} onClose={closeSearch} focusRequest={searchFocus} />
        ) : null}
        {history ? (
          <div className="mx-auto flex w-full max-w-[46rem] items-center justify-between px-6 py-1 text-xs text-muted-foreground">
            历史消息
            <Button type="button" variant="ghost" size="xs" onClick={closeSearch}><ArrowDown />返回最新</Button>
          </div>
        ) : null}
        {!loaded ? (
          <div className="flex min-h-0 flex-1 items-center justify-center px-6">
            {!view.loading && view.error ? (
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
            ) : !connected ? (
              <Empty className="py-24">
                <EmptyHeader>
                  <EmptyTitle>等待重连后加载会话…</EmptyTitle>
                  <EmptyDescription>与 core 重新连接后会自动恢复消息和任务状态。</EmptyDescription>
                </EmptyHeader>
                <Spinner />
              </Empty>
            ) : (
              <Empty className="py-24">
                <EmptyHeader>
                  <EmptyTitle>正在打开会话</EmptyTitle>
                  <EmptyDescription>正在读取消息和任务状态。</EmptyDescription>
                </EmptyHeader>
                <Spinner />
              </Empty>
            )}
          </div>
        ) : (
          <MessageScrollerProvider
            key={history ? "history" : sessionId}
            autoScroll={!transcriptTarget?.query && !history}
            defaultScrollPosition={history || initialScrollTopRef.current !== undefined ? "start" : "end"}
            scrollEdgeThreshold={24}
            scrollPreviousItemPeek={48}
          >
            <TranscriptNavigation viewportRef={viewportRef} target={transcriptTarget} scrollToLatest={scrollToLatest} scrollEndSeq={scrollEndSeq} />
            <MessageScroller className="@container min-h-0 flex-1">
              <MessageScrollerViewport
                ref={viewportRef}
                onScroll={(event) => {
                  if (history) return;
                  const node = event.currentTarget;
                  const atBottom = node.scrollHeight - node.scrollTop - node.clientHeight <= 24;
                  scrollTopRef.current = atBottom ? undefined : node.scrollTop;
                }}
              >
                <MessageScrollerContent className={cn("mx-auto w-full max-w-[46rem] gap-3 px-6 pt-5 pb-10", hasTurnRail && "ps-11 @min-[52rem]:ps-6")}>
                  {ready && !state.pending.length && !view.error && view.items.length === 0 ? (
                    <MessageScrollerItem messageId="empty">
                      <Empty className="py-24">
                        <EmptyHeader>
                          <EmptyTitle>要做点什么？</EmptyTitle>
                          <EmptyDescription>Agent 会在项目目录里读代码、改文件、跑命令，过程都会显示在这里。</EmptyDescription>
                        </EmptyHeader>
                      </Empty>
                    </MessageScrollerItem>
                  ) : null}
                  {!history && view.hasEarlier ? (
                    <MessageScrollerItem messageId="load-earlier">
                      <div className="flex justify-center py-1">
                        <Button variant="ghost" size="sm" disabled={view.loadingEarlier} onClick={() => void actions.loadEarlier(sessionId)}>
                          {view.loadingEarlier ? <Spinner /> : <ChevronUp />}
                          {view.loadingEarlier ? "正在加载更早消息…" : "加载更早消息"}
                        </Button>
                      </div>
                    </MessageScrollerItem>
                  ) : null}
                  <TranscriptBlocks blocks={renderBlocks} sessionId={sessionId}
                    revealId={transcriptTarget?.query ? transcriptTarget.id : undefined}
                    running={!ready || running || state.compacting}
                    onReview={handleReview}
                  />
                  {showResponseStatus ? (
                    <MessageScrollerItem messageId="response-status">
                      <Trace className="py-1">
                        <TraceLine status="running" className="text-[13px]">
                          <TraceIcon>
                            <Spinner aria-hidden className="size-3.5" />
                          </TraceIcon>
                          <TraceTitle role="status">正在思考…</TraceTitle>
                        </TraceLine>
                      </Trace>
                    </MessageScrollerItem>
                  ) : null}
                </MessageScrollerContent>
              </MessageScrollerViewport>
              {hasTurnRail ? <TurnRail items={visibleItems} blocks={renderBlocks} onJump={jumpToItem} /> : null}
              {!history ? <MessageScrollerButton /> : null}
            </MessageScroller>
          </MessageScrollerProvider>
        )}
        <div className="mx-auto flex w-full max-w-[46rem] shrink-0 flex-col gap-1.5 px-6 pt-1 pb-4">
          {state && !readOnly ? <ComposerStatus sessionId={sessionId} state={state} /> : null}
          {state ? <PendingPanel sessionId={sessionId} pending={state.pending} /> : null}
          {state?.access === "failed" ? (
            <p role="alert" className="break-words px-1 text-sm text-destructive">会话初始化失败：{state.startupError}</p>
          ) : null}
          {readOnly ? (
            <SubsessionActions sessionId={sessionId} state={state} />
          ) : (
            <>
              {ready && !state.modelId ? <ModelSetup /> : null}
              <Composer
                draftKey={sessionId}
                sessionId={sessionId}
                projectId={projectId}
                running={running}
                disabled={!loaded}
                sendDisabled={!ready || state.compacting}
                abortArmed={abortArmed}
                placeholder={running ? "补充说明会在当前工具结束后送达… 双击 Esc 停止" : "继续对话，/ 使用命令和技能，@ 引用文件"}
                onSubmit={(mode) => {
                  setScrollEndSeq((current) => current + 1);
                  return actions.sendDraft(sessionId, mode);
                }}
                onAbort={canStop ? handleAbort : undefined}
                footer={ready ? <SessionControls sessionId={sessionId} state={state} /> : null}
                trailing={ready ? <ContextUsage sessionId={sessionId} state={state} /> : null}
              />
            </>
          )}
          {state ? <StatusLines lines={state.statusLines} /> : null}
        </div>
        <ResourcesSheet sessionId={sessionId} open={resourcesOpen} onOpenChange={actions.setResourcesOpen} />
      </div>
    </MarkdownBasePathProvider>
  );
}
