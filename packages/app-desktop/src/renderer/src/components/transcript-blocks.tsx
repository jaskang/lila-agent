import { AgentGroup } from "@/components/agent-card";
import { ReplyChanges, type ChangeSelection } from "@/components/reply-changes";
import { TranscriptActivity } from "@/components/transcript-activity";
import { TranscriptEntry } from "@/components/transcript-item";
import { MessageScrollerItem } from "@/components/ui/message-scroller";
import { cn } from "cn";
import type { TranscriptBlock } from "@/lib/transcript-layout";

export function TranscriptBlocks({ blocks, sessionId, running, revealId, onReview }: {
  blocks: TranscriptBlock[];
  sessionId: string;
  running: boolean;
  revealId?: string | undefined;
  onReview: (selection: ChangeSelection) => void;
}) {
  return blocks.map((block) => {
    if (block.kind === "activity") {
      return (
        <MessageScrollerItem key={block.id} messageId={block.id}>
          <TranscriptActivity items={block.items} sessionId={sessionId} revealId={revealId} />
        </MessageScrollerItem>
      );
    }
    if (block.kind === "agents") {
      return (
        <MessageScrollerItem key={block.id} messageId={block.id}>
          <AgentGroup tools={block.tools} sessionId={sessionId} revealId={revealId} />
        </MessageScrollerItem>
      );
    }
    if (block.kind === "changes") {
      return (
        <MessageScrollerItem key={block.id} messageId={block.id}>
          <ReplyChanges changes={block.changes} onOpen={onReview} />
        </MessageScrollerItem>
      );
    }
    const user = block.item.kind === "user";
    const matched = !user && block.id === revealId;
    return (
      <MessageScrollerItem
        key={block.id}
        messageId={block.id}
        data-transcript-item={block.id}
        tabIndex={user ? -1 : undefined}
        className={cn(user && "group/transcript-item mt-6 outline-none first:mt-0", matched && "rounded-lg bg-accent/60 ring-1 ring-ring/25")}
      >
        <TranscriptEntry
          item={block.item}
          sessionId={sessionId}
          reveal={matched}
          running={user && running}
          showFooter={block.showFooter}
          turnChanges={block.turnChanges}
          onReview={onReview}
        />
      </MessageScrollerItem>
    );
  });
}
