Type: wayfinder:grilling
Status: resolved

Blocked by: 01, 02

## Question

在事件协议和 omp 生命周期确定后，core 应如何持久化项目、会话、事件、待处理项和运行状态？需要确定事件日志的唯一事实来源、幂等键/序号约束、未完成运行的中断标记、窗口重连补发，以及 core 重启后的继续运行边界。

## Answer

- 表：`projects`（路径唯一）、`sessions`（标题、模型、审批方式、状态、omp 会话文件）、`events`（主键 `(session_id, seq)`）。版本用 `PRAGMA user_version`；没有版本号的原型库把旧表改名为 `legacy_*` 保留，不删除。
- 只有定稿事实落库：用户消息、完整助手消息、工具开始与结束（输出截断到 2 万字符，含 diff）、待处理项创建与结果、运行终态。文本增量和工具进度只推给正在看该会话的客户端。一轮真实任务的日志约 6 KB，原型是约 137 KB。
- `events.resume { afterSeq }` 在同一个同步步骤里切换订阅并读日志，不会漏事件；运行中的会话额外返回未完成输出的快照。客户端发现序号断档时重新 resume。
- 运行中途结束时（停止、失败、omp 退出），未定稿的文本和工具先补写成定稿事件，再写终态，日志保持完整。
- `core` 启动时把状态为 `running`/`waiting` 的会话补一条 `run.interrupted` 并改成 `interrupted`。待处理项不落库：旧的 omp UI 请求 ID 不能回给新进程。
- 第一版不做命令幂等记录：`requestId` 只用来配对响应，断线时客户端把未完成的请求当失败处理并提示。
