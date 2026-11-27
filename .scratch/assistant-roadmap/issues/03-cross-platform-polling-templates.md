# 03：GitHub / Asana 跨平台定时巡检预设模板与阻塞唤醒

Status: needs-triage
Blocked by: 01

## 背景与目标

后台心跳（`SchedulerService`）与任务收件箱（`TaskService`）已就绪，但 GitHub 与 Asana 的任务同步仍需用户手写定时提示词。需要提供开箱即用的巡检模板与幂等去重约定，让官方 API 平台稳定自动入站。

## 交付范围

1. **一键预设定时巡检模板**（在 `schedules-panel.tsx` 创建表单提供模板选择）：
   - **GitHub 巡检**：扫描分配给我的 Issue、PR Review 评论及失败 CI，借助 `gh` 或 GitHub MCP 拉取增量，自动去重调用 `task_upsert` 入库；
   - **Asana 巡检**：通过 Asana MCP / API 拉取指派给我的任务与新评论，提取描述中的设计稿 URL 入库；
   - **每日工程早报**：工作日早上汇总待处理/待验收/待澄清任务与昨日进展。
2. **外部来源标识与去重（`externalId`）**
   - 视需要为 `AssistantTask` 增加可选 `externalRef?: string`（如 `github:owner/repo#42`、`asana:123456`），避免定时轮询重复建单；
   - 当处于 `clarifying` 状态的任务在其对应 Issue/Asana 评论收到新回复时，自动把新回复追加到任务上下文并提示/唤醒继续推进。
3. **项目级验证配方（Verification Profile）**
   - 允许为项目沉淀默认验收命令（`typecheck / lint / test / build / dev`），在定时巡检或任务开工时自动注入，减少重复描述。
