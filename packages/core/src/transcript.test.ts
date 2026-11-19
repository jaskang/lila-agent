import { expect, test } from "bun:test";
import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import { earlier, parseUserItemId, projectBranch, snapshot } from "./transcript.ts";

const usage = { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } };
const base = (id: string, parentId: string | null) => ({ id, parentId, timestamp: new Date(1_000).toISOString() });
const patch = "--- a/src/a.ts\n+++ b/src/a.ts\n@@ -1 +1 @@\n-old\n+new\n";

const branch = [
  { ...base("m0", null), type: "model_change", provider: "anthropic", modelId: "claude" },
  { ...base("e1", "m0"), type: "message", message: { role: "user", content: "Fix a.ts", timestamp: 1 } },
  {
    ...base("e2", "e1"),
    type: "message",
    message: {
      role: "assistant",
      content: [
        { type: "thinking", thinking: "Need to edit." },
        { type: "text", text: "Editing." },
        { type: "toolCall", id: "call-1", name: "edit", arguments: { path: "src/a.ts" } },
      ],
      api: "anthropic-messages",
      provider: "anthropic",
      model: "claude",
      usage,
      stopReason: "toolUse",
      timestamp: 2,
    },
  },
  {
    ...base("e3", "e2"),
    type: "message",
    message: {
      role: "toolResult",
      toolCallId: "call-1",
      toolName: "edit",
      content: [{ type: "text", text: "Edited src/a.ts" }],
      details: { diff: "-old\n+new", patch, firstChangedLine: 1 },
      isError: false,
      timestamp: 3,
      durationMs: 12,
    },
  },
  { ...base("e4", "e3"), type: "compaction", summary: "…", firstKeptEntryId: "e1", tokensBefore: 1234 },
] as unknown as SessionEntry[];

test("projects a branch into transcript items", () => {
  const items = projectBranch(branch);
  expect(items).toEqual([
    { kind: "user", id: "u:1", text: "Fix a.ts", images: [], canRewind: true },
    { kind: "assistant", id: "a:2", text: "Editing.", thinking: "Need to edit.", model: "anthropic/claude", streaming: false },
    {
      kind: "tool",
      id: "call-1",
      name: "edit",
      args: { path: "src/a.ts" },
      status: "done",
      output: "Edited src/a.ts",
      diff: patch,
      details: { diff: "-old\n+new", patch, firstChangedLine: 1 },
      durationMs: 12,
    },
    { kind: "notice", id: "n:e4", tone: "info", text: "已压缩上下文（压缩前 1234 tokens）" },
  ]);

  expect(snapshot(items, 2)).toEqual({ items: items.slice(2), hasEarlier: true });
  expect(earlier(items, "call-1", 1)).toEqual({ items: items.slice(1, 2), hasEarlier: true });
});

test("marks a tool call without a result as unfinished unless the agent still runs", () => {
  const cut = branch.slice(0, 3);
  expect(projectBranch(cut).at(-1)).toMatchObject({ id: "call-1", status: "error" });
  expect(projectBranch(cut, { running: true }).at(-1)).toMatchObject({ id: "call-1", status: "running" });
});

test("disambiguates items that share the same millisecond timestamp", () => {
  const sameTs = [
    { ...base("u1", null), type: "message", message: { role: "user", content: "First", timestamp: 100 } },
    { ...base("u2", "u1"), type: "message", message: { role: "user", content: "Second", timestamp: 100 } },
  ] as unknown as SessionEntry[];
  const items = projectBranch(sameTs);
  expect(items.map((item) => item.id)).toEqual(["u:100", "u:100#2"]);
  expect(parseUserItemId("u:100")).toEqual({ timestamp: 100, occurrence: 1 });
  expect(parseUserItemId("u:100#2")).toEqual({ timestamp: 100, occurrence: 2 });
  expect(parseUserItemId("a:100")).toBeUndefined();
});
