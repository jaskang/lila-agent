import { expect, test } from "vitest";
import { mkdtempSync, mkdirSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ToolInfo } from "@earendil-works/pi-coding-agent";
import { approvalReason, isSafeReadonlyBash, matchesApprovalRule, suggestApprovalRule } from "./approval-policy.ts";

const builtin = (name: string, annotations?: ToolInfo["annotations"]): Pick<ToolInfo, "sourceInfo" | "annotations"> => ({
  sourceInfo: { path: `builtin:${name}`, source: "builtin", scope: "temporary", origin: "top-level" },
  ...(annotations ? { annotations } : {}),
});

test("allows project writes and prompts for external paths, including through symlinks", () => {
  const root = mkdtempSync(join(tmpdir(), "lila-approval-"));
  try {
    const projectRoot = join(root, "project");
    const outside = join(root, "project-sibling");
    mkdirSync(projectRoot);
    mkdirSync(outside);
    symlinkSync(outside, join(projectRoot, "link"), "dir");
    symlinkSync(join(outside, "missing"), join(projectRoot, "dangling"), "dir");
    const write = (path: string) => approvalReason({
      mode: "ask-all", projectRoot, cwd: projectRoot, name: "write", input: { path }, tool: builtin("write"),
    });

    expect(write("new-dir/notes.md")).toBeUndefined();
    expect(write(join(outside, "notes.md"))).toContain("写入项目目录外");
    expect(write("../project-sibling/notes.md")).toContain("写入项目目录外");
    expect(write("link/new-dir/notes.md")).toContain("project-sibling");
    expect(write("dangling/notes.md")).toBe("无法确认文件写入位置");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("distinguishes per-call approval, low-risk MCP approval, and full access", () => {
  const context = { projectRoot: process.cwd(), cwd: process.cwd() };
  const list = { ...context, name: "list_mcp_resources", input: {}, tool: builtin("mcp") };
  const lookup = { ...context, name: "mcp__catalog__lookup", input: {}, tool: builtin("mcp", { readOnlyHint: true, openWorldHint: false }) };
  const unknownMcp = { ...lookup, tool: builtin("mcp", { readOnlyHint: true }) };
  const shell = { ...context, name: "bash", input: { command: "npm test" }, tool: builtin("bash") };

  expect(approvalReason({ ...list, mode: "ask-all" })).toContain("MCP");
  expect(approvalReason({ ...list, mode: "ask-exec" })).toBeUndefined();
  expect(approvalReason({ ...lookup, mode: "ask-exec" })).toBeUndefined();
  expect(approvalReason({ ...unknownMcp, mode: "ask-exec" })).toContain("MCP");
  expect(approvalReason({ ...shell, mode: "ask-exec" })).toContain("无法确认");
  expect(approvalReason({ ...shell, mode: "auto" })).toBeUndefined();
});

test("preserves built-in read access and treats overridden tools as extensions", () => {
  const projectRoot = process.cwd();
  const read = { projectRoot, cwd: projectRoot, mode: "ask-all", name: "read", input: { path: "../notes.md" } } as const;
  expect(approvalReason({ ...read, tool: builtin("read") })).toBeUndefined();
  expect(approvalReason({
    ...read,
    tool: { sourceInfo: { ...builtin("read").sourceInfo, path: "project-extension.ts" }, annotations: { readOnlyHint: true } },
  })).toContain("无法确认");
});

test("auto-allows statically verified read-only bash and memory tools in ask-exec mode", () => {
  const context = { projectRoot: process.cwd(), cwd: process.cwd() };
  expect(isSafeReadonlyBash("git status")).toBe(true);
  expect(isSafeReadonlyBash("git log -n 5 | head -n 3")).toBe(true);
  expect(isSafeReadonlyBash("find src -name '*.ts'")).toBe(true);
  expect(isSafeReadonlyBash("node --version")).toBe(true);

  expect(isSafeReadonlyBash("git status > out.txt")).toBe(false);
  expect(isSafeReadonlyBash("echo $(whoami)")).toBe(false);
  expect(isSafeReadonlyBash("git -c core.pager=sh log")).toBe(false);
  expect(isSafeReadonlyBash("find . -delete")).toBe(false);
  expect(isSafeReadonlyBash("npm test")).toBe(false);

  expect(approvalReason({ ...context, mode: "ask-exec", name: "bash", input: { command: "git diff --stat" }, tool: builtin("bash") })).toBeUndefined();
  expect(approvalReason({ ...context, mode: "ask-all", name: "bash", input: { command: "git diff --stat" }, tool: builtin("bash") })).toContain("无法确认");

  const memTool = { sourceInfo: { ...builtin("memory_add").sourceInfo, path: "pi-memory.ts" } };
  expect(approvalReason({ ...context, mode: "ask-exec", name: "memory_add", input: { content: "x" }, tool: memTool })).toBeUndefined();
  expect(approvalReason({ ...context, mode: "ask-all", name: "memory_add", input: { content: "x" }, tool: memTool })).toContain("长期记忆");
});

test("suggests and matches scoped approval rules without allowing chained shell bypasses", () => {
  const context = { projectRoot: process.cwd(), cwd: process.cwd() };
  const bashTool = builtin("bash");

  expect(suggestApprovalRule("bash", { command: "pnpm test --run" }, bashTool)).toEqual({
    pattern: "bash:pnpm test",
    label: "pnpm test *",
  });
  expect(suggestApprovalRule("bash", { command: "rm -rf /tmp/x" }, bashTool)).toBeUndefined();
  expect(suggestApprovalRule("bash", { command: "pnpm test && rm -rf /tmp/x" }, bashTool)).toBeUndefined();

  const rules = ["bash:pnpm test"];
  expect(matchesApprovalRule(rules, "bash", { command: "pnpm test src/git.test.ts" }, bashTool)).toBe(true);
  expect(matchesApprovalRule(rules, "bash", { command: "pnpm test && rm -rf /" }, bashTool)).toBe(false);
  expect(approvalReason({ ...context, mode: "ask-exec", name: "bash", input: { command: "pnpm test" }, tool: bashTool, rules })).toBeUndefined();
  expect(approvalReason({ ...context, mode: "ask-exec", name: "bash", input: { command: "pnpm build" }, tool: bashTool, rules })).toContain("无法确认");
});
