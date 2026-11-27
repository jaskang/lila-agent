# 01：sandbox-runtime 能力、并发与子 Agent 注入 Spike 验证

Status: resolved

## 目标

在正式修改 `packages/core` 之前，针对 `@anthropic-ai/sandbox-runtime`（`0.0.83`）与 `@tintinweb/pi-subagents`（`0.26.1`）做最小无污染原型验证，逐项消除高风险未知项：

1. **P0.1 纯文件系统隔离可行性**：确认在网络放行（`allowedDomains` / `allowAllUnixSockets` 或 `network` 选配）时，`@anthropic-ai/sandbox-runtime` 是否能跳过或无副作用处理本地代理，且 macOS Seatbelt 规则正常生效。
2. **无全局 `rg` 的兼容**：确认传入 `~/.pi/agent/bin/rg`（或临时追加 `PATH`）能通过初始化检查，不依赖 Homebrew `ripgrep`。
3. **多项目并发隔离**：在单 Node 进程内并发对 Project A 与 Project B 调用 `wrapWithSandbox(..., customConfig)`，验证：
   - Project A 的命令可写 A 不可写 B；
   - Project B 的命令可写 B 不可写 A；
   - 均无法读取 `~/.ssh`、`~/.pi/agent/auth.json`、`core-token`；
   - 均无法改写 `.git/hooks` / `.env`。
4. **日常开发命令兼容性**：在沙箱内实测 `git status`、Worktree 场景、`node -v`、`pnpm` 缓存路径需求。
5. **`pi-subagents` 子会话沙箱注入路径**：核实 `agent-runner.ts` 如何发现和加载扩展，确定如何让子 Agent 的 `bash` 也自动继承父会话的沙箱策略（以及 `extensions: false` 子 Agent 的降级拦截策略）。

## 交付物

将全部实测结果与推荐实现细节追加到本文件 `## Comments` 及更新 `spec.md`。

## Comments

### Spike 实测结果（2026-11-27，macOS / Node v24.18.0 / `@anthropic-ai/sandbox-runtime@0.0.83`）

1. **平台支持与 `ripgrep` 依赖**：
   - `SandboxManager.isSupportedPlatform()` 返回 `true`；
   - 在本机默认 `PATH` 无 `rg` 的环境下，`SandboxManager.checkDependencies()` 直接返回 `{ errors: [], warnings: [] }`（0.0.83 在 macOS 上已原生使用 Seatbelt regex，不再强依赖 `rg`）。Linux 上仍建议自动探测系统 `rg` 并回退到 `join(getAgentDir(), "bin", "rg")`。
2. **P0.1 纯文件系统隔离（网络放行）的关键细节**：
   - 调用 `SandboxManager.initialize(config)` 时，`config.network` **必须传入 `{}`（不能省略）**，否则 `resolveParentProxy(runtimeConfig.network.parentProxy)` 会抛 `TypeError`；
   - 只要 `network.allowedDomains` 为 `undefined`，`wrapWithSandbox` 内部的 `needsNetworkRestriction` 与 `needsNetworkProxy` 均为 `false`，不会将流量劫持到代理；实测沙箱内 `fetch("https://example.com")` 返回 HTTP 200。
3. **单进程多项目并发隔离（12/12 全部通过）**：
   - 同一个 Node 进程内并发对 `proj-a`、`proj-b`、`proj-b-wt`（Git Worktree）调用 `SandboxManager.wrapWithSandbox(cmd, "/bin/zsh", customConfig, undefined, { commandId, commandText })`：
     - `proj-a` 写 `proj-a` PASS，写 `proj-b` FAIL（`operation not permitted`）；
     - `proj-b` 写 `proj-b` PASS，写 `proj-a` FAIL（`operation not permitted`）；
     - 读取 `fake-secret` 与 `~/.pi/agent/auth.json` 均 FAIL（`Operation not permitted`）；
     - 覆写 `.env`、写 `.git/hooks/pre-commit` 均 FAIL，且 `annotateStderrWithSandboxFailures(commandId, stderr)` 能精确附带 `<sandbox_violations>` 拦截路径；
     - `git status`、Worktree `git commit`（放行 `wtDir` + `gitCommonDir` 且保护 `.git/hooks`）、`node -v` 均 PASS。
4. **日常构建工具实测与踩坑点（重要）**：
   - `@anthropic-ai/sandbox-runtime` 默认将沙箱内子进程的 `TMPDIR` 重写为 `/tmp/claude`，且 `getDefaultWritePaths()` 包含 `/tmp/claude` 与 `/private/tmp/claude`（从而避免暴露宿主 `/var/folders/.../T`）；
   - **踩坑点**：如果宿主机尚未创建 `/tmp/claude`，依赖 `os.tmpdir()` 的测试（例如本仓库的 `packages/core/src/store.test.ts`）会报 `ENOENT: mkdtemp '/tmp/claude/...'`；
   - **验证修复**：在 `SandboxService.initialize()` 时先 `mkdirSync("/tmp/claude", { recursive: true })` 并把 `...getDefaultWritePaths()` 并入 `allowWrite` 后，本仓库实测 `git status`、`pnpm --version`、`pnpm --filter @lila/core typecheck`、`pnpm vitest run packages/core/src/store.test.ts` **全部 0 退出码通过（无需额外暴露宿主的 `/var/folders`）**。
5. **`pi-subagents` 子会话沙箱注入核实**：
   - 经阅读 `@tintinweb/pi-subagents@0.26.1` 的 `src/agent-runner.ts` 与 `src/default-agents.ts` 确认：子 Agent 在同进程内自建 `DefaultResourceLoader`（仅硬编码 `codemode`、`tool-search`、`mcp`、`scopeGate` 四个 factory）并加载磁盘/包扩展，**不继承主会话的 `lila` 内联扩展**，也无 `before_spawn` 钩子；且内置 `Explore` / `Plan` / `general-purpose` 全部包含 `bash` 工具。
   - **落地结论（严守不改 `node_modules`、不污染 `~/.pi/agent/settings.json` 原则）**：
     - 在 P0.1 中，当父会话开启沙箱时，由 `lila-extension.ts` 的 `tool_call` 对 `Agent` / `SubagentWorkflow` 执行 **Fail-closed 门控**：
       - 后台无人值守场景（定时任务 / `auto` 且无交互审批通道）：直接阻止派发未沙箱子 Agent，并提示模型在主会话内直接执行；
       - 交互会话场景：弹出 `PendingRequest` 审批卡，明确告知用户“该子 Agent 的 `bash` 暂不受沙箱保护，是否放行？”。
     - 后续若需真正透明沙箱化子 Agent，可评估向上游 `pi-subagents` 提扩展注入点，或提供用户显式安装的全局薄扩展（配合 `parentSession` 映射读取父会话策略，且仍保留对 `extensions: false` 的 Fail-closed 兜底）。
