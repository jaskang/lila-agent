# 02：公开 HTML 交互设计稿自动探索、工程转译与双页视觉对拍闭环

Status: ready-for-agent

## 背景与场景

设计师交付的是**公开免登的静态托管 HTML 页面 URL**，且包含**可点击的交互与多页面跳转**（Tab、弹窗、抽屉、步骤流、子路由）。这本质上是一份可执行的交互规格（Executable Spec），非常适合在 `Git Worktree + workspace-write OS 沙箱` 内做高确定性的全自动开发与验收。

## 交付范围

1. **有界交互探索能力（基于 Playwright，绝不用 `curl`）**
   - 编写标准 Pi Skill（如 `html-design-to-code`）及轻量辅助脚本/指引（复用 Playwright / Playwright MCP）：
     - 严格限制在同源/同路径前缀内，设置最大状态数（如 15~20 个节点）与同类列表去重规则，防止状态爆炸；
     - 扫描页面链接/路由并触发可点击交互（Tab、弹窗、抽屉、子页跳转），输出结构化 **`Flow Map`** + 分段视口截图 + 去噪后的语义 DOM / Computed Style 摘要；
     - 支持同一 URL 二次抓取时与基线快照做 **Design Diff**（设计师原地更新设计稿后只做增量修改）。
2. **工程化转译强约束（严禁照抄原型代码）**
   - 写代码前强制盘点目标仓库组件库（如 shadcn/ui / 内部组件）、Design Tokens、Router 与 API 类型；
   - 区分「纯 UI 交互」（全自动按仓库规范实现）与「涉及真实后端数据的写操作/新字段」（缺接口时自动调用 `task_upsert` 记入 `blockingQuestions` 并起草 `draftReply`，或按约定用类型化 Mock 占位并高亮标注）。
3. **双页同视口 E2E 回放 + 视觉对拍自修复环**
   - 在隔离 Worktree 分配空闲端口启动本地 Dev Server；
   - 按 `Flow Map` 逐态在本地页面重放交互：校验状态/路由正确、Console 零报错、无意外 4xx/5xx，并与原稿 URL 对应状态同视口截屏并排比对；
   - 发现明显错位/缺失自动回炉修复（上限 2~3 轮），同步跑通项目 `typecheck / lint / test / build`。
4. **任务收件箱验收故事板（Storyboard）**
   - 扩展 `AssistantTask`（可选 `artifacts` / 对比截图引用），在任务卡片进入 `review` 态时直接展示「原稿 vs 实装」关键交互态左右对比图与拟发给设计师/团队的汇报草稿。

## 验收标准

- 给定一个带多页面/弹窗交互的公开静态 HTML URL，Agent 能自动产出 `Flow Map` 与状态快照而不陷入死循环；
- 产出代码遵循目标仓库既有组件体系，不直接拷贝原型散装 HTML；
- 验收阶段自动完成交互回放与同视口视觉对比，并在任务卡片呈现验收摘要与回复草稿。
