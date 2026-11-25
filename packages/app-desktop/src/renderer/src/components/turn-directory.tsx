import * as React from "react";
import type { TranscriptItem } from "@lila/protocol";
import { Button } from "@/components/ui/button";
import { HoverCard, HoverCardContent, HoverCardTrigger } from "@/components/ui/hover-card";
import { useMessageScrollerVisibility } from "@/components/ui/message-scroller";
import { cn } from "cn";
import type { TranscriptBlock } from "@/lib/transcript-layout";

const PREVIEW_TICK_WIDTHS = [26, 19, 13, 9];

type TurnPreview = { itemId: string; number: number; prompt: string; reply?: string };

function handleTurnKeys(event: React.KeyboardEvent<HTMLElement>): void {
  if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
  const buttons = Array.from(event.currentTarget.querySelectorAll("button"));
  const index = buttons.indexOf(event.target as HTMLButtonElement);
  const target = event.key === "Home" ? buttons[0] : event.key === "End" ? buttons.at(-1)
    : buttons[index + (event.key === "ArrowDown" ? 1 : -1)];
  if (!target) return;
  event.preventDefault();
  target.focus();
}

export function TurnRail({ items, blocks, onJump }: {
  items: TranscriptItem[];
  blocks: TranscriptBlock[];
  onJump: (itemId: string) => Promise<boolean>;
}) {
  const { visibleMessageIds } = useMessageScrollerVisibility();
  const [previewOpen, setPreviewOpen] = React.useState(false);
  const railRef = React.useRef<HTMLDivElement>(null);
  const turns = React.useMemo(() => {
    const result: TurnPreview[] = [];
    for (const item of items) {
      if (item.kind === "user") {
        result.push({
          itemId: item.id,
          number: result.length + 1,
          prompt: item.text.trim().slice(0, 280) || (item.images.length ? "图片消息" : "空消息"),
        });
      } else if (item.kind === "assistant" && !item.streaming && item.text) {
        const turn = result.at(-1);
        if (turn) turn.reply = item.text.trim().slice(0, 280);
      }
    }
    return result;
  }, [items]);

  const blockToTurnId = React.useMemo(() => {
    const map = new Map<string, string>();
    let currentTurnId: string | undefined;
    for (const block of blocks) {
      if (block.kind === "item" && block.item.kind === "user") {
        currentTurnId = block.item.id;
      }
      if (currentTurnId) map.set(block.id, currentTurnId);
    }
    return map;
  }, [blocks]);

  let currentTurnId: string | undefined;
  for (const id of visibleMessageIds) {
    const turnId = blockToTurnId.get(id);
    if (turnId) {
      currentTurnId = turnId;
      break;
    }
  }

  const effectiveTurnId = currentTurnId ?? turns.at(-1)?.itemId;

  React.useEffect(() => {
    if (!effectiveTurnId || previewOpen) return;
    const activeButton = railRef.current?.querySelector<HTMLElement>(`[data-turn-anchor="${CSS.escape(effectiveTurnId)}"]`);
    activeButton?.scrollIntoView({ block: "nearest" });
  }, [effectiveTurnId, previewOpen]);

  return (
    <nav aria-label="轮次导航" className="pointer-events-none absolute inset-y-0 start-2 z-10 flex w-8 items-center">
      <HoverCard<TurnPreview> onOpenChange={setPreviewOpen}>
        {({ payload }) => {
          const hovered = previewOpen ? turns.findIndex((turn) => turn.itemId === payload?.itemId) : -1;
          return (
            <>
              <div
                ref={railRef}
                className="pointer-events-auto flex max-h-[min(18rem,55%)] flex-col overflow-y-auto py-1 [scrollbar-width:none]"
                onKeyDown={handleTurnKeys}
              >
                {turns.map((turn, index) => {
                  const current = turn.itemId === effectiveTurnId;
                  const previewWidth = hovered >= 0 ? PREVIEW_TICK_WIDTHS[Math.abs(hovered - index)] : undefined;
                  return (
                    <HoverCardTrigger
                      key={turn.itemId}
                      payload={turn}
                      delay={60}
                      closeDelay={75}
                      render={
                        <Button
                          type="button"
                          size="icon-xs"
                          variant="ghost"
                          className="h-3.5 w-8 shrink-0 justify-start rounded px-0.5 hover:bg-transparent focus-visible:ring-0 focus-visible:inset-ring-1 focus-visible:inset-ring-ring/60"
                          data-turn-anchor={turn.itemId}
                          aria-label={`第 ${turn.number} 轮：${turn.prompt.replace(/\s+/g, " ")}`}
                          aria-current={current ? "location" : undefined}
                          onClick={() => void onJump(turn.itemId)}
                        />
                      }
                    >
                      <span
                        aria-hidden
                        className={cn(
                          "h-0.5 rounded-full bg-muted-foreground/45 transition-[width,background-color] duration-150 motion-reduce:transition-none",
                          current && "bg-info",
                          hovered === index && "bg-foreground",
                        )}
                        style={{ width: Math.max(current ? 13 : 7, previewWidth ?? 0) }}
                      />
                    </HoverCardTrigger>
                  );
                })}
              </div>
              {payload ? (
                <HoverCardContent side="right" alignOffset={0} sideOffset={5} className="w-[min(18rem,calc(100vw-4rem))] space-y-1">
                  <div className="flex items-center justify-between gap-2 text-[11px] text-muted-foreground tabular-nums">
                    <span>第 {payload.number} 轮</span>
                    <span>共 {turns.length} 轮</span>
                  </div>
                  <p className="truncate text-xs font-semibold">{payload.prompt}</p>
                  {payload.reply ? <p className="line-clamp-3 wrap-anywhere text-xs leading-relaxed text-muted-foreground">{payload.reply}</p> : null}
                </HoverCardContent>
              ) : null}
            </>
          );
        }}
      </HoverCard>
    </nav>
  );
}
