# 第一版任务单

目标：在 Mac mini 上用 Lila 完成一次真实的编程任务。架构见 [architecture.md](architecture.md)。

按顺序做，每项做完能单独验证。

## 下一个任务

### 1. 实测 omp RPC（收尾）

主体已完成，结论是 omp RPC 可用：Claude Code 和 Codex 两条通道都跑通了真实对话、工具调用和一次审批拒绝，记录在 [omp-rpc.md](omp-rpc.md)。

剩下几项用 `scripts/omp-probe.ts` 补测，可以和第 2 项并行，结果补进 `omp-rpc.md`：

- 审批选 `Approve` 后的完整流程，以及多个工具同时等待审批的情况。
- `always-ask-cached` 档的授权选项在 RPC 下的实际格式。
- `abort`、`switch_session`、`get_messages_page` 的行为。
- 试用官方 `RpcClient`（npm 包 `@oh-my-pi/pi-coding-agent`），确认在 Bun 下可以直接用。
- 在 Claude 和 ChatGPT 的用量页面核对额度扣在订阅上（需要用户自己看）。

## 之后的任务

### 2. 搭仓库骨架

Bun workspace，三个包：`packages/protocol`、`packages/core`、`apps/desktop`。`core` 用 Bun 能启动并监听本机 WebSocket，Electron 窗口能打开并连上它，shadcn/ui（Base UI）初始化完成，lint、类型检查、测试命令可用。

### 3. 事件协议

在 `protocol` 里用 Zod 定义 Lila 事件和命令：会话创建、用户消息、助手文本增量、工具调用开始和结束、审批请求和答复、运行结束、出错。每个事件带会话 ID 和序号。

### 4. core：会话与 omp 适配

- 用 drizzle 建表：项目、会话、事件、待审批项。
- 启动和看护 omp 子进程，把 RPC 消息翻译成 Lila 事件并落库。
- 通过 WebSocket 提供订阅：从指定序号开始补发并持续推送。
- 定时记录自身内存占用，用来观察 Bun 常驻是否稳定。
- 测试用录制的 omp 输出回放，不依赖真实模型。

### 5. 桌面界面：会话

- 窗口通过 WebSocket 连接 `core`，断开后自动重连并从最后的序号续上。
- 用 Message Scroller、Message、Bubble、Trace、Code Block 拼出会话界面，流式显示文本、工具输出和 diff。
- 输入框发送消息，可以中断运行。
- 流式增量按帧合并，只更新正在输出的那条消息；代码高亮放在 Worker 里。

### 6. 提问与审批

- 会话默认用 `yolo` 档启动 omp，不审批。每个会话可以切到 `write` 或 `always-ask`，切换时重启该会话的 omp 进程。
- Agent 的提问（`ask`）和非 `yolo` 档下的审批请求，在界面上显示为待处理项（Questionnaire），用户选择后回传。
- 有待处理项而应用不在前台时，发系统通知。

### 7. 项目与模型选择

侧边栏列出项目和会话，可以添加本地目录为项目、新建会话、恢复旧会话。按会话选择模型，模型列表来自 omp。

### 8. 常驻与恢复

- 关窗口时窗口隐藏而不销毁，应用留在菜单栏。
- 应用启动时，`core` 已在运行就直接连上，否则拉起它；应用退出后 `core` 继续跑。
- `core` 自己重启后，未完成的运行标记为中断，可以从已保存的会话继续。
- 发布时把 `core` 编译成单个可执行文件，放进应用包。

### 9. 用它干一件真事，并验收响应速度

用 Lila 给 Lila 自己做一个小改动，记录不顺手的地方，决定下一步。

同时按架构文档的三条标准实测：

- 窗口从菜单栏重新打开，感觉不到延迟。
- 流式输出时，滚动和输入不掉帧。
- 几百条消息的长会话，打开不卡。

达不到就先查应用代码；确认是 Electron 本身的限制，再讨论换壳。
