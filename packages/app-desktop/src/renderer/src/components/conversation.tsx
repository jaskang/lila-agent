import * as React from "react";
import { ArrowUp, Ban, Brain, Check, CircleAlert, Info, Square, TriangleAlert } from "lucide-react";
import type { ApprovalMode } from "@lila/protocol";
import { Markdown } from "@/components/markdown";
import { optionLabel, PendingPanel } from "@/components/pending-panel";
import { ToolCall } from "@/components/tool-call";
import { Bubble, BubbleContent } from "@/components/ui/bubble";
import { Button } from "@/components/ui/button";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Kbd } from "@/components/ui/kbd";
import { Marker, MarkerContent, MarkerIcon } from "@/components/ui/marker";
import { Message, MessageContent } from "@/components/ui/message";
import {
  MessageScroller,
  MessageScrollerButton,
  MessageScrollerContent,
  MessageScrollerItem,
  MessageScrollerProvider,
  MessageScrollerViewport,
} from "@/components/ui/message-scroller";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Textarea } from "@/components/ui/textarea";
import {
  Trace,
  TraceCollapsible,
  TraceCollapsibleContent,
  TraceCollapsibleTrigger,
  TraceIcon,
  TraceLine,
  TraceTitle,
} from "@/components/ui/trace";
import { actions, APPROVAL_LABEL, emptyView, useLila, type TranscriptItem } from "@/lib/store";

const NOTICE_ICON = { info: Info, warning: TriangleAlert, error: CircleAlert } as const;

const Item = React.memo(function Item({ item }: { item: TranscriptItem }) {
  switch (item.kind) {
    case "user":
      return (
        <Message align="end">
          <MessageContent>
            <Bubble align="end" variant="muted">
              <BubbleContent className="text-sm whitespace-pre-wrap">{item.text}</BubbleContent>
            </Bubble>
          </MessageContent>
        </Message>
      );
    case "assistant":
      return (
        <Message>
          <MessageContent>
            {item.thinking ? (
              <Trace>
                <TraceCollapsible>
                  <TraceCollapsibleTrigger>
                    <TraceLine {...(item.streaming && !item.text ? { status: "running" as const } : {})}>
                      <TraceIcon>
                        <Brain />
                      </TraceIcon>
                      <TraceTitle>{item.streaming && !item.text ? "思考中" : "思考过程"}</TraceTitle>
                    </TraceLine>
                  </TraceCollapsibleTrigger>
                  <TraceCollapsibleContent>
                    <Markdown text={item.thinking} streaming={item.streaming} />
                  </TraceCollapsibleContent>
                </TraceCollapsible>
              </Trace>
            ) : null}
            {item.text ? <Markdown text={item.text} streaming={item.streaming} /> : null}
          </MessageContent>
        </Message>
      );
    case "tool":
      return <ToolCall tool={item} />;
    case "decision":
      return (
        <Marker>
          <MarkerIcon>{item.status === "answered" ? <Check /> : <Ban />}</MarkerIcon>
          <MarkerContent>
            {item.title.replace(/^Allow tool:\s*/, "运行 ")}
            {" · "}
            {item.status === "answered" ? (item.answer ? optionLabel(item.answer) : "已回答") : item.status === "cancelled" ? "已取消" : "已失效"}
          </MarkerContent>
        </Marker>
      );
    case "notice": {
      const Icon = NOTICE_ICON[item.tone];
      return (
        <Marker
          className={
            item.tone === "error" ? "text-destructive" : item.tone === "warning" ? "text-amber-700 dark:text-amber-400" : undefined
          }
        >
          <MarkerIcon>
            <Icon />
          </MarkerIcon>
          <MarkerContent className="whitespace-pre-wrap">{item.text}</MarkerContent>
        </Marker>
      );
    }
  }
});

function Composer({ sessionId, running }: { sessionId: string; running: boolean }) {
  const [text, setText] = React.useState("");
  const [sending, setSending] = React.useState(false);
  const connected = useLila((state) => state.connection === "connected");
  const ref = React.useRef<HTMLTextAreaElement>(null);
  React.useEffect(() => ref.current?.focus(), [sessionId]);
  const canSend = connected && !running && !sending && text.trim().length > 0;

  async function submit(): Promise<void> {
    if (!canSend) return;
    setSending(true);
    const ok = await actions.send(sessionId, text.trim());
    setSending(false);
    if (ok) setText("");
  }

  return (
    <form
      className="rounded-2xl border bg-background shadow-xs transition-colors focus-within:border-ring"
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <Textarea
        ref={ref}
        value={text}
        onChange={(event) => setText(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
            event.preventDefault();
            void submit();
          }
          if (event.key === "Escape" && running) void actions.abort(sessionId);
        }}
        placeholder={running ? "任务进行中… 按 Esc 停止" : "描述你要完成的编程任务"}
        className="max-h-60 min-h-14 resize-none border-0 bg-transparent px-4 pt-3 shadow-none focus-visible:ring-0 dark:bg-transparent"
      />
      <div className="flex items-center justify-between gap-2 px-3 pb-2">
        <span className="text-xs text-muted-foreground">
          <Kbd>↵</Kbd> 发送 · <Kbd>⇧↵</Kbd> 换行
        </span>
        {running ? (
          <Button type="button" size="sm" variant="outline" onClick={() => void actions.abort(sessionId)}>
            <Square className="fill-current" />
            停止
          </Button>
        ) : (
          <Button type="submit" size="icon-sm" disabled={!canSend} aria-label="发送">
            {sending ? <Spinner /> : <ArrowUp />}
          </Button>
        )}
      </div>
    </form>
  );
}

function SessionToolbar({ sessionId }: { sessionId: string }) {
  const session = useLila((state) => state.sessions[sessionId]);
  const project = useLila((state) => state.projects.find((item) => item.id === session?.projectId));
  const models = useLila((state) => state.models);
  const defaultModelId = useLila((state) => state.defaultModelId);
  if (!session) return null;
  const busy = session.status === "running" || session.status === "waiting";
  const modelValue = session.modelId ?? defaultModelId ?? "";
  return (
    <header className="flex h-12 shrink-0 items-center gap-2 border-b px-4 [-webkit-app-region:drag]">
      <div className="flex min-w-0 flex-1 items-baseline gap-2">
        <h1 className="truncate text-sm font-medium">{session.title || "新会话"}</h1>
        <span className="truncate text-xs text-muted-foreground" title={project?.path}>
          {project?.name}
        </span>
      </div>
      <div className="flex items-center gap-2 [-webkit-app-region:no-drag]">
        <Select
          value={modelValue}
          disabled={busy || !models.length}
          onValueChange={(value) => {
            if (typeof value === "string" && value !== modelValue) void actions.configure(sessionId, { modelId: value });
          }}
          items={models.map((model) => ({ value: model.id, label: model.label }))}
        >
          <SelectTrigger size="sm" className="max-w-56" aria-label="模型">
            <SelectValue placeholder="默认模型" />
          </SelectTrigger>
          <SelectContent alignItemWithTrigger={false} align="end">
            {models.map((model) => (
              <SelectItem key={model.id} value={model.id}>
                {model.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select
          value={session.approvalMode}
          disabled={busy}
          onValueChange={(value) => {
            if (value && value !== session.approvalMode) void actions.configure(sessionId, { approvalMode: value as ApprovalMode });
          }}
          items={Object.entries(APPROVAL_LABEL).map(([value, label]) => ({ value, label }))}
        >
          <SelectTrigger size="sm" aria-label="审批方式">
            <SelectValue />
          </SelectTrigger>
          <SelectContent alignItemWithTrigger={false} align="end">
            {Object.entries(APPROVAL_LABEL).map(([value, label]) => (
              <SelectItem key={value} value={value}>
                {label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
    </header>
  );
}

function RunStatus({ status }: { status: string | undefined }) {
  if (status !== "running" && status !== "waiting") return null;
  return (
    <Marker>
      <MarkerIcon>
        <Spinner />
      </MarkerIcon>
      <MarkerContent className="shimmer">{status === "waiting" ? "等你处理上面的请求" : "正在工作…"}</MarkerContent>
    </Marker>
  );
}

export function Conversation({ sessionId }: { sessionId: string }) {
  const view = useLila((state) => state.views[sessionId]) ?? emptyView();
  const status = useLila((state) => state.sessions[sessionId]?.status);
  const running = Boolean(view.runId) || status === "running" || status === "waiting";
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <SessionToolbar sessionId={sessionId} />
      <MessageScrollerProvider key={sessionId} autoScroll defaultScrollPosition="last-anchor" scrollPreviousItemPeek={48}>
        <MessageScroller className="min-h-0 flex-1">
          <MessageScrollerViewport>
            <MessageScrollerContent className="mx-auto w-full max-w-3xl gap-4 px-6 pt-6 pb-10">
              {view.loaded && view.items.length === 0 ? (
                <MessageScrollerItem messageId="empty">
                  <Empty className="py-24">
                    <EmptyHeader>
                      <EmptyTitle>要做点什么？</EmptyTitle>
                      <EmptyDescription>Agent 会在项目目录里读代码、改文件、跑命令，过程都会显示在这里。</EmptyDescription>
                    </EmptyHeader>
                  </Empty>
                </MessageScrollerItem>
              ) : null}
              {view.items.map((item) => (
                <MessageScrollerItem key={item.id} messageId={item.id} scrollAnchor={item.kind === "user"}>
                  <Item item={item} />
                </MessageScrollerItem>
              ))}
              {running && !view.pending.length ? (
                <MessageScrollerItem messageId="status">
                  <RunStatus status={status} />
                </MessageScrollerItem>
              ) : null}
            </MessageScrollerContent>
          </MessageScrollerViewport>
          <MessageScrollerButton />
        </MessageScroller>
      </MessageScrollerProvider>
      <div className="mx-auto flex w-full max-w-3xl shrink-0 flex-col gap-3 px-6 pb-5">
        <PendingPanel sessionId={sessionId} pending={view.pending} />
        <Composer sessionId={sessionId} running={running} />
      </div>
    </div>
  );
}
