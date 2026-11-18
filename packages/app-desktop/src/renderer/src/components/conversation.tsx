import * as React from "react";
import { Blocks, ChevronUp, RotateCw } from "lucide-react";
import type { SessionState } from "@lila/protocol";
import { Composer } from "@/components/composer";
import { PendingPanel } from "@/components/pending-panel";
import { ResourcesSheet } from "@/components/resources-sheet";
import { ApprovalSelect, ContextUsage, ModelPicker, ThinkingSelect } from "@/components/session-controls";
import { TranscriptEntry } from "@/components/transcript-item";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Marker, MarkerContent, MarkerIcon } from "@/components/ui/marker";
import {
  MessageScroller,
  MessageScrollerButton,
  MessageScrollerContent,
  MessageScrollerItem,
  MessageScrollerProvider,
  MessageScrollerViewport,
} from "@/components/ui/message-scroller";
import { Spinner } from "@/components/ui/spinner";
import { actions, emptyView, useLila } from "@/lib/store";

function SessionToolbar({ sessionId, onResources }: { sessionId: string; onResources: () => void }) {
  const session = useLila((store) => store.sessions[sessionId]);
  const project = useLila((store) => store.projects.find((item) => item.id === session?.projectId));
  return (
    <header className="flex h-12 shrink-0 items-center gap-2 px-4 [-webkit-app-region:drag]">
      <div className="flex min-w-0 flex-1 items-baseline gap-2">
        <h1 className="truncate text-sm font-medium">{session?.title || "新会话"}</h1>
        <span className="truncate text-xs text-muted-foreground" title={project?.path}>
          {project?.name}
        </span>
      </div>
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
  const [resourcesOpen, setResourcesOpen] = React.useState(false);
  const state = view.state;
  const running = state ? state.status === "running" || state.status === "waiting" || state.compacting : false;
  const loaded = state !== undefined;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <SessionToolbar sessionId={sessionId} onResources={() => setResourcesOpen(true)} />
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
              {view.items.map((item) => (
                <MessageScrollerItem key={item.id} messageId={item.id} scrollAnchor={item.kind === "user"}>
                  <TranscriptEntry item={item} sessionId={sessionId} running={running} />
                </MessageScrollerItem>
              ))}
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
        {state ? <QueueChips queue={state.queue} /> : null}
        <Composer
          draftKey={sessionId}
          sessionId={sessionId}
          projectId={projectId}
          running={running}
          disabled={!loaded}
          placeholder={running ? "补充说明会在当前工具结束后送达… 按 Esc 停止" : "继续对话，/ 使用命令和技能，@ 引用文件"}
          onSubmit={(text, images, mode) => actions.prompt(sessionId, text, images, mode)}
          onAbort={() => void actions.abort(sessionId)}
          footer={state ? <SessionControls sessionId={sessionId} state={state} /> : null}
          trailing={state ? <ContextUsage sessionId={sessionId} state={state} /> : null}
        />
        {state ? <StatusLines lines={state.statusLines} /> : null}
      </div>
      <ResourcesSheet sessionId={sessionId} open={resourcesOpen} onOpenChange={setResourcesOpen} />
    </div>
  );
}
