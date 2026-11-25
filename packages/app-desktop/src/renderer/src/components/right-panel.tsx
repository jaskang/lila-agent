import * as React from "react";
import { cn } from "cn";
import { Sheet, SheetContent, SheetTitle } from "@/components/ui/sheet";

const WIDTH_STORAGE_KEY = "lila.rightPanelWidth";
const DEFAULT_WIDTH_PCT = 44;
const MIN_WIDTH_PCT = 26;
const MAX_WIDTH_PCT = 70;
const TRANSITION_MS = 220;
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
  const [mounted, setMounted] = React.useState(open);
  const [shown, setShown] = React.useState(open);
  const [animating, setAnimating] = React.useState(false);
  const panelId = React.useId();
  const mountedRef = React.useRef(mounted);
  mountedRef.current = mounted;
  const lastContentRef = React.useRef<{ label: string; children: React.ReactNode } | undefined>(
    open && label ? { label, children } : undefined,
  );

  if (open && label) {
    lastContentRef.current = { label, children };
  }

  React.useEffect(() => {
    if (open) {
      setMounted(true);
      setAnimating(true);
      let frame2 = 0;
      const frame1 = window.requestAnimationFrame(() => {
        frame2 = window.requestAnimationFrame(() => {
          setShown(true);
        });
      });
      const timer = window.setTimeout(() => {
        setAnimating(false);
      }, TRANSITION_MS + 40);
      return () => {
        window.cancelAnimationFrame(frame1);
        window.cancelAnimationFrame(frame2);
        window.clearTimeout(timer);
      };
    }

    if (!mountedRef.current) return;
    setAnimating(true);
    setShown(false);
    const timer = window.setTimeout(() => {
      setMounted(false);
      setAnimating(false);
      lastContentRef.current = undefined;
    }, TRANSITION_MS + 20);
    return () => window.clearTimeout(timer);
  }, [open]);

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
    if (!container) return;
    event.preventDefault();
    const target = event.currentTarget;
    target.setPointerCapture(event.pointerId);
    setDragging(true);
    const rect = container.getBoundingClientRect();
    let latest = panelWidthPct;

    const onMove = (moveEvent: PointerEvent) => {
      if (rect.width <= 0) return;
      const fromRight = rect.right - moveEvent.clientX;
      const rawPct = (fromRight / rect.width) * 100;
      latest = clampWidthPct(rawPct, minWidthPct);
      setWidthPct(latest);
    };

    const onUp = () => {
      setDragging(false);
      persistWidth(latest);
      target.removeEventListener("pointermove", onMove);
      target.removeEventListener("pointerup", onUp);
      target.removeEventListener("pointercancel", onUp);
    };

    target.addEventListener("pointermove", onMove);
    target.addEventListener("pointerup", onUp);
    target.addEventListener("pointercancel", onUp);
  };

  if (!wide) {
    const content = lastContentRef.current;
    return (
      <Sheet open={open} onOpenChange={(next) => { if (!next) onClose(); }}>
        <SheetContent
          showCloseButton={false}
          aria-modal="true"
          data-workspace-sheet
          className="@container min-h-0 gap-0 overflow-hidden data-[side=right]:w-[min(32rem,calc(100%-2rem))] data-[side=right]:sm:max-w-none"
        >
          <SheetTitle className="sr-only">{content?.label ?? "工作区面板"}</SheetTitle>
          {content?.children}
        </SheetContent>
      </Sheet>
    );
  }

  if (!mounted) return null;
  const active = open && label ? { label, children } : lastContentRef.current;
  if (!active) return null;

  const transitionClass = dragging
    ? "transition-none"
    : "transition-[width,transform,opacity] duration-[220ms] ease-[cubic-bezier(0.22,1,0.36,1)] motion-reduce:transition-none";

  return (
    <>
      <div
        role="separator"
        tabIndex={shown ? 0 : -1}
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
        className={cn(
          "group/sash flex shrink-0 cursor-col-resize items-center justify-center select-none focus-visible:outline-none",
          transitionClass,
          shown ? "w-2 opacity-100" : "pointer-events-none w-0 opacity-0",
        )}
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
        style={{ "--right-panel-width": `${panelWidthPct}%`, "--right-panel-min": `${MIN_PANEL_WIDTH_REM}rem` } as React.CSSProperties}
        className={cn(
          "flex min-h-0 shrink-0",
          transitionClass,
          animating ? "overflow-hidden" : "overflow-visible",
          shown
            ? "w-[clamp(var(--right-panel-min),var(--right-panel-width),100%)] opacity-100"
            : "pointer-events-none w-0 opacity-0",
        )}
      >
        <aside
          data-surface="panel"
          id={panelId}
          aria-label={active.label}
          aria-hidden={!shown || undefined}
          inert={!shown}
          className={cn(
            "@container flex size-full min-h-0 flex-col overflow-hidden rounded-xl bg-workspace-surface shadow-[var(--panel-shadow)] ring-1 ring-[var(--panel-border)]",
            "min-w-(--right-panel-min)",
            dragging && "pointer-events-none select-none",
          )}
          onKeyDown={(event) => {
            if (event.key !== "Escape" || event.defaultPrevented || !event.currentTarget.contains(event.target as Node)) return;
            if ((event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement) && event.target.value) return;
            event.stopPropagation();
            onClose();
          }}
        >
          {active.children}
        </aside>
      </div>
    </>
  );
}
