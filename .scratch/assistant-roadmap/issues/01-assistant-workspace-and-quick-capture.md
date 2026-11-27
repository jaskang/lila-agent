# 01：独立个人助理工作目录、外部快捷投递与任务开工沙箱对齐

Status: ready-for-agent

## 背景与问题

1. 目前定时任务和任务开工都必须绑定到某个代码项目，日常整理文档、调研、早报巡检或刚收到的碎片需求只能“借宿”在某个代码仓库下。
2. 微信 / WhatsApp 中的需求和公开 HTML 设计稿链接目前仍需切回 Lila 主窗口手动点开弹窗粘贴，收集链路不够顺手。
3. 协议 v20 的 `tasks.start` 已支持 `sandboxMode`，但 `packages/app-desktop/src/renderer/src/components/automation/tasks-panel.tsx` 尚未暴露或默认启用 `workspace-write` 沙箱，与定时任务默认策略不一致。

## 交付范围

1. **任务开工沙箱对齐（小改动高收益）**
   - 在 `tasks-panel.tsx` 的开工交互中支持选择 `sandboxMode`（默认与定时任务一致，推荐默认 `workspace-write`），调用 `actions.startTask` 时透传 `sandboxMode`。
   - 同步核对 `packages/app-desktop/src/renderer/src/lib/store/index.ts` 的 `startTask` 类型签名并透传 `sandboxMode`。
2. **独立个人助理工作目录**
   - 为个人助理提供专属默认目录（如 `~/Lila`，可在设置中心配置或自动初始化，不污染代码仓库）；
   - 任务收件箱与定时任务在不指定代码项目时，可直接落到该默认个人助理目录执行。
3. **轻量外部快速投递入口（零微信封号风险）**
   - **macOS 全局快捷收集浮窗**（Electron `globalShortcut`）或快速聚焦投喂框：在微信/WhatsApp 复制聊天摘录或设计稿链接后，按全局热键 1 秒录入新任务（自动提取并归类链接），或追加到已有未完成任务；
   - **本机投递接口（`lila://` URL Scheme 或轻量 CLI 子命令）**：便于配合 Raycast、macOS Shortcuts 或终端脚本一键将链接/文本丢入 `tasks.json`。

## 验收标准

- 不绑定具体代码项目也能创建并启动个人助理任务与定时任务；
- 从外部应用复制一段带 HTML 设计稿 URL 的微信消息，可通过快捷入口 1 步入库并自动识别为 `design` 链接；
- 从任务卡片点「隔离 Worktree 开工」默认在 `workspace-write` 沙箱内启动；
- `pnpm typecheck && pnpm lint && pnpm test && pnpm build` 全部通过。
