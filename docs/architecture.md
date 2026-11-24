# Lila 架构

Lila 是个人用的 Agent 图形界面，内核是 [Pi](https://pi.dev)。界面自己做，Agent 能力全部复用 Pi 和它的生态：凡是 Pi 已经有的（模型、认证、技能、MCP、扩展、子 Agent、压缩、会话格式），Lila 不再造一遍。

## 总体结构

```text
macOS 原生客户端（Rust + GPUI Kit）
  ├─ 窗口：项目/会话侧栏、消息流、输入区、审批
  ├─ 系统桥接：选目录、打开/定位文件
  └─ CoreHost：发现 Node、启动或复用 core
                 │ WebSocket（localhost，token 子协议）
core（Node，独立常驻进程）←────┘←── 以后的手机端（经 Tailscale）
  ├─ server.ts：鉴权、命令分发、按会话推送
  ├─ 会话宿主：每个 Lila 管理的可写会话一个 Pi AgentSession（pi-coding-agent SDK）
  │    ├─ Pi 资源加载：AGENTS.md、技能、提示模板、用户扩展、Pi 包
  │    ├─ 内置扩展：MCP、codemode、tool_search、子 Agent（pi-subagents）
  │    └─ Lila 扩展：工具审批、ask_user 提问
  ├─ 界面桥：Pi 扩展的 select/confirm/input/editor → 待处理项
  └─ transcript 投影：Pi 会话条目 → 界面条目
```

## 关键决定

- **内核用 `pi-coding-agent` SDK，不用 `pi-durable`。** 见 [ADR 0001](adr/0001-pi-sdk-kernel.md)。SDK 就是 `pi` 命令行本身用的那套运行时，MCP、技能、扩展、子 Agent 都在这一层；`pi-durable` 是实验性的底层运行时，这些都没有。
- **配置和会话与 `pi` 命令行共用。** 模型、认证、`settings.json`、`mcp.json`、技能、扩展和 Pi 包都在 `~/.pi/agent`（`PI_CODING_AGENT_DIR` 可改）。会话就是 Pi 的 JSONL 会话文件，在终端里 `pi --resume` 能接着 Lila 的会话做，反过来也一样。装 MCP 或技能用 Pi 的方式：`pi mcp add …`、`pi install …`，或直接编辑 `mcp.json`，界面里“能力”面板可打开配置并重新加载。
- **Lila 自己只存很少的东西。** 数据目录（`~/Library/Application Support/Lila`）里只有 `core-token`、`state.json`（项目列表、各会话的审批方式）、`core.log` 和删除会话时移入的 `trash/`。没有自己的消息库。
- **core 是独立的 Node 进程。** 原生客户端发现本机 Node ≥ 22.19，也可以通过 `LILA_NODE_BIN` 显式指定。应用退出后 core 可继续运行。客户端通过健康检查和协议版本判断是否复用当前进程。
- **所有客户端都走 WebSocket 并鉴权。** core 首次启动生成 `core-token`（0600），客户端用 `lila.<token>` 子协议连接；core 拒绝网页 `Origin`。
- **界面只认 Lila 的协议。** `packages/protocol` 定义命令（Zod 校验）和推送消息（TS 类型）。Pi 的消息结构只在 core 的 transcript 投影里出现，以后的手机端不必理解 Pi 的内部类型。
- **打开会话 = 拿快照 + 收推送。** `session.open` 返回最近的条目和当前状态（含正在流式输出的部分），之后 core 推 `item`（新增/替换条目）、`delta`（流式文本）、`state`。断线重连就重新 open，不需要序号补发。会话内查找由 core 匹配完整投影，客户端只读取命中条目附近的窗口。回退、压缩这类改变形状的操作推 `reset`，客户端重新 open。协议分别报告本会话的 Run 和后台子 Agent；子 Agent 的原会话始终由 pi-subagents 写入，桌面实时读取已落盘的记录，结束后可从该记录继续为新会话。
- **审批和提问是 Pi 扩展。** Lila 内置的扩展在 `tool_call` 钩子里按会话的审批方式拦截工具，等用户在界面上回答；任何 Pi 扩展调用 `ctx.ui.select/confirm/input/editor` 也会变成同样的待处理卡片。会话在扩展初始化完成前即可订阅和应答，首条消息会等初始化完成再送达。待处理项只在内存里：core 重启时正在跑的任务本来也会中断。
- **流式更新按批交给界面。** WebSocket 客户端按约 16 ms 合并推送，消息流使用 GPUI Kit 的虚拟列表。
- **历史导航与子会话。** core 提供会话内搜索和轮次分页接口。原生客户端当前支持加载较早消息，以及在主工作区打开只读子会话、返回父会话或继续为独立会话。Run 状态和耗时随 Pi 会话记录保存。
- **界面复用 GPUI Kit 组件。** 消息流、Markdown、输入区、命令面板和菜单复用 `MessageScroller`、`TextView`、`Textarea`、`Command`、`Popover`、`Dialog` 等组件。玻璃窗口和卡片规范见 [DESIGN.md](../DESIGN.md)。

## 能力从哪来

| 能力 | 来源 |
| --- | --- |
| 模型、认证、思考深度 | Pi `ModelRuntime`，配置在 `~/.pi/agent` |
| 读写改、bash、grep/find/ls | Pi 内置工具 |
| MCP（stdio / HTTP / OAuth） | Pi 内置 MCP 扩展，配置 `~/.pi/agent/mcp.json` 与项目 `.pi/mcp.json` |
| codemode、工具搜索 | Pi 内置扩展，MCP 服务器连上后自动启用 |
| 技能、提示模板、斜杠命令 | Pi 资源加载（`~/.pi/agent/skills`、`~/.agents/skills`、项目 `.agents/skills` 等） |
| 子 Agent（前台/后台、并行、工作流、定时） | [`@tintinweb/pi-subagents`](https://github.com/tintinweb/pi-subagents)，用户自己 `pi install` 过就用用户的那份 |
| AGENTS.md / CLAUDE.md | Pi 上下文文件 |
| 压缩、自动重试、插话与追加消息 | Pi `AgentSession` |
| 工具审批、`ask_user` 提问 | Lila 内置 Pi 扩展 |

## 技术栈

| 部分 | 选择 |
| --- | --- |
| 仓库 | Bun workspace 管理 `packages/protocol` 和 `packages/core`，独立 Cargo workspace 管理 `packages/app-native` |
| 桌面壳 | Rust + GPUI Kit，macOS 原生窗口 |
| core 运行时 | Node ≥ 22.19，WebSocket 用 `ws` |
| Agent 内核 | `@earendil-works/pi-coding-agent` SDK |
| 协议 | Zod 校验命令，TS 类型描述推送 |
| 界面 | GPUI Kit + gpui-component |
| 状态 | GPUI `Entity<AppStore>` |

## 响应速度验收目标

- 从 Dock 重新打开窗口时快速恢复原会话。
- 流式输出时滚动和输入不掉帧（帧合并、只更新正在输出的条目）。
- 长会话打开不卡（首次只给最近 300 条，往上滚再取）。

## 已知取舍

- core 被系统杀掉或崩溃时，正在跑的那一轮会中断（已完成的消息都在会话文件里），用户发一句“继续”即可。需要时再把 core 改成 launchd 用户级服务。
- 子 Agent 包里依赖终端界面的部分（实时小组件、`/agents` 菜单）在 GUI 里不可用；工具调用、结果、后台完成通知都正常显示。
- 同一个会话不要同时在终端 `pi` 和 Lila 里写。

## 范围

- **现在：编程。** 选项目 → 发任务 → 看工具输出、diff、子 Agent → 审批/回答问题 → 回退重发 → 关窗口再打开接着看。
- **之后：个人助理。** 定时任务（pi-subagents 已有定时能力，需要做界面）、通知、个人工具，以及原生手机端（同一套协议）。
