import * as React from "react";
import { ArrowUpRight, X } from "lucide-react";
import type { SessionState } from "@lila/protocol";
import { MarkdownBasePathProvider } from "@/components/markdown";
import type { ChangeSelection } from "@/components/reply-changes";
import { TranscriptBlocks } from "@/components/transcript-blocks";
import { Button } from "@/components/ui/button";
import { MessageScroller, MessageScrollerButton, MessageScrollerContent, MessageScrollerProvider, MessageScrollerViewport } from "@/components/ui/message-scroller";
import { Spinner } from "@/components/ui/spinner";
import { actions, EMPTY_VIEW, useLila } from "@/lib/store";
import { layoutTranscript } from "@/lib/transcript-layout";

export function SubsessionActions({ sessionId, state }: { sessionId: string; state: SessionState | undefined }) {
  const connected = useLila((store) => store.connection === "connected");
  const [forking, setForking] = React.useState(false);
  return (
    <div className="flex flex-wrap items-center justify-between gap-2 rounded-xl border bg-muted/30 px-3 py-2 text-sm">
      <p role="status" className="text-muted-foreground">
        {state?.status === "running" ? "子 Agent 正在运行，记录会自动更新。" : "这是子 Agent 的只读记录。"}
      </p>
      <Button
        type="button"
        size="sm"
        variant="outline"
        disabled={!connected || state?.access !== "read-only" || state.status === "running" || forking}
        onClick={() => {
          setForking(true);
          void actions.continueSubsession(sessionId).finally(() => setForking(false));
        }}
      >
        {forking ? <Spinner /> : <ArrowUpRight />}
        继续为新会话
      </Button>
    </div>
  );
}

export function SubsessionPreview({ sessionId }: { sessionId: string }) {
  const session = useLila((store) => store.sessions[sessionId]);
  const projectPath = useLila((store) => store.projects.find((project) => project.id === session?.projectId)?.path);
  const connected = useLila((store) => store.connection === "connected");
  const view = useLila((store) => store.views[sessionId]) ?? EMPTY_VIEW;
  const blocks = React.useMemo(() => layoutTranscript(view.items, projectPath), [view.items, projectPath]);
  const handleReview = React.useCallback(
    (selection: ChangeSelection) => actions.openReplyReview(sessionId, selection.changes, selection.path),
    [sessionId],
  );
  const headingId = React.useId();
  const closeButton = React.useRef<HTMLButtonElement>(null);
  const returnFocus = React.useRef<Element | null>(null);

  React.useEffect(() => {
    returnFocus.current = document.activeElement;
    closeButton.current?.focus({ preventScroll: true });
  }, []);

  function close() {
    actions.closePreview();
    if (returnFocus.current instanceof HTMLElement && returnFocus.current.isConnected) {
      returnFocus.current.focus({ preventScroll: true });
    }
  }

  return (
    <MarkdownBasePathProvider value={projectPath}>
      <div aria-labelledby={headingId} className="flex min-h-0 flex-1 flex-col">
        <header className="flex h-11 shrink-0 items-center gap-1 border-b border-border/60 px-3">
          <h2 id={headingId} className="min-w-0 flex-1 truncate text-sm font-medium" title={session?.title}>{session?.title ?? "子 Agent"}</h2>
          <Button type="button" size="icon-sm" variant="ghost" aria-label="在主对话区打开子会话" title="在主对话区打开" onClick={() => actions.select(sessionId)}><ArrowUpRight /></Button>
          <Button ref={closeButton} type="button" size="icon-sm" variant="ghost" aria-label="关闭子会话预览" onClick={close}><X /></Button>
        </header>
        {view.state ? (
          <MessageScrollerProvider key={sessionId} autoScroll defaultScrollPosition="end" scrollEdgeThreshold={24}>
            <MessageScroller className="min-h-0 flex-1">
              <MessageScrollerViewport>
                <MessageScrollerContent className="gap-2.5 px-3 pt-3 pb-7">
                  {view.hasEarlier ? (
                    <Button type="button" variant="ghost" size="sm" className="self-center" disabled={view.loadingEarlier} onClick={() => void actions.loadEarlier(sessionId)}>
                      {view.loadingEarlier ? "正在加载更早消息…" : "加载更早消息"}
                    </Button>
                  ) : null}
                  <TranscriptBlocks blocks={blocks} sessionId={sessionId} running={false} onReview={handleReview} />
                  {!blocks.length ? <p className="text-sm text-muted-foreground">还没有子任务记录。</p> : null}
                </MessageScrollerContent>
              </MessageScrollerViewport>
              <MessageScrollerButton />
            </MessageScroller>
          </MessageScrollerProvider>
        ) : (
          <div role="status" className="flex min-h-0 flex-1 flex-col items-center justify-center gap-2 p-4 text-sm text-muted-foreground">
            {view.error && !view.loading ? (
              <>
                <p className="break-words">{view.error}</p>
                <Button type="button" variant="outline" size="sm" disabled={!connected} onClick={() => actions.retrySession(sessionId)}>重试</Button>
              </>
            ) : <><Spinner aria-hidden />{connected ? "正在打开子会话…" : "等待重新连接…"}</>}
          </div>
        )}
        <div className="shrink-0 p-3">
          <SubsessionActions sessionId={sessionId} state={view.state} />
        </div>
      </div>
    </MarkdownBasePathProvider>
  );
}
