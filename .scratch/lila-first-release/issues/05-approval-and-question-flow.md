Type: wayfinder:grilling
Status: resolved

Blocked by: 01, 02

## Question

第一版如何统一处理 agent 提问、工具审批和会话 approval mode？需要确定 `yolo`、`write`、`always-ask` 的用户可见行为，待处理项的持久化/恢复，Approve/Reject/回答的回传，多个待审批工具，以及应用不在前台时的通知边界。

## Answer

- 档位：`yolo` 自动执行、`write` 执行命令前询问、`always-ask` 修改和执行都询问；新会话默认 `yolo`，界面上用中文名并附一句说明。
- 只有 `confirm`、`select`、`input`、`editor`、`ask` 会成为待处理项；`notify`、`setStatus`、`setWidget` 忽略。omp 发 `cancel` 时对应待处理项以 `cancelled` 关闭。
- 标题以 `Allow tool:` 开头的 `select` 视为工具审批，关联上一条 `tool_execution_start` 的 `toolCallId`；卡片显示「Agent 想要运行 X」和命令或路径。
- 选项按 omp 原文回传，core 校验答案在选项里；界面上翻译成「允许 / 本会话内始终允许 X / 拒绝」，数字键 1–9 直接选择。
- 先记录 `pending.resolved` 再回给 omp，保证日志里回答排在它引发的工具结果之前。
- 有多个待处理项时只显示最早的一个，其余排队并显示数量。
- 待处理项不落库；运行结束或 omp 退出时未回答的项以 `cancelled` 或 `interrupted` 关闭。
- 窗口不在前台时，新的待处理项、任务完成和任务失败会发系统通知，点击通知把窗口带到前台。
