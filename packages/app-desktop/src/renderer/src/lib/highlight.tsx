import * as React from "react";
import type { HighlightRequest, HighlightResponse } from "./highlight.worker.ts";

export const HIGHLIGHT_MAX_CHARS = 80_000;
const CACHE_LIMIT = 200;

type PendingTask = {
  resolve: (lines: HighlightResponse["lines"]) => void;
  reject: (error: Error) => void;
};

let worker: Worker | undefined;
let nextId = 0;
const waiting = new Map<number, PendingTask>();
const cache = new Map<string, HighlightResponse["lines"]>();

function resetWorker(reason: string): void {
  const current = worker;
  worker = undefined;
  current?.terminate();
  const error = new Error(reason);
  for (const task of waiting.values()) task.reject(error);
  waiting.clear();
}

function getWorker(): Worker {
  if (!worker) {
    const next = new Worker(new URL("./highlight.worker.ts", import.meta.url), { type: "module" });
    next.onmessage = (event: MessageEvent<HighlightResponse>) => {
      const task = waiting.get(event.data.id);
      if (!task) return;
      waiting.delete(event.data.id);
      task.resolve(event.data.lines);
    };
    next.onerror = (event) => {
      event.preventDefault();
      resetWorker(event.message || "代码高亮 Worker 异常");
    };
    next.onmessageerror = () => {
      resetWorker("代码高亮 Worker 消息解码失败");
    };
    worker = next;
  }
  return worker;
}

function highlight(code: string, language: string): Promise<HighlightResponse["lines"]> {
  const key = `${language}\u0000${code}`;
  if (cache.has(key)) return Promise.resolve(cache.get(key) ?? null);
  const id = ++nextId;
  return new Promise((resolve, reject) => {
    waiting.set(id, {
      resolve: (lines) => {
        if (cache.size >= CACHE_LIMIT) cache.delete(cache.keys().next().value!);
        cache.set(key, lines);
        resolve(lines);
      },
      reject,
    });
    try {
      getWorker().postMessage({ id, code, language } satisfies HighlightRequest);
    } catch (error) {
      waiting.delete(id);
      resetWorker(error instanceof Error ? error.message : String(error));
      reject(error instanceof Error ? error : new Error(String(error)));
    }
  });
}

/** Highlighted lines for settled code, computed in a Worker; `null` until ready or when not highlightable. */
export function useHighlighted(code: string, language: string | undefined, enabled: boolean): React.ReactNode[] | null {
  const [result, setResult] = React.useState<{ key: string; lines: React.ReactNode[] } | null>(null);
  const key = `${language}\u0000${code}`;
  React.useEffect(() => {
    if (!enabled || !language || code.length > HIGHLIGHT_MAX_CHARS) return;
    let cancelled = false;
    void highlight(code, language)
      .then((lines) => {
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
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [code, language, enabled, key]);
  return result?.key === key ? result.lines : null;
}
