# Lila 架构

Lila 是个人用的 Agent harness，主打编程和个人助理。界面完全自己掌控，Agent 能力复用现成运行时。

## 总体结构

Lila 是一个运行在 Mac mini 上的 Electron 应用。应用自己就是后端：主进程持有 Agent 进程和数据，窗口只负责显示。

```text
Electron 应用（Mac mini）
  ├─ 窗口：React 界面
  │        ↕ IPC（事件协议）
  └─ 主进程（Node，TypeScript）
       └─ core
            ├─ 会话、事件日志、待审批项（SQLite）
            ├─ omp RPC 子进程（每个会话一个）
            └─ WebSocket 服务（以后给手机端，经 Tailscale 访问）
```

## 设计约束

- **关窗口不等于退出。** 关掉窗口后应用留在菜单栏，任务继续跑。重新打开窗口时从事件日志恢复显示。
- **`core` 不依赖 Electron。** 它对外只暴露事件协议。窗口通过 IPC 用它，手机端以后通过 WebSocket 用它。需要时可以把它拆成独立的后台服务。
- **界面只认 Lila 自己的事件协议。** omp 的 RPC 消息在 `core` 的适配层里翻译成 Lila 事件。以后换运行时或加第二个运行时，只改适配层。
- **事件日志是唯一事实来源。** 每个事件有会话内递增的序号并落库。客户端连接时带上最后收到的序号，`core` 补发之后的事件。
- **审批走 `rpc-ui` 模式。** omp 以 `--mode rpc-ui` 启动，审批请求由 `core` 落库并转给界面，用户的选择再回传给 omp。
- **界面组件用现成的。** 会话界面用 shadcn/ui 自带的 AI 组件拼装，不自己重写一套。

## 技术栈

| 部分 | 选择 |
|---|---|
| 仓库 | pnpm workspace：`packages/protocol`、`packages/core`、`apps/desktop` |
| 桌面壳 | Electron + electron-vite，打包用 electron-builder |
| Agent 运行时 | omp（`--mode rpc-ui`），作为子进程 |
| 协议 | Zod 定义事件类型，IPC 和 WebSocket 共用 |
| 存储 | better-sqlite3 + drizzle-orm `1.0.0-rc.5`（drizzle-kit 同版本，版本号写死） |
| 界面 | React + Tailwind + shadcn/ui（Base UI 版本） |
| 状态 | Zustand，接收主进程事件 |
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

## 范围

- **第一版：编程流程。** 选项目 → 选模型 → 发任务 → 看工具输出和 diff → 审批 → 关窗口再打开后接着看。
- **之后：个人助理。** 定时任务、通知、个人工具，以及原生手机端。手机端通过 WebSocket 接入同一套事件协议。

## 待验证

- omp 用 Claude 和 ChatGPT 订阅登录后，在 RPC 模式下能否正常对话、额度如何扣除。见任务单第 1 项。
- drizzle-orm v1 的 API 和 0.x 不同，写代码时以 rc.5 的官方文档为准。
