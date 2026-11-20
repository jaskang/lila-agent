import * as React from "react";
import { ChevronDown, ChevronUp, Search, X } from "lucide-react";
import { TRANSCRIPT_SEARCH_PAGE_SIZE, type TranscriptSearchMatch, type TranscriptSearchResults } from "@lila/protocol";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from "@/components/ui/input-group";
import { actions } from "@/lib/store";

const MATCH_LABEL: Record<TranscriptSearchMatch["kind"], string> = {
  user: "你的消息",
  assistant: "助手消息",
  tool: "工具",
  custom: "扩展",
  notice: "提示",
  run: "轮次状态",
};

export function ConversationSearch({
  sessionId,
  connected,
  onHit,
  onClear,
  onClose,
  focusRequest,
}: {
  sessionId: string;
  connected: boolean;
  onHit: (hit: TranscriptSearchMatch, query: string) => Promise<boolean>;
  onClear: () => void;
  onClose: () => void;
  focusRequest: number;
}) {
  const [text, setText] = React.useState("");
  const [index, setIndex] = React.useState(0);
  const [result, setResult] = React.useState<(TranscriptSearchResults & { query: string; offset: number })>();
  const [failed, setFailed] = React.useState(false);
  const [retry, setRetry] = React.useState(0);
  const [missing, setMissing] = React.useState(false);
  const input = React.useRef<HTMLInputElement>(null);
  const query = text.trim();
  const offset = Math.floor(index / TRANSCRIPT_SEARCH_PAGE_SIZE) * TRANSCRIPT_SEARCH_PAGE_SIZE;
  const total = result?.query === query ? result.total : undefined;
  const hit = result?.query === query && result.offset === offset ? result.matches[index - offset] : undefined;

  React.useLayoutEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, [focusRequest]);

  React.useEffect(() => {
    if (!connected || !query) return;
    let cancelled = false;
    const timer = window.setTimeout(() => {
      void actions.searchTranscript(sessionId, query, offset).then(
        (data) => {
          if (cancelled) return;
          setResult({ ...data, query, offset });
          if (offset && offset >= data.total) setIndex(Math.max(0, data.total - 1));
        },
        () => { if (!cancelled) setFailed(true); },
      );
    }, 140);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [sessionId, connected, query, offset, retry]);

  React.useEffect(() => {
    if (!hit) return;
    let cancelled = false;
    void onHit(hit, query).then((found) => { if (!cancelled) setMissing(!found); });
    return () => { cancelled = true; };
  }, [hit, onHit, query]);

  function move(direction: number) {
    if (!total) return;
    setFailed(false);
    setIndex((current) => (current + direction + total) % total);
  }

  const count = !query ? "" : !connected ? "等待连接" : failed ? "搜索失败，按 Enter 重试" :
    total === undefined ? "查找中…" : total ? `${index + 1} / ${total}` : "没有匹配";

  return (
    <div role="search" aria-label="查找会话内容" className="mx-auto w-full max-w-3xl px-5 pt-2">
      <InputGroup className="min-h-8 bg-background has-disabled:bg-background has-disabled:opacity-100 dark:has-disabled:bg-background">
        <InputGroupAddon><Search aria-hidden /></InputGroupAddon>
        <InputGroupInput
          ref={input}
          aria-label="查找会话内容"
          placeholder="查找消息、文件或工具输出…"
          value={text}
          maxLength={500}
          onChange={(event) => {
            onClear();
            setMissing(false);
            setFailed(false);
            setIndex(0);
            setText(event.target.value);
          }}
          onKeyDown={(event) => {
            if (event.nativeEvent.isComposing) return;
            if (event.key === "Enter") {
              event.preventDefault();
              if (failed) {
                setFailed(false);
                setRetry((current) => current + 1);
              } else {
                move(event.shiftKey ? -1 : 1);
              }
            }
          }}
        />
        <InputGroupAddon align="inline-end" className="gap-1">
          <span role="status" className="whitespace-nowrap text-xs tabular-nums">{count}</span>
          <InputGroupButton aria-label="上一条匹配" title="上一条（Shift+Enter）" size="icon-xs" disabled={!total} onClick={() => move(-1)}><ChevronUp /></InputGroupButton>
          <InputGroupButton aria-label="下一条匹配" title="下一条（Enter）" size="icon-xs" disabled={!total} onClick={() => move(1)}><ChevronDown /></InputGroupButton>
          <InputGroupButton aria-label="关闭会话查找" title="关闭（Esc）" size="icon-xs" onClick={onClose}><X /></InputGroupButton>
        </InputGroupAddon>
      </InputGroup>
      {missing ? <p role="status" className="px-2 py-1 text-xs text-muted-foreground">这条记录已变更。请重新搜索。</p> :
        hit ? <p className="truncate px-2 py-1 text-xs text-muted-foreground" title={hit.excerpt}>{MATCH_LABEL[hit.kind]} · {hit.excerpt}</p> : null}
    </div>
  );
}
