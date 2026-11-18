import * as React from "react";
import { Brain, Check, CircleAlert, Copy, Info, PencilLine, Puzzle, TriangleAlert } from "lucide-react";
import type { TranscriptItem } from "@lila/protocol";
import { focusComposer } from "@/components/composer";
import { Markdown } from "@/components/markdown";
import { ToolCall } from "@/components/tool-call";
import { Bubble, BubbleContent } from "@/components/ui/bubble";
import { Button } from "@/components/ui/button";
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

function UserMessage({ item, sessionId, running }: { item: Extract<TranscriptItem, { kind: "user" }>; sessionId: string; running: boolean }) {
  const [copied, setCopied] = React.useState(false);
  return (
    <Message align="end">
      <MessageContent>
        {item.images.length ? (
          <div className="flex flex-wrap justify-end gap-2">
            {item.images.map((image, index) => (
              <img
                key={index}
                src={`data:${image.mimeType};base64,${image.data}`}
                alt={`图片 ${index + 1}`}
                className="size-24 rounded-lg border object-cover"
              />
            ))}
          </div>
        ) : null}
        {item.text ? (
          <Bubble align="end" variant="muted">
            <BubbleContent className="text-sm whitespace-pre-wrap">{item.text}</BubbleContent>
          </Bubble>
        ) : null}
        <MessageFooter className="gap-0.5 px-0 opacity-0 transition-opacity group-hover/message:opacity-100 focus-within:opacity-100">
          <IconAction
            label={copied ? "已复制" : "复制"}
            onClick={() => {
              void navigator.clipboard.writeText(item.text).then(() => {
                setCopied(true);
                setTimeout(() => setCopied(false), 1_500);
              });
            }}
          >
            {copied ? <Check /> : <Copy />}
          </IconAction>
          {item.canRewind ? (
            <IconAction
              label={running ? "任务进行中，不能编辑重发" : "编辑重发"}
              disabled={running}
              onClick={() => void actions.rewind(sessionId, item.id).then(focusComposer)}
            >
              <PencilLine />
            </IconAction>
          ) : null}
        </MessageFooter>
      </MessageContent>
    </Message>
  );
}

function AssistantMessage({ item }: { item: Extract<TranscriptItem, { kind: "assistant" }> }) {
  const thinkingNow = item.streaming && !item.text;
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
      return <ToolCall tool={item} />;
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
