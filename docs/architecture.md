# Lila 架构

Lila 是个人用的 Agent 桌面图形界面，内核是 [Pi](https://pi.dev)。界面自己做，Agent 能力全部复用 Pi 和它的生态：凡是 Pi 已经有的（模型、认证、技能、MCP、扩展、Pi 包、子 Agent、长期记忆、压缩、会话格式），Lila 不再造一遍。

## 总体结构

```text
Electron 应用（packages/app-desktop）
  ├─ 窗口：React + Tailwind + shadcn/ui（Base UI） ──┐
  ├─ preload：连接信息、选目录、打开/定位文件、外链、  │
  │          重启 core、Dock 徽章、主题与菜单事件     │ WebSocket（localhost，token 子协议）
  └─ 主进程：单实例锁、窗口状态持久化、原生菜单、       │
            lila-media 协议、拉起/复用/替换 core     │
                                                     ▼
core（packages/core，独立常驻 Node 进程）←────────────┘←── 以后的手机端（经 Tailscale）
  ├─ server.ts：token 鉴权、命令校验分发、按会话推送
  ├─ host.ts：项目列表、会话索引、活跃会话调度与回收
  ├─ live-session.ts：单个可写会话的 Pi AgentSession 封装
  │    ├─ Pi 资源加载：AGENTS.md、技能、提示模板、用户扩展、Pi 包
  │    ├─ 内置扩展：MCP、codemode、tool_search、子 Agent（pi-subagents）、长期记忆（pi-memory）
  │    └─ Lila 扩展：工具审批、ask_user 提问、子 Agent 事件与响应头采集
  ├─ settings-service.ts：模型目录与筛选、OAuth/API Key 登录、默认设置、本地用量与服务商配额
  ├─ extensions-service.ts：MCP 读写、Pi 扩展包安装/更新/卸载、能力快照与重载
  ├─ workspace-service.ts：项目文件搜索/目录树/预览、工作区 Git 状态与 Diff
  ├─ memory.ts：长期记忆（MEMORY.md / USER.md）读写与 Dreaming 配置
  ├─ pending-bridge.ts：Pi 扩展的 select/confirm/input/editor + 审批/提问 → 待处理项
  └─ transcript.ts：Pi 会话条目 → 界面条目投影、搜索与轮次目录
```

## 关键决定

- **内核用 `pi-coding-agent` SDK，不用 `pi-durable`。** 见 [ADR 0001](adr/0001-pi-sdk-kernel.md)。SDK 就是 `pi` 命令行本身用的运行时，MCP、技能、扩展、子 Agent、长期记忆都在这一层；`pi-durable` 是实验性的底层运行时，这些都没有。
- **配置和会话与 `pi` 命令行共用，同时提供完整图形化设置中心。** 模型、认证、`settings.json`、`models.json`、`mcp.json`、`memories/`、技能、扩展和 Pi 包都在 `~/.pi/agent`（`PI_CODING_AGENT_DIR` 可改）。用户在 Lila 的统一设置中心（`⌘,`）里即可完成服务商 OAuth/API Key 登录、模型启停、余额/配额与本地用量查看、全局/项目级 MCP 增删改、长期记忆可视化编辑，以及 Pi 扩展包的安装/更新/卸载；同时会话就是 Pi 的 JSONL 文件，终端里 `pi --resume` 能直接接着 Lila 的会话做，反过来也一样。
- **Lila 自己只存很少的状态。** 数据目录（macOS 为 `~/Library/Application Support/Lila`）里只有 `core-token`、`core.pid`、`state.json`（项目列表、各会话的审批方式）、`quotas.json`（服务商响应头配额快照）、`window-state.json`（窗口位置与尺寸）、`core.log` 和删除会话时移入的 `trash/`。没有自己的消息库。
- **core 是独立的 Node 进程。** 用 Electron 自带的 Node（`ELECTRON_RUN_AS_NODE=1`）运行，Node 24 原生剥离 TS 类型，开发和运行不需要额外编译步骤。应用退出后 core 继续跑完任务；应用启动时复用已在运行的 core，协议版本不一致时自动替换。
- **所有客户端都走 WebSocket 并鉴权。** core 首次启动生成 `core-token`（0600），客户端用 `lila.<token>` 子协议连接；core 拒绝非本地网页 `Origin`，客户端带心跳保活与自动重连。
- **界面只认 Lila 的协议。** `packages/protocol`（当前协议 v15）定义命令（Zod 校验）和推送消息（TS 类型）。Pi 的消息结构只在 core 的 transcript 投影里出现，以后的手机端不必理解 Pi 内部类型。
- **打开会话 = 拿快照 + 收推送。** `session.open` 返回最近的条目和当前状态（含流式输出部分），之后 core 推 `item`、`delta`、`state`。断线重连直接重新 open。支持会话内全文查找与窗口定位、轮次目录跳转、从任意用户消息编辑重发（`session.rewind`）或分叉为新会话（`session.fork`）、排队消息管理（插话/追加移除与提权）及启动失败重试。
- **审批和提问是 Pi 扩展。** Lila 内置扩展在 `tool_call` 钩子里按会话审批方式拦截工具，等待用户在待处理面板回答（拒绝时支持附带补充说明）；任何 Pi 扩展调用 `ctx.ui.select/confirm/input/editor` 也会变成同样的待处理卡片。待处理项只在内存里，core 重启时正在跑的任务本来也会中断。
- **子 Agent 与右侧多模式面板。** 子 Agent 的原会话由 `pi-subagents` 写入，core 转发实时进度事件并在桌面实时读取已落盘记录；运行中默认展开第一层，结束后可从子会话继续为新会话。右侧面板支持文件树与代码预览、工作区 Git Diff、本轮回复改动审阅、子 Agent 只读预览四种视图。
- **界面线程只管渲染。** Shiki 代码高亮跑在 Web Worker，流式帧按动画帧合并，KaTeX 数学公式与 Mermaid 图表按需懒加载。
- **界面组件用现成的。** 统一基于 shadcn/ui（Base UI 版）及其 AI 组件（Message Scroller、Message、Bubble、Trace、Code Block、Questionnaire、Marker、Attachment 等），避免重复造轮子。

## 能力从哪来

| 能力 | 来源 |
| --- | --- |
| 模型、认证、思考深度、默认偏好 | Pi `ModelRuntime` + `SettingsManager`，配置在 `~/.pi/agent`，设置页可直接登录与筛选模型 |
| 读写改、bash、grep/find/ls | Pi 内置工具 |
| MCP（stdio / HTTP / OAuth） | Pi 内置 MCP 扩展，配置 `~/.pi/agent/mcp.json` 与项目 `.pi/mcp.json`，设置页可视化管理 |
| codemode、工具搜索 | Pi 内置扩展，MCP 服务器连上后自动启用 |
| 技能、提示模板、斜杠命令、Pi 包 | Pi 资源加载与 `DefaultPackageManager`，设置页支持安装/更新/卸载并重载 |
| 子 Agent（前台/后台、并行、工作流、定时） | [`@tintinweb/pi-subagents`](https://github.com/tintinweb/pi-subagents)，用户自己 `pi install` 过时自动用用户的那份 |
| 长期记忆（MEMORY.md / USER.md、后台 Dreaming） | [`@amaster.ai/pi-memory`](https://www.npmjs.com/package/@amaster.ai/pi-memory)，存在 `~/.pi/agent/memories/`，设置页可视化管理，用户自装记忆扩展时自动让位 |
| AGENTS.md / CLAUDE.md | Pi 上下文文件 |
| 本地用量统计与服务商配额/余额 | core 扫描 Pi 会话日志 + 采集响应头与账单/配额接口 |
| 压缩、自动重试、插话（Steer）与追加（Follow-up） | Pi `AgentSession` |
| 工具审批、`ask_user` 提问 | Lila 内置 Pi 扩展 |

## 技术栈

| 部分 | 选择 |
| --- | --- |
| 仓库与工具链 | Bun workspace（`packages/protocol`、`packages/core`、`packages/app-desktop`）、TypeScript、oxlint、oxfmt |
| 桌面壳 | Electron + electron-vite |
| core 运行时 | Node 24（Electron 自带），WebSocket 用 `ws` |
| Agent 内核与内置包 | `@earendil-works/pi-coding-agent` SDK、`@tintinweb/pi-subagents`、`@amaster.ai/pi-memory` |
| 协议 | Zod 校验命令，TS 类型描述推送 |
| 界面 | React 19 + Tailwind CSS v4 + shadcn/ui（Base UI）、Zustand、cmdk、sonner |
| 内容渲染 | react-markdown + remark-gfm + remark-math / rehype-katex、mermaid、shiki（Worker） |

## 响应速度

- 窗口从 Dock 重新打开没有等待（关窗口只隐藏，位置尺寸自动记忆）。
- 流式输出时滚动和输入不掉帧（帧合并、只更新正在输出的条目）。
- 长会话打开不卡（首次只取最近 300 条，向上滚动按需加载更早条目）。

## 已知取舍

- core 被系统杀掉或崩溃时，正在跑的那一轮会中断（已完成的消息都在会话文件里），发一句“继续”即可；需要时再考虑改为 launchd 用户级服务。
- 第三方 Pi 扩展里强依赖终端 TUI 覆层的界面（如 `pi-subagents` 的终端小组件、`/agents` 菜单）在 GUI 里不可用；其工具调用、结果、后台通知和标准 `ctx.ui` 交互都正常工作。
- 同一个会话不要同时在终端 `pi` 和 Lila 里并发写入。

## 范围

- **现在：完整的编程桌面端。** 项目管理 → 统一设置中心（登录/模型/用量/MCP/长期记忆/扩展包）→ 对话与子 Agent 编排 → 审批与提问 → 队列管理、回退重发与消息分叉 → 文件浏览、Git Diff 与本轮改动审阅。
- **之后：能力深化与个人助理方向。** 工作区 Git 可写操作闭环、更多精选 Pi 社区扩展（联网搜索、文件级 Checkpoint 回滚等）、定时任务管理界面（复用 `pi-subagents` 的 `schedule`）、独立个人助理工作目录、应用打包分发与原生手机端（复用同一套 WebSocket 协议）。
