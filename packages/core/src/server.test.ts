import { afterEach, expect, test } from "bun:test";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ClientCommand, ProtocolMessage, SessionEvent } from "@lila/protocol";
import type { CreateOmp, OmpFrame, OmpSessionOptions } from "./omp.ts";
import { startCore } from "./server.ts";

// Frame shapes below are copied from recorded omp 18.22 rpc-ui output (always-ask, write tool approved).
const TOOL_CALL_ID = "call_1|fc_1";
function approvalRun(promptId: string): OmpFrame[] {
  return [
    { type: "agent_start" },
    { type: "message_start", messageId: "msg-2", message: { role: "assistant" } },
    { type: "message_update", messageId: "msg-2", assistantMessageEvent: { type: "text_delta", delta: "先创建" } },
    { type: "message_update", messageId: "msg-2", assistantMessageEvent: { type: "text_delta", delta: "文件。" } },
    {
      type: "message_end",
      messageId: "msg-2",
      message: { role: "assistant", stopReason: "toolUse", model: "codex/gpt-6.1-sol", content: [{ type: "text", text: "先创建文件。" }] },
    },
    {
      type: "tool_stream_update",
      toolCallId: TOOL_CALL_ID,
      toolName: "write",
      update: { streaming: false, files: [{ path: "hello.txt", diff: "+1|hi", op: "create" }] },
    },
    { type: "tool_execution_start", toolCallId: TOOL_CALL_ID, toolName: "write", args: { path: "hello.txt", content: "hi" }, intent: "创建 hello 文件" },
    {
      type: "extension_ui_request",
      id: "ui-1",
      method: "select",
      title: "Allow tool: write\nPath: hello.txt\nContent:\nhi",
      options: ["Approve", "Approve write for this session", "Deny"],
    },
    { type: "extension_ui_request", id: "ui-2", method: "setWidget", widgetKey: "autoresearch" },
    { type: "__wait_for_answer" },
    {
      type: "tool_execution_end",
      toolCallId: TOOL_CALL_ID,
      toolName: "write",
      isError: false,
      result: { content: [{ type: "text", text: "Successfully wrote 2 bytes to hello.txt" }], details: {} },
    },
    {
      type: "message_end",
      messageId: "msg-4",
      message: { role: "assistant", stopReason: "stop", content: [{ type: "text", text: "已创建 hello.txt。" }] },
    },
    { type: "prompt_result", id: promptId, status: "completed", sessionSettled: true },
  ];
}

class FakeOmp {
  static last?: FakeOmp;
  readonly responses: Array<{ id: string; response: Record<string, unknown> }> = [];
  #rest: OmpFrame[] = [];
  constructor(readonly options: OmpSessionOptions) {
    FakeOmp.last = this;
  }
  async start() {}
  async stop() {}
  async abort() {}
  async request(type: string) {
    return type === "get_state" ? { type: "response", data: { sessionFile: "/tmp/omp-session.jsonl" } } : { type: "response", data: {} };
  }
  async prompt(id: string) {
    this.#rest = approvalRun(id);
    queueMicrotask(() => this.#play());
  }
  async respond(id: string, response: Record<string, unknown>) {
    this.responses.push({ id, response });
    queueMicrotask(() => this.#play());
  }
  #play() {
    while (this.#rest.length) {
      const frame = this.#rest.shift()!;
      if (frame.type === "__wait_for_answer") return;
      this.options.onFrame(frame);
    }
  }
}

const createFake: CreateOmp = (options) => new FakeOmp(options);
const servers: Array<{ stop(force?: boolean): Promise<void> }> = [];
afterEach(async () => {
  await Promise.all(servers.splice(0).map((server) => server.stop(true)));
});

// Bun 1.4.4 rejects port 0, so each test uses its own fixed port.
function start(port: number, databasePath = ":memory:") {
  const server = startCore({ port, databasePath, token: "secret", createOmp: createFake });
  servers.push(server);
  return server;
}

async function connect(port: number) {
  const ws = new WebSocket(`ws://127.0.0.1:${port}/ws`, ["lila.secret"]);
  const inbox: ProtocolMessage[] = [];
  const waiters: Array<() => void> = [];
  ws.onmessage = (event) => {
    inbox.push(JSON.parse(String(event.data)));
    for (const wake of waiters.splice(0)) wake();
  };
  async function until<T extends ProtocolMessage>(match: (message: ProtocolMessage) => message is T): Promise<T> {
    for (;;) {
      const index = inbox.findIndex(match);
      if (index >= 0) return inbox.splice(index, 1)[0] as T;
      await new Promise<void>((resolve) => waiters.push(resolve));
    }
  }
  async function call<D>(command: ClientCommand): Promise<D> {
    ws.send(JSON.stringify(command));
    const response = await until((m): m is Extract<ProtocolMessage, { type: "response" }> => m.type === "response" && m.requestId === command.requestId);
    if (!response.ok) throw new Error(response.error);
    return response.data as D;
  }
  const event = (eventType: SessionEvent["eventType"]) =>
    until((m): m is SessionEvent => m.type === "event" && m.eventType === eventType);
  await until((m) => m.type === "hello");
  return { ws, inbox, call, event };
}

test("rejects sockets without the token", async () => {
  start(17461);
  const ws = new WebSocket("ws://127.0.0.1:17461/ws");
  const closed = await new Promise<boolean>((resolve) => {
    ws.onopen = () => resolve(false);
    ws.onerror = () => resolve(true);
  });
  expect(closed).toBe(true);
});

test("an approved tool run streams deltas, waits for the answer, and persists only finished facts", async () => {
  start(17462);
  const client = await connect(17462);
  const { project } = await client.call<{ project: { id: string } }>({ type: "projects.add", requestId: "p", path: mkdtempSync(join(tmpdir(), "lila-")) });
  const { session } = await client.call<{ session: { id: string } }>({ type: "session.create", requestId: "s", projectId: project.id, approvalMode: "always-ask" });
  await client.call({ type: "events.resume", requestId: "r", sessionId: session.id, afterSeq: 0 });
  await client.call({ type: "message.send", requestId: "m", sessionId: session.id, text: "创建 hello.txt" });

  const pending = await client.event("pending.created");
  expect(pending.payload).toMatchObject({
    kind: "approval",
    title: "Allow tool: write",
    toolCallId: TOOL_CALL_ID,
    options: ["Approve", "Approve write for this session", "Deny"],
  });
  expect(client.inbox.some((m) => m.type === "stream" && m.kind === "assistant.delta" && m.delta === "先创建")).toBe(true);

  const pendingId = pending.eventType === "pending.created" ? pending.payload.pendingId : "";
  await client.call({ type: "pending.respond", requestId: "a", sessionId: session.id, pendingId, answer: { type: "option", value: "Approve" } });
  expect(FakeOmp.last?.responses).toEqual([{ id: "ui-1", response: { value: "Approve" } }]);
  await client.event("run.finished");

  const replay = await client.call<{ events: SessionEvent[] }>({ type: "events.resume", requestId: "r2", sessionId: session.id, afterSeq: 0 });
  expect(replay.events.map((e) => e.eventType)).toEqual([
    "session.created",
    "message.user",
    "run.started",
    "assistant.message",
    "tool.started",
    "pending.created",
    "pending.resolved",
    "tool.finished",
    "assistant.message",
    "run.finished",
  ]);
  const finished = replay.events.find((e) => e.eventType === "tool.finished");
  expect(finished?.payload).toMatchObject({ diffs: [{ path: "hello.txt", diff: "+1|hi" }] });
  const sessions = await client.call<{ sessions: Array<{ title: string; status: string }> }>({ type: "sessions.list", requestId: "l" });
  expect(sessions.sessions[0]).toMatchObject({ title: "创建 hello.txt", status: "finished" });
  client.ws.close();
});

test("a core restart marks the unfinished run as interrupted", async () => {
  const databasePath = join(mkdtempSync(join(tmpdir(), "lila-db-")), "lila.sqlite");
  const first = start(17463, databasePath);
  const client = await connect(17463);
  const { project } = await client.call<{ project: { id: string } }>({ type: "projects.add", requestId: "p", path: tmpdir() });
  const { session } = await client.call<{ session: { id: string } }>({ type: "session.create", requestId: "s", projectId: project.id });
  await client.call({ type: "events.resume", requestId: "r", sessionId: session.id, afterSeq: 0 });
  await client.call({ type: "message.send", requestId: "m", sessionId: session.id, text: "hi" });
  await client.event("pending.created");
  client.ws.close();
  servers.splice(servers.indexOf(first), 1);
  // Simulate a crash: drop the server without letting runners finish their runs.
  await first.stop(true);

  start(17464, databasePath);
  const again = await connect(17464);
  const replay = await again.call<{ events: SessionEvent[] }>({ type: "events.resume", requestId: "r", sessionId: session.id, afterSeq: 0 });
  expect(replay.events.at(-1)).toMatchObject({ eventType: "run.interrupted" });
  const sessions = await again.call<{ sessions: Array<{ status: string }> }>({ type: "sessions.list", requestId: "l" });
  expect(sessions.sessions[0]?.status).toBe("interrupted");
  again.ws.close();
});
