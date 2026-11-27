# 02：Core、Protocol 与定时任务沙箱集成

Status: resolved
Blocked by: 01 (resolved)

## 范围

1. **`packages/protocol`**：
   - 新增 `SandboxMode`（`"off" | "workspace-write"`）与 `SandboxAvailability`；
   - 扩展 `SessionState`、`ScheduledJob`、`ScheduleCreateInput`、`ScheduleUpdatePatch`；
   - 新增 `session.setSandboxMode` 命令并递增 `PROTOCOL_VERSION`。
2. **`packages/core`**：
   - 实现 `sandbox-service.ts`（封装 `@anthropic-ai/sandbox-runtime`，自动回退 `~/.pi/agent/bin/rg`，Fail-closed 语义，Worktree `.git` common dir 解析，敏感路径 `denyRead` 清单）；
   - 通过 `createBashToolDefinition(cwd, { operations })` 与 `user_bash` 挂载到主会话，并按 Ticket 01 结论接入 `pi-subagents` 子会话；
   - 自动检测用户自装的外部沙箱扩展（如 `@erichll/pi-sandbox`）并优雅让位；
   - 扩展 `store.ts`（持久化 `sandboxModes`）与 `scheduler-service.ts`（新定时任务默认 `workspace-write`）；
   - 针对真实沙箱行为编写克制、直击核心边界的自动化测试。
