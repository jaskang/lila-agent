import type { TranscriptItem } from "@lila/protocol";
import type { PushMessage } from "../client.ts";
import { MAX_CACHED_VIEWS, MAX_SCROLL_POSITIONS, type SessionView } from "./types.ts";

export type SessionFrame = Extract<PushMessage, { type: "item" | "delta" | "state" }>;

export function indexOf(items: TranscriptItem[]): Record<string, number> {
  const index: Record<string, number> = {};
  items.forEach((item, at) => {
    index[item.id] = at;
  });
  return index;
}

export function applyFrame(
  view: SessionView,
  frame: SessionFrame,
  items: TranscriptItem[],
  index: Record<string, number>,
): SessionView {
  if (frame.type === "state") return { ...view, state: frame.state };
  if (frame.type === "item") {
    const at = index[frame.item.id];
    if (at === undefined) {
      index[frame.item.id] = items.length;
      items.push(frame.item);
    } else {
      items[at] = frame.item;
    }
    return view;
  }
  const at = index[frame.itemId];
  const current = at === undefined ? undefined : items[at];
  if (current && current.kind !== "assistant") return view;
  const base = current ?? { kind: "assistant" as const, id: frame.itemId, text: "", thinking: "", streaming: true };
  const next = { ...base, [frame.channel]: base[frame.channel] + frame.delta };
  if (at === undefined) {
    index[frame.itemId] = items.length;
    items.push(next);
  } else {
    items[at] = next;
  }
  return view;
}

export function applyFrameBatch(views: Record<string, SessionView>, frames: SessionFrame[]): Record<string, SessionView> {
  const nextViews = { ...views };
  const touched = new Map<string, { items: TranscriptItem[]; index: Record<string, number> }>();
  for (const frame of frames) {
    let view = nextViews[frame.sessionId];
    if (!view) continue;
    let work = touched.get(frame.sessionId);
    if (!work) {
      work = { items: view.items.slice(), index: { ...view.index } };
      touched.set(frame.sessionId, work);
    }
    view = applyFrame(view, frame, work.items, work.index);
    nextViews[frame.sessionId] = view;
  }
  for (const [sessionId, work] of touched) {
    const view = nextViews[sessionId];
    if (view) nextViews[sessionId] = { ...view, items: work.items, index: work.index };
  }
  return nextViews;
}

export function createViewCache() {
  const recentViews: string[] = [];
  const scrollPositions = new Map<string, number>();

  return {
    recordAccess(sessionId: string): void {
      const existing = recentViews.indexOf(sessionId);
      if (existing >= 0) recentViews.splice(existing, 1);
      recentViews.push(sessionId);
    },

    remove(sessionId: string): void {
      const idx = recentViews.indexOf(sessionId);
      if (idx >= 0) recentViews.splice(idx, 1);
      scrollPositions.delete(sessionId);
    },

    removeMany(ids: Set<string>): void {
      for (let i = recentViews.length - 1; i >= 0; i--) {
        if (ids.has(recentViews[i]!)) recentViews.splice(i, 1);
      }
      for (const id of ids) scrollPositions.delete(id);
    },

    collectPrunable(keep: Set<string | undefined>): string[] {
      if (recentViews.length <= MAX_CACHED_VIEWS) return [];
      const drop: string[] = [];
      for (let i = 0; i < recentViews.length && recentViews.length - drop.length > MAX_CACHED_VIEWS; i++) {
        const candidate = recentViews[i]!;
        if (!keep.has(candidate)) drop.push(candidate);
      }
      if (!drop.length) return [];
      const dropped = new Set(drop);
      for (let i = recentViews.length - 1; i >= 0; i--) {
        if (dropped.has(recentViews[i]!)) recentViews.splice(i, 1);
      }
      return drop;
    },

    getScrollTop(sessionId: string): number | undefined {
      return scrollPositions.get(sessionId);
    },

    setScrollTop(sessionId: string, scrollTop: number | undefined): void {
      if (scrollTop === undefined) {
        scrollPositions.delete(sessionId);
        return;
      }
      if (!Number.isFinite(scrollTop) || scrollTop < 0) return;
      if (scrollPositions.has(sessionId)) scrollPositions.delete(sessionId);
      scrollPositions.set(sessionId, scrollTop);
      if (scrollPositions.size > MAX_SCROLL_POSITIONS) {
        const oldest = scrollPositions.keys().next().value;
        if (oldest !== undefined) scrollPositions.delete(oldest);
      }
    },
  };
}
