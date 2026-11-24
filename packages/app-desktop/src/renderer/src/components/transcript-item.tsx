import * as React from "react";
import { Brain, Check, CheckCircle2, CircleAlert, CircleStop, Copy, Info, PencilLine, Puzzle, TriangleAlert } from "lucide-react";
import type { ImageInput, SubagentNotification, TranscriptItem } from "@lila/protocol";
import { SubSessionLink } from "@/components/agent-card";
import { focusComposer } from "@/components/composer";
import { Markdown } from "@/components/markdown";
import { ReplyChanges, type ChangeSelection } from "@/components/reply-changes";
import { ToolCall } from "@/components/tool-call";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Bubble, BubbleContent } from "@/components/ui/bubble";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Marker, MarkerContent, MarkerIcon } from "@/components/ui/marker";
import { Message, MessageContent, MessageFooter } from "@/components/ui/message";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
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
import { formatCompactTokens, formatDuration, formatElapsed, formatMessageTime, isSubagentErrorStatus } from "@/lib/format";
import { actions, useLila } from "@/lib/store";
import type { FileChange } from "@/lib/transcript-layout";

const NOTICE_ICON = { info: Info, warning: TriangleAlert, error: CircleAlert } as const;

function IconAction({ label, disabled, onClick, children }: { label: string; disabled?: boolean; onClick: () => void; children: React.ReactNode }) {
  return (
    <Tooltip>
      <TooltipTrigger render={<Button size="icon-xs" variant="ghost" aria-label={label} disabled={disabled} onClick={onClick} />}>{children}</TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

function resolveAssistantTimestamp(item: Extract<TranscriptItem, { kind: "assistant" }>): number | undefined {
  if (typeof item.timestamp === "number" && Number.isFinite(item.timestamp) && item.timestamp > 0) return item.timestamp;
  const match = /^a:(\d+)(?::\d+)?$/.exec(item.id);
  if (!match) return undefined;
  const parsed = Number(match[1]);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : undefined;
}

function CopyAction({ text }: { text: string }) {
  const [copied, setCopied] = React.useState(false);
  return (
    <IconAction
      label={copied ? "已复制" : "复制"}
      onClick={() => {
        void navigator.clipboard.writeText(text).then(() => {
          setCopied(true);
          setTimeout(() => setCopied(false), 1_500);
        });
      }}
    >
      {copied ? <Check /> : <Copy />}
    </IconAction>
  );
}

function UserMessage({ item, sessionId, running }: { item: Extract<TranscriptItem, { kind: "user" }>; sessionId: string; running: boolean }) {
  const [preview, setPreview] = React.useState<{ image: ImageInput; index: number } | null>(null);
  const [confirmRewind, setConfirmRewind] = React.useState(false);
  const hasExistingDraft = React.useMemo(() => {
    if (!confirmRewind) return false;
    const state = useLila.getState();
    return Boolean((state.drafts[sessionId] ?? "").trim() || (state.draftImages[sessionId]?.length ?? 0) > 0);
  }, [confirmRewind, sessionId]);

  return (
    <Message align="end">
      <MessageContent className="gap-1">
        {item.images.length ? (
          <div className="flex flex-wrap justify-end gap-2">
            {item.images.map((image, index) => (
              <button
                key={index}
                type="button"
                aria-label={`放大查看图片 ${index + 1}`}
                onClick={() => setPreview({ image, index })}
                className="cursor-zoom-in overflow-hidden rounded-xl border transition-opacity hover:opacity-90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring group-focus-visible/transcript-item:outline-2 group-focus-visible/transcript-item:-outline-offset-2 group-focus-visible/transcript-item:outline-ring"
              >
                <img
                  src={`data:${image.mimeType};base64,${image.data}`}
                  alt={`图片 ${index + 1}`}
                  className="size-24 max-w-full object-cover"
                />
              </button>
            ))}
          </div>
        ) : null}
        {item.text ? (
          <Bubble align="end" variant="muted" className="max-w-[82%]">
            <BubbleContent className="rounded-[18px] px-4 py-2.5 text-[14.5px] leading-relaxed whitespace-pre-wrap group-focus-visible/transcript-item:inset-ring-1 group-focus-visible/transcript-item:inset-ring-ring">
              {item.text}
            </BubbleContent>
          </Bubble>
        ) : null}
        {item.text || item.canRewind ? (
          <MessageFooter className="gap-0.5 px-0 opacity-0 transition-opacity group-hover/message:opacity-100 group-focus-within/message:opacity-100">
            {item.text ? <CopyAction text={item.text} /> : null}
            {item.canRewind ? (
              <IconAction
                label={running ? "任务进行中，不能编辑重发" : "编辑重发"}
                disabled={running}
                onClick={() => setConfirmRewind(true)}
              >
                <PencilLine />
              </IconAction>
            ) : null}
          </MessageFooter>
        ) : null}
      </MessageContent>
      <Dialog open={preview !== null} onOpenChange={(open) => !open && setPreview(null)}>
        <DialogContent className="w-fit max-w-[92vw] p-2 sm:max-w-[92vw]">
          <DialogTitle className="sr-only">{preview ? `图片 ${preview.index + 1}` : "图片预览"}</DialogTitle>
          {preview ? (
            <img
              src={`data:${preview.image.mimeType};base64,${preview.image.data}`}
              alt={`图片 ${preview.index + 1}`}
              className="max-h-[85vh] max-w-[88vw] rounded-lg object-contain"
            />
          ) : null}
        </DialogContent>
      </Dialog>
      <AlertDialog open={confirmRewind} onOpenChange={setConfirmRewind}>
        <AlertDialogContent size="sm">
          <AlertDialogHeader>
            <AlertDialogTitle>编辑并重发这条消息？</AlertDialogTitle>
            <AlertDialogDescription>
              当前分支中这条消息之后的回复和工具结果将被移出，这条消息的{item.images.length ? "文本和图片" : "文本"}会放回输入框。
              {hasExistingDraft ? "输入框中未发送的草稿将被替换。" : ""}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>取消</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                setConfirmRewind(false);
                void actions.rewind(sessionId, item.id, item.images).then((ok) => {
                  if (ok) focusComposer();
                });
              }}
            >
              继续编辑
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Message>
  );
}

function AssistantMessage({
  item,
  reveal,
  showFooter = true,
  turnChanges,
  onReview,
}: {
  item: Extract<TranscriptItem, { kind: "assistant" }>;
  reveal: boolean;
  showFooter?: boolean | undefined;
  turnChanges?: FileChange[] | undefined;
  onReview?: ((selection: ChangeSelection) => void) | undefined;
}) {
  const [reasoningOpen, setReasoningOpen] = React.useState<boolean | undefined>(undefined);
  const thinkingNow = item.streaming && !item.text;
  const thinkingStartedAt = React.useRef<number | null>(null);
  const [thoughtMs, setThoughtMs] = React.useState<number | undefined>(undefined);
  const [now, setNow] = React.useState(Date.now);

  React.useEffect(() => {
    if (thinkingNow && item.thinking) {
      if (thinkingStartedAt.current === null) {
        thinkingStartedAt.current = Date.now();
        setNow(thinkingStartedAt.current);
      }
      const timer = window.setInterval(() => setNow(Date.now()), 1_000);
      return () => window.clearInterval(timer);
    }
    if (!thinkingNow && thinkingStartedAt.current !== null && thoughtMs === undefined) {
      setThoughtMs(Math.max(1_000, Date.now() - thinkingStartedAt.current));
    }
  }, [thinkingNow, item.thinking, thoughtMs]);

  const thinkingLabel = thinkingNow
    ? thinkingStartedAt.current !== null
      ? `思考中 · ${formatElapsed(thinkingStartedAt.current, now)}`
      : "思考中"
    : thoughtMs !== undefined
      ? `思考了 ${formatDuration(thoughtMs)}`
      : "思考过程";

  const renderFooter = showFooter && !item.streaming && Boolean(item.text);
  const time = renderFooter ? formatMessageTime(resolveAssistantTimestamp(item)) : undefined;
  return (
    <Message>
      <MessageContent className="gap-1">
        {item.thinking ? (
          <Trace>
            <TraceCollapsible open={reveal || (reasoningOpen ?? thinkingNow)} onOpenChange={setReasoningOpen}>
              <TraceCollapsibleTrigger>
                <TraceLine className="text-[13px]" {...(thinkingNow ? { status: "running" as const } : {})}>
                  <TraceIcon>
                    <Brain />
                  </TraceIcon>
                  <TraceTitle>{thinkingLabel}</TraceTitle>
                </TraceLine>
              </TraceCollapsibleTrigger>
              <TraceCollapsibleContent>
                <Markdown text={item.thinking} streaming={item.streaming} />
              </TraceCollapsibleContent>
            </TraceCollapsible>
          </Trace>
        ) : null}
        {item.text ? <Markdown text={item.text} streaming={item.streaming} /> : null}
        {item.error ? (
          <Marker className="text-destructive">
            <MarkerIcon>
              <CircleAlert />
            </MarkerIcon>
            <MarkerContent className="whitespace-pre-wrap">{item.error}</MarkerContent>
          </Marker>
        ) : null}
        {turnChanges?.length && onReview ? <ReplyChanges changes={turnChanges} onOpen={onReview} /> : null}
        {renderFooter ? (
          <MessageFooter className="-ms-1 w-fit gap-1.5 px-0 pt-0.5 text-muted-foreground">
            {item.text ? <CopyAction text={item.text} /> : null}
            {time ? (
              <time className="text-xs font-normal tabular-nums" dateTime={time.iso} title={time.title}>
                {time.label}
              </time>
            ) : null}
          </MessageFooter>
        ) : null}
      </MessageContent>
    </Message>
  );
}

function NotificationSubLink({ sessionId, entry }: { sessionId: string; entry: SubagentNotification }) {
  return <SubSessionLink sessionId={sessionId} agentId={entry.id} description={entry.description} className="ms-auto" />;
}

function SubagentNotificationCard({
  item,
  sessionId,
  reveal,
}: {
  item: Extract<TranscriptItem, { kind: "custom" }>;
  sessionId: string;
  reveal: boolean;
}) {
  const entries = item.notifications ?? [];
  const hasError = entries.some((entry) => isSubagentErrorStatus(entry.status));
  const [open, setOpen] = React.useState(hasError);

  if (!entries.length) {
    if (!item.text) return null;
    return (
      <div className="rounded-xl border border-dashed bg-muted/30 px-4 py-3">
        <Markdown text={item.text} />
      </div>
    );
  }

  const single = entries.length === 1 ? entries[0] : undefined;
  const headerMeta = single
    ? [hasError ? "出错" : "已完成", formatDuration(single.durationMs)].filter(Boolean)
    : [];

  return (
    <Trace>
      <TraceCollapsible open={open || reveal} onOpenChange={setOpen}>
        <div className="flex min-w-0 items-center gap-1">
          <TraceCollapsibleTrigger className="py-0.5">
            <TraceLine className="text-[13px]" {...(hasError ? { status: "error" as const } : {})}>
              <TraceIcon>
                {hasError ? <CircleAlert /> : <CheckCircle2 className="text-success-foreground" />}
              </TraceIcon>
              <TraceTitle className="max-w-[26rem] shrink truncate font-medium text-foreground">
                {single ? single.description || (hasError ? "子 Agent 出错" : "子 Agent 已完成") : `${entries.length} 个子 Agent 已完成`}
              </TraceTitle>
              {headerMeta.length ? <TraceDescription className="text-xs tabular-nums">{headerMeta.join(" · ")}</TraceDescription> : null}
            </TraceLine>
          </TraceCollapsibleTrigger>
          {single ? <SubSessionLink sessionId={sessionId} agentId={single.id} description={single.description} /> : null}
        </div>
        <TraceCollapsibleContent>
          <div className="flex flex-col gap-2 text-foreground">
            {entries.map((entry, index) => {
              const err = isSubagentErrorStatus(entry.status);
              const meta = [
                entry.turnCount ? `${entry.turnCount} 轮` : undefined,
                entry.toolUses ? `${entry.toolUses} 次工具` : undefined,
                entry.totalTokens ? formatCompactTokens(entry.totalTokens) : undefined,
                entry.totalCost ? `$${entry.totalCost.toFixed(4)}` : undefined,
                formatDuration(entry.durationMs),
              ].filter(Boolean);
              return (
                <div key={entry.id ?? index} className="flex flex-col gap-2 rounded-lg border bg-card px-3 py-2.5">
                  <div className="flex flex-wrap items-center gap-1.5 text-xs">
                    <Badge variant={err ? "destructive" : "secondary"}>{err ? "出错" : "已完成"}</Badge>
                    <span className="font-medium">{entry.description}</span>
                    {meta.length ? <span className="text-muted-foreground tabular-nums">· {meta.join(" · ")}</span> : null}
                    {single ? null : <NotificationSubLink sessionId={sessionId} entry={entry} />}
                  </div>
                  {entry.error ? <p className="text-sm text-destructive">{entry.error}</p> : null}
                  {entry.preview ? <Markdown text={entry.preview} /> : null}
                </div>
              );
            })}
          </div>
        </TraceCollapsibleContent>
      </TraceCollapsible>
    </Trace>
  );
}

export const TranscriptEntry = React.memo(function TranscriptEntry({
  item,
  sessionId,
  running,
  reveal = false,
  showFooter = true,
  turnChanges,
  onReview,
}: {
  item: TranscriptItem;
  sessionId: string;
  running: boolean;
  reveal?: boolean;
  showFooter?: boolean | undefined;
  turnChanges?: FileChange[] | undefined;
  onReview?: ((selection: ChangeSelection) => void) | undefined;
}) {
  switch (item.kind) {
    case "user":
      return <UserMessage item={item} sessionId={sessionId} running={running} />;
    case "assistant":
      return <AssistantMessage item={item} reveal={reveal} showFooter={showFooter} turnChanges={turnChanges} onReview={onReview} />;
    case "tool":
      return <ToolCall tool={item} sessionId={sessionId} reveal={reveal} />;
    case "custom":
      if (item.notifications !== undefined) return <SubagentNotificationCard item={item} sessionId={sessionId} reveal={reveal} />;
      return (
        <div className="rounded-xl border border-dashed bg-muted/30 px-4 py-3">
          <div className="mb-1 flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
            <Puzzle className="size-3.5" />
            {item.customType}
          </div>
          <Markdown text={item.text} />
        </div>
      );
    case "run": {
      if (item.status === "completed") return null;
      const Icon = item.status === "aborted" ? CircleStop : CircleAlert;
      const label = { aborted: "本轮已停止", error: "本轮出错", incomplete: "本轮未完成" }[item.status];
      const duration = formatDuration(item.durationMs);
      return (
        <div data-run-status={item.status} className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <Icon aria-hidden className="size-3.5 shrink-0" />
          <span>{label}</span>
          {duration ? <span className="tabular-nums">· 用时 {duration}</span> : null}
        </div>
      );
    }
    case "notice": {
      const Icon = NOTICE_ICON[item.tone];
      return (
        <Marker className={item.tone === "error" ? "text-destructive-foreground" : item.tone === "warning" ? "text-warning-foreground" : undefined}>
          <MarkerIcon>
            <Icon />
          </MarkerIcon>
          <MarkerContent className="whitespace-pre-wrap">{item.text}</MarkerContent>
        </Marker>
      );
    }
  }
});
