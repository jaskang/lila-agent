import { PROTOCOL_VERSION, ServerHello } from "@lila/protocol";
import pkg from "../package.json" with { type: "json" };

export interface CoreOptions {
  port: number;
  hostname?: string;
}

export function startCore({ port, hostname = "127.0.0.1" }: CoreOptions) {
  const hello = ServerHello.parse({
    type: "hello",
    protocolVersion: PROTOCOL_VERSION,
    coreVersion: pkg.version,
  });

  return Bun.serve({
    hostname,
    port,
    fetch(req, server) {
      const { pathname } = new URL(req.url);
      if (pathname === "/health") {
        return Response.json({ ok: true, coreVersion: pkg.version, pid: process.pid });
      }
      if (pathname === "/ws") {
        if (server.upgrade(req)) return;
        return new Response("Expected a WebSocket upgrade", { status: 426 });
      }
      return new Response("Not found", { status: 404 });
    },
    websocket: {
      open(ws) {
        console.log(`[core] client connected (${ws.remoteAddress})`);
        ws.send(JSON.stringify(hello));
      },
      message() {},
      close(ws, code) {
        console.log(`[core] client disconnected (${ws.remoteAddress}, code ${code})`);
      },
    },
  });
}
