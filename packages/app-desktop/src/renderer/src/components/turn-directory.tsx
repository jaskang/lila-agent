import * as React from "react";
import type { TranscriptItem } from "@lila/protocol";
import { Button } from "@/components/ui/button";
import { HoverCard, HoverCardContent, HoverCardTrigger } from "@/components/ui/hover-card";
import { useMessageScrollerVisibility } from "@/components/ui/message-scroller";
import { cn } from "cn";

const RAIL_SIZE = 9;
const PREVIEW_TICK_WIDTHS = [26, 19, 13, 9];

type TurnPreview = { itemId: string; prompt: string; reply?: string };

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

export function TurnRail({ items, onJump }: {
  items: TranscriptItem[];
  onJump: (itemId: string) => Promise<boolean>;
}) {
  const { currentAnchorId } = useMessageScrollerVisibility();
  const [previewOpen, setPreviewOpen] = React.useState(false);
  const [pointerStart, setPointerStart] = React.useState<number>();
  const turns = React.useMemo(() => {
    const result: TurnPreview[] = [];
    for (const item of items) {
      if (item.kind === "user") {
        result.push({ itemId: item.id, prompt: item.text.trim().slice(0, 280) || (item.images.length ? "图片消息" : "空消息") });
      } else if (item.kind === "assistant" && !item.streaming && item.text) {
        const turn = result.at(-1);
        if (turn) turn.reply = item.text.trim().slice(0, 280);
      }
    }
    return result;
  }, [items]);

  const active = turns.findIndex((turn) => turn.itemId === currentAnchorId);
  const center = active >= 0 ? active : turns.length - 1;
  const start = Math.max(0, Math.min(pointerStart ?? center - Math.floor(RAIL_SIZE / 2), turns.length - RAIL_SIZE));
  const visible = turns.slice(start, start + RAIL_SIZE);
  return (
    <nav aria-label="轮次导航" className="pointer-events-none absolute inset-y-0 start-2 z-10 flex w-8 items-center">
      <HoverCard<TurnPreview> onOpenChange={setPreviewOpen}>
        {({ payload }) => {
          const hovered = previewOpen ? visible.findIndex((turn) => turn.itemId === payload?.itemId) : -1;
          return (
            <>
              <div
                className="pointer-events-auto flex max-h-[calc(100%_-_1rem)] flex-col overflow-y-auto [scrollbar-width:none]"
                onKeyDown={handleTurnKeys}
                onPointerEnter={() => setPointerStart(start)}
                onPointerLeave={() => setPointerStart(undefined)}
              >
                {visible.map((turn, index) => {
                  const current = turn.itemId === currentAnchorId;
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
                          className="h-3.5 w-8 justify-start rounded px-0.5 hover:bg-transparent focus-visible:ring-0 focus-visible:inset-ring-1 focus-visible:inset-ring-ring/60"
                          data-turn-anchor={turn.itemId}
                          aria-label={`跳转到消息：${turn.prompt.replace(/\s+/g, " ")}`}
                          aria-current={current ? "location" : undefined}
                          onClick={() => void onJump(turn.itemId)}
                        />
                      }
                    >
                      <span
                        aria-hidden
                        className={cn(
                          "h-0.5 rounded-full bg-muted-foreground/45 transition-[width,background-color] duration-150 motion-reduce:transition-none",
                          current && "bg-sky-600 dark:bg-sky-400",
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
