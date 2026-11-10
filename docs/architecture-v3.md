# Lila 架构 v3：多 agent 运行时与原生 GUI（草案）

> 2026-11-09 · 用户的决定：**默认用原生 GUI**。Codex 走官方的 `codex app-server`；omp 走 ACP；Claude Code 因为官方对 `-p` 和 SDK 的限制，**内部照旧跑 TUI，由 Lila 代理出一个 GUI**，并且可以在 TUI 和 GUI 之间切换。
>
> 一句话结论：**每个成员仍然是一个 hc 终端**，lilad 现有的生命周期、状态和派活机制一律不动。新增两样东西：一套与 agent 无关的**对话事件模型**，以及按运行时实现的**驱动**。Claude 的驱动从 TUI 旁边收集事件。Codex 和 omp 的驱动是 Lila 自己的托管进程 `lila host`，它在 hc 终端里运行，通过官方协议驱动 agent，并像普通程序一样向 hc 报告状态。窗口只认对话事件，所以 GUI 只需要写一份。
>
> 本文核对过的版本：Claude Code 2.1.319、codex-cli 0.166.0、omp 18.18.3（都在本机）。

---

## 1. 目标与约束

**目标**
- 不熟悉 TUI 的人也能看懂、管好每个成员：对话、工具调用、改了什么、卡在哪里、要它做什么决定。
- 再接一种 agent 时，只需要写一个驱动，不用改 lilad 的调度和窗口。

**不变的约束**
- lilad 是唯一的写入方。窗口和 CLI 都通过 socket 跟它说话。
- Claude 和 Codex 走用户自己的订阅，Lila 不碰它们的 token（`docs/HANDOFF.md`）。
- 成员还是通过 `lila` 命令参与协议（`lila task done …`），由 `LILA_PROJECT` 和 `LILA_MEMBER` 环境变量表明身份。
- 界面不写说明文字（DESIGN.md）。
- 优先用成熟的库：ACP 用官方 crate `agent-client-protocol`。

**不做**
- 不用 `claude -p` 或 Agent SDK 驱动 Claude：订阅政策和计费都有疑问。
- 不在窗口里直接跑 agent：agent 进程一律由 lilad 通过 hc 管理。

---

## 2. 现状：lilad 怎么驱动成员

| 环节 | 现在的做法 | 代码 |
|---|---|---|
| 启动 | 在 hc 里起一个终端，跑官方 CLI；带标签 `lila.project`、`lila.member`，环境变量里有身份 | `daemon/requests.rs::start_session`、`lila_core::runtime::launch` |
| 派活、发通知 | 把文字**打进终端**再回车；只在空闲时打字（hc 的 `if`） | `Terminals::send`、`Daemon::pump` |
| 状态 | 程序通过 **OSC 7501** 向 hc 报告 `idle`、`working`、`done`、`blocked`、`error`，hc 推给 lilad | `Daemon::on_event`、`set_status` |
| 会话、用量、额度 | 事后读 CLI 自己写的记录文件：Claude 的 `<id>.jsonl`；Codex 的 rollout，会话 id 还得靠猜 | `sessions.rs`、`usage.rs`、`watch::watch_quota`、`watch::find_sessions` |
| 特殊对话框 | 信任目录、登录：成员一直不报状态时出「启动卡住」卡片，由人去终端里处理 | `watch.rs`、`inbox::startup` |
| 窗口 | 中间显示成员的终端（`lila-term`，基于 libghostty） | `lila-app/src/ui.rs::center` |

这套做法的长处是**和 agent 的种类基本无关**：只要是能在终端里跑、会报 OSC 7501 状态的程序就行。v3 保留这一点。它缺的是**内容**：窗口只能显示终端画面，lilad 也不知道成员在做什么。

---

## 3. 三种运行时怎么接（2026-11-09 实测）

| | Claude Code | Codex | omp |
|---|---|---|---|
| 进程 | 官方 TUI（不变） | `codex app-server`，在 stdio 上收发 JSON-RPC | `omp acp`，在 stdio 上跑 ACP |
| 谁来跑 | hc 终端直接跑 `claude` | hc 终端跑 `lila host codex`，它再起子进程 | hc 终端跑 `lila host omp`，同上 |
| 对话内容 | 会话记录文件 `~/.claude/projects/<目录>/<id>.jsonl`，边跑边追加，含消息、思考、`tool_use`、`tool_result`、权限决定 | `item/started`、`item/completed`，以及消息、思考、命令输出的增量 | `session/update`：`agent_message_chunk`、`agent_thought_chunk`、`tool_call`、`tool_call_update`、`plan`、`usage_update` |
| 轮次和状态 | hc 的 OSC 7501；hooks 里的 `UserPromptSubmit`、`Stop`、`StopFailure`、`Notification` | `turn/started`、`turn/completed`、`thread/status/changed` | `session/prompt` 返回时附带 `stopReason` |
| 权限确认 | hooks 里的 `PermissionRequest`：可以返回 allow 或 deny，默认超时 600 秒 | 服务端请求 `item/commandExecution/requestApproval`、`item/fileChange/requestApproval`、`item/permissions/requestApproval` | `session/request_permission`：执行命令、编辑、删除、移动这几类工具要确认，选项有 allow 或 reject 的一次和总是 |
| 系统提示词 | `--append-system-prompt-file`（不变） | `thread/start` 的 `developerInstructions` | 待原型验证（`omp` 有 `--append-system-prompt`） |
| 会话 id | Lila 用 `--session-id` 指定 | `thread/start` 直接返回 `threadId`，不用再猜 rollout | `session/new` 返回 |
| 续接 | `--resume <id>` | `thread/resume` | `session/load` 或 `session/resume` |
| 运行中插话 | 工作时也能打字，CLI 会排队 | `turn/steer` | 无；由 Lila 排队，等这一轮结束再发 |
| 中断 | hc 的 `interrupt` | `turn/interrupt` | `session/cancel` |
| 换目录（worktree 槽） | 在新目录重启 CLI（不变） | `turn/start` 的 `cwd` 参数，**不用重启** | 在新目录开一个新会话 |
| token 和上下文 | 读会话记录文件（不变） | `thread/tokenUsage/updated` | `usage_update` |
| 额度 | hooks：`StopFailure`、`Notification` 的 `quota_*` | `account/rateLimits/updated`、`account/rateLimits/read` | 错误事件 |
| 登录 | `claude auth login`（不变） | `account/login/start`（ChatGPT 订阅） | omp 自己的登录或 API key |
| 能切到 TUI | ✅ | ✗ | ✗ |

Claude 的 hooks 只对 Lila 起的会话生效，不动用户自己的配置：通过 `--settings <文件>` 或 `--plugin-dir <目录>` 加载（插件是官方列出的 hook 来源之一）。用哪一种由原型 S1 决定。

---

## 4. 总体结构

```
lila-app（GPUI）
   │  socket：现有方法 + conversation.* + 订阅推送
   ▼
lilad（唯一写入方）
   ├─ 调度、任务、请求、收件箱、worktree 槽……（不变）
   ├─ 对话存储：每个成员的规范化条目（SQLite）
   └─ 驱动注册表：按 runtime 分派
        │
        ├─ ClaudeDriver ──hc──▶ 终端：claude（TUI）
        │     ├─ 读会话记录文件 → 条目
        │     └─ hooks：claude 调用 `lila hook` → socket → lilad（权限确认在这里等答复）
        │
        └─ HostDriver ──unix socket──▶ 终端：lila host <runtime>
                                          ├─ 发 OSC 7501 状态 → hc → lilad（沿用现有状态机）
                                          ├─ codex：子进程 `codex app-server`（JSON-RPC）
                                          └─ omp：  子进程 `omp acp`（agent-client-protocol crate）
```

### D1 每个成员仍是一个 hc 终端；Codex 和 omp 由 `lila host` 托管

为什么不让 lilad 直接起 `codex app-server` 子进程：
- **lilad 重启时成员不能断。** 现在 `lila daemon stop` 时成员照样运行，lilad 回来后通过 hc 重新对上。如果是 lilad 的子进程，stdio 一断，成员就跟着没了。
- **生命周期不用改。** 起停、标签、`list`、`reconcile`、`kill`、「成员退出」的处理都已经基于 hc，`lila host` 只是换了个被启动的程序。
- **状态机不用改。** host 自己按照对话事件发 OSC 7501 状态。lilad 现有的空闲才派活、`stall`、`unstick`、`runaway` 检测、「卡住」卡片全都照常工作。
- **方便排查。** host 的终端会打印一行一条的可读日志，出问题时能直接看到。界面上默认不显示。

host 和 lilad 之间：unix socket（`$LILA_HOME/state/hosts/<project>.<member>.sock`），每行一个 JSON。
- lilad 发给 host 的是**命令**（§6）。
- host 发给 lilad 的是**带序号的事件**。host 保留尚未确认的事件；lilad 重连时报告最后收到的序号，host 从那里补发。

### D2 一套与 agent 无关的对话事件模型

新 crate `lila-agent` 定义事件（§5）。三个驱动都把自己的数据**转换**成这套事件，lilad 和窗口只认这一种。形状参照 ACP 的会话更新，因为 ACP 本来就是为渲染 agent 界面设计的。

### D3 lilad 只存规范化后的条目，不存增量

条目（一条消息、一次工具调用）以 id 为键，更新时覆盖原来的记录，并分配一个递增的序号。逐字的增量只放在内存里转发给窗口，不落盘。窗口按「从某个序号之后」增量读取，再加上订阅推送。

### D4 权限确认统一处理：先看角色策略，剩下的才问人

三种运行时的权限请求都转成同一种 `PermissionRequest`。lilad 先按角色的权限规则自动决定（例如 `lila …` 命令一律允许，读文件一律允许），剩下的才变成收件箱卡片，同时显示在对话视图底部。人的回答再沿原路送回去：Claude 是 hook 的返回值，Codex 是服务端请求的响应，omp 是 `request_permission` 的结果。

### D5 Claude 的 TUI/GUI 切换

Claude 成员的 TUI 一直在 hc 里运行。窗口显示 GUI（对话视图）还是 TUI（`lila-term` 终端视图），由标题行上的一个两段式切换控件决定，每个成员单独记住。只有 TUI 能处理的场合（信任目录、登录、`/model` 这类菜单）会提示切过去。

---

## 5. 事件模型（`lila-agent`）

```rust
/// 驱动 → lilad。顺序就是发生的顺序；host 发出的事件带 seq。
pub enum AgentEvent {
    /// 会话 id 一旦知道就报（Claude 是启动时，Codex 是 thread/start 之后，omp 是 session/new 之后）。
    Session { id: String },
    /// 只有 host 驱动会发；Claude 的状态仍然来自 hc（§7.1）。
    Status(Status),
    TurnStarted { turn: String, origin: Origin },
    TurnEnded { turn: String, end: TurnEnd },
    /// 新建或更新一个条目（以 id 为键，整条覆盖）。
    Item(Item),
    /// 进行中条目的文字增量：只转发给窗口，不落盘。
    Delta { item: String, part: Part, text: String },
    Permission(PermissionRequest),
    PermissionResolved { id: String, by: Resolver },
    Usage { input: u64, output: u64, cached: u64, context: Option<u64> },
    Limits(Vec<Limit>),
    Notice { level: Level, text: String },
}

pub enum Status { Idle, Working, Blocked { why: String }, Error { why: String } }
pub enum TurnEnd { Done, Interrupted, Failed(String), Quota { resets_ms: Option<i64> } }
pub enum Origin { Person, Lila { task: Option<String> }, Agent }

pub struct Item { pub id: String, pub turn: Option<String>, pub at_ms: i64, pub kind: ItemKind, pub state: ItemState }
pub enum ItemState { Running, Done, Failed }
pub enum ItemKind {
    UserMessage { text: String, origin: Origin },
    AgentMessage { text: String },          // markdown
    Thought { text: String },
    ToolCall { tool: ToolKind, title: String, input: serde_json::Value,
               output: Option<String>, diffs: Vec<FileDiff>, exit: Option<i32> },
    Plan { steps: Vec<(String, StepState)> },
}
/// 和 ACP 的 ToolKind 一致，窗口按它选图标。
pub enum ToolKind { Read, Edit, Delete, Move, Search, Execute, Fetch, Think, Other }

pub struct PermissionRequest { pub id: String, pub item: Option<String>, pub tool: ToolKind,
                               pub title: String, pub detail: String, pub options: Vec<PermOption> }
pub struct PermOption { pub id: String, pub label: String, pub kind: PermKind }
pub enum PermKind { AllowOnce, AllowAlways, RejectOnce, RejectAlways }

/// lilad → 驱动。
pub enum Command {
    Prompt { text: String, origin: Origin },   // 派活、通知、人在输入框里说的话
    Steer { text: String },                    // 插进正在进行的这一轮
    Interrupt,
    Answer { permission: String, option: String },
    Cwd(std::path::PathBuf),                   // 换到 worktree 槽
    Shutdown,
}

/// 每种运行时的能力。窗口和 lilad 按能力决定行为，而不是按名字判断。
pub struct Caps {
    pub tui: bool,              // 能切到终端
    pub steer: bool,            // 运行中能插话
    pub cwd_without_restart: bool,
    pub stream: bool,           // 有逐字增量
    pub limits: bool,           // 能直接读额度
}
```

**派活时的行为**由能力决定：成员空闲时发 `Prompt`；正在工作时，能插话就发 `Steer`，不能就留在 outbox 里，等这一轮结束再发。这和现在通知（notes）的处理方式一样。

---

## 6. 驱动接口（lilad 里）

```rust
pub trait Driver {
    fn caps(&self) -> Caps;
    /// hc 里要跑的程序：claude 的 argv，或 `lila host <runtime> …`。
    fn launch(&self, member: &Member, session: &Session, cwd: &Path) -> Launch;
    /// 把命令交给成员：Claude 是 hc 的 send、interrupt 或 hook 的答复；host 是 socket。
    fn send(&mut self, member: &str, cmd: Command) -> Result<()>;
}
```

事件统一进入 lilad 的输入通道（`server.rs` 里的 `Input::Agent { project, member, event }`），和 hc 的事件、socket 请求排在同一个循环里处理。所以 lilad 仍然是单线程写入。

运行时自己的代码按目录分开：

```
crates/lila-agent/
  src/model.rs        事件、命令、能力（§5）
  src/claude/         启动参数（从 lila_core::runtime 移过来）、会话记录解析、hook 载荷解析
  src/codex/          app-server 客户端：按用到的那部分手写 serde 类型，忽略不认识的字段
  src/omp/            ACP 客户端（agent-client-protocol crate）
  src/host.rs         `lila host` 的主循环：起子进程、转换事件、发 OSC 7501、连 lilad 的 socket
```

- **异步代码只放在 host 进程里**（ACP crate 是异步的）。lilad 保持现在的同步线程模型。
- **Codex 协议的版本校验：** 有一个测试会用 `codex app-server generate-json-schema` 生成本机版本的协议定义，检查 Lila 用到的方法都还在。在设置页显示支持的 Codex 版本范围。

---

## 7. 三个驱动

### 7.1 ClaudeDriver：TUI 照跑，从旁边拿事件

- **启动：** 现在的参数不变，加上 `--settings <$LILA_HOME/run/<project>.<member>.json>`，里面配置 hooks，或者用 `--plugin-dir`（原型 S1 决定）。hook 的命令都是 `lila hook`：它从 stdin 读事件的 JSON，经 socket 交给 lilad，再把答复写回 stdout。
- **对话条目：** 读会话记录文件（lilad 已经知道路径），从上次读到的位置继续读。只认 `user`、`assistant`、`tool_use`、`tool_result`、`thinking`，不认识的行跳过。会话记录用 `transcript_path`（hook 的输入里就有）对上。
- **轮次：** `UserPromptSubmit` 是一轮开始，`Stop` 是正常结束，`StopFailure` 是 API 错误或额度用完。
- **状态：** 仍然用 hc 的 OSC 7501（不变）。hooks 用来补准确性：`Notification`（`permission_prompt`）表示卡在权限确认。
- **权限：** `PermissionRequest` hook 会**一直等到** lilad 给出答复。lilad 先按角色策略自动答；剩下的出卡片。人在超时前答了，就返回 allow 或 deny。超时或窗口没开，就返回「不决定」，让 TUI 自己问，卡片上提示切到 TUI 去处理。超时时间取 hook 的配置，比如 590 秒。
- **派活和插话：** 用 hc 的 send（不变）。Claude 工作时打进去的字会排队，所以 `Steer` 等于工作时直接 send。
- **逐字显示：** 默认按条更新（会话记录是按条写的）。原型 S1 会验证 `MessageDisplay` hook 能不能拿到正在显示的文字，能的话就用来做逐字显示。

### 7.2 HostDriver + Codex（`lila host codex`）

- **启动：** hc 里跑 `lila host codex --model … --effort … --sandbox …`。host 起子进程 `codex app-server`，先 `initialize`，再用 `account/read` 查登录状态，没登录就报 `Status::Error`，配上「登录」卡片。
- **会话：** 新建用 `thread/start {cwd, model, developerInstructions: <Lila 的提示词>, approvalPolicy, sandbox}`；续接用 `thread/resume {threadId}`。`threadId` 就是会话 id，不再去猜 rollout，`watch::find_sessions` 对 host 成员不再需要。
- **命令对应：**
  - `Prompt` → `turn/start {threadId, input, effort, cwd}`
  - `Steer` → `turn/steer`
  - `Interrupt` → `turn/interrupt`
  - `Cwd` → 下一次 `turn/start` 带上新的 `cwd`，不重启进程
  - `Answer` → 回复对应的服务端审批请求
- **事件对应：**
  - `turn/started` / `turn/completed` → `TurnStarted` / `TurnEnded`
  - `item/*` → `Item`，增量 → `Delta`
  - 审批请求 → `Permission`
  - `thread/tokenUsage/updated` → `Usage`
  - `account/rateLimits/updated` → `Limits`，替换现在 `watch_quota` 读 rollout 的做法
- **状态：** host 根据轮次和待处理的审批，算出 idle、working、blocked、error，用 OSC 7501 报给 hc（`app = "lila-codex"`）。

### 7.3 HostDriver + omp（`lila host omp`）

- **启动：** 子进程 `omp acp`，用官方 crate `agent-client-protocol` 作为客户端：`initialize`，再 `session/new {cwd, mcpServers: []}` 或 `session/load`。模型通过 `--model` 或 `session/set_config_option` 设置。系统提示词的注入方式在原型 S3 里确认；实在不行，就把提示词作为第一条消息发出去。
- **命令对应：**
  - `Prompt` → `session/prompt`。omp 不能插话，所以运行中收到的 `Steer` 先在 host 里排队，这一轮结束再发。
  - `Interrupt` → `session/cancel`
  - `Answer` → `request_permission` 的结果
  - `Cwd` → 开一个新会话
- **事件对应：**
  - `agent_message_chunk` 和 `agent_thought_chunk` → `Delta`，一段完整后合成 `Item`
  - `tool_call` 和 `tool_call_update` → `Item::ToolCall`；ACP 的 `kind` 直接对应 `ToolKind`
  - `plan` → `Item::Plan`
  - `usage_update` → `Usage`
- **账号：** omp 用的是 API key 还是订阅，要等「是否放开 API key」的决定。放开的话，key 存在钥匙串里，用 omp 的 `--profile` 按账号隔离。

---

## 8. lilad 的改动

| 地方 | 改动 |
|---|---|
| `Live` | 增加驱动类型、能力，以及 host 的 socket 连接；`session` 可以直接来自 `AgentEvent::Session` |
| `Terminals::send` 的调用点 | 改成 `Driver::send(Command::Prompt …)`；Claude 内部仍然用 hc 的 send |
| `pump` | 「空闲才派活」不变。新增：工作时有插话能力，就把通知作为 `Steer` 发出去 |
| 状态 | hc 的 OSC 7501 仍然是唯一来源，host 也走这条路。`Notice` 和 `Error` 进收件箱 |
| worktree 槽 | 有 `cwd_without_restart` 能力的成员换槽时发 `Cwd`，不重启；`switch_plan` 根据能力判断 |
| 额度 | Codex 改用 `Limits` 事件；Claude 增加 `TurnEnd::Quota`；`codex_paused` 改成按运行时记录 |
| 用量 | 有 `Usage` 事件的运行时直接累计；Claude 继续读会话记录文件 |
| 收件箱 | 新增 `permission` 类型（选项来自 `PermOption`），答复之后发 `Command::Answer` |
| 角色配置 | 增加 `[permissions] allow = […]`（运行时无关的自动允许规则）。Claude 现有的 `tools.allow` 继续作为 CLI 参数 |

---

## 9. 存储与 socket

**表**

```sql
CREATE TABLE conv_items (
  project TEXT NOT NULL, member TEXT NOT NULL, session TEXT,
  seq INTEGER NOT NULL,            -- 每个成员内递增；更新条目时重新分配 seq
  item TEXT NOT NULL, turn TEXT, kind TEXT NOT NULL, state TEXT NOT NULL,
  body TEXT NOT NULL,              -- ItemKind 的 JSON
  at_ms INTEGER NOT NULL,
  PRIMARY KEY (project, member, item)
);
CREATE INDEX conv_items_seq ON conv_items(project, member, seq);
```

保留时间跟着会话走：会话被清掉时一起删，或者超过 30 天删除。

**socket 方法**
- `conversation.read {member, after?, limit?}` → 条目、进行中的条目、待处理的权限请求、当前状态、能力
- `conversation.send {member, text}` → 记成 `UserMessage{origin: Person}`，再根据状态和能力决定发 `Prompt` 还是 `Steer`
- `conversation.interrupt {member}`
- 订阅通道（现有的 `subscribe`）新增两种推送：`{"event":"conversation","project","member","seq"}`，以及节流到约每秒 30 次的 `{"event":"delta","member","item","text"}`

---

## 10. 窗口（lila-app）

- **中间区域：** 在侧栏选中成员后，默认显示**对话视图**。标题行是成员名和状态标签；只有能切 TUI 的运行时（Claude）才在右边显示 `GUI | TUI` 两段式切换。
- **对话视图从上到下：**
  - Lila 派来的任务：一行显示「T3 标题」，点开看全文。人说的话，按普通消息显示。
  - 成员的回复：markdown（`TextView`）。思考过程默认折叠，用次要颜色。
  - 工具调用：按 `ToolKind` 显示图标，加标题和状态。点开看命令输出，或者看 diff。
  - 计划：清单。
  - 轮次之间的分隔：显示耗时和 token。
  - 待处理的权限请求：贴在最底部，和收件箱卡片用同一个组件。
- **底部输入框：** ⌘↩ 发送。工作中能插话就直接发，否则显示为排队中。
- **请求面板的「看现场」：** 打开成员的对话视图，并定位到这个任务所在的那一轮。
- **原则：** 不写说明文字（DESIGN.md）；每个成员的终端视图只在 TUI 模式下创建。

---

## 11. 落地顺序

**P0 原型**（每项是一个用完就扔的小实验，结果记进本文）
- **S1 Claude：**
  - 用 `--settings` 还是 `--plugin-dir` 注入只在本会话生效的 hooks；
  - `PermissionRequest` 能不能停下来等，返回的决定是否生效；
  - `Stop`、`StopFailure`、`Notification` 的载荷是什么样；
  - `MessageDisplay` 能不能拿到文字；
  - 会话记录文件的写入粒度。
- **S2 Codex：**
  - 用 ChatGPT 订阅登录的情况下，`codex app-server` 跑通 `thread/start`（含 developerInstructions）、`turn/start`、`turn/steer`；
  - 执行 `lila …` 命令时的审批流程；
  - `account/rateLimits/read`。
- **S3 omp：**
  - 用 `agent-client-protocol` crate 跑通 `omp acp` 的会话、提示、权限；
  - 系统提示词能否注入。
- **S4 host：** `lila host` 发出的 OSC 7501 状态能出现在 `hc get` 里。

**P1 事件模型、存储、对话视图，先接 Claude**（只读会话记录，加输入框）和 TUI/GUI 切换。做完后，现有成员马上就能用 GUI。

**P2 `lila host` 和 Codex 驱动：** Codex 成员从 TUI 改到 app-server；换 worktree 槽不再重启；额度和会话 id 直接从协议拿。

**P3 Claude 的 hooks：** 权限卡片、准确的轮次结束、额度用完时停下来。

**P4 omp 驱动：** 以及 API key 和钥匙串，取决于 §13 的决定。

**P5（可选）把 Lila 的协议做成 MCP 工具：** Claude（`--mcp-config`）、Codex（配置里的 MCP）、omp（`session/new` 的 `mcpServers`）都支持 MCP。这样成员不用再通过 shell 调 `lila`，权限规则也更简单。

每个阶段结束都真跑一轮，按截图打磨（和 v2 一样）。

---

## 12. 风险

| 风险 | 对策 |
|---|---|
| Claude 的会话记录文件是内部格式、没有文档 | 解析时宽松处理，不认识的行跳过；按版本保存样本做测试；解析失败时提示切到 TUI |
| `codex app-server` 在命令行里标着 experimental | 写明支持的最低版本，用协议定义做校验测试，功能按能力开关 |
| Claude hook 的超时上限（默认 600 秒） | 超时就交还给 TUI 自己问；卡片保留，提示切到 TUI |
| 状态有多个来源（hc、hooks、协议事件） | 规定优先级：协议事件 > hooks > hc；只有 host 往 hc 报状态，所以对 lilad 来说只有一个来源 |
| host 崩溃 | 按「成员退出」处理（现有逻辑），任务退回队列 |
| 增量太频繁 | 只在内存里转发，按约每秒 30 次节流；不落盘 |
| 用了 omp 就需要 API key | 要等 §13 的决定；key 只放钥匙串，界面标出按量计费的成员 |

---

## 13. 需要你决定的

1. **从哪里开始：**
   - **建议 P1 先做 Claude 的 GUI。** 不需要 host 这套基础设施，就能把事件模型、存储、对话视图从头到尾跑通，现有成员马上受益。
   - **另一个选择是先做 Codex。** 它的数据最规整，用来定事件模型最顺；但第一步就要把 host 做出来。
2. **对话存在 SQLite（建议）** 还是每次从各自的记录文件现读。SQLite 的好处是三种运行时一视同仁，窗口按序号增量读，也和「lilad 是唯一写入方」一致。
3. **Codex 成员要不要保留一个隐藏的原始日志视图**，就是 host 终端里的那些日志，用来排查问题。建议放在成员菜单里，平时不显示。
4. **omp 用 API key**（DeepSeek、MiniMax 等）：要不要放开「只用订阅」的原则。

---

## 14. P0 原型记录（2026-11-09）

决定按 §13 的建议处理：P1 先做 Claude；对话存进 SQLite；host 的原始日志放在成员菜单里，平时不显示；omp 的账号交给 omp 自己管理（`~/.omp`），Lila 不保存任何 key。

**S4 host 的状态报告：✅** 在 hc 终端里，一个 shell 脚本依次写出 `ESC ]7501;state=…:app=lila-probe[:kind=…][:msg=<base64>] ESC \`，`hc get` 报出 `idle`、`working`，以及带 kind 的 `blocked`（msg 已经解码成 `waiting on Q7`），`reported` 为 true。`lila host` 只要写这几个序列就够了。

**S1 Claude 的 hooks：✅**
- `--settings <文件>` 能注入只对这一次会话生效的 hooks（不用 `--plugin-dir`）。
- `PermissionRequest` 只在需要问人的时候触发。本机默认的权限模式是 auto，执行 Bash 不会问；加上 `--permission-mode manual`（Lila 给有 allow 规则的角色就是这么启动的）才触发。hook 等了 4 秒后输出 `{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}`，命令随后执行，说明**它能停下来等**。
- 载荷：所有事件都带 `session_id`、`transcript_path`、`cwd`、`prompt_id`。`PermissionRequest` 带 `tool_name`、`tool_input`（Bash 的是 `command` 和 `description`）、`permission_suggestions`。`PostToolUse` 带 `tool_response`、`tool_use_id`、`duration_ms`。`Stop` 带 `last_assistant_message`。`SessionStart` 带 `source` 和 `model`。
- **`MessageDisplay` 带回复的文字增量：** `turn_id`、`message_id`、`index`、`final`、`delta`。所以 Claude 的 GUI 也能逐字显示，不用等整条消息写进会话记录文件。
- 顺序：SessionStart → UserPromptSubmit → PreToolUse → PermissionRequest → PostToolUse → MessageDisplay → Stop。
- 等待 hook 期间，hc 的状态仍然由 Claude 自己报告。

**S2 Codex app-server：✅（用 ChatGPT 订阅）**
- `initialize`，然后 `account/read`，返回 `{"type":"chatgpt","planType":"pro"}`。
- `thread/start`（cwd、model、`approvalPolicy: "untrusted"`、sandbox、`developerInstructions`）返回 `thread.id`。开发者指令生效：回复以 PROBE 结尾。
- 审批：线程状态出现 `activeFlags: ["waitingOnApproval"]`，服务端发来 `item/commandExecution/requestApproval`（带 `command`、`cwd`、`itemId`），回复 `{"decision":"accept"}` 后收到 `serverRequest/resolved`。
- `turn/steer`（`expectedTurnId`）生效，最终回复是 `WAITED STEERED PROBE`。
- 推送有：`thread/tokenUsage/updated`（含缓存的 token）、`account/rateLimits/updated`（`primary.usedPercent`、`resetsAt`、`windowDurationMins`）、`item/agentMessage/delta`、`item/completed`（`userMessage`、`reasoning`、`agentMessage`（`phase` 为 commentary 或 final_answer）、`commandExecution`）、`thread/status/changed`（idle 或 active）。
- **要注意的两点：**
  - 一轮还没结束时再调 `turn/start`，消息会被并进正在进行的这一轮（turn id 不变）。所以运行中一律用 `turn/steer`。
  - 成员会继承用户全局的 `~/.codex/config.toml`：14 个 MCP 服务器（其中一个一直返回 503），以及 Rex 注入的 hook（会让模型先跑 `rex session rename`）。第一轮在 120 秒内都没开始执行命令，很可能是在等 MCP 服务器启动。TUI 模式下也是这样，不是 app-server 带来的问题；以后可以考虑在 `thread/start` 的 `config` 里给成员关掉 MCP。

**S3 omp 的 ACP：✅**
- `omp --append-system-prompt "…" acp`：**全局参数要放在 `acp` 前面**，系统提示词生效（回复是 `DONE OMPPROBE`）。
- `initialize` 返回 `agentCapabilities.loadSession`、`sessionCapabilities.list/fork`；认证方式只有一种，就是 `agent`，用 `~/.omp` 里已有的凭据。
- `session/new` 返回 `sessionId` 和 `configOptions`：`mode`（default、plan）以及 `model`，可选的模型就是 omp 能用的全部模型（本机经 magpie 管理，比如 `magpie/codex/gpt-6.1-sol`、`magpie/claude/…`）。
- `session/request_permission` 带 `toolCall`（`title`、`kind: "execute"`、`rawInput.command`）和标准的四个选项；回复 `{"outcome":{"outcome":"selected","optionId":…}}`。
- 更新：`agent_message_chunk`（带 `messageId`）、`tool_call`、`tool_call_update`（从 pending 到 in_progress 再到 completed，`rawOutput.content`）、`usage_update`（`size`、`used`、`cost`）、`available_commands_update`、`session_info_update`。`session/prompt` 返回 `stopReason` 和 `usage`。

## 15. P1 记录：Claude 的对话视图（2026-11-09）

**做了什么**
- 新 crate `lila-agent`：`model.rs`（条目：用户消息（来自你或 Lila、带任务号）、成员消息、思考、工具调用（种类、标题、输出、diff）；`Caps`），`claude.rs`（会话记录解析：逐行读，工具调用先是 running，结果回来改成 done/failed 并带上输出和 `structuredPatch` 变成的 diff；跳过 `isMeta`、`isSidechain`、不认识的行；空的 thinking 不显示）。
- 存储：`conv_items(project, member, session, item, seq, at_ms, body)`，按成员递增的 seq；内容没变的条目不改（lilad 重启后重读不会产生变化，也不会让窗口白刷新）。
- lilad：`daemon/conversation.rs`。spool 那一拍（250ms）跟读运行中 Claude 成员的会话记录（从上次的偏移读，半行留到下次）；`conversation.read {member, after, session}`、`conversation.send {member, text}`（打进终端；成员卡在对话框时拒绝）；`status` 的成员带 `caps`。
- 窗口：选中成员默认显示对话视图；标题行的对话/终端两段切换（按成员记在 `app-views.json`）；输入框 ⌘↩ 发送。规范写进 DESIGN.md 的 Conversation。

**真跑**（空项目，Haiku）
- engineer 做完一个小任务：对话里是「T1 Create hello.txt」、两次 Bash 调用、markdown 的回复；点 T1 在右边推进这个任务。
- 通过 socket 发「hello.txt 里是什么？」：记成「你说的」，成员的回答跟着出现。
- 切到终端：显示 Claude Code 的 TUI 和搜索按钮；切回对话。

**发现并修好的**
- Lila 投递的任务在会话记录里被包在 `<pasted_content id="…">…</pasted_content id="…">` 里（hc 用粘贴打字），开头不是 `<lila`，认不出是 Lila 派的：解析时先剥掉粘贴外壳。
- markdown 的行内代码默认用 accent（亮蓝底）太抢眼：改成 Fill 底、正文色。

**留到 P3**：逐字显示（`MessageDisplay` hook）、权限卡片、轮次的准确结束。

## 16. P2 记录：`lila host` 和 Codex app-server（2026-11-09）

**做了什么**
- `lila_agent::codex::AppServer`：app-server 协议的纯翻译。握手（`initialize` → `initialized` → `account/read`，没登录就报错误状态）；`thread/start` 或 `thread/resume`（找不到就新开），带 `developerInstructions`、`approvalPolicy: "on-request"`、角色的 sandbox；话来了：空闲时 `turn/start`，运行中（包括 `turn/start` 刚发出、还没开始时）先排队再 `turn/steer`；审批请求变成 `PermissionRequest`（允许 / 本会话都允许 / 拒绝），其余服务端请求直接回 -32601；条目翻译（用户消息、成员消息、计划、思考摘要、命令（剥掉 `/bin/zsh -lc '…'`）、文件修改（diff）、MCP、网页搜索）；用量、额度、错误。
- `lila host codex`（`crates/lila/src/host.rs`）：起 app-server，`host.attach` 挂到 lilad（lilad 回来时重连并重发），OSC 7501 报状态（`app=lila-codex`），终端打印日志。
- lilad（`daemon/host.rs`）：Codex 成员改由 `lila host codex --program <codex> …` 启动（`lila_core::runtime::launch` 的 Codex 分支）；`tell` 统一了「给成员的话」；host 报的线程就是会话 id（不再靠找 rollout）；条目进 `conv_items`；`lila …` 命令的审批自动允许，其余变成 `permission` 卡片，回答送回 host；额度事件更新 `codex_paused`。
- 窗口：Codex 成员默认显示对话；成员右键菜单「原始日志」；权限卡片。
- 测试：翻译器 6 个、lilad 1 个（会话、条目、权限卡片的去重、回答、撤回）；测试里 Codex 成员用替身 host（`World::attach`、`commands`）。

**真跑**（空项目，tester 用 Codex gpt-6.1-sol low）
- `lila up`：tester 的终端里是 `lila host codex`，`hc get` 报 `{"app":"lila-codex","state":"idle"}`，日志里「lilad attached」「thread …」。
- 派 T2：tester 在 worktree 2 里重启（新线程），`lila task start`、`sed` 读 README，对话视图逐条出现。
- `lilad` 重启：host 打印「lilad went away」「lilad attached」，重发的条目都没变（seq 不动）。
- 原始日志：右键菜单打开 host 的终端，标题行的切换回到对话。

**发现的**
- **Codex 偶尔停在工具结果之后**：rollout 里 09:21:10 `sed` 的结果回来后，到 09:27:00 一点动静都没有，直到 Lila 插了一句话（`conversation.send` → `turn/steer`），它才接着跑完（`lila task done`）。P0 S2 的第一轮也是这样。关掉 MCP 的 `-c mcp_servers={}` 没有效果（MCP 照样启动），而且不是每次都会停。原因不明，先在 host 里兜底：一轮在进行、没有在跑的条目、没有待答的审批、120 秒没有任何消息，就插一句「(Lila: no progress for a while. Continue the task.)」，每轮一次，并记一条 notice。
- 成员会继承用户全局的 `~/.codex/config.toml`（MCP 服务器、Rex 的 hook），这一点 TUI 时也一样。

**还没做的**：Codex 的逐字显示（`item/agentMessage/delta`）；中断按钮；`Usage` 事件还没用上（用量页仍读 rollout，app-server 也写 rollout，所以照常）。

## 17. P3 记录：Claude Code 的 hooks（2026-11-09）

**做了什么**
- 每个 Claude 成员带 `--settings <state>/prompts/<项目>/<成员>.settings.json` 启动，里面五个 hook 都跑 `lila hook <事件>`：PermissionRequest（超时 600s）、MessageDisplay、UserPromptSubmit、Stop、StopFailure。只对 Lila 起的会话生效。
- `lila hook`（`client::hook`）：把 stdin 的载荷交给 lilad 的 `hook`。权限请求：lilad 用 `ask_permission`（和 Codex 共用）出 `permission` 卡片，返回 `wait`；hook 每 0.5s 问 `hook.answer`，最多 570s。答「允许」输出 `{"decision":{"behavior":"allow"}}`，「拒绝」输出 deny 和「The person said no in Lila.」；没答、卡片被关掉、lilad 丢了这个请求：什么都不输出，Claude Code 在终端里自己问，Lila 补一张「终端在等你」。lilad 不在时 hook 立刻退出，不卡会话。
- 一张卡就够：成员已有权限卡片时，状态变成 blocked 不再出「终端在等你」；后到的权限卡片会关掉先到的终端卡片。
- MessageDisplay 的增量攒成「正在写」，窗口在对话最后显示（转圈 + Secondary），Stop、新一轮、或会话记录里出现回复时清掉。StopFailure 记成 `turn_failed` 事件。
- 窗口：对话最底部贴着这个成员待答的权限卡片；权限卡片不显示「知道了」。

**真跑**（PM 用 Haiku，`--permission-mode manual`）
- 让 PM 跑 `ls -la`：Claude Code 当作只读，直接跑了，不问。
- 跑 `touch probe-lila.txt`：出现 Q5「pm 想运行：touch probe-lila.txt」，下面是它自己写的说明；PM 的状态是 blocked。答「允许」：文件建好，PM 说 DONE。
- 当时还多出一张 Q6「终端在等你」（同一件事），按上面的规则修掉；换新 lilad 再试，只有权限卡片，贴在对话底部。
- 答「拒绝」：Claude Code 显示「Denied by PermissionRequest hook · Reason: The person said no in Lila.」，没有重试。

**还没做的**：Codex 的逐字显示；中断按钮。

## 18. P4 记录：omp 经 ACP（2026-11-09）

**做了什么**
- `Runtime::Omp`（角色 `runtime = "omp"`，`model` 是 omp 认得的 `provider/model`）。启动成 `lila host omp --program <omp> --model … --prompt-file … [--session …] [-- extra]`；host 起 `omp --append-system-prompt <提示词> [--model …] … acp`（omp 的全局参数必须在 `acp` 前面，P0 S3）。
- `lila_agent::acp::Acp`：ACP 客户端的纯翻译。收到的东西（`session/update` 的各种更新、`session/request_permission`）用官方的类型 crate `agent-client-protocol-schema`（`default-features = false`，只有 serde 类型，没有异步运行时）解析；发出的请求（`initialize`、`session/new`、`session/load`（不行就新开）、`session/prompt`、`session/cancel`）直接拼 JSON。回复按块攒，每 0.8s 显示一次正在写的样子，换了别的更新或这一轮结束时定稿；工具调用和更新、计划；编辑的 `oldText`/`newText` 用 `similar` 生成统一 diff；权限选项映射成 允许 / 总是允许 / 拒绝 / 总是拒绝；Lila 不给它文件和终端能力（`fs/*`、`terminal/*` 回 -32601）。ACP 不能插话：运行中来的话排队，这一轮结束后合成一条发出。ACP 也不回显提示词，所以发出时自己记成这一轮的第一条。
- `lila host` 的主循环改成对 `Protocol` 泛型，Codex 和 ACP 共用；OSC 7501 的 `app` 分别是 `lila-codex`、`lila-omp`。
- lilad：omp 和 Codex 一样是 host 成员（`hosted`）；能力是有对话、没终端、不能插话；会话行的 runtime 按成员写。账号：omp 的登录在 omp 自己（`~/.omp`、`--profile`），Lila 不存 omp 的账号和 key（§13 第 4 条按建议）。
- 窗口：角色的模型选择多一组「omp」（放当前的 omp 模型）；成员图标是蓝色的终端提示符。
- README：三种 runtime 和 omp 角色的例子。

**真跑**（omp 18.18.3，模型 `magpie/codex/gpt-6.1-sol`）
- `lila up`：deep 的终端里是 `lila host omp`，`hc get` 报 `{"app":"lila-omp","state":"idle"}`，日志里「lilad attached」和会话 id。
- 派 T3、T4：在 worktree 里开新会话，omp 跑 `lila task start` / `done` 前都按 ACP 请求权限，被 Lila 的规则直接允许（日志「allowed deep: lila task …」），二三十秒做完，出验收卡片。对话视图里是任务、回复、命令（第一次 omp 自己失败重试）、思考、总结。

**v3 计划之外、还没做的**
- 中断按钮（`Command::Interrupt` 三种运行时都能处理，窗口还没有按钮）。
- Codex 的逐字显示（`item/agentMessage/delta`）。
- `Usage` 事件还没进用量页（用量页仍读 Claude 和 Codex 的会话记录；omp 的用量还看不到）。
- omp 的模型列表：角色设置里只能保留当前写的模型，还不能从 omp 列出可选的（ACP 的 `configOptions` 里有）。
- 把 Lila 的协议做成 MCP 工具（P5，可选）。
- Codex 偶尔停在工具结果之后的原因（§16）。

## 19. 输入区记录：附件、`@`、`/`、会话级模型（2026-11-10）

**四个运行时的事实（都是实测或读协议，不是印象）**

- **Claude Code 的 TUI**：`hc send` 是粘贴加回车。粘贴进去的 `@/绝对路径`（带空格的用 `@"…"`）提交时会被当成文件提及：文本文件读进来，图片作为图片内容附上（真跑：问 `note.txt` 里的暗号答对了，问红色 PNG 答「Red」）；粘贴的文字不会触发 `@` 的补全浮层。粘贴进去的 `/pong hello`（自定义命令）正常执行。`/compact`、`/context` 不弹窗；`/usage` 弹交互界面（Esc 关闭）；`/clear` 会换一个新的会话 id（lilad 跟着的文件就断了）；**`/model <名>`、`/effort <级>` 在 TUI 里会写进用户的 `~/.claude/settings.json`（「saved as your default for new sessions」）**，`claude -p` 里才是只管当前会话。所以 Lila 不在 Claude 成员上提供切模型和努力程度。
- **转写文件里的斜杠命令**：`<command-name>/x</command-name>…<command-args>` 在 `user` 或 `system/local_command` 条目里（两种都见过，标签顺序也不固定），它打印的结果是 `system/local_command` 的 `<local-command-stdout>`（TUI 里带 ANSI 颜色）。窗口把前者显示成「你说的话」`/x args`，后者显示成一行安静的记号（`from: local`）。
- **Codex app-server**（0.166，`generate-json-schema`）：`turn/start` 的 `model`、`effort`「对这一轮和之后的轮次生效」；输入项有 `text`、`image`、`localImage {path}`、`skill {name, path}`、`mention {name, path}`；`thread/compact/start {threadId}`；`skills/list` 的位置：`$CWD` 往上到仓库根的 `.agents/skills`、`~/.agents/skills`、`$CODEX_HOME/skills`（自带的在 `.system`）。自定义 prompt（`~/.codex/prompts`）是 TUI 的功能，app-server 不展开。
- **omp 的 ACP**（18.18.3）：`promptCapabilities.image` 为真；`session/new` 的 `configOptions` 有 `model` 和 `thinking`（`category: thought_level`）两个 select，用 `session/set_config_option {sessionId, configId, value}` 改，回复里是新的 `configOptions`（真跑：模型和努力程度都改成了）；`available_commands_update` 声明了一百多条命令，skill 叫 `skill:名`。

**做法**：`Command::Prompt` 带 `attachments`（图片、文件、skill）；Claude 把文件接在话后面当 `@路径`，Codex 翻成 `localImage`/`mention`/`skill`（话里的 `@相对路径` 和 `$skill` 由 lilad 认出来补上），omp 图片内联（`image` 块，8 MB 以内）其余是 `resource_link`。新增 `Command::{SetModel, SetEffort, Compact}`、`AgentEvent::{Config, Commands}`；lilad 在 `Live` 里记每个成员当前会话的模型和努力程度（Claude 取自会话记录里最近一条回复的 `model`），重启后回到角色的设置。窗口从 `conversation.read` 的 `config` 读。
