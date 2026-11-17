# 第一版任务单

目标：在 Mac mini 上用 Lila 完成一次真实的编程任务。架构见 [architecture.md](architecture.md)。

按顺序做，每项做完能单独验证。

## 当前状态：内部可用的 MVP

已经能在桌面应用里走完整个编程闭环：添加项目目录 → 新建会话（选模型、审批方式）→ 发任务 → 看流式回复、工具调用、diff → 在界面上允许或拒绝工具 → 停止任务 → 关窗口再打开、`core` 重启后接着用。2026-11-17 在 Electron 里用真实 omp（Codex 通道）实测通过：修复一个文件里的 bug，经过两次审批，文件实际被改对；`core` 被杀掉后窗口自动拉起新的 `core` 并续上，追问时 omp 记得上一轮内容。

本地运行：`bun run dev`（同时启动 `core` 和 Electron）。数据在 `~/Library/Application Support/Lila`，`core` 日志在同目录的 `core.log`。

## 下一个任务

### 1. 用它干一件真事，并验收响应速度

用 Lila 给 Lila 自己做一个小改动，记录不顺手的地方，决定下一步。

同时按架构文档的三条标准实测：

- 窗口从 Dock 重新打开，感觉不到延迟。
- 流式输出时，滚动和输入不掉帧。
- 几百条消息的长会话，打开不卡。

达不到就先查应用代码；确认是 Electron 本身的限制，再讨论换壳。

### 2. omp RPC 补测

用 `scripts/omp-probe.ts` 补测，结果补进 [omp-rpc.md](omp-rpc.md)：

- 多个工具同时等待审批的顺序（界面目前一次只显示最早的一个，其余排队）。
- `always-ask-cached` 档的授权选项在 RPC 下的实际格式。
- 在 Claude 和 ChatGPT 的用量页面核对额度扣在订阅上（需要用户自己看）。

## 之后的任务

### 3. 发布打包

- 把 `core` 用 `bun build --compile` 编译成单个可执行文件，用 electron-builder 放进应用包（主进程已经按 `resources/core/lilad` 找它）。
- 视使用情况决定是否把 `core` 改成 launchd 用户级常驻服务，让它被系统杀掉后也能自己起来。

### 4. 体验补齐

- 输入框支持附件（图片、文件），用 shadcn 的 Attachment。
- 会话列表支持重命名、删除、按项目折叠。
- 长会话按需加载更早的事件（`events.resume` 已经支持从任意序号开始，Message Scroller 支持向上追加）。

## 已完成

- **仓库骨架。** Bun workspace，三个包：`packages/protocol`、`packages/core`、`packages/app-desktop`。
- **事件协议 v2。** 每种事件的载荷有明确类型；只有定稿的事实落库，流式增量只推送不落库；命令和响应用 `requestId` 配对。
- **core。** 每个会话一个 `SessionRunner`，串行处理命令，翻译 omp 帧；WebSocket 用 token 鉴权并拒绝网页来源；`core` 重启时把未完成的运行标记为中断；omp 进程空闲 10 分钟回收，下次用 `--resume` 恢复；每 10 分钟记录内存。测试用录制的 omp 帧回放。
- **桌面界面。** 侧边栏按项目列会话和状态；会话界面用 Message Scroller、Message、Bubble、Trace、Code Block、Marker、Questionnaire、Typeset 拼装；工具调用显示意图、命令、输出和 diff；审批卡片支持数字键选择；断线自动重连并按序号续上；应用不在前台时有待处理项或任务结束会发系统通知；代码高亮在 Worker 里做，流式增量按帧合并。
- **常驻与恢复。** 关窗口只隐藏；应用启动时复用已在运行的 `core`，协议版本不一致时替换它；应用退出后 `core` 继续跑。
