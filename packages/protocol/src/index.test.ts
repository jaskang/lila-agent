import { expect, test } from "bun:test";
import { PROTOCOL_VERSION, ServerHello } from "./index.ts";

test("ServerHello accepts a well-formed hello", () => {
  const hello = { type: "hello", protocolVersion: PROTOCOL_VERSION, coreVersion: "0.0.0" } as const;
  expect(ServerHello.parse(hello)).toEqual(hello);
});

test("ServerHello rejects a different protocol version", () => {
  const result = ServerHello.safeParse({
    type: "hello",
    protocolVersion: 999,
    coreVersion: "0.0.0",
  });
  expect(result.success).toBe(false);
});
