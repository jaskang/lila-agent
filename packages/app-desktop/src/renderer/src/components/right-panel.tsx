import * as React from "react";
import { cn } from "cn";
import { Sheet, SheetContent, SheetTitle } from "@/components/ui/sheet";

const WIDTH_STORAGE_KEY = "lila.rightPanelWidth";
const DEFAULT_WIDTH_PCT = 44;
const MIN_WIDTH_PCT = 26;
const MAX_WIDTH_PCT = 70;
const WIDE_PANEL_QUERY = "(min-width: 64rem)";
const KEYBOARD_WIDTH_STEP = 2;
const MIN_PANEL_WIDTH_REM = 20;

function clampWidthPct(width: number, minimum: number): number {
  return Math.min(MAX_WIDTH_PCT, Math.max(minimum, width));
}

function subscribeToPanelWidth(onChange: () => void): () => void {
  const query = window.matchMedia(WIDE_PANEL_QUERY);
  query.addEventListener("change", onChange);
  return () => query.removeEventListener("change", onChange);
}

function isWidePanel(): boolean {
  return window.matchMedia(WIDE_PANEL_QUERY).matches;
}

function readWidthPct(): number {
  try {
    const parsed = Number(localStorage.getItem(WIDTH_STORAGE_KEY));
    if (Number.isFinite(parsed) && parsed >= MIN_WIDTH_PCT && parsed <= MAX_WIDTH_PCT) {
      return parsed;
    }
  } catch {
    // Ignore storage read errors.
  }
  return DEFAULT_WIDTH_PCT;
}

/**
 * Shared floating-card container for workspace side panels (files, git changes,
 * turn diff review, sub-agent sessions, and future embedded tools like a browser preview).
 */
export function RightPanelShell({
  open,
  label,
  onClose,
  containerRef,
  children,
}: {
  open: boolean;
  label?: string | undefined;
  onClose: () => void;
  containerRef: React.RefObject<HTMLDivElement | null>;
  children?: React.ReactNode;
}) {
  const wide = React.useSyncExternalStore(subscribeToPanelWidth, isWidePanel);
  const [widthPct, setWidthPct] = React.useState(readWidthPct);
  const [minWidthPct, setMinWidthPct] = React.useState(MIN_WIDTH_PCT);
  const panelWidthPct = clampWidthPct(widthPct, minWidthPct);
  const [dragging, setDragging] = React.useState(false);
  const panelId = React.useId();
  const panelWrapRef = React.useRef<HTMLDivElement>(null);
  const finishDrag = React.useRef<(() => void) | undefined>(undefined);

  React.useLayoutEffect(() => () => finishDrag.current?.(), [wide, open]);

  React.useLayoutEffect(() => {
    const container = containerRef.current;
    if (!wide || !open || !container) return;
    const measure = () => {
      const width = container.getBoundingClientRect().width;
      const rootFontSize = Number.parseFloat(getComputedStyle(document.documentElement).fontSize);
      if (width > 0) setMinWidthPct(clampWidthPct((rootFontSize * MIN_PANEL_WIDTH_REM / width) * 100, MIN_WIDTH_PCT));
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(container);
    return () => observer.disconnect();
  }, [wide, open, containerRef]);

  const persistWidth = React.useCallback((next: number) => {
    try {
      localStorage.setItem(WIDTH_STORAGE_KEY, String(Math.round(next * 10) / 10));
    } catch {
      // Ignore storage write errors.
    }
  }, []);

  const handlePointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    const container = containerRef.current;
    if (!container || event.button !== 0 || !event.isPrimary || finishDrag.current) return;
    event.preventDefault();
    const target = event.currentTarget;
    const { pointerId } = event;
    target.setPointerCapture(pointerId);
    setDragging(true);
    const rect = container.getBoundingClientRect();
    let latest = panelWidthPct;

    const onMove = (moveEvent: PointerEvent) => {
      if (moveEvent.pointerId !== pointerId || rect.width <= 0) return;
      const fromRight = rect.right - moveEvent.clientX;
      const rawPct = (fromRight / rect.width) * 100;
      latest = clampWidthPct(rawPct, minWidthPct);
      const rounded = Math.round(latest);
      if (panelWrapRef.current) {
        panelWrapRef.current.style.width = `clamp(${MIN_PANEL_WIDTH_REM}rem, ${latest}%, 100%)`;
      }
      target.setAttribute("aria-valuenow", String(rounded));
      target.setAttribute("aria-valuetext", `${rounded}%`);
    };

    const finish = () => {
      target.removeEventListener("pointermove", onMove);
      target.removeEventListener("pointerup", onUp);
      target.removeEventListener("pointercancel", onUp);
      target.removeEventListener("lostpointercapture", onUp);
      if (target.hasPointerCapture(pointerId)) target.releasePointerCapture(pointerId);
      finishDrag.current = undefined;
      setDragging(false);
      setWidthPct(latest);
      persistWidth(latest);
    };
    const onUp = (pointerEvent: PointerEvent) => {
      if (pointerEvent.pointerId === pointerId) finish();
    };
    finishDrag.current = finish;

    target.addEventListener("pointermove", onMove);
    target.addEventListener("pointerup", onUp);
    target.addEventListener("pointercancel", onUp);
    target.addEventListener("lostpointercapture", onUp);
  };

  if (!wide) {
    return (
      <WorkspaceSheet open={open} label={label} onClose={onClose}>
        {children}
      </WorkspaceSheet>
    );
  }

  if (!open || !label) return null;

  return (
    <>
      <div
        role="separator"
        tabIndex={0}
        aria-orientation="vertical"
        aria-label="右侧面板宽度"
        aria-controls={panelId}
        aria-valuemin={Math.round(minWidthPct)}
        aria-valuemax={MAX_WIDTH_PCT}
        aria-valuenow={Math.round(panelWidthPct)}
        aria-valuetext={`${Math.round(panelWidthPct)}%`}
        title="拖动或用方向键调整宽度，双击恢复默认"
        data-dragging={dragging || undefined}
        onPointerDown={handlePointerDown}
        onKeyDown={(event) => {
          const target = containerRef.current;
          if (!target || event.altKey || event.ctrlKey || event.metaKey) return;
          const isRtl = getComputedStyle(target).direction === "rtl";
          const step = isRtl ? -KEYBOARD_WIDTH_STEP : KEYBOARD_WIDTH_STEP;
          const next =
            event.key === "ArrowLeft" ? panelWidthPct + step :
            event.key === "ArrowRight" ? panelWidthPct - step :
            event.key === "Home" ? minWidthPct :
            event.key === "End" ? MAX_WIDTH_PCT :
            undefined;
          if (next === undefined) return;
          event.preventDefault();
          const clamped = clampWidthPct(next, minWidthPct);
          setWidthPct(clamped);
          persistWidth(clamped);
        }}
        onDoubleClick={() => {
          setWidthPct(DEFAULT_WIDTH_PCT);
          persistWidth(DEFAULT_WIDTH_PCT);
        }}
        className="group/sash flex w-2 shrink-0 cursor-col-resize items-center justify-center select-none focus-visible:outline-none"
      >
        <span
          aria-hidden
          className={cn(
            "h-10 w-1 rounded-full bg-transparent transition-colors group-hover/sash:bg-border group-focus-visible/sash:bg-ring",
            dragging && "bg-foreground/35",
          )}
        />
      </div>
      <div
        ref={panelWrapRef}
        style={{ width: `clamp(${MIN_PANEL_WIDTH_REM}rem, ${panelWidthPct}%, 100%)` }}
        className="flex min-h-0 shrink-0"
      >
        <aside
          data-surface="panel"
          id={panelId}
          aria-label={label}
          className={cn(
            "@container flex size-full min-h-0 min-w-[20rem] flex-col overflow-hidden rounded-xl bg-workspace-surface shadow-[var(--panel-shadow)] ring-1 ring-[var(--panel-border)]",
            dragging && "pointer-events-none select-none",
          )}
          onKeyDown={(event) => {
            if (event.key !== "Escape" || event.defaultPrevented || !event.currentTarget.contains(event.target as Node)) return;
            if ((event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement) && event.target.value) return;
            event.stopPropagation();
            onClose();
          }}
        >
          {children}
        </aside>
      </div>
    </>
  );
}

function WorkspaceSheet({ open, label, onClose, children }: {
  open: boolean;
  label?: string;
  onClose: () => void;
  children?: React.ReactNode;
}) {
  const content = React.useRef<{ label: string; children: React.ReactNode } | undefined>(undefined);
  const popup = React.useRef<HTMLDivElement>(null);
  if (open && label) content.current = { label, children };

  return (
    <Sheet
      open={open}
      onOpenChange={(next) => { if (!next) onClose(); }}
      onOpenChangeComplete={(next) => { if (!next) content.current = undefined; }}
    >
      <SheetContent
        ref={popup}
        initialFocus={popup}
        showCloseButton={false}
        aria-modal="true"
        data-workspace-sheet
        className="@container min-h-0 gap-0 overflow-hidden data-[side=right]:w-[min(32rem,calc(100%-2rem))] data-[side=right]:sm:max-w-none"
      >
        <SheetTitle className="sr-only">{content.current?.label ?? "工作区面板"}</SheetTitle>
        {content.current?.children}
      </SheetContent>
    </Sheet>
  );
}
