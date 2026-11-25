import { afterAll, beforeAll, expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { SessionManager } from "@earendil-works/pi-coding-agent";
import type { ClientCommand, CommandResults, CommandType, ServerMessage } from "@lila/protocol";
import { type Core, startCore } from "./server.ts";

let core: Core;

beforeAll(async () => {
  // Keep the user's Pi configuration and sessions out of the test.
  process.env.PI_CODING_AGENT_DIR = mkdtempSync(join(tmpdir(), "lila-agent-dir-"));
  process.env.PI_OFFLINE = "1";
  core = await startCore({ port: 0, dataDir: mkdtempSync(join(tmpdir(), "lila-core-")), token: "secret" });
});

afterAll(async () => {
  await core.stop();
});

async function connect(target = core) {
  const ws = new WebSocket(`ws://127.0.0.1:${target.port}/ws`, ["lila.secret"]);
  const inbox: ServerMessage[] = [];
  const waiters: Array<() => void> = [];
  ws.onmessage = (event) => {
    inbox.push(JSON.parse(String(event.data)) as ServerMessage);
    for (const wake of waiters.splice(0)) wake();
  };
  async function until(match: (message: ServerMessage) => boolean): Promise<ServerMessage> {
    for (;;) {
      const index = inbox.findIndex(match);
      if (index >= 0) return inbox.splice(index, 1)[0] as ServerMessage;
      await new Promise<void>((resolve) => waiters.push(resolve));
    }
  }
  async function call<T extends CommandType>(command: Extract<ClientCommand, { type: T }>): Promise<CommandResults[T]> {
    ws.send(JSON.stringify(command));
    const response = await until((message) => message.type === "response" && message.requestId === command.requestId);
    if (response.type !== "response") throw new Error("unreachable");
    if (!response.ok) throw new Error(response.error);
    return response.data as CommandResults[T];
  }
  await until((message) => message.type === "hello");
  return { ws, call, inbox, until };
}

test("rejects sockets without the token", async () => {
  const ws = new WebSocket(`ws://127.0.0.1:${core.port}/ws`);
  const opened = await new Promise<boolean>((resolve) => {
    ws.onopen = () => resolve(true);
    ws.onerror = () => resolve(false);
  });
  expect(opened).toBe(false);
});

test("creates a session in an added project, watches and unwatches it, and validates session.open", async () => {
  const client = await connect();
  const { project } = await client.call({ type: "projects.add", requestId: "p", path: mkdtempSync(join(tmpdir(), "lila-project-")) });
  const { session } = await client.call({ type: "session.create", requestId: "s", projectId: project.id, approvalMode: "ask-all" });
  expect(session).toMatchObject({ projectId: project.id, status: "idle", pendingCount: 0, title: "新会话" });

  const { sessions } = await client.call({ type: "sessions.list", requestId: "l" });
  expect(sessions.map((item) => item.id)).toContain(session.id);

  // Invalid session.open (unknown id) returns a clean error.
  await expect(client.call({ type: "session.open", requestId: "bad-open", sessionId: "non-existent-session" })).rejects.toThrow("会话不存在");

  const opened = await client.call({ type: "session.open", requestId: "o", sessionId: session.id });
  expect(opened.items).toEqual([]);
  expect(opened.state.approvalMode).toBe("ask-all");

  // Watching receives state pushes on session.configure.
  client.inbox.length = 0;
  await client.call({ type: "session.configure", requestId: "cfg-1", sessionId: session.id, approvalMode: "auto" });
  const stateMsg = await client.until((message) => message.type === "state" && message.sessionId === session.id);
  expect(stateMsg.type === "state" && stateMsg.state.approvalMode).toBe("auto");

  // After session.unwatch, further session.configure calls do not push state frames to this socket.
  await client.call({ type: "session.unwatch", requestId: "u" });
  client.inbox.length = 0;
  await client.call({ type: "session.configure", requestId: "cfg-2", sessionId: session.id, approvalMode: "ask-exec" });
  expect(client.inbox.some((message) => message.type === "state" && message.sessionId === session.id)).toBe(false);

  client.ws.close();
}, 60_000);

function writePiTurn(manager: SessionManager, text: string, timestamp = Date.now()): void {
  manager.appendMessage({ role: "user", content: text, timestamp });
  manager.appendMessage({
    role: "assistant",
    content: [{ type: "text", text: "已收到" }],
    api: "openai-completions",
    provider: "fixture",
    model: "fixture",
    usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
    stopReason: "stop",
    timestamp: timestamp + 1,
  });
}

test("exposes an extension's startup question before session initialization finishes", async () => {
  const client = await connect();
  try {
    const path = mkdtempSync(join(tmpdir(), "lila-project-startup-"));
    const extensions = join(path, ".pi", "extensions");
    mkdirSync(extensions, { recursive: true });
    writeFileSync(join(extensions, "question.ts"), `
      export default function (pi) {
        pi.registerCommand("after-startup", {
          description: "Record when the first submission arrives",
          handler: async (_args, ctx) => ctx.ui.setStatus("first", "已收到"),
        });
        pi.on("session_start", async (_event, ctx) => {
          const answer = await ctx.ui.input("启动问题");
          ctx.ui.setStatus("startup", answer);
        });
      }
    `);
    const { project } = await client.call({ type: "projects.add", requestId: "p", path });
    const { session } = await client.call({ type: "session.create", requestId: "c", projectId: project.id });
    const opened = await client.call({ type: "session.open", requestId: "o", sessionId: session.id });
    const isStartupQuestion = (item: { title: string }) => item.title === "启动问题";
    const frame = opened.state.pending.some(isStartupQuestion) ? undefined : await client.until(
      (message) => message.type === "state" && message.sessionId === session.id && message.state.pending.some(isStartupQuestion),
    );
    const state = frame?.type === "state" ? frame.state : opened.state;
    const pending = state.pending.find(isStartupQuestion);
    expect(state.access).toBe("starting");
    expect(pending).toBeDefined();
    const prompting = client.call({ type: "session.prompt", requestId: "first", sessionId: session.id, text: "/after-startup" });
    const waiting = await client.call({ type: "session.open", requestId: "waiting", sessionId: session.id });
    expect(waiting.state.statusLines.first).toBeUndefined();
    await client.call({ type: "session.respond", requestId: "respond", sessionId: session.id, pendingId: pending!.id, answer: { type: "text", value: "继续" } });
    await prompting;
    const finished = await client.call({ type: "session.open", requestId: "ready", sessionId: session.id });
    expect(finished.state).toMatchObject({ access: "ready", statusLines: { startup: "继续", first: "已收到" } });
  } finally {
    client.ws.close();
  }
}, 3_000);

test("deletes an existing Pi file before the session index is loaded", async () => {
  const coldCore = await startCore({ port: 0, dataDir: mkdtempSync(join(tmpdir(), "lila-cold-core-")), token: "secret" });
  const client = await connect(coldCore);
  try {
    const path = mkdtempSync(join(tmpdir(), "lila-project-cold-delete-"));
    await client.call({ type: "projects.add", requestId: "p", path });
    const manager = SessionManager.create(path);
    writePiTurn(manager, "已有会话");
    const sessionId = manager.getSessionId();
    const file = manager.getSessionFile()!;
    expect(existsSync(file)).toBe(true);
    await client.call({ type: "session.delete", requestId: "d", sessionId });
    expect(existsSync(file)).toBe(false);
    const { sessions } = await client.call({ type: "sessions.list", requestId: "l" });
    expect(sessions.some((item) => item.id === sessionId)).toBe(false);
  } finally {
    client.ws.close();
    await coldCore.stop();
  }
});

test("previews a read-only sub-agent while the parent stays subscribed", async () => {
  const client = await connect();
  try {
    const path = mkdtempSync(join(tmpdir(), "lila-project-subagent-"));
    await client.call({ type: "projects.add", requestId: "p", path });
    const parent = SessionManager.create(path);
    writePiTurn(parent, "父会话");
    const child = SessionManager.create(path, undefined, { parentSession: parent.getSessionFile()! });
    child.appendSessionInfo("Code Reviewer#01234567");
    writePiTurn(child, "开始检查");
    const extensions = join(path, ".pi", "extensions");
    mkdirSync(extensions, { recursive: true });
    writeFileSync(join(extensions, "background.ts"), `
      export default function (pi) {
        const child = { id: "0123456789abcdef", type: "Code Reviewer", description: "检查", isBackground: true };
        pi.on("session_start", (_event, ctx) => {
          if (ctx.sessionManager.getSessionId() !== ${JSON.stringify(parent.getSessionId())}) return;
          pi.events.emit("subagents:created", child);
          pi.events.emit("subagents:started", child);
        });
        pi.registerCommand("finish-fixture-child", {
          description: "Finish the test child",
          handler: async () => pi.events.emit("subagents:completed", { ...child, status: "completed" }),
        });
      }
    `);
    const parentId = parent.getSessionId();
    const parentOpened = await client.call({ type: "session.open", requestId: "parent", sessionId: parentId });
    if (parentOpened.state.access !== "ready") {
      await client.until((message) => message.type === "state" && message.sessionId === parentId && message.state.access === "ready");
    }
    const parentView = await client.call({ type: "session.open", requestId: "parent-ready", sessionId: parentId });
    expect(parentView.state).toMatchObject({ status: "running", runActive: false });
    await expect(client.call({ type: "session.prompt", requestId: "stale-queue", sessionId: parentId, text: "排队", mode: "followUp" })).rejects.toThrow("当前回复已结束");

    const sessionId = child.getSessionId();
    await expect(client.call({ type: "session.open", requestId: "bad-preview", sessionId: parentId, slot: "preview" })).rejects.toThrow("仅支持子 Agent");
    const opened = await client.call({ type: "session.open", requestId: "o", sessionId, slot: "preview" });
    expect(opened.items).toContainEqual(expect.objectContaining({ text: "开始检查", canRewind: false }));
    expect(opened.state).toMatchObject({ access: "read-only", status: "running", runActive: false });
    await expect(client.call({ type: "session.fork", requestId: "early-fork", sessionId })).rejects.toThrow("结束后");
    await expect(client.call({ type: "session.delete", requestId: "early-delete", sessionId })).rejects.toThrow("结束后");
    await expect(client.call({ type: "session.rename", requestId: "r", sessionId, title: "覆盖" })).rejects.toThrow("只读");
    writePiTurn(child, "新的结果", Date.now() + 100);
    await client.until((message) => message.type === "reset" && message.sessionId === sessionId);
    const refreshed = await client.call({ type: "session.open", requestId: "refresh", sessionId, slot: "preview" });
    expect(refreshed.items).toContainEqual(expect.objectContaining({ text: "新的结果" }));
    client.inbox.length = 0;
    await client.call({ type: "session.prompt", requestId: "finish-child", sessionId: parentId, text: "/finish-fixture-child" });
    await client.until((message) => message.type === "state" && message.sessionId === parentId && message.state.status === "idle");
    await client.call({ type: "session.unwatch", requestId: "close-preview", slot: "preview" });
    await client.call({ type: "session.configure", requestId: "parent-configure", sessionId: parentId, approvalMode: "ask-exec" });
    await client.until((message) => message.type === "state" && message.sessionId === parentId && message.state.approvalMode === "ask-exec");
    const { session: fork } = await client.call({ type: "session.fork", requestId: "fork", sessionId });
    expect(fork.id).not.toBe(sessionId);
    const forked = await client.call({ type: "session.open", requestId: "open-fork", sessionId: fork.id });
    expect(forked.state.access).not.toBe("read-only");
    expect(forked.items).toContainEqual(expect.objectContaining({ text: "新的结果", canRewind: true }));
    const { sessions } = await client.call({ type: "sessions.list", requestId: "list" });
    expect(sessions.find((entry) => entry.id === fork.id)?.parentId).toBeUndefined();
  } finally {
    client.ws.close();
  }
}, 5_000);

test("searches beyond the open page and returns a window around a folded tool result", async () => {
  const client = await connect();
  try {
    const path = mkdtempSync(join(tmpdir(), "lila-project-search-"));
    await client.call({ type: "projects.add", requestId: "search-project", path });
    const manager = SessionManager.create(path);
    const time = Date.now();
    writePiTurn(manager, "最早的消息", time);
    manager.appendMessage({
      role: "toolResult", toolCallId: "history-tool", toolName: "read", content: [{ type: "text", text: "旧工具 Needle-789 输出" }],
      isError: false, timestamp: time + 2,
    });
    for (let index = 0; index < 160; index++) writePiTurn(manager, `重复消息 ${index}`, time + 3 + index * 2);
    const sessionId = manager.getSessionId();

    const opened = await client.call({ type: "session.open", requestId: "search-open", sessionId });
    expect(opened.hasEarlier).toBe(true);
    expect(opened.items.some((item) => item.id === "history-tool")).toBe(false);
    const results = await client.call({ type: "session.search", requestId: "search", sessionId, query: "needle-789", offset: 0 });
    expect(results).toEqual({ matches: [{ itemId: "history-tool", kind: "tool", excerpt: "旧工具 Needle-789 输出" }], total: 1 });
    const context = await client.call({ type: "session.around", requestId: "search-around", sessionId, itemId: "history-tool" });
    expect(context.items).toContainEqual(expect.objectContaining({ id: "history-tool", output: "旧工具 Needle-789 输出" }));
    expect(context.items).toContainEqual(expect.objectContaining({ text: "最早的消息" }));
    expect(context.items.length).toBeLessThanOrEqual(49);

    const laterMatches = await client.call({ type: "session.search", requestId: "search-offset", sessionId, query: "重复消息", offset: 100 });
    expect(laterMatches.total).toBe(160);
    expect(laterMatches.matches[0]?.excerpt).toBe("重复消息 59");

    const newestTurns = await client.call({ type: "session.turns", requestId: "turns", sessionId, offset: 0 });
    expect(newestTurns.total).toBe(161);
    expect(newestTurns.turns[0]).toMatchObject({ number: 161, preview: "重复消息 159" });
    const earlierTurns = await client.call({ type: "session.turns", requestId: "older-turns", sessionId, offset: 150 });
    expect(earlierTurns.turns.at(-1)).toEqual({ itemId: `u:${time}`, number: 1, preview: "最早的消息" });
    expect(opened.items.some((item) => item.id === earlierTurns.turns.at(-1)?.itemId)).toBe(false);
  } finally {
    client.ws.close();
  }
});

test("lists project files and reads in-project markdown while blocking out-of-project paths", async () => {
  const client = await connect();
  const projectDir = mkdtempSync(join(tmpdir(), "lila-files-"));
  const outsideFile = join(mkdtempSync(join(tmpdir(), "lila-outside-")), "secret.txt");
  writeFileSync(outsideFile, "top secret");
  mkdirSync(join(projectDir, "docs", "设计参考"), { recursive: true });
  writeFileSync(join(projectDir, "docs", "plan.md"), "# Plan\n\nHello Lila\n");
  writeFileSync(join(projectDir, "docs", "设计参考", "规范.md"), "# Spec\n");

  try {
    const { project } = await client.call({ type: "projects.add", requestId: "files-add", path: projectDir });
    const listed = await client.call({ type: "files.list", requestId: "files-list", projectId: project.id });
    expect(listed.files).toContain("docs/plan.md");
    expect(listed.files).toContain("docs/设计参考/规范.md");

    const { file } = await client.call({ type: "files.read", requestId: "files-read", projectId: project.id, path: "docs/plan.md" });
    expect(file).toMatchObject({ path: "docs/plan.md", isBinary: false, isImage: false, truncated: false, content: "# Plan\n\nHello Lila\n" });

    await expect(
      client.call({ type: "files.read", requestId: "files-outside", projectId: project.id, path: outsideFile }),
    ).rejects.toThrow("不允许读取项目目录之外的文件");
  } finally {
    client.ws.close();
  }
});

test("reads and updates Pi settings and runs interactive API key login/logout", async () => {
  const client = await connect();
  try {
    const initial = await client.call({ type: "settings.get", requestId: "s-get-1" });
    expect(initial.settings.paths.agentDir).toBe(process.env.PI_CODING_AGENT_DIR);

    const updated = await client.call({
      type: "settings.update",
      requestId: "s-upd",
      defaultThinkingLevel: "high",
      autoCompaction: false,
      autoRetry: false,
    });
    expect(updated.settings).toMatchObject({ defaultThinkingLevel: "high", autoCompaction: false, autoRetry: false });

    const { providers } = await client.call({ type: "providers.list", requestId: "p-list-1" });
    const apiKeyProvider = providers.find((provider) => provider.id === "anthropic" && provider.supportsApiKey) ?? providers.find((provider) => provider.supportsApiKey);
    expect(apiKeyProvider).toBeDefined();

    const { loginId } = await client.call({
      type: "auth.login.start",
      requestId: "auth-start",
      providerId: apiKeyProvider!.id,
      method: "api_key",
    });

    const promptFrame = await client.until((message) => message.type === "auth.login" && message.loginId === loginId && message.event.type === "prompt");
    expect(promptFrame.type === "auth.login" && promptFrame.event.type === "prompt").toBe(true);
    if (promptFrame.type !== "auth.login" || promptFrame.event.type !== "prompt") throw new Error("unreachable");

    await client.call({
      type: "auth.login.respond",
      requestId: "auth-resp",
      loginId,
      promptId: promptFrame.event.prompt.promptId,
      value: "test-api-key-12345",
    });

    const doneFrame = await client.until((message) => message.type === "auth.login" && message.loginId === loginId && (message.event.type === "done" || message.event.type === "error"));
    expect(doneFrame.type === "auth.login" && doneFrame.event.type).toBe("done");

    const afterLogin = await client.call({ type: "providers.list", requestId: "p-list-2" });
    const loggedIn = afterLogin.providers.find((provider) => provider.id === apiKeyProvider!.id);
    expect(loggedIn?.configured).toBe(true);
    expect(loggedIn?.storedType).toBe("api_key");

    const listedModels = await client.call({ type: "models.list", requestId: "m-list-1" });
    const providerModels = listedModels.models.filter((item) => item.provider === apiKeyProvider!.id);
    expect(providerModels.length).toBeGreaterThan(1);
    const picked = providerModels[0]!;
    const filtered = await client.call({
      type: "provider.models.set",
      requestId: "p-models-set-1",
      providerId: apiKeyProvider!.id,
      modelIds: [picked.id],
    });
    const filteredProviderModels = filtered.models.filter((item) => item.provider === apiKeyProvider!.id);
    expect(filteredProviderModels.filter((item) => item.enabled).map((item) => item.id)).toEqual([picked.id]);

    const restored = await client.call({
      type: "provider.models.set",
      requestId: "p-models-set-2",
      providerId: apiKeyProvider!.id,
      modelIds: null,
    });
    const restoredProviderModels = restored.models.filter((item) => item.provider === apiKeyProvider!.id);
    expect(restoredProviderModels.every((item) => item.enabled)).toBe(true);

    await client.call({ type: "auth.logout", requestId: "auth-logout", providerId: apiKeyProvider!.id });
    const afterLogout = await client.call({ type: "providers.list", requestId: "p-list-3" });
    const loggedOut = afterLogout.providers.find((provider) => provider.id === apiKeyProvider!.id);
    expect(loggedOut?.storedType).toBeUndefined();
  } finally {
    client.ws.close();
  }
}, 30_000);

test("manages global and project MCP servers and reads capabilities snapshot", async () => {
  const client = await connect();
  const projectDir = mkdtempSync(join(tmpdir(), "lila-mcp-proj-"));
  writeFileSync(join(projectDir, "AGENTS.md"), "# Project context\n");
  try {
    const { project } = await client.call({ type: "projects.add", requestId: "mcp-proj", path: projectDir });

    const addedGlobal = await client.call({
      type: "mcp.upsert",
      requestId: "mcp-add-1",
      scope: "global",
      projectId: project.id,
      name: "docs",
      config: { transport: "http", url: "https://example.com/mcp", description: "文档服务" },
    });
    expect(addedGlobal.config.servers).toContainEqual(
      expect.objectContaining({ name: "docs", scope: "global", transport: "http", url: "https://example.com/mcp", enabled: true, exposure: "codemode" }),
    );

    const addedProject = await client.call({
      type: "mcp.upsert",
      requestId: "mcp-add-2",
      scope: "project",
      projectId: project.id,
      name: "fs",
      config: { transport: "stdio", command: "npx", args: ["-y", "demo-mcp"], exposure: "direct" },
    });
    expect(addedProject.config.servers).toContainEqual(
      expect.objectContaining({ name: "fs", scope: "project", transport: "stdio", command: "npx", args: ["-y", "demo-mcp"], exposure: "direct" }),
    );

    const toggled = await client.call({
      type: "mcp.toggle",
      requestId: "mcp-toggle",
      scope: "global",
      projectId: project.id,
      name: "docs",
      enabled: false,
      exposure: "deferred",
    });
    expect(toggled.config.servers.find((item) => item.name === "docs")).toMatchObject({ enabled: false, exposure: "deferred" });

    const removed = await client.call({
      type: "mcp.remove",
      requestId: "mcp-remove",
      scope: "global",
      projectId: project.id,
      name: "docs",
    });
    expect(removed.config.servers.some((item) => item.name === "docs")).toBe(false);

    const { capabilities } = await client.call({ type: "capabilities.get", requestId: "cap-get", projectId: project.id });
    expect(capabilities.agentDir).toBe(process.env.PI_CODING_AGENT_DIR);
    expect(capabilities.contextFiles.some((file) => file.endsWith("AGENTS.md"))).toBe(true);
  } finally {
    client.ws.close();
  }
}, 30_000);

test("MCP import validates the whole batch before writing and replaces entries together", async () => {
  const client = await connect();
  const projectDir = mkdtempSync(join(tmpdir(), "lila-mcp-import-"));
  try {
    const { project } = await client.call({ type: "projects.add", requestId: "import-project", path: projectDir });
    const request = { type: "mcp.import", requestId: "import", scope: "project", projectId: project.id } as const;
    const docs = { name: "docs", config: { transport: "http", url: "http://localhost:9999/mcp", enabled: false } } as const;
    const invalid = { name: "invalid", config: { transport: "http", url: "file:///tmp/invalid", enabled: false } } as const;
    await expect(client.call({ ...request, entries: [docs, invalid] })).rejects.toThrow("请输入有效的 HTTP 或 HTTPS 地址");
    expect(existsSync(join(projectDir, ".pi", "mcp.json"))).toBe(false);

    const reference = { name: "reference", config: { transport: "stdio", command: "fixture", enabled: false } } as const;
    const { config } = await client.call({ ...request, entries: [docs, reference] });
    const servers = config.servers.filter((item) => item.scope === "project");
    expect(servers.map((item) => item.name)).toEqual(["docs", "reference"]);
    expect(servers.every((item) => !item.enabled)).toBe(true);
  } finally {
    client.ws.close();
  }
});

test("manages persistent memory entries, config, and registers built-in pi-memory extension", async () => {
  const client = await connect();
  const projectDir = mkdtempSync(join(tmpdir(), "lila-memory-proj-"));
  try {
    const { project } = await client.call({ type: "projects.add", requestId: "mem-proj", path: projectDir });

    const initial = await client.call({ type: "memory.get", requestId: "mem-get-1", projectId: project.id });
    expect(initial.memory.enabled).toBe(true);
    expect(initial.memory.dir).toBe(join(process.env.PI_CODING_AGENT_DIR!, "memories"));
    expect(initial.memory.memory.entries).toEqual([]);
    expect(initial.memory.user.entries).toEqual([]);

    const added = await client.call({
      type: "memory.add",
      requestId: "mem-add-1",
      target: "user",
      content: "使用中文回复并优先使用 Bun",
    });
    expect(added.memory.user.entries).toEqual(["使用中文回复并优先使用 Bun"]);
    expect(added.memory.user.charCount).toBeGreaterThan(0);

    const replaced = await client.call({
      type: "memory.replace",
      requestId: "mem-rep-1",
      target: "user",
      oldText: "优先使用 Bun",
      newContent: "始终使用中文回复，包管理统一用 Bun",
    });
    expect(replaced.memory.user.entries).toEqual(["始终使用中文回复，包管理统一用 Bun"]);

    const updatedCfg = await client.call({
      type: "memory.config.update",
      requestId: "mem-cfg-1",
      memoryCharLimit: 3200,
      userCharLimit: 1800,
      dreamingEnabled: false,
      dreamingMinHours: 12,
      dreamingMinTurns: 8,
    });
    expect(updatedCfg.memory.memory.charLimit).toBe(3200);
    expect(updatedCfg.memory.user.charLimit).toBe(1800);
    expect(updatedCfg.memory.dreaming).toMatchObject({
      enabled: false,
      minHoursSinceLastRun: 12,
      minTurnsSinceLastRun: 8,
    });

    const { commands } = await client.call({ type: "commands.list", requestId: "mem-cmds", projectId: project.id });
    expect(commands.some((cmd) => cmd.name === "memory")).toBe(true);

    // Open a real session to verify the built-in pi-memory extension loads the saved USER.md snapshot on startup.
    const { session } = await client.call({ type: "session.create", requestId: "mem-sess", projectId: project.id });
    await client.call({ type: "session.open", requestId: "mem-open", sessionId: session.id });
    const readyState = await client.until(
      (message) => message.type === "state" && message.sessionId === session.id && message.state.access === "ready",
    );
    expect(readyState.type === "state" && readyState.state.statusLines["pi-memory"]).toBe("memory: loaded");

    const removed = await client.call({
      type: "memory.remove",
      requestId: "mem-rm-1",
      target: "user",
      oldText: "统一用 Bun",
    });
    expect(removed.memory.user.entries).toEqual([]);
  } finally {
    client.ws.close();
  }
}, 30_000);
