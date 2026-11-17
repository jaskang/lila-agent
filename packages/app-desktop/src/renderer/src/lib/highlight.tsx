import * as React from "react";
import type { HighlightRequest, HighlightResponse } from "./highlight.worker.ts";

const MAX_CHARS = 20_000;
const CACHE_LIMIT = 200;

let worker: Worker | undefined;
let nextId = 0;
const waiting = new Map<number, (lines: HighlightResponse["lines"]) => void>();
const cache = new Map<string, HighlightResponse["lines"]>();

function highlight(code: string, language: string): Promise<HighlightResponse["lines"]> {
  const key = `${language}\u0000${code}`;
  if (cache.has(key)) return Promise.resolve(cache.get(key) ?? null);
  if (!worker) {
    worker = new Worker(new URL("./highlight.worker.ts", import.meta.url), { type: "module" });
    worker.onmessage = (event: MessageEvent<HighlightResponse>) => {
      waiting.get(event.data.id)?.(event.data.lines);
      waiting.delete(event.data.id);
    };
  }
  const id = ++nextId;
  worker.postMessage({ id, code, language } satisfies HighlightRequest);
  return new Promise((resolve) => {
    waiting.set(id, (lines) => {
      if (cache.size >= CACHE_LIMIT) cache.delete(cache.keys().next().value!);
      cache.set(key, lines);
      resolve(lines);
    });
  });
}

/** Highlighted lines for settled code, computed in a Worker; `null` until ready or when not highlightable. */
export function useHighlighted(code: string, language: string | undefined, enabled: boolean): React.ReactNode[] | null {
  const [result, setResult] = React.useState<{ key: string; lines: React.ReactNode[] } | null>(null);
  const key = `${language}\u0000${code}`;
  React.useEffect(() => {
    if (!enabled || !language || code.length > MAX_CHARS) return;
    let cancelled = false;
    void highlight(code, language).then((lines) => {
      if (cancelled || !lines) return;
      setResult({
        key,
        lines: lines.map((line) =>
          line.map(([content, light, dark], index) => (
            <span
              key={index}
              style={{ "--shiki-light": light, "--shiki-dark": dark } as React.CSSProperties}
              className="text-(--shiki-light) dark:text-(--shiki-dark)"
            >
              {content}
            </span>
          )),
        ),
      });
    });
    return () => {
      cancelled = true;
    };
  }, [code, language, enabled, key]);
  return result?.key === key ? result.lines : null;
}
