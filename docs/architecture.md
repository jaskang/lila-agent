# Lila 架构

Lila 是个人用的 Agent harness，主打编程和个人助理。界面完全自己掌控，Agent 能力复用现成运行时。

选型只有一个标准：产品体验。下面列出的库和方案都是当前的选择，不是约束，发现更好的就换。

## 总体结构

Lila 运行在 Mac mini 上，由两个进程组成：Electron 桌面应用负责界面，`core` 是独立的后台进程，持有 Agent 进程和数据。

```text
Electron 应用
  ├─ 窗口：React 界面 ──┐
  └─ 主进程：窗口、菜单栏、通知；负责拉起 core
                        │ WebSocket（localhost）
core（Bun 独立进程）←───┘←── 以后的手机端（经 Tailscale）
  ├─ 会话、事件日志、待审批项（SQLite）
  └─ omp RPC 子进程（每个会话一个）
```

## 设计约束

- **`core` 独立于桌面应用。** 应用退出或升级时 `core` 继续跑，任务不中断。应用启动时发现 `core` 已在运行就直接连上，否则拉起它。
- **所有客户端都走 WebSocket。** 窗口直接连 `core`，不经过 Electron 的 IPC。桌面端和以后的手机端用同一套协议。
- **界面只认 Lila 自己的事件协议。** omp 的 RPC 消息在 `core` 的适配层里翻译成 Lila 事件。以后换运行时或加第二个运行时，只改适配层。
- **事件日志是唯一事实来源。** 每个事件有会话内递增的序号并落库。客户端连接时带上最后收到的序号，`core` 补发之后的事件。
- **审批走 `rpc-ui` 模式。** omp 以 `--mode rpc-ui` 启动，审批请求由 `core` 落库并转给界面，用户的选择再回传给 omp。
- **关窗口不等于退出。** 窗口隐藏而不销毁，应用留在菜单栏，再次打开没有等待。
- **界面线程只管渲染。** 重活不放在界面线程：代码高亮放 Worker，流式增量按帧合并，只更新正在输出的那条消息。
- **界面组件用现成的。** 会话界面用 shadcn/ui 自带的 AI 组件拼装，不自己重写一套。

## 响应速度

响应速度是第一版的验收标准，不是事后优化项：

- 窗口从菜单栏重新打开，感觉不到延迟。
- 流式输出时，滚动和输入不掉帧。
- 几百条消息的长会话，打开不卡。

## 技术栈

| 部分 | 选择 |
|---|---|
| 仓库 | Bun workspace：`packages/protocol`、`packages/core`、`apps/desktop` |
| 桌面壳 | Electron + electron-vite，打包用 electron-builder |
| `core` 运行时 | Bun；发布时用 `bun build --compile` 编译成单个可执行文件，放进应用包 |
| Agent 运行时 | omp（`--mode rpc-ui`），作为 `core` 的子进程 |
| 协议 | Zod 定义事件和命令类型，经 WebSocket 传输 |
| 存储 | `bun:sqlite` + drizzle-orm `1.0.0-rc.5`（drizzle-kit 同版本，版本号写死） |
| 界面 | React + Tailwind + shadcn/ui（Base UI 版本） |
| 状态 | Zustand，接收 `core` 推来的事件 |
| 路由 | 第一版不用；以后需要时用 TanStack Router |

### 会话界面用到的 shadcn 组件

| 用途 | 组件 |
|---|---|
| 消息滚动、流式跟随、历史加载 | Message Scroller |
| 消息与气泡 | Message、Bubble |
| 思考过程、工具调用 | Trace |
| 代码、终端输出、diff | Code Block（高亮用 shiki） |
| Agent 向用户提问、审批选项 | Questionnaire |
| 状态提示、分隔 | Marker |
| 文件和图片附件 | Attachment |
| Markdown 排版 | Typeset |

文档地址形如 `https://ui.shadcn.com/docs/components/base/message-scroller`。写界面前先看对应组件的文档，缺的能力优先找 shadcn 里有没有，再考虑自己写。

## 参考实现

DeepSeek Harness 的官方桌面端（`deepseek-ai/deepseek-harness`）也是 Electron + React，后端在单独的 `apps/desktop-host` 包里。它的进程划分、界面和后端的通信方式、性能测试配置都可以参考。

## 范围

- **第一版：编程流程。** 选项目 → 选模型 → 发任务 → 看工具输出和 diff → 审批 → 关窗口再打开后接着看。
- **之后：个人助理。** 定时任务、通知、个人工具，以及原生手机端。手机端通过 WebSocket 接入同一套事件协议。

## 待验证

- omp 用 Claude 和 ChatGPT 订阅登录后，在 RPC 模式下能否正常对话、额度如何扣除。见任务单第 1 项。
- Bun 作为常驻进程的内存是否稳定。第一版记录 `core` 的内存占用，连续跑几天看曲线；有问题就换回 Node 运行，改动集中在 SQLite 和 WebSocket 两处。
- drizzle-orm v1 的 API 和 0.x 不同，写代码时以 rc.5 的官方文档为准。
