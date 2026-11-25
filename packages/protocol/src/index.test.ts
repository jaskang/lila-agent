import { expect, test } from "bun:test";
import { ClientCommand, MAX_PROMPT_IMAGE_BASE64_BYTES } from "./index.ts";

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


test("rejects an image batch above the total attachment limit", () => {
  const image = { mimeType: "image/png", data: "A".repeat(MAX_PROMPT_IMAGE_BASE64_BYTES / 4) };
  const prompt = { type: "session.prompt", requestId: "r", sessionId: "s", text: "look" };
  expect(ClientCommand.safeParse({ ...prompt, images: Array(4).fill(image) }).success).toBe(true);
  expect(ClientCommand.safeParse({ ...prompt, images: Array(5).fill(image) }).success).toBe(false);
});
