import { randomUUID } from "node:crypto";
import type { AuthEvent, AuthInteraction, AuthPrompt } from "@earendil-works/pi-ai";
import type { AuthLoginEvent, AuthLoginPrompt } from "@lila/protocol";
import { errorMessage, UserError } from "./errors.ts";

type PendingPrompt = {
  resolve: (value: string) => void;
  reject: (error: Error) => void;
};

type ActiveLogin = {
  id: string;
  owner: unknown;
  providerId: string;
  controller: AbortController;
  prompts: Map<string, PendingPrompt>;
};

function toProtocolPrompt(promptId: string, prompt: AuthPrompt): AuthLoginPrompt {
  if (prompt.type === "select") {
    return {
      promptId,
      type: "select",
      message: prompt.message,
      options: prompt.options.map((option) => ({
        id: option.id,
        label: option.label,
        ...(option.description ? { description: option.description } : {}),
      })),
    };
  }
  return {
    promptId,
    type: prompt.type,
    message: prompt.message,
    ...(prompt.placeholder ? { placeholder: prompt.placeholder } : {}),
  };
}

function toProtocolEvent(event: AuthEvent): AuthLoginEvent {
  switch (event.type) {
    case "auth_url":
      return { type: "auth_url", url: event.url, ...(event.instructions ? { instructions: event.instructions } : {}) };
    case "device_code":
      return { type: "device_code", userCode: event.userCode, verificationUri: event.verificationUri };
    case "info":
      return {
        type: "info",
        message: event.message,
        ...(event.links?.length
          ? { links: event.links.map((link) => ({ url: link.url, ...(link.label ? { label: link.label } : {}) })) }
          : {}),
      };
    case "progress":
      return { type: "progress", message: event.message };
  }
}

/** Tracks interactive provider login flows (API key / OAuth) bridged over WebSocket. */
export class AuthLoginManager {
  readonly #active = new Map<string, ActiveLogin>();

  start(
    owner: unknown,
    providerId: string,
    run: (interaction: AuthInteraction) => Promise<void>,
    emit: (event: AuthLoginEvent) => void,
    onSuccess: () => Promise<void>,
  ): string {
    this.cancelByOwner(owner);
    const id = randomUUID();
    const controller = new AbortController();
    const entry: ActiveLogin = { id, owner, providerId, controller, prompts: new Map() };
    this.#active.set(id, entry);

    const interaction: AuthInteraction = {
      signal: controller.signal,
      notify: (event) => {
        if (controller.signal.aborted) return;
        emit(toProtocolEvent(event));
      },
      prompt: (prompt) => {
        if (controller.signal.aborted || prompt.signal?.aborted) {
          return Promise.reject(new Error("Login cancelled"));
        }
        const promptId = randomUUID();
        return new Promise<string>((resolve, reject) => {
          const cleanup = () => {
            entry.prompts.delete(promptId);
            prompt.signal?.removeEventListener("abort", onPromptAbort);
            controller.signal.removeEventListener("abort", onFlowAbort);
          };
          const onPromptAbort = () => {
            cleanup();
            emit({ type: "prompt_cleared", promptId });
            reject(new Error("Login cancelled"));
          };
          const onFlowAbort = () => {
            cleanup();
            reject(new Error("Login cancelled"));
          };
          entry.prompts.set(promptId, {
            resolve: (value) => {
              cleanup();
              resolve(value);
            },
            reject: (error) => {
              cleanup();
              reject(error);
            },
          });
          prompt.signal?.addEventListener("abort", onPromptAbort, { once: true });
          controller.signal.addEventListener("abort", onFlowAbort, { once: true });
          emit({ type: "prompt", prompt: toProtocolPrompt(promptId, prompt) });
        });
      },
    };

    // Defer running the login flow until after the `auth.login.start` response reaches the client.
    setImmediate(() => {
      if (controller.signal.aborted) return;
      void (async () => {
        try {
          await run(interaction);
          if (controller.signal.aborted) return;
          await onSuccess();
          emit({ type: "done" });
        } catch (error) {
          if (controller.signal.aborted) return;
          const msg = errorMessage(error);
          emit({ type: "error", message: msg === "Login cancelled" ? "已取消登录" : msg });
        } finally {
          this.#cleanup(id);
        }
      })();
    });

    return id;
  }

  respond(loginId: string, promptId: string, value: string): void {
    const entry = this.#active.get(loginId);
    if (!entry) throw new UserError("登录会话已结束，请重新开始");
    const prompt = entry.prompts.get(promptId);
    if (!prompt) throw new UserError("该输入步骤已过期");
    prompt.resolve(value);
  }

  cancel(loginId: string): void {
    const entry = this.#active.get(loginId);
    if (!entry) return;
    entry.controller.abort();
    this.#cleanup(loginId);
  }

  cancelByOwner(owner: unknown): void {
    for (const [id, entry] of this.#active) {
      if (entry.owner === owner) this.cancel(id);
    }
  }

  dispose(): void {
    for (const id of this.#active.keys()) this.cancel(id);
  }

  #cleanup(loginId: string): void {
    const entry = this.#active.get(loginId);
    if (!entry) return;
    this.#active.delete(loginId);
    for (const prompt of entry.prompts.values()) {
      prompt.reject(new Error("Login cancelled"));
    }
    entry.prompts.clear();
  }
}
