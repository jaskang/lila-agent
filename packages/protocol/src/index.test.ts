import { expect, test } from "bun:test";
import { ClientCommand, PROTOCOL_VERSION, ServerHello, SessionEvent } from "./index.ts";

test("ServerHello accepts a well-formed hello", () => {
  const hello = { type: "hello", protocolVersion: PROTOCOL_VERSION, coreVersion: "0.0.0" } as const;
  expect(ServerHello.parse(hello)).toEqual(hello);
});

test("ClientCommand parses a session creation command", () => {
  expect(
    ClientCommand.parse({
      type: "session.create",
      requestId: "request-1",
      projectId: "workspace",
      approvalMode: "yolo",
    }),
  ).toMatchObject({ type: "session.create", projectId: "workspace" });
});

test("SessionEvent requires a positive sequence", () => {
  expect(
    SessionEvent.safeParse({
      type: "event",
      sessionId: "session-1",
      seq: 0,
      eventType: "session.created",
      payload: {},
    }).success,
  ).toBe(false);
});
