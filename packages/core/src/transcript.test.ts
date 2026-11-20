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

test("merges persisted subagents:record entries back onto background Agent tool items", () => {
  const withSubagent = [
    {
      ...base("a1", null),
      type: "message",
      message: {
        role: "assistant",
        content: [{ type: "toolCall", id: "call-bg", name: "Agent", arguments: { subagent_type: "Explore", description: "检查源码", prompt: "分析目录结构" } }],
        api: "openai",
        provider: "openai",
        model: "gpt",
        usage,
        stopReason: "toolUse",
        timestamp: 10,
      },
    },
    {
      ...base("r1", "a1"),
      type: "message",
      message: {
        role: "toolResult",
        toolCallId: "call-bg",
        toolName: "Agent",
        content: [{ type: "text", text: "Agent started in background. Agent ID: ag-1" }],
        details: { agentId: "ag-1", status: "background", durationMs: 0 },
        isError: false,
        timestamp: 11,
        durationMs: 5,
      },
    },
    {
      ...base("c1", "r1"),
      type: "custom",
      customType: "subagents:record",
      data: { id: "ag-1", type: "Explore", description: "检查源码", status: "completed", result: "源码结构如下", startedAt: 1_000, completedAt: 16_000 },
    },
  ] as unknown as SessionEntry[];

  // Before subagents:record arrives, background launch boilerplate is filtered out of `item.subagent.output`.
  const pendingItems = projectBranch(withSubagent.slice(0, 2));
  expect(pendingItems[0]).toMatchObject({
    kind: "tool",
    id: "call-bg",
    name: "Agent",
    subagent: {
      id: "ag-1",
      type: "Explore",
      description: "检查源码",
      prompt: "分析目录结构",
      background: true,
      status: "running",
    },
  });
  expect(pendingItems[0]?.kind === "tool" ? pendingItems[0].subagent?.output : undefined).toBeUndefined();

  const items = projectBranch(withSubagent);
  expect(items).toEqual([
    {
      kind: "tool",
      id: "call-bg",
      name: "Agent",
      args: { subagent_type: "Explore", description: "检查源码", prompt: "分析目录结构" },
      status: "done",
      output: "源码结构如下",
      subagent: {
        id: "ag-1",
        type: "Explore",
        description: "检查源码",
        prompt: "分析目录结构",
        background: true,
        status: "completed",
        durationMs: 15_000,
        output: "源码结构如下",
      },
      details: { agentId: "ag-1", subagentType: "Explore", description: "检查源码", status: "completed", durationMs: 15_000 },
      durationMs: 15_000,
    },
  ]);
});

test("cleans subagent-notification XML and transcript file paths into structured custom items", () => {
  const xml = [
    "<task-notification>",
    "<task-id>ee9166ea-d698-4b4</task-id>",
    "<output-file>/var/folders/tmp/ee9166ea-d698-4b4.output</output-file>",
    "<status>Done</status>",
    '<summary>Agent "检查项目开发规范" completed</summary>',
    "<result>API 放在 src/api/&lt;domain&gt;.ts\n...(truncated, use get_subagent_result for full output)</result>",
    "<usage><total_tokens>2863</total_tokens><tool_uses>3</tool_uses><duration_ms>23882</duration_ms></usage>",
    "</task-notification>",
    "Full transcript available at: /var/folders/tmp/ee9166ea-d698-4b4.output",
  ].join("\n");

  const entries = [
    { ...base("n1", null), type: "custom_message", customType: "subagent-notification", content: xml, display: true },
  ] as unknown as SessionEntry[];

  const items = projectBranch(entries);
  expect(items).toEqual([
    {
      kind: "custom",
      id: `c:${Date.parse(new Date(1_000).toISOString())}`,
      customType: "subagent-notification",
      text: "API 放在 src/api/<domain>.ts…",
      notifications: [
        {
          id: "ee9166ea-d698-4b4",
          description: "检查项目开发规范",
          status: "completed",
          toolUses: 3,
          totalTokens: 2863,
          durationMs: 23882,
          preview: "API 放在 src/api/<domain>.ts…",
        },
      ],
    },
  ]);
});
