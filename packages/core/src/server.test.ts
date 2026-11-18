import { afterEach, expect, test } from "bun:test";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ClientCommand, ProtocolMessage } from "@lila/protocol";
import { startCore } from "./server.ts";

const servers: Array<{ stop(force?: boolean): Promise<void> }> = [];
afterEach(async () => { await Promise.all(servers.splice(0).map((server) => server.stop(true))); });

async function start(port: number) {
  const server = await startCore({ port, databasePath: join(mkdtempSync(join(tmpdir(), "lila-core-")), "lila.sqlite"), token: "secret" });
  servers.push(server);
  return server;
}

async function connect(port: number) {
  const ws = new WebSocket(`ws://127.0.0.1:${port}/ws`, ["lila.secret"]);
  const inbox: ProtocolMessage[] = [];
  const waiters: Array<() => void> = [];
  ws.onmessage = (event) => { inbox.push(JSON.parse(String(event.data))); for (const wake of waiters.splice(0)) wake(); };
  async function until<T extends ProtocolMessage>(match: (message: ProtocolMessage) => message is T): Promise<T> {
    for (;;) { const index = inbox.findIndex(match); if (index >= 0) return inbox.splice(index, 1)[0] as T; await new Promise<void>((resolve) => waiters.push(resolve)); }
  }
  async function call<D>(command: ClientCommand): Promise<D> {
    ws.send(JSON.stringify(command));
    const response = await until((message): message is Extract<ProtocolMessage, { type: "response" }> => message.type === "response" && message.requestId === command.requestId);
    if (!response.ok) throw new Error(response.error);
    return response.data as D;
  }
  await until((message) => message.type === "hello");
  return { ws, call };
}

test("rejects sockets without the token", async () => {
  await start(17461);
  const ws = new WebSocket("ws://127.0.0.1:17461/ws");
  const closed = await new Promise<boolean>((resolve) => { ws.onopen = () => resolve(false); ws.onerror = () => resolve(true); });
  expect(closed).toBe(true);
});

test("stores projects and durable sessions", async () => {
  await start(17462);
  const client = await connect(17462);
  const project = await client.call<{ project: { id: string } }>({ type: "projects.add", requestId: "p", path: mkdtempSync(join(tmpdir(), "lila-project-")) });
  const session = await client.call<{ session: { id: string; projectId: string; status: string } }>({ type: "session.create", requestId: "s", projectId: project.project.id });
  expect(session.session).toMatchObject({ projectId: project.project.id, status: "idle" });
  const list = await client.call<{ sessions: Array<{ id: string }> }>({ type: "sessions.list", requestId: "l" });
  expect(list.sessions.map((item) => item.id)).toContain(session.session.id);
  client.ws.close();
});
