# Lila 第一版真实编程闭环开发路线

## Destination

完成第一版可用的 Lila 编程工作台：用户可以选择项目和模型、发送任务、查看流式文本与工具输出/diff、处理提问和审批，并在关闭窗口或 core 重启后恢复未完成会话。

## Notes

- 领域：Lila 的 Electron 桌面端、Bun core、共享协议和 omp RPC 适配。
- 每次工作先读取 `docs/architecture.md`、`docs/plan.md`、`docs/omp-rpc.md`，并遵守 `docs/agents/domain.md` 的领域文档规则。
- 既有约束：core 独立运行；客户端通过 WebSocket；事件日志是事实来源；默认 `yolo`；界面优先使用 shadcn/ui Base UI 组件。
- 本 map 先搭建可运行的端到端骨架，再补齐协议细节、持久化、审批和性能；用户明确允许实现先行，后续决策以运行结果校正。

## Decisions so far

<!-- Open tickets are intentionally omitted. Each resolved ticket is added here as a one-line index entry. -->
- [omp RPC 客户端与运行生命周期的最小事实集](issues/02-omp-rpc-boundary-and-lifecycle.md)：官方 RpcClient 可在 Bun 使用，但不能直接承载 rpc-ui 审批/提问，core 需要保留 RPC UI 适配层。
- [第一版会话协议与事件闭环](issues/01-protocol-and-event-contract.md)：采用会话级单调 `seq`，事件先落日志再推送；命令用 `requestId` 幂等，运行和待处理项分别用 `runId`、`pendingId` 关联。
- [可运行的端到端骨架](issues/08-running-vertical-slice.md)：protocol、core、SQLite、omp RPC 和桌面工作台已打通，真实任务已收到流式回复并完成。




## Not yet specified

- 第一轮运行暴露的审批并发顺序、缓存授权交互和 crash 恢复语义仍需专项决策。
- SQLite 事务、命令幂等和长会话性能优化的具体方案仍需在后续票据中确定。

## Out of scope

- 定时任务、个人助理工具和原生手机端；这些属于第一版编程闭环之后的范围。
- 更换 Electron、Bun 或 omp；除非第一版验收证明当前运行时无法满足要求。
- 生产发布、自动更新和多用户/远程协作。
