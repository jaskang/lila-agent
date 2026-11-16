import { afterAll, expect, test } from "bun:test";
import { CoreResponse, ServerHello, SessionEvent } from "@lila/protocol";
import { startCore } from "./server.ts";

// Bun 1.4.4 rejects port 0, so keep the test isolated on a fixed development port.
const server = startCore({ port: 17457, databasePath: ":memory:", projectPath: "/tmp/lila-test" });
afterAll(() => server.stop(true));

test("GET /health reports ok", async () => {
  const res = await fetch(new URL("/health", server.url));
  expect(res.status).toBe(200);
  expect(await res.json()).toMatchObject({ ok: true });
});

test("a WebSocket client receives a valid hello first", async () => {
  const ws = new WebSocket(`ws://${server.hostname}:${server.port}/ws`);
  const data = await new Promise<string>((resolve, reject) => {
    ws.onmessage = (e) => resolve(String(e.data));
    ws.onerror = () => reject(new Error("WebSocket error"));
  });
  ws.close();
  expect(ServerHello.safeParse(JSON.parse(data)).success).toBe(true);
});

test("creates a session and emits an ordered event", async () => {
  const ws = new WebSocket(`ws://${server.hostname}:${server.port}/ws`);
  const hello = await new Promise<string>((resolve, reject) => {
    ws.onopen = () => undefined;
    ws.onmessage = (event) => resolve(String(event.data));
    ws.onerror = () => reject(new Error("WebSocket error"));
  });
  expect(ServerHello.safeParse(JSON.parse(hello)).success).toBe(true);

  const messages = new Promise<unknown[]>((resolve, reject) => {
    const received: unknown[] = [];
    ws.onmessage = (event) => {
      received.push(JSON.parse(String(event.data)));
      if (received.length === 2) resolve(received);
    };
    ws.onerror = () => reject(new Error("WebSocket error"));
  });
  ws.send(JSON.stringify({ type: "session.create", requestId: "request-1", projectId: "workspace" }));
  const [event, response] = await messages;
  ws.close();

  expect(SessionEvent.parse(event)).toMatchObject({
    type: "event",
    seq: 1,
    eventType: "session.created",
    requestId: "request-1",
  });
  expect(CoreResponse.parse(response)).toMatchObject({
    type: "response",
    command: "session.create",
    requestId: "request-1",
    success: true,
  });
});
