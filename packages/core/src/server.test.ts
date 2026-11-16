import { afterAll, expect, test } from "bun:test";
import { ServerHello } from "@lila/protocol";
import { startCore } from "./server.ts";

// Bun 1.4.4 rejects port 0, so keep the test isolated on a fixed development port.
const server = startCore({ port: 17457 });
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
