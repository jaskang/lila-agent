# 第一版任务单

目标：在 Mac mini 上用 Lila 完成一次真实的编程任务。架构见 [architecture.md](architecture.md)。

按顺序做，每项做完能单独验证。

## 当前状态：pi-durable 接入完成，真实模型冒烟进行中

Lila 的 core 已切换到 `pi-durable`。当前使用 `@earendil-works/pi-durable`、`pi-coding-agent` 和官方 durable tools，模型、会话、任务恢复和 SQLite 持久化由 Harness 直接管理。已完成真实模型调用和 Core WebSocket 冒烟。

本地运行：`bun run dev`（同时启动 `core` 和 Electron）。数据在 `~/Library/Application Support/Lila`，`core` 日志在同目录的 `core.log`。

## 下一个任务

### 1. 用它干一件真事，并验收响应速度

用 Lila 给 Lila 自己做一个小改动，记录不顺手的地方，决定下一步。

同时按架构文档的三条标准实测：

- 窗口从 Dock 重新打开，感觉不到延迟。
- 流式输出时，滚动和输入不掉帧。
- 几百条消息的长会话，打开不卡。

达不到就先查应用代码；确认是 Electron 本身的限制，再讨论换壳。

### 2. durable 完整任务验收

验证真实工具执行、审批文档恢复、MCP 和 sub-agent 接入，再记录运行速度和资源占用。

### 3. 发布打包

- 把 `core` 用 `bun build --compile` 编译成单个可执行文件，用 electron-builder 放进应用包（主进程已经按 `resources/core/lilad` 找它）。
- 视使用情况决定是否把 `core` 改成 launchd 用户级常驻服务，让它被系统杀掉后也能自己起来。

### 4. 体验补齐

- 输入框支持图片和文本文件附件，单个文件最多 5 MiB，最多发送 10 个附件。
- 会话列表支持重命名、删除、按项目折叠。
- 长会话首次只加载最近 200 条事件，可从顶部继续加载更早的事件。

## 已完成

- **仓库骨架。** Bun workspace，三个包：`packages/protocol`、`packages/core`、`packages/app-desktop`。
- **事件协议 v2。** 每种事件的载荷有明确类型；只有定稿的事实落库，流式增量只推送不落库；命令和响应用 `requestId` 配对。
- **core。** `pi-durable` Harness 管理会话、任务串行化、工具执行、恢复和 SQLite；WebSocket 用 token 鉴权并拒绝网页来源；测试覆盖 token 和 durable 会话持久化。
- **桌面界面。** 侧边栏按项目列会话和状态；会话界面用 Message Scroller、Message、Bubble、Trace、Code Block、Marker、Questionnaire、Typeset 拼装；工具调用显示意图、命令、输出和 diff；审批卡片支持数字键选择；断线自动重连并按序号续上；应用不在前台时有待处理项或任务结束会发系统通知；代码高亮在 Worker 里做，流式增量按帧合并。
- **常驻与恢复。** 关窗口只隐藏；应用启动时复用已在运行的 `core`，协议版本不一致时替换它；应用退出后 `core` 继续跑。
