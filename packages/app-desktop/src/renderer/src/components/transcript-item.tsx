import * as React from "react";
import { Brain, Check, CheckCircle2, CircleAlert, CircleStop, Copy, Info, PencilLine, Puzzle, TriangleAlert } from "lucide-react";
import type { ImageInput, SubagentNotification, TranscriptItem } from "@lila/protocol";
import { SubSessionLink } from "@/components/agent-card";
import { focusComposer } from "@/components/composer";
import { Markdown } from "@/components/markdown";
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
import { formatCompactTokens, formatDuration, isSubagentErrorStatus } from "@/lib/format";
import { actions, useLila } from "@/lib/store";

const NOTICE_ICON = { info: Info, warning: TriangleAlert, error: CircleAlert } as const;

function IconAction({ label, disabled, onClick, children }: { label: string; disabled?: boolean; onClick: () => void; children: React.ReactNode }) {
  return (
    <Tooltip>
      <TooltipTrigger render={<Button size="icon-xs" variant="ghost" aria-label={label} disabled={disabled} onClick={onClick} />}>{children}</TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

function shortModelName(modelId: string): string {
  const slash = modelId.indexOf("/");
  return slash >= 0 ? modelId.slice(slash + 1) : modelId;
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
                className="cursor-zoom-in overflow-hidden rounded-lg border transition-opacity hover:opacity-90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring group-focus-visible/transcript-item:outline-2 group-focus-visible/transcript-item:-outline-offset-2 group-focus-visible/transcript-item:outline-ring"
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
          <Bubble align="end" variant="muted">
            <BubbleContent className="text-sm whitespace-pre-wrap group-focus-visible/transcript-item:inset-ring-1 group-focus-visible/transcript-item:inset-ring-ring">
              {item.text}
            </BubbleContent>
          </Bubble>
        ) : null}
        {item.text || item.canRewind ? (
          <MessageFooter className="pointer-events-none gap-0.5 px-1 pb-1 opacity-0 transition-opacity group-hover/message:pointer-events-auto group-hover/message:opacity-100 group-focus-within/message:pointer-events-auto group-focus-within/message:opacity-100">
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

function AssistantMessage({ item, reveal }: { item: Extract<TranscriptItem, { kind: "assistant" }>; reveal: boolean }) {
  const [reasoningOpen, setReasoningOpen] = React.useState(false);
  const thinkingNow = item.streaming && !item.text;
  const showFooter = !item.streaming && Boolean(item.text);
  return (
    <Message>
      <MessageContent className="gap-1">
        {item.thinking ? (
          <Trace>
            <TraceCollapsible open={reasoningOpen || reveal} onOpenChange={setReasoningOpen}>
              <TraceCollapsibleTrigger>
                <TraceLine {...(thinkingNow ? { status: "running" as const } : {})}>
                  <TraceIcon>
                    <Brain />
                  </TraceIcon>
                  <TraceTitle>{thinkingNow ? "思考中" : "思考过程"}</TraceTitle>
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
        {showFooter ? (
          <MessageFooter className="pointer-events-none w-fit gap-1.5 px-1 pb-1 opacity-0 transition-opacity group-hover/message:pointer-events-auto group-hover/message:opacity-100 group-focus-within/message:pointer-events-auto group-focus-within/message:opacity-100">
            {item.text ? <CopyAction text={item.text} /> : null}
            {item.model ? (
              <span className="truncate font-mono text-[11px] text-muted-foreground" title={item.model}>
                {shortModelName(item.model)}
              </span>
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
  const totalTokens = entries.reduce((sum, entry) => sum + (entry.totalTokens ?? 0), 0);
  const totalCost = entries.reduce((sum, entry) => sum + (entry.totalCost ?? 0), 0);
  const headerMeta = single
    ? [
        single.toolUses ? `${single.toolUses} 次工具` : undefined,
        single.totalTokens ? formatCompactTokens(single.totalTokens) : undefined,
        formatDuration(single.durationMs),
      ].filter(Boolean)
    : [
        totalTokens > 0 ? formatCompactTokens(totalTokens) : undefined,
        totalCost > 0 ? `$${totalCost.toFixed(4)}` : undefined,
      ].filter(Boolean);

  return (
    <Trace>
      <TraceCollapsible open={open || reveal} onOpenChange={setOpen}>
        <TraceCollapsibleTrigger>
          <TraceLine {...(hasError ? { status: "error" as const } : {})}>
            <TraceIcon>
              {hasError ? <CircleAlert /> : <CheckCircle2 className="text-success-foreground" />}
            </TraceIcon>
            <TraceTitle>
              {single ? (hasError ? "子 Agent 出错" : "子 Agent 完成") : `${entries.length} 个子 Agent 完成`}
            </TraceTitle>
            {single ? <TraceDescription className="max-w-[22rem] shrink truncate">{single.description}</TraceDescription> : null}
            {headerMeta.length ? <TraceDescription className="text-xs tabular-nums">{headerMeta.join(" · ")}</TraceDescription> : null}
          </TraceLine>
        </TraceCollapsibleTrigger>
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
                    <NotificationSubLink sessionId={sessionId} entry={entry} />
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
}: {
  item: TranscriptItem;
  sessionId: string;
  running: boolean;
  reveal?: boolean;
}) {
  switch (item.kind) {
    case "user":
      return <UserMessage item={item} sessionId={sessionId} running={running} />;
    case "assistant":
      return <AssistantMessage item={item} reveal={reveal} />;
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
      const Icon = item.status === "completed" ? CheckCircle2 : item.status === "aborted" ? CircleStop : CircleAlert;
      const label = { completed: "本轮已完成", aborted: "本轮已停止", error: "本轮出错", incomplete: "本轮未完成" }[item.status];
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
