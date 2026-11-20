# 任务单

架构见 [architecture.md](architecture.md)，内核选择见 [ADR 0001](adr/0001-pi-sdk-kernel.md)。

本地运行：`bun install` 后 `bun run dev`（同时启动 core 和 Electron）。core 日志在 `~/Library/Application Support/Lila/core.log`。模型、认证、MCP、技能都读 `~/.pi/agent`，先用 `pi` 命令行配好（`/login`、`pi mcp add`、`pi install`）。

## 下一个任务

### 1. 用它干真事并验收

用 Lila 给 Lila 自己做几个改动，覆盖：子 Agent（`Agent` 工具前台和后台）、一个 stdio MCP 服务器、一个技能（`/skill:…`）、`ask-exec` 审批、运行中插话（Enter）和追加（⌥Enter）、编辑重发、手动压缩。记录不顺手的地方。

响应速度按架构文档三条标准实测；长会话用一个几百条消息的 Pi 会话文件打开试。

### 2. 发布打包

- electron-builder 把 `packages/core`（含 `node_modules`）放进 `resources/core`，主进程已经按 `ELECTRON_RUN_AS_NODE=1 <app> resources/core/src/main.ts` 启动它，不需要编译。
- 签名与公证。
- 视使用情况决定是否把 core 改成 launchd 用户级常驻服务。

### 3. 个人助理方向

- 定时任务界面：pi-subagents 的 `schedule` 已经能定时跑子 Agent，缺的是列表和取消的界面。
- 一个不绑定代码仓库的“助理”项目（专用目录 + 个人技能）。
- 手机端：同一套 WebSocket 协议，经 Tailscale。

### 4. 体验待补

- 会话树导航（Pi 的 `/tree`）和从任意消息分叉成新会话。

## 已完成

- **内核换成 pi-coding-agent SDK。** 会话是 Pi 的 JSONL 文件，与 `pi` 命令行共用配置；MCP、codemode、tool_search、技能、提示模板、AGENTS.md、用户扩展和 Pi 包都生效；子 Agent 用 pi-subagents。
- **core 改在 Node 上运行**（Electron 自带 Node 24），WebSocket 用 `ws`，token 鉴权、拒绝网页来源。
- **协议 v3**：打开会话拿快照，之后推 `item`/`delta`/`state`；不再有自存事件库和序号。
- **审批与提问**：Lila 内置 Pi 扩展（`tool_call` 钩子 + `ask_user` 工具），任何扩展的对话框都会变成待处理卡片。
- **子 Agent 会话与实时进度**：pi-subagents 给每个子 Agent 写自己的 Pi 会话（带 `parentSession`），侧栏折叠展示；接入 `subagents:*` 事件总线与前台工具更新，在工具卡片和输入区上方实时展示前/后台子 Agent 的活动、轮次、工具调用、花费与耗时，并支持父子会话互跳。
- **工作区 Git 改动面板**：顶部栏显示未提交文件数，抽屉内按全部/已暂存/未暂存查看文件列表与 diff（含未跟踪文件与二进制检测），支持直接打开或在访达中定位文件。
- **界面**：新会话直接是输入框；斜杠命令与 @文件补全；粘贴/拖入图片和文本；运行中插话/追加与队列显示；模型、思考深度、审批方式切换；上下文用量、花费与手动压缩；编辑重发；子 Agent / MCP / codemode 的工具卡片；“能力”面板（技能、工具、MCP、扩展、上下文，可打开 mcp.json 并重新加载）；⌘K 命令面板；会话重命名/删除（移入回收目录）；系统通知。
- **常驻与恢复**：关窗口只隐藏；应用启动复用已运行的 core，协议版本不一致时替换；应用退出后 core 继续跑完任务。
