# Lila 架构

Lila 是个人用的 Agent harness，主打编程和个人助理。界面完全自己掌控，Agent 能力复用现成运行时。

选型只有一个标准：产品体验。下面列出的库和方案都是当前的选择，不是约束，发现更好的就换。

## 总体结构

Lila 运行在 Mac mini 上，由两个进程组成：Electron 桌面应用负责界面，`core` 是独立的后台进程，持有 durable Agent Runtime 和数据。

```text
Electron 应用
  ├─ 窗口：React 界面 ──┐
  ├─ preload：只暴露取连接信息、选目录、聚焦窗口三个能力
  └─ 主进程：窗口、通知；负责拉起 core
                        │ WebSocket（localhost，带 token）
core（Bun 独立进程）←───┘←── 以后的手机端（经 Tailscale）
  ├─ server.ts：鉴权、命令分发、按会话推送
  └─ durable.ts：pi-durable Harness、官方工具、SQLite 持久化和 Lila 事件投影
```

## 设计约束

- **`core` 独立于桌面应用。** 应用退出或升级时 `core` 继续跑，任务不中断。应用启动时发现 `core` 已在运行就直接连上，否则拉起它；`core` 的协议版本和应用不一致时，应用先停掉旧的再拉起新的。`core` 的输出写到数据目录下的 `core.log`。
- **所有客户端都走 WebSocket，并且要鉴权。** `core` 首次启动时在数据目录（`~/Library/Application Support/Lila`）生成 `core-token`（权限 0600），客户端用 `lila.<token>` 子协议连接；`core` 同时拒绝浏览器网页的 `Origin`，因为任何网页都能访问 localhost，而默认档位下 Agent 能执行任意命令。Electron 的 IPC 只用来把 token 交给窗口，协议本身不走 IPC。
- **界面只认 Lila 自己的事件协议。** 每种事件的载荷在 `protocol` 里都有明确的类型，durable 的 transcript 结构只在 `core` 的 `durable.ts` 里转换。
- **事件日志只存定稿的事实。** 用户消息、完整的助手消息、工具开始和结束（含 diff）、待处理项、运行终态落库，每个事件有会话内递增的序号。流式增量（文本增量、工具进度）只推给正在看这个会话的客户端，不落库；客户端打开一个正在运行的会话时，`core` 在补发事件之外附带当前未完成输出的快照。这样长会话的日志不会被 token 级增量撑大，打开时也不用重放增量。
- **每个会话的命令串行执行。** 发消息、停止、回答、改配置在同一个会话里排队处理，避免改配置时重启 Pi 和发消息互相踩。
- **默认不审批，按会话可调。** Lila 通过 durable 的工具注册表执行工具，默认 `yolo`（全部工具直接执行），每个会话可以切到 `write` 或 `always-ask`。
- **审批和提问由 durable 文档记录。** 工具 hook 写入待处理项和用户回答，任务在等待期间保持可恢复，Core 重启后由 Harness 继续调度。
- **关窗口不等于退出。** 窗口隐藏而不销毁，应用留在 Dock，再次打开没有等待。
- **界面线程只管渲染。** 代码高亮放 Worker，流式增量按帧合并，只更新正在输出的那条消息。
- **界面组件用现成的。** 会话界面用 shadcn/ui 自带的 AI 组件拼装，不自己重写一套。

## 响应速度

响应速度是第一版的验收标准，不是事后优化项：

- 窗口从 Dock 重新打开，感觉不到延迟。
- 流式输出时，滚动和输入不掉帧。
- 几百条消息的长会话，打开不卡。

## 技术栈

| 部分          | 选择                                                                                                     |
| ------------- | -------------------------------------------------------------------------------------------------------- |
| 仓库          | Bun workspace：`packages/protocol`、`packages/core`、`packages/app-desktop`                              |
| 桌面壳        | Electron + electron-vite，打包用 electron-builder                                                        |
| `core` 运行时 | Bun；发布时用 `bun build --compile` 编译成单个可执行文件，放进应用包                                     |
| Agent 运行时  | `pi-durable` Harness、`pi-coding-agent` ModelRuntime 和官方 durable tools，直接嵌入 `core`；细节见 [pi-durable.md](pi-durable.md) |
| 协议          | Zod 定义事件和命令类型，经 WebSocket 传输                                                                |
| 存储          | `pi-durable` 官方 SQLite storage，文档和 transcript 由 Harness 管理                               |
| 界面          | React + Tailwind + shadcn/ui（Base UI 版本），Markdown 用 react-markdown + shadcn Typeset，提示用 sonner |
| 状态          | Zustand，接收 `core` 推来的事件                                                                          |
| 路由          | 第一版不用；以后需要时用 TanStack Router                                                                 |

### 会话界面用到的 shadcn 组件

| 用途                         | 组件                       |
| ---------------------------- | -------------------------- |
| 消息滚动、流式跟随、历史加载 | Message Scroller           |
| 消息与气泡                   | Message、Bubble            |
| 思考过程、工具调用           | Trace                      |
| 代码、终端输出、diff         | Code Block（高亮用 shiki） |
| Agent 向用户提问、审批选项   | Questionnaire              |
| 状态提示、分隔               | Marker                     |
| 文件和图片附件               | Attachment                 |
| Markdown 排版                | Typeset                    |

文档地址形如 `https://ui.shadcn.com/docs/components/base/message-scroller`。写界面前先看对应组件的文档，缺的能力优先找 shadcn 里有没有，再考虑自己写。

## 参考实现

DeepSeek Harness 的官方桌面端（`deepseek-ai/deepseek-harness`）也是 Electron + React，后端在单独的 `apps/desktop-host` 包里。它的进程划分、界面和后端的通信方式、性能测试配置都可以参考。

## 范围

- **第一版：编程流程。** 选项目 → 选模型 → 发任务 → 看工具输出和 diff → 关窗口再打开后接着看。
- **之后：个人助理。** 定时任务、通知、个人工具，以及原生手机端。手机端通过 WebSocket 接入同一套事件协议。

## 待验证

- `pi-durable` 已接入，已完成真实模型调用、Core WebSocket 冒烟和 durable SQLite 会话创建验证。完整编程任务、审批恢复、响应速度和额度扣费仍需分别验收。
- Bun 作为常驻进程的内存是否稳定。`core` 每 10 分钟在 `core.log` 记录一次内存占用，连续跑几天看曲线；有问题就换回 Node 运行，改动集中在 SQLite 和 WebSocket 两处。
- `core` 被系统杀掉后不会自动重启，要等下次打开应用。如果实际用下来经常发生，再改成 launchd 用户级常驻服务。
