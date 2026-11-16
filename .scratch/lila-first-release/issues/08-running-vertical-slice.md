Type: wayfinder:task
Status: resolved

## Question

在已确定的 WebSocket、会话级事件序号和 core/desktop 分层之上，先搭一条可运行的端到端骨架：桌面端点击“新建会话”发送命令，core 创建会话并返回事件，桌面端进入会话界面；保留后续接入 SQLite、omp、流式消息和审批的清晰 seam。验收标准是本地开发命令启动后，用户可以完成“连接 core → 新建会话 → 看到会话状态”的完整路径。

## Execution override

用户明确要求先搭整体框架并让项目跑起来，再调整和完善细节。本票据允许直接修改 protocol、core 和 app-desktop；不在本票据中实现真实 omp 对话、持久化或审批细节。

## Answer

- protocol 已提供项目、模型、会话、消息、终止、待处理项回复和事件恢复命令；事件按会话递增 `seq`。
- core 已接入 SQLite 项目/会话/事件/待处理项表、omp `rpc-ui` 子进程、协议 v2 握手、分片帧、流式文本、工具事件、审批/提问、终止和退出中断事件。
- 桌面端已接入项目目录添加、omp 动态模型列表、历史会话、自动重连、事件恢复、消息/工具/diff/pending 展示和会话配置。
- 验证通过：`bun run typecheck`、`bun test`、桌面构建；隔离 core 的真实 omp 探针收到 assistant 增量并以 `run.finished` 结束；SQLite 已记录该会话的 omp `session_file` 和 `finished` 状态。
- 第一轮有意保留的细节：存储目前直接使用 `bun:sqlite`，模型目录以 core 内存缓存，审批并发和 crash 恢复仍需下一轮专项完善。
