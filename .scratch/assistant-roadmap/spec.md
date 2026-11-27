# Lila 下一步演进计划：个人工程助理闭环与全自动开发工作流

Status: ready-for-agent
Baseline: `4fb5477`（协议 v20，2026-11-27）

## 1. 当前基线（已完成）

截至 `4fb5477`，Lila 已具备完整的编程桌面端与安全后台自动化底座：

- **交互与多会话 Harness**（`c7b4f46`）：智能审批规则（会话/项目白名单 + 只读 Bash 静态放行）、侧栏「需要关注」聚合、Git 写操作闭环 + Worktree 隔离 + Diff 行内批注、Context X-Ray + 定向压缩、子 Agent 运行中 Stop/Steer、`pnpm + vitest` 工具链。
- **后台心跳与任务底座**（`e8af978`）：基于 `croner` 的持久化调度器（`schedules.json`）、5 态任务收件箱（`tasks.json`，支持来源标注、自动提取设计稿/Issue/PR 链接、待澄清问题与拟对外汇报草稿）、Agent 双向工具 `schedule_*`/`task_*`、自动化中枢弹窗（`⇧⌘J`）。
- **OS 级进程沙箱（P0.1）与生命周期治理（P1）**（`f9db28c`）：基于 `@anthropic-ai/sandbox-runtime` 的内核级隔离（macOS Seatbelt / Linux bwrap），与 `ApprovalMode` 正交解耦；支持多项目并发隔离、Git Worktree 共享 `.git` 放行、屏蔽 `~/.ssh`/`auth.json`/`core-token` 读取、保护 `.env`/`.git/hooks`、子 Agent Fail-closed 门控、定时任务默认 `workspace-write`；新增零依赖 `DisposableScope` 统一回收 `core` 资源。

### 已确立的核心产品原则

1. **有界自治（Bounded Autonomy）**：把人工介入压缩到 2 个极轻量闸门——**Gate 1（确认 Task Packet 开工）** 与 **Gate 2（审阅交付包：对比图 / 关键 Diff / 回复草稿）**，中间编码、自测、视觉对拍全部在 **`Git Worktree + workspace-write 沙箱`** 黄金执行舱内无人值守完成。
2. **入站自动收集，出站草稿审稿**：GitHub/Asana 可自动轮询入站；微信严禁对个人主力号做协议 Hook（采用全局热键/快捷投喂零封号风险方案）；所有对外沟通回复一律先写入 `draftReply` 进「需要关注」，人工确认后再发出。
3. **优先复用社区成熟方案与 shadcn/ui（Base UI）**：浏览器自动化复用 Playwright / Playwright MCP，沙箱复用 `@anthropic-ai/sandbox-runtime`，调度复用 `croner`，不重复造轮子。

---

## 2. 回家后建议的第一步（5 分钟实测）

开始编码前，建议先在桌面端实测一轮现有能力，把真实摩擦记录下来：
1. 开一个 `workspace-write` 沙箱会话，在日常仓库里跑一轮 `git status`、`pnpm test`、`pnpm typecheck`，观察是否有常见缓存目录被 Seatbelt 拦截；
2. 创建一个测试定时任务（如 `+1m` 单次或手动点「立即运行」），验证后台触发、沙箱执行与侧栏「需要关注」联动是否顺畅。

---

## 3. 演进主线与工单拆解

按投入产出比与依赖关系拆为 5 张独立工单（均位于 `.scratch/assistant-roadmap/issues/`）：

| 编号 | 工单 | 状态 | 核心交付 |
| --- | --- | --- | --- |
| 01 | [01-assistant-workspace-and-quick-capture.md](issues/01-assistant-workspace-and-quick-capture.md) | `ready-for-agent` | 独立个人助理工作目录 + 外部快捷投递入口（全局热键浮窗 / `lila://` URL / CLI）+ 任务开工沙箱开关对齐 |
| 02 | [02-html-design-workflow-and-visual-diff.md](issues/02-html-design-workflow-and-visual-diff.md) | `ready-for-agent` | 公开免登可交互多页面 HTML 设计稿自动探索（`Flow Map`）+ 工程组件映射 + 双页同视口 E2E 回放与视觉对拍自修 + 验收故事板 |
| 03 | [03-cross-platform-polling-templates.md](issues/03-cross-platform-polling-templates.md) | `needs-triage` | GitHub / Asana 一键预设定时巡检模板 + 自动去重写入任务收件箱 |
| 04 | [04-sandbox-network-approval-and-exceptions.md](issues/04-sandbox-network-approval-and-exceptions.md) | `needs-triage` | 沙箱 P0.2：基于 `PendingRequest` 审批卡的出网域名白名单 + 项目级额外可写目录 + 中文拦截高亮 |
| 05 | [05-packaging-launchd-and-mobile-console.md](issues/05-packaging-launchd-and-mobile-console.md) | `needs-triage` | macOS `.app`/`.dmg` 打包实测 + `launchd` 开机常驻 + Tailscale 移动端轻审批控制台 |

### 推荐执行顺序

- **首选路线（最快跑出“全自动个人工程助理”体验）**：先做 **Ticket 01**（补齐独立工作目录、快捷收集与任务开工沙箱对齐），紧接着做 **Ticket 02**（打穿公开 HTML 交互设计稿到代码落地与视觉对拍的端到端闭环），再按需叠加 **Ticket 03**（GitHub/Asana 定时巡检）。
- **如果实测沙箱时发现需要更细的网络/缓存控制**：可优先切入 **Ticket 04**。
- **准备把 Lila 当作每天开机主力使用时**：推进 **Ticket 05**。

## Comments

- 2026-11-27：整合 CLI 会话（`e89b237d-ad6e-43e3-89aa-b26d105056c6`）与桌面端会话（`993b5990-63ac-4638-84e5-d2c6e2b0f1e3`）的规划与落地成果，形成统一路线图。
