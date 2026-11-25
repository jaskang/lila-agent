import * as React from "react";
import { cn } from "cn";

const WIDTH_STORAGE_KEY = "lila.rightPanelWidth";
const DEFAULT_WIDTH_PCT = 44;
const MIN_WIDTH_PCT = 26;
const MAX_WIDTH_PCT = 70;
const TRANSITION_MS = 220;

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
  const [widthPct, setWidthPct] = React.useState(readWidthPct);
  const [dragging, setDragging] = React.useState(false);
  const [mounted, setMounted] = React.useState(open);
  const [shown, setShown] = React.useState(open);
  const [animating, setAnimating] = React.useState(false);
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
    let latest = widthPct;

    const onMove = (moveEvent: PointerEvent) => {
      if (rect.width <= 0) return;
      const fromRight = rect.right - moveEvent.clientX;
      const rawPct = (fromRight / rect.width) * 100;
      latest = Math.min(MAX_WIDTH_PCT, Math.max(MIN_WIDTH_PCT, rawPct));
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
        aria-orientation="vertical"
        aria-label="调整右侧面板宽度（双击恢复默认）"
        title="拖动调整宽度，双击恢复默认"
        data-dragging={dragging || undefined}
        onPointerDown={handlePointerDown}
        onDoubleClick={() => {
          setWidthPct(DEFAULT_WIDTH_PCT);
          persistWidth(DEFAULT_WIDTH_PCT);
        }}
        className={cn(
          "group/sash hidden shrink-0 cursor-col-resize items-center justify-center select-none lg:flex",
          transitionClass,
          shown ? "w-2 opacity-100" : "pointer-events-none w-0 opacity-0",
        )}
      >
        <span
          aria-hidden
          className={cn(
            "h-10 w-1 rounded-full bg-transparent transition-colors group-hover/sash:bg-border",
            dragging && "bg-foreground/35",
          )}
        />
      </div>
      <div
        style={{ "--right-panel-width": `${widthPct}%` } as React.CSSProperties}
        className={cn(
          "flex min-h-0 shrink-0",
          "absolute inset-y-0 end-0 z-20 w-[min(32rem,calc(100%-2rem))]",
          "lg:static lg:z-auto lg:translate-x-0",
          transitionClass,
          animating ? "overflow-hidden" : "overflow-visible",
          shown
            ? "translate-x-0 opacity-100 lg:w-[clamp(20rem,var(--right-panel-width),100%)]"
            : "pointer-events-none translate-x-6 opacity-0 lg:w-0",
        )}
      >
        <aside
          aria-label={active.label}
          aria-hidden={!shown || undefined}
          className={cn(
            "@container flex size-full min-h-0 flex-col overflow-hidden rounded-xl bg-background shadow-xl ring-1 ring-border",
            "lg:min-w-[20rem] lg:shadow-sm lg:ring-border/60",
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
