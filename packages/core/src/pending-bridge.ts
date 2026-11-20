import { randomUUID } from "node:crypto";
import type { ExtensionUIContext, ExtensionUIDialogOptions } from "@earendil-works/pi-coding-agent";
import type { PendingAnswer, PendingRequest } from "@lila/protocol";
import { UserError } from "./errors.ts";

type PendingEntry = { request: PendingRequest; settle(answer: PendingAnswer | undefined): void };

/** Identity styling: extensions that style text for the TUI get their plain text back. */
const plainTheme = new Proxy(
  {},
  { get: () => (...args: unknown[]) => args.findLast((arg) => typeof arg === "string") ?? "" },
) as ExtensionUIContext["theme"];

export interface PendingBridgeCallbacks {
  onPendingChanged(): void;
  onNotify(level: "info" | "warning" | "error", message: string): void;
  onStatusLine(key: string, value: string | undefined): void;
}

/** Manages in-flight user prompts (approvals, ask_user, extension UI dialogs) for a live session. */
export class PendingBridge {
  readonly #pending = new Map<string, PendingEntry>();
  readonly #callbacks: PendingBridgeCallbacks;

  constructor(callbacks: PendingBridgeCallbacks) {
    this.#callbacks = callbacks;
  }

  get size(): number {
    return this.#pending.size;
  }

  list(): PendingRequest[] {
    return Array.from(this.#pending.values(), (entry) => entry.request);
  }

  request(request: Omit<PendingRequest, "id">, options: ExtensionUIDialogOptions = {}): Promise<PendingAnswer | undefined> {
    const id = randomUUID();
    const { signal, timeout } = options;
    const { promise, resolve } = Promise.withResolvers<PendingAnswer | undefined>();
    if (signal?.aborted) {
      resolve(undefined);
      return promise;
    }
    let timer: NodeJS.Timeout | undefined;
    const onAbort = () => settle(undefined);
    const settle = (answer: PendingAnswer | undefined) => {
      if (!this.#pending.delete(id)) return;
      clearTimeout(timer);
      signal?.removeEventListener("abort", onAbort);
      this.#callbacks.onPendingChanged();
      resolve(answer);
    };
    this.#pending.set(id, { request: { ...request, id }, settle });
    signal?.addEventListener("abort", onAbort, { once: true });
    if (timeout) {
      timer = setTimeout(onAbort, timeout);
      timer.unref();
    }
    this.#callbacks.onPendingChanged();
    return promise;
  }

  respond(pendingId: string, answer: PendingAnswer): void {
    const entry = this.#pending.get(pendingId);
    if (!entry) throw new UserError("这个请求已经结束了");
    entry.settle(answer);
  }

  cancelAll(): void {
    for (const entry of Array.from(this.#pending.values())) entry.settle(undefined);
  }

  createUIContext(): ExtensionUIContext {
    const dialog = <T>(
      request: Omit<PendingRequest, "id">,
      options: ExtensionUIDialogOptions | undefined,
      map: (answer: PendingAnswer | undefined) => T,
    ) => this.request(request, options).then(map);
    const text = (answer: PendingAnswer | undefined) => (answer?.type === "text" ? answer.value : undefined);
    return {
      select: (title, options, opts) => dialog({ kind: "select", title, options }, opts, (answer) => (answer?.type === "option" ? answer.value : undefined)),
      confirm: (title, message, opts) => dialog({ kind: "confirm", title, message, options: [] }, opts, (answer) => answer?.type === "confirm" && answer.confirmed),
      input: (title, placeholder, opts) => dialog({ kind: "input", title, options: [], ...(placeholder ? { placeholder } : {}) }, opts, text),
      editor: (title, prefill) => dialog({ kind: "editor", title, options: [], ...(prefill ? { prefill } : {}) }, undefined, text),
      notify: (message, type) => this.#callbacks.onNotify(type ?? "info", message),
      setStatus: (key, value) => {
        // pi-subagents writes a TUI status line (`1 running agent`) that duplicates Lila's native sub-agent UI.
        if (key === "subagents") return;
        this.#callbacks.onStatusLine(key, value);
      },
      // TUI-only capabilities.
      onTerminalInput: () => () => {},
      setWorkingMessage: () => {},
      setWorkingVisible: () => {},
      setWorkingIndicator: () => {},
      setHiddenThinkingLabel: () => {},
      setWidget: () => {},
      setFooter: () => {},
      setHeader: () => {},
      setTitle: () => {},
      custom: async () => undefined as never,
      pasteToEditor: () => {},
      setEditorText: () => {},
      getEditorText: () => "",
      addAutocompleteProvider: () => {},
      setEditorComponent: () => {},
      getEditorComponent: () => undefined,
      theme: plainTheme,
      getAllThemes: () => [],
      getTheme: () => undefined,
      setTheme: () => ({ success: false, error: "Lila 不支持主题" }),
      getToolsExpanded: () => false,
      setToolsExpanded: () => {},
    };
  }
}
