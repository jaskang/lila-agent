# pi-durable 运行时

Lila Core 直接嵌入 `@earendil-works/pi-durable`，由 durable Harness 管理会话、任务调度、工具执行、恢复和 SQLite 持久化。Core 只把 durable transcript 投影为 Lila 的 WebSocket 事件。

## 运行时结构

- `@earendil-works/pi-durable` 提供 Harness、Conversation、任务恢复、文档和 watch。
- `@earendil-works/pi-coding-agent` 提供 `ModelRuntime`，复用 Pi 的模型目录和认证。
- `@earendil-works/pi-durable/tools` 提供 read、write、edit、bash 等官方 durable 工具。
- `@earendil-works/pi-durable/storage/sqlite/node` 提供 Node SQLite 存储，Bun 运行时使用同一套接口。
- `packages/core/src/durable.ts` 保存项目目录、会话元数据和 Lila 事件投影。

## MCP 和 sub-agent

Pi 的模型、MCP 和扩展能力通过 `ModelRuntime` 与 durable Registry 接入。需要扩展时注册官方 durable Extension 或 Tool，不再启动 Pi RPC 子进程。sub-agent 应使用 durable 的普通 Conversation 和任务所有权模型，因而可以在恢复后继续运行。

## 数据恢复

会话输入、assistant 消息、tool result 和 durable 文档都写入 SQLite。Core 重启时 Harness.resume 会重新调度未完成任务。Lila 的 `events.resume` 从 ProjectionDoc 重放已完成事件，运行中的 live 状态由 durable watch 提供。

## 版本

当前依赖固定为 `1.5.2`，升级时需要重新执行真实模型、工具执行、审批和重启恢复冒烟测试。
