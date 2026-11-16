Type: wayfinder:grilling
Status: resolved

## Question

第一版 Lila 的最小会话协议和事件闭环应该是什么？需要确定客户端命令、core 事件、会话 ID、事件序号、流式增量、工具调用、审批/提问、运行结束与错误的字段和状态转移，并明确客户端从任意序号重连时的补发语义。

## Answer

### Envelope and ordering

- 连接级 `hello` 保持现有形状，不占用会话事件序号。会话事件统一使用 `{ type: "event", sessionId, seq, eventType, payload, requestId?, runId?, pendingId? }`；`sessionId` 和 `seq` 必填，关联字段按事件类型出现。
- 每个会话独立维护从 `1` 开始的单调递增 `seq`，唯一约束为 `(sessionId, seq)`。事件先持久化，再广播给已连接客户端；事件顺序以 `seq` 为准，工具、文本增量和待处理项可以交错出现。
- 客户端连接后发送 `events.resume { sessionId, lastEventSeq }`。core 原子地切换到该会话的订阅，再补发所有 `seq > lastEventSeq` 的事件并继续实时推送；首次连接使用 `lastEventSeq: 0`。不提供全局序号。

### Client commands

- `session.create { requestId, projectId, modelId?, approvalMode? }`
- `message.send { requestId, sessionId, text }`
- `run.abort { requestId, sessionId, runId }`
- `pending.respond { requestId, sessionId, pendingId, answer }`
- `events.resume { sessionId, lastEventSeq }`

除 `events.resume` 外的变更命令必须带 `requestId`。同一会话内重复使用同一个 `requestId` 不得重复产生副作用，core 返回第一次命令的结果或错误；持久化实现留给会话存储票据。

### Event types

- `session.created`: 返回新会话 ID、项目和初始运行配置。
- `message.user`: 记录已接受的用户消息。
- `run.started`: 新一轮运行开始，带 `runId`。
- `assistant.delta`、`assistant.completed`: 助手流式文本和最终文本。
- `tool.started`、`tool.output`、`tool.finished`: 工具调用、增量输出和结束结果。
- `pending.created`、`pending.resolved`: Agent 提问或工具审批的待处理项及其结果。
- `run.finished`、`run.failed`、`run.aborted`、`run.interrupted`: 一轮运行的终态。

协议错误用带 `requestId` 的命令响应返回；运行失败使用 `run.failed` 事件。`run.abort` 的命令响应只表示中断请求已被接受，最终结果仍由 `run.aborted` 或 `run.failed` 事件表示。

### State and pending items

运行状态为 `idle → running → waiting → running → finished|failed|aborted`；`waiting` 表示一个或多个 `pending.created` 尚未完成。core 重启或 omp 进程丢失时，未完成运行进入 `interrupted`，旧的待处理项失效，不能把旧 RPC UI ID 重放给新进程。

Lila 为每个待处理项生成稳定的 `pendingId`，客户端只使用它。omp 的原始 UI request ID 只保存在 core 适配层；审批回答必须保留上游动态 `options` 的原文并原样回传，不能把选项硬编码成固定的 Approve/Deny 三选一。

这解决了首版客户端和 core 的协议边界；事件表、幂等记录、恢复标记和并发写入细节由“会话、事件和待处理项的持久化边界”票据决定。
