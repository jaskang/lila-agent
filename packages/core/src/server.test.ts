import { afterAll, beforeAll, expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { SessionManager } from "@earendil-works/pi-coding-agent";
import type { ClientCommand, CommandResults, CommandType, ServerMessage } from "@lila/protocol";
import { type Core, startCore } from "./server.ts";

let core: Core;

beforeAll(async () => {
  // Keep the user's Pi configuration and sessions out of the test.
  process.env.PI_CODING_AGENT_DIR = mkdtempSync(join(tmpdir(), "lila-agent-dir-"));
  core = await startCore({ port: 0, dataDir: mkdtempSync(join(tmpdir(), "lila-core-")), token: "secret" });
});

afterAll(async () => {
  await core.stop();
});

async function connect(target = core) {
  const ws = new WebSocket(`ws://127.0.0.1:${target.port}/ws`, ["lila.secret"]);
  const inbox: ServerMessage[] = [];
  const waiters: Array<() => void> = [];
  ws.onmessage = (event) => {
    inbox.push(JSON.parse(String(event.data)) as ServerMessage);
    for (const wake of waiters.splice(0)) wake();
  };
  async function until(match: (message: ServerMessage) => boolean): Promise<ServerMessage> {
    for (;;) {
      const index = inbox.findIndex(match);
      if (index >= 0) return inbox.splice(index, 1)[0] as ServerMessage;
      await new Promise<void>((resolve) => waiters.push(resolve));
    }
  }
  async function call<T extends CommandType>(command: Extract<ClientCommand, { type: T }>): Promise<CommandResults[T]> {
    ws.send(JSON.stringify(command));
    const response = await until((message) => message.type === "response" && message.requestId === command.requestId);
    if (response.type !== "response") throw new Error("unreachable");
    if (!response.ok) throw new Error(response.error);
    return response.data as CommandResults[T];
  }
  await until((message) => message.type === "hello");
  return { ws, call, inbox, until };
}

test("rejects sockets without the token", async () => {
  const ws = new WebSocket(`ws://127.0.0.1:${core.port}/ws`);
  const opened = await new Promise<boolean>((resolve) => {
    ws.onopen = () => resolve(true);
    ws.onerror = () => resolve(false);
  });
  expect(opened).toBe(false);
});

test("creates a session in an added project, watches and unwatches it, and validates session.open", async () => {
  const client = await connect();
  const { project } = await client.call({ type: "projects.add", requestId: "p", path: mkdtempSync(join(tmpdir(), "lila-project-")) });
  const { session } = await client.call({ type: "session.create", requestId: "s", projectId: project.id, approvalMode: "ask-all" });
  expect(session).toMatchObject({ projectId: project.id, status: "idle", pendingCount: 0, title: "新会话" });

  const { sessions } = await client.call({ type: "sessions.list", requestId: "l" });
  expect(sessions.map((item) => item.id)).toContain(session.id);

  // Invalid session.open (unknown id) returns a clean error.
  await expect(client.call({ type: "session.open", requestId: "bad-open", sessionId: "non-existent-session" })).rejects.toThrow("会话不存在");

  const opened = await client.call({ type: "session.open", requestId: "o", sessionId: session.id });
  expect(opened.items).toEqual([]);
  expect(opened.state.approvalMode).toBe("ask-all");

  // Watching receives state pushes on session.configure.
  client.inbox.length = 0;
  await client.call({ type: "session.configure", requestId: "cfg-1", sessionId: session.id, approvalMode: "auto" });
  const stateMsg = await client.until((message) => message.type === "state" && message.sessionId === session.id);
  expect(stateMsg.type === "state" && stateMsg.state.approvalMode).toBe("auto");

  // After session.unwatch, further session.configure calls do not push state frames to this socket.
  await client.call({ type: "session.unwatch", requestId: "u" });
  client.inbox.length = 0;
  await client.call({ type: "session.configure", requestId: "cfg-2", sessionId: session.id, approvalMode: "ask-exec" });
  expect(client.inbox.some((message) => message.type === "state" && message.sessionId === session.id)).toBe(false);

  client.ws.close();
}, 60_000);

function writePiTurn(manager: SessionManager, text: string, timestamp = Date.now()): void {
  manager.appendMessage({ role: "user", content: text, timestamp });
  manager.appendMessage({
    role: "assistant",
    content: [{ type: "text", text: "已收到" }],
    api: "openai-completions",
    provider: "fixture",
    model: "fixture",
    usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
    stopReason: "stop",
    timestamp: timestamp + 1,
  });
}

test("exposes an extension's startup question before session initialization finishes", async () => {
  const client = await connect();
  try {
    const path = mkdtempSync(join(tmpdir(), "lila-project-startup-"));
    const extensions = join(path, ".pi", "extensions");
    mkdirSync(extensions, { recursive: true });
    writeFileSync(join(extensions, "question.ts"), `
      export default function (pi) {
        pi.registerCommand("after-startup", {
          description: "Record when the first submission arrives",
          handler: async (_args, ctx) => ctx.ui.setStatus("first", "已收到"),
        });
        pi.on("session_start", async (_event, ctx) => {
          const answer = await ctx.ui.input("启动问题");
          ctx.ui.setStatus("startup", answer);
        });
      }
    `);
    const { project } = await client.call({ type: "projects.add", requestId: "p", path });
    const { session } = await client.call({ type: "session.create", requestId: "c", projectId: project.id });
    const opened = await client.call({ type: "session.open", requestId: "o", sessionId: session.id });
    const isStartupQuestion = (item: { title: string }) => item.title === "启动问题";
    const frame = opened.state.pending.some(isStartupQuestion) ? undefined : await client.until(
      (message) => message.type === "state" && message.sessionId === session.id && message.state.pending.some(isStartupQuestion),
    );
    const state = frame?.type === "state" ? frame.state : opened.state;
    const pending = state.pending.find(isStartupQuestion);
    expect(state.access).toBe("starting");
    expect(pending).toBeDefined();
    const prompting = client.call({ type: "session.prompt", requestId: "first", sessionId: session.id, text: "/after-startup" });
    const waiting = await client.call({ type: "session.open", requestId: "waiting", sessionId: session.id });
    expect(waiting.state.statusLines.first).toBeUndefined();
    await client.call({ type: "session.respond", requestId: "respond", sessionId: session.id, pendingId: pending!.id, answer: { type: "text", value: "继续" } });
    await prompting;
    const finished = await client.call({ type: "session.open", requestId: "ready", sessionId: session.id });
    expect(finished.state).toMatchObject({ access: "ready", statusLines: { startup: "继续", first: "已收到" } });
  } finally {
    client.ws.close();
  }
}, 3_000);

test("deletes an existing Pi file before the session index is loaded", async () => {
  const coldCore = await startCore({ port: 0, dataDir: mkdtempSync(join(tmpdir(), "lila-cold-core-")), token: "secret" });
  const client = await connect(coldCore);
  try {
    const path = mkdtempSync(join(tmpdir(), "lila-project-cold-delete-"));
    await client.call({ type: "projects.add", requestId: "p", path });
    const manager = SessionManager.create(path);
    writePiTurn(manager, "已有会话");
    const sessionId = manager.getSessionId();
    const file = manager.getSessionFile()!;
    expect(existsSync(file)).toBe(true);
    await client.call({ type: "session.delete", requestId: "d", sessionId });
    expect(existsSync(file)).toBe(false);
    const { sessions } = await client.call({ type: "sessions.list", requestId: "l" });
    expect(sessions.some((item) => item.id === sessionId)).toBe(false);
  } finally {
    client.ws.close();
    await coldCore.stop();
  }
});

test("previews a read-only sub-agent while the parent stays subscribed", async () => {
  const client = await connect();
  try {
    const path = mkdtempSync(join(tmpdir(), "lila-project-subagent-"));
    await client.call({ type: "projects.add", requestId: "p", path });
    const parent = SessionManager.create(path);
    writePiTurn(parent, "父会话");
    const child = SessionManager.create(path, undefined, { parentSession: parent.getSessionFile()! });
    child.appendSessionInfo("Code Reviewer#01234567");
    writePiTurn(child, "开始检查");
    const extensions = join(path, ".pi", "extensions");
    mkdirSync(extensions, { recursive: true });
    writeFileSync(join(extensions, "background.ts"), `
      export default function (pi) {
        const child = { id: "0123456789abcdef", type: "Code Reviewer", description: "检查", isBackground: true };
        pi.on("session_start", (_event, ctx) => {
          if (ctx.sessionManager.getSessionId() !== ${JSON.stringify(parent.getSessionId())}) return;
          pi.events.emit("subagents:created", child);
          pi.events.emit("subagents:started", child);
        });
        pi.registerCommand("finish-fixture-child", {
          description: "Finish the test child",
          handler: async () => pi.events.emit("subagents:completed", { ...child, status: "completed" }),
        });
      }
    `);
    const parentId = parent.getSessionId();
    const parentOpened = await client.call({ type: "session.open", requestId: "parent", sessionId: parentId });
    if (parentOpened.state.access !== "ready") {
      await client.until((message) => message.type === "state" && message.sessionId === parentId && message.state.access === "ready");
    }
    const parentView = await client.call({ type: "session.open", requestId: "parent-ready", sessionId: parentId });
    expect(parentView.state).toMatchObject({ status: "running", runActive: false });
    await expect(client.call({ type: "session.prompt", requestId: "stale-queue", sessionId: parentId, text: "排队", mode: "followUp" })).rejects.toThrow("当前回复已结束");

    const sessionId = child.getSessionId();
    await expect(client.call({ type: "session.open", requestId: "bad-preview", sessionId: parentId, slot: "preview" })).rejects.toThrow("仅支持子 Agent");
    const opened = await client.call({ type: "session.open", requestId: "o", sessionId, slot: "preview" });
    expect(opened.items).toContainEqual(expect.objectContaining({ text: "开始检查", canRewind: false }));
    expect(opened.state).toMatchObject({ access: "read-only", status: "running", runActive: false });
    await expect(client.call({ type: "session.fork", requestId: "early-fork", sessionId })).rejects.toThrow("结束后");
    await expect(client.call({ type: "session.delete", requestId: "early-delete", sessionId })).rejects.toThrow("结束后");
    await expect(client.call({ type: "session.rename", requestId: "r", sessionId, title: "覆盖" })).rejects.toThrow("只读");
    writePiTurn(child, "新的结果", Date.now() + 100);
    await client.until((message) => message.type === "reset" && message.sessionId === sessionId);
    const refreshed = await client.call({ type: "session.open", requestId: "refresh", sessionId, slot: "preview" });
    expect(refreshed.items).toContainEqual(expect.objectContaining({ text: "新的结果" }));
    client.inbox.length = 0;
    await client.call({ type: "session.prompt", requestId: "finish-child", sessionId: parentId, text: "/finish-fixture-child" });
    await client.until((message) => message.type === "state" && message.sessionId === parentId && message.state.status === "idle");
    await client.call({ type: "session.unwatch", requestId: "close-preview", slot: "preview" });
    await client.call({ type: "session.configure", requestId: "parent-configure", sessionId: parentId, approvalMode: "ask-exec" });
    await client.until((message) => message.type === "state" && message.sessionId === parentId && message.state.approvalMode === "ask-exec");
    const { session: fork } = await client.call({ type: "session.fork", requestId: "fork", sessionId });
    expect(fork.id).not.toBe(sessionId);
    const forked = await client.call({ type: "session.open", requestId: "open-fork", sessionId: fork.id });
    expect(forked.state.access).not.toBe("read-only");
    expect(forked.items).toContainEqual(expect.objectContaining({ text: "新的结果", canRewind: true }));
    const { sessions } = await client.call({ type: "sessions.list", requestId: "list" });
    expect(sessions.find((entry) => entry.id === fork.id)?.parentId).toBeUndefined();
  } finally {
    client.ws.close();
  }
}, 5_000);

test("searches beyond the open page and returns a window around a folded tool result", async () => {
  const client = await connect();
  try {
    const path = mkdtempSync(join(tmpdir(), "lila-project-search-"));
    await client.call({ type: "projects.add", requestId: "search-project", path });
    const manager = SessionManager.create(path);
    const time = Date.now();
    writePiTurn(manager, "最早的消息", time);
    manager.appendMessage({
      role: "toolResult", toolCallId: "history-tool", toolName: "read", content: [{ type: "text", text: "旧工具 Needle-789 输出" }],
      isError: false, timestamp: time + 2,
    });
    for (let index = 0; index < 160; index++) writePiTurn(manager, `重复消息 ${index}`, time + 3 + index * 2);
    const sessionId = manager.getSessionId();

    const opened = await client.call({ type: "session.open", requestId: "search-open", sessionId });
    expect(opened.hasEarlier).toBe(true);
    expect(opened.items.some((item) => item.id === "history-tool")).toBe(false);
    const results = await client.call({ type: "session.search", requestId: "search", sessionId, query: "needle-789", offset: 0 });
    expect(results).toEqual({ matches: [{ itemId: "history-tool", kind: "tool", excerpt: "旧工具 Needle-789 输出" }], total: 1 });
    const context = await client.call({ type: "session.around", requestId: "search-around", sessionId, itemId: "history-tool" });
    expect(context.items).toContainEqual(expect.objectContaining({ id: "history-tool", output: "旧工具 Needle-789 输出" }));
    expect(context.items).toContainEqual(expect.objectContaining({ text: "最早的消息" }));
    expect(context.items.length).toBeLessThanOrEqual(49);

    const laterMatches = await client.call({ type: "session.search", requestId: "search-offset", sessionId, query: "重复消息", offset: 100 });
    expect(laterMatches.total).toBe(160);
    expect(laterMatches.matches[0]?.excerpt).toBe("重复消息 59");

    const newestTurns = await client.call({ type: "session.turns", requestId: "turns", sessionId, offset: 0 });
    expect(newestTurns.total).toBe(161);
    expect(newestTurns.turns[0]).toMatchObject({ number: 161, preview: "重复消息 159" });
    const earlierTurns = await client.call({ type: "session.turns", requestId: "older-turns", sessionId, offset: 150 });
    expect(earlierTurns.turns.at(-1)).toEqual({ itemId: `u:${time}`, number: 1, preview: "最早的消息" });
    expect(opened.items.some((item) => item.id === earlierTurns.turns.at(-1)?.itemId)).toBe(false);
  } finally {
    client.ws.close();
  }
});
