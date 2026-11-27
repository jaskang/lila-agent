export type Disposer = () => void | Promise<void>;

/**
 * Lightweight LIFO resource scope (inspired by reversible effect tracking).
 *
 * Timers, subscriptions, child scopes, and service teardowns registered onto a
 * scope are unwound automatically in reverse order when `dispose()` runs.
 * Disposal is idempotent and continues through remaining disposers if one fails.
 */
export class DisposableScope {
  readonly #disposers = new Set<Disposer>();
  #disposed = false;
  #disposing?: Promise<void>;

  get disposed(): boolean {
    return this.#disposed;
  }

  /** Registers one or more cleanup callbacks. Returns a detach handle; runs immediately if already disposed. */
  add(...disposers: Disposer[]): () => void {
    if (this.#disposed) {
      for (const disposer of disposers.toReversed()) {
        void Promise.resolve(disposer()).catch((error) => {
          console.error("[core] late disposer failed", error);
        });
      }
      return () => {};
    }
    for (const disposer of disposers) this.#disposers.add(disposer);
    return () => {
      for (const disposer of disposers) this.#disposers.delete(disposer);
    };
  }

  /** Creates a nested scope that disposes automatically when this scope disposes. */
  child(): DisposableScope {
    const nested = new DisposableScope();
    const detach = this.add(() => nested.dispose());
    nested.add(detach);
    return nested;
  }

  /** Registers an unref'd interval cleared automatically on dispose. */
  setInterval(fn: () => void, ms: number): NodeJS.Timeout {
    const timer = setInterval(fn, ms);
    timer.unref();
    if (this.#disposed) {
      clearInterval(timer);
      return timer;
    }
    this.add(() => clearInterval(timer));
    return timer;
  }

  /** Registers an unref'd timeout cleared on dispose and auto-detached once it fires. */
  setTimeout(fn: () => void, ms: number): NodeJS.Timeout {
    let detach = () => {};
    const timer = setTimeout(() => {
      detach();
      fn();
    }, ms);
    timer.unref();
    if (this.#disposed) {
      clearTimeout(timer);
      return timer;
    }
    detach = this.add(() => clearTimeout(timer));
    return timer;
  }

  /** Disposes every registered resource in LIFO order. Safe to call multiple times. */
  dispose(): Promise<void> {
    if (this.#disposing) return this.#disposing;
    this.#disposed = true;
    const entries = Array.from(this.#disposers).reverse();
    this.#disposers.clear();
    this.#disposing = (async () => {
      for (const disposer of entries) {
        try {
          await disposer();
        } catch (error) {
          console.error("[core] disposer failed", error);
        }
      }
    })();
    return this.#disposing;
  }
}
