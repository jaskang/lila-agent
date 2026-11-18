import { expect, test } from "bun:test";
import { ClientCommand } from "./index.ts";

test("a prompt sent while the agent runs carries its delivery mode", () => {
  const command = ClientCommand.parse({ type: "session.prompt", requestId: "r", sessionId: "s", text: "also run the tests", mode: "steer" });
  expect(command).toMatchObject({ type: "session.prompt", mode: "steer" });
});

test("attachments must be images", () => {
  const result = ClientCommand.safeParse({
    type: "session.prompt",
    requestId: "r",
    sessionId: "s",
    text: "look",
    images: [{ mimeType: "application/pdf", data: "AAAA" }],
  });
  expect(result.success).toBe(false);
});
