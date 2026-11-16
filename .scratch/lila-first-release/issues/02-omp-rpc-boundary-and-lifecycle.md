Type: wayfinder:research
Status: resolved

## Question

整理 `docs/omp-rpc.md` 和现有探针结果之外仍影响第一版边界的事实：官方 `RpcClient` 在 Bun 下是否可直接使用，Approve/always-ask-cached、abort、switch_session、get_messages_page 的实际消息形状，以及 omp 退出/重启时可依赖的生命周期行为。输出只保留会改变 Lila 适配设计的事实。

## Answer

- 官方 npm `@oh-my-pi/pi-coding-agent` `18.20.0` 的 `RpcClient` 可在 Bun `1.4.4` 下运行，包声明的 engine 下限为 `1.4.4`。
- 它默认启动 `--mode rpc`，没有公开的 `extension_ui_request` 订阅或 `extension_ui_response` 回复 API；相关 listener Set 和写帧方法是私有实现，只在 `login()` 内部使用。
- 因此它不能直接承载 Lila 需要的 `rpc-ui` 审批和提问闭环。core 必须保留自己的 RPC UI 帧适配层，官方客户端最多复用其基础连接能力，且需要锁定版本并持续核对源码。
- 未解决的具体审批字段和生命周期边界继续由协议契约与 core 适配决策票据确定。

补充影响后续设计的事实：

- 审批 wire 只有动态 `options` 和 UI request `id`，没有可靠的 `toolCallId`；`always-ask-cached` 可能产生目录/文件级的临时或永久授权选项，不能固定成三选一。
- `abort` 的成功响应不等于 prompt 已结束；最终状态仍由 `prompt_result`/Lila 的运行终态事件表示。`switch_session` 可能返回 `cancelled: true`，切换后需要重新读取实际 session state。
- `get_messages_page` 是绑定 session snapshot 的正序分页；cursor 过期或 session busy 时会失败，不能把它当作 Lila 自己的事件补发游标。
- RpcClient 没有自动重试或自动恢复会话的公开保证。core 必须监督进程、记录进程代次，进程丢失时将未完成 Run 标记为中断，并使旧 pending item 失效。
