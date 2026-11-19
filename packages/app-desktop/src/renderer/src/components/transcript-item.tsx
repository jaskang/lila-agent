import * as React from "react";
import { Brain, Check, CircleAlert, Copy, Info, PencilLine, Puzzle, TriangleAlert } from "lucide-react";
import type { ImageInput, TranscriptItem } from "@lila/protocol";
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
  TraceIcon,
  TraceLine,
  TraceTitle,
} from "@/components/ui/trace";
import { actions } from "@/lib/store";

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
  return (
    <Message align="end">
      <MessageContent>
        {item.images.length ? (
          <div className="flex flex-wrap justify-end gap-2">
            {item.images.map((image, index) => (
              <button
                key={index}
                type="button"
                aria-label={`放大查看图片 ${index + 1}`}
                onClick={() => setPreview({ image, index })}
                className="cursor-zoom-in overflow-hidden rounded-lg border transition-opacity hover:opacity-90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              >
                <img
                  src={`data:${image.mimeType};base64,${image.data}`}
                  alt={`图片 ${index + 1}`}
                  className="size-24 object-cover"
                />
              </button>
            ))}
          </div>
        ) : null}
        {item.text ? (
          <Bubble align="end" variant="muted">
            <BubbleContent className="text-sm whitespace-pre-wrap">{item.text}</BubbleContent>
          </Bubble>
        ) : null}
        <MessageFooter className="gap-0.5 px-0 opacity-0 transition-opacity group-hover/message:opacity-100 focus-within:opacity-100">
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
            <AlertDialogDescription>当前分支中这条消息之后的回复和工具结果将被移出，这条消息的文本会放回输入框。</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>取消</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                setConfirmRewind(false);
                void actions.rewind(sessionId, item.id).then(focusComposer);
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

function AssistantMessage({ item }: { item: Extract<TranscriptItem, { kind: "assistant" }> }) {
  const thinkingNow = item.streaming && !item.text;
  const showFooter = !item.streaming && Boolean(item.text || item.model);
  return (
    <Message>
      <MessageContent>
        {item.thinking ? (
          <Trace>
            <TraceCollapsible>
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
          <MessageFooter className="items-center gap-1.5 px-0 opacity-0 transition-opacity group-hover/message:opacity-100 focus-within:opacity-100">
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

export const TranscriptEntry = React.memo(function TranscriptEntry({
  item,
  sessionId,
  running,
}: {
  item: TranscriptItem;
  sessionId: string;
  running: boolean;
}) {
  switch (item.kind) {
    case "user":
      return <UserMessage item={item} sessionId={sessionId} running={running} />;
    case "assistant":
      return <AssistantMessage item={item} />;
    case "tool":
      return <ToolCall tool={item} sessionId={sessionId} />;
    case "custom":
      return (
        <div className="rounded-xl border border-dashed bg-muted/30 px-4 py-3">
          <div className="mb-1 flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
            <Puzzle className="size-3.5" />
            {item.customType}
          </div>
          <Markdown text={item.text} />
        </div>
      );
    case "notice": {
      const Icon = NOTICE_ICON[item.tone];
      return (
        <Marker className={item.tone === "error" ? "text-destructive" : item.tone === "warning" ? "text-amber-700 dark:text-amber-400" : undefined}>
          <MarkerIcon>
            <Icon />
          </MarkerIcon>
          <MarkerContent className="whitespace-pre-wrap">{item.text}</MarkerContent>
        </Marker>
      );
    }
  }
});
