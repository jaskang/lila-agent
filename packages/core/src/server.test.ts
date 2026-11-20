import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
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

async function connect() {
  const ws = new WebSocket(`ws://127.0.0.1:${core.port}/ws`, ["lila.secret"]);
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
