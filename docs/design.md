# Lila 设计文档（草案 v0.1）

> 状态：待评审 · 2026-11-05
> 一句话：用 herdr-core 跑一支由不同模型组成的 agent 团队，你只和 Owner 对话，Owner 规划并分派任务，Lila 负责角色配置、任务流转、阻塞上报和可视化。

---

## 1. 目标与非目标

**目标**

1. **角色**：可配置的角色库（工程师、测试工程师、架构师、Owner……），每个角色绑定运行时（Claude Code / Codex）、模型、effort 和预设提示词。
2. **项目**：一个项目 = 一个工作目录 + 一个 Owner + 若干其他角色成员。
3. **任务协议**：agent 之间可以互相指派、交付、退回任务，状态可追踪。
4. **人只对接 Owner**：Owner 自动规划和分派；阻塞和需要人工决策的事由 Owner 汇总上报给你。
5. **GUI**：类似 Herdr 的桌面应用，能看到每个角色的实时终端、状态、当前任务和任务流转。

**非目标（v1 不做）**

- 不自己实现 PTY、会话保活、终端状态检测：这些交给 herdr-core。
- 不自己写 agent 循环、不直接调模型 API：用官方 CLI（`claude`、`codex`），沿用你现有的订阅和它们自带的写代码能力。
  **硬约束：只有订阅（Claude 和 ChatGPT 账号登录），没有 API key。** 所有模型调用都必须经过官方 CLI 的交互式会话，Lila 本身不持有、也不需要任何模型凭据。
- 不做多人协作和云端同步。

---

## 2. 技术栈结论

你提的 **Rust + GPUI + libghostty + herdr-core** 可行，而且正是 herdr-app 自己的技术栈（已读源码确认）：

| 组件 | herdr-app 的做法 | Lila 的做法 |
|---|---|---|
| UI | `gpui-pre =0.3.8`（社区发布的 GPUI），并 vendor 了打过补丁的版本 | 跟随同一版本并 vendor，**不要混用**不同 GPUI 发行版（类型不兼容） |
| 终端渲染 | 自带 `ghostty-sys` crate 绑定 libghostty-vt（需 Zig 0.16） | 复用 herdr-app 的 `ghostty-sys` 和终端视图代码（Apache-2.0，保留署名） |
| 终端运行时 | 通过 socket 调 herdr-core（`hc`）的 JSON-RPC | 一样 |

本机现状：`hc 0.1.18`、`herdr-app`、`claude 2.1.318`、`codex-cli 0.166.0`、`cargo 1.99.1` 已安装；**缺 Zig 0.16**（编译 libghostty 必需）。

herdr-core 已经提供了 Lila 需要的大部分底层能力：

- `terminal.run`（含模板）、`terminal.send`、`terminal.wait`、`terminal.read`、`terminal.attach`（原始流，给 libghostty 渲染）
- **OSC 7501 状态协议**：程序自报 `idle / working / done / blocked / error`，`blocked` 还带 `kind`（permission / question / auth）。本机 `hc ls` 显示 Claude Code 已经在上报（`agent: claude-code, status: working`）。
- `events.subscribe`：状态、标题、退出等事件流，带 `seq` 和断线快照。
- `terminal.label`：任意 `key=value` 标签，可按标签过滤事件 → 用来标记"这个终端属于哪个项目的哪个成员"。
- 父子关系、`<hc from=... reach=...>` 消息格式、`doc.get/set` 小文档存储。

**所以 Lila 自己只需要做三件事：角色/项目配置、任务协议（编排层）、GUI。**

---

## 3. 术语

| 术语 | 含义 |
|---|---|
| **Role 角色** | 模板：运行时 + 模型 + effort + 提示词。全局共享。 |
| **Project 项目** | 工作目录 + 团队成员列表 + 指派规则。 |
| **Member 成员** | 某项目里的一个角色实例，对应一个 hc 终端。id 如 `owner`、`engineer`、`engineer-2`。 |
| **Task 任务** | 从一个成员（或你）指派给另一个成员的工作单元。 |
| **Inbox 人工收件箱** | 需要你处理的事：Owner 的提问，或终端里的权限确认。 |
| **lilad** | Lila 的常驻守护进程，任务状态的唯一写入方。 |

---

## 4. 总体架构

```
                    ┌──────────────────────────────┐
                    │        Lila App (GPUI)       │
                    │ 侧栏 · Owner 终端 · 看板 · Inbox │
                    └───────┬──────────────┬───────┘
          任务/事件 (JSON-RPC)│              │ attach 流 + 状态事件
                    ┌───────▼──────┐  ┌────▼─────────────┐
   agent 调用         │    lilad     │  │   herdr-core     │
   `lila task ...` ──▶│ 任务状态机    │──▶│ (hc) 终端/状态    │
                    │ 派发队列      │  │ OSC 7501 / send  │
                    │ SQLite       │◀─│ events.subscribe │
                    └──────────────┘  └────┬─────────────┘
                                           │ PTY
                    ┌──────────┬───────────┼───────────┐
                    │ owner    │ architect │ engineer  │ tester
                    │ claude   │ claude    │ claude    │ codex
                    │ opus-5-5 │ opus-5-5  │ sonnet-5-5│ gpt-6.1-sol
```

要点：

1. **agent 通过 `lila` CLI 参与协议**，而不是 MCP。Claude Code 和 Codex 都能执行 shell 命令，不需要给每种 agent 单独配 MCP；和 `hc` 的用法一致；提供 `lila --skill` 输出协议说明。以后需要时再加 MCP 适配。
2. **lilad 是唯一写入方**：CLI 和 GUI 都通过 Unix socket（JSON-RPC，一行一条，和 hc 同风格）访问 lilad。GUI 关掉不影响团队运转。
3. **lilad 自己也跑在 hc 终端里**：`hc run --bg --keep --name lilad lila daemon`，由 herdr-core 负责保活和重启。
4. **任务投递走 `hc send`**：消息出现在目标 agent 的终端输入里，你在 GUI 里看得到。

---

## 5. 角色配置

### 5.1 文件格式

存放在 `~/.config/lila/roles/<id>.toml`，提示词放在同名 `.md`，方便编辑和版本管理：

```toml
# ~/.config/lila/roles/engineer.toml
name        = "工程师"
runtime     = "claude"              # claude | codex
model       = "claude-sonnet-5-5"
effort      = "high"
extra_args  = []                    # 透传给 CLI
env         = {}                    # 例：tester 需要的 PATH
can_assign  = ["tester"]            # 默认可以指派给谁（项目里可覆盖）
prompt_file = "engineer.md"
```

默认角色（M3 起五个；PM 是项目经理，每个项目有且只有一个）：

| id | 名称 | runtime | model | effort |
|---|---|---|---|---|
| `pm` | PM（项目经理） | claude | `claude-sonnet-5-5`（v2 起，工具受限，见 `architecture-v2.md`） | high |
| `designer` | 设计师 | claude | `claude-opus-5-5` | high |
| `architect` | 架构师 | claude | `claude-opus-5-5` | high |
| `engineer` | 开发工程师 | claude | `claude-sonnet-5-5` | high |
| `tester` | 测试工程师 | codex | `gpt-6.1-sol` | high |

你已有的 `~/.config/herdr-core/templates/engineer.md` 和 `tester.md` 的提示词可以直接迁过来；里面的 `hc send` 交接说明改由 Lila 自动注入（见 5.3）。

### 5.2 运行时适配（Runtime Adapter）

每种运行时把角色配置渲染成命令行。系统提示词优先走 CLI 的系统提示参数，这样上下文压缩后也不会丢：

| runtime | 生成的命令 |
|---|---|
| claude | `claude --model {model} --effort {effort} --append-system-prompt-file {rendered.md} {extra_args}` |
| codex | `codex --no-daemon -m {model} -c model_reasoning_effort="{effort}" -c developer_instructions=<TOML 字符串> {extra_args}`（M0 已验证） |

如果某个运行时不支持系统提示参数，就退回 hc 模板的方式：提示词作为第一条消息发送。

启动前，lilad 用角色的 `env`（合并到当前 PATH）把命令解析成绝对路径，找不到就直接报错（M0 发现 `hc run` 的 PATH 不一定包含 pnpm / fnm 目录）。

```rust
trait RuntimeAdapter {
    fn command(&self, role: &Role, rendered_prompt: &Path) -> Vec<String>;
    fn prompt_delivery(&self) -> PromptDelivery;   // SystemFlag | FirstMessage
    fn expects_status_reports(&self) -> bool;       // 是否会上报 OSC 7501
}
```

### 5.3 提示词渲染

最终提示词 = **角色提示词** + **Lila 自动生成的协议段**。角色提示词只写职责，不用写协议细节：

```markdown
## 你在 Lila 团队中
项目：{project.name}，工作目录：{cwd}
你的身份：{member.id}（{role.name}）
团队：owner（Owner）、architect（架构师）、tester（测试工程师）
你可以指派任务给：tester

## 协议（必须遵守）
- 收到 <lila task="T12" ...> 消息就是一个任务。开始时运行 `lila task start T12`。
- 完成：`lila task done T12 --summary "<一句话>" [--file <详细说明路径>]`
- 卡住：`lila task block T12 --reason "<缺什么>"`。不要猜。
- 需要别人做事：`lila task new --to tester --title ... --body-file ...`
- 长内容写到文件里，只传路径。
```

### 5.4 默认提示词要点

- **Owner**：把你的需求拆成任务并指派，不自己写实现代码；跟踪进度；汇总各成员的阻塞；只有需要你决策或授权时才调 `lila ask`；定期用 `lila status` 看全局。
- **架构师**：出技术方案和接口约定，写到 `docs/`；评审重大改动；不写业务实现。
- **工程师**：沿用你现有的 engineer 提示词；完成后指派给 tester 验证。
- **测试工程师**：沿用你现有的 tester 提示词；结论用 `lila task done`（PASS）或新建退回任务给 engineer（FAIL）。

---

## 6. 项目与团队

### 6.1 项目配置

`~/.config/lila/projects/<id>.toml`：

```toml
name = "shop"
cwd  = "/Users/jarax/codes/shop"

[[members]]
id   = "owner"
role = "owner"

[[members]]
id   = "architect"
role = "architect"

[[members]]
id   = "engineer"
role = "engineer"
count = 1            # 同时开几个：设为 2 会生成 engineer、engineer-2

[[members]]
id   = "tester"
role = "tester"
# 成员可以覆盖角色的设置（窗口里的「项目设置」写的就是这些）：
# runtime = "codex"; model = "gpt-6.1-sol"; effort = "medium"
# prompt_file = "prompts/tester.md"   # 相对 .lila/，代替角色的提示词

[assign]             # 覆盖角色的 can_assign；owner 默认可以指派给所有人
engineer  = ["tester", "architect"]
tester    = ["engineer"]
architect = ["engineer"]
```

### 6.2 启动团队

`lila project up shop`（或在 GUI 里点"启动"）：

1. 确保 lilad 在运行。
2. 为每个成员渲染提示词到 `~/.local/state/lila/<project>/<member>.md`。
3. 调 `terminal.run` 启动每个成员，带上：
   - `cwd = project.cwd`
   - 环境变量 `LILA_PROJECT`、`LILA_MEMBER`、`LILA_SOCKET`（`lila` CLI 靠这些知道"我是谁"）
   - 标签 `lila.project=shop`、`lila.member=engineer`
   - `--outlive-parent`，由 Lila 统一管理成员生命周期，不挂在 Owner 名下
4. 订阅 `events.subscribe {labels: ["lila.project=shop"]}`，把 hc 终端地址回填到成员记录。成员启动 20 秒内没有任何状态上报（通常是 Claude Code 的"信任此文件夹"对话框），就在 Inbox 提示你去终端里确认。Lila 不会替你点"信任"。
5. 把项目简介作为第一个任务（T1，from=human）发给 Owner。

**成员是长期在线的**：同一个会话依次接多个任务，团队成员互相"认识"。上下文膨胀的应对见 §11。

---

## 7. 任务协议

### 7.1 状态机

```
            start              done
  queued ─▶ delivered ─▶ in_progress ─────▶ done
                │            │   ▲
                │      block │   │ unblock / 回复
                │            ▼   │
                │          blocked
                └──────────────────────────▶ cancelled (由指派人或 owner 取消)
```

- `queued`：已创建，等目标成员空闲。
- `delivered`：已通过 `hc send` 投递，等对方 `lila task start`。
- 交付后不经过 `review` 状态：需要验收就新建一个任务（如 engineer → tester，`--parent T12`），用父子链表达"实现 → 测试 → 退回"的流转，状态机保持简单。

### 7.2 CLI

```
lila task new  --to <member> --title <t> [--body <b> | --body-file <f>] [--parent T12] [--priority p1]
lila task start  T12
lila task done   T12 --summary <s> [--file <path>]...
lila task block  T12 --reason <r>
lila task cancel T12 [--reason <r>]
lila task show   T12
lila task ls     [--mine] [--state blocked]
lila status                    # 项目全局：成员状态 + 进行中任务 + 阻塞
lila ask   <question> [--task T12] [--option A --option B]     # 仅 owner：向人提问
lila --skill                   # 打印协议说明给 agent 读
```

`lila` 用 `LILA_MEMBER` 识别调用者，并按 §6.1 的 `assign` 规则校验，越权指派直接报错并提示可指派的对象。

### 7.3 派发与通知（lilad）

**每个成员一个队列，同一时刻只有一个 active 任务**（可配置）。lilad 监听 hc 状态事件：

| 事件 | lilad 的动作 |
|---|---|
| 新任务入队，目标成员 `idle` / `done` 且没有 active 任务 | `hc send` 投递 → `delivered` |
| 成员从 `working` 变成 `idle`，队列不空 | 投递下一个 |
| `lila task done` | 记录交付物；`hc send` 通知指派人 `<lila done task="T12" from="engineer">summary</lila>`；释放该成员，投递下一个 |
| `lila task block` | 通知指派人；如果指派人不是 owner，同时抄送 owner |
| 成员 `idle` 了，但 active 任务还是 `delivered` / `in_progress`，超过 N 分钟没调协议 | 提醒一次："你是否完成了 T12？请调用 lila task done/block"；还不调用就标记 `stalled` 并通知 owner |
| 成员终端退出或丢失 | 任务退回 `queued`，通知 owner，GUI 标红 |

投递消息格式（沿用 hc 的标签风格，agent 已经熟悉）：

```
<lila task="T12" from="owner" parent="T7" reach="lila task done T12 / lila task block T12">
实现登录接口

详细说明见 /Users/jarax/codes/shop/.lila/tasks/T12.md
</lila>
```

### 7.4 防失控

- 每条父子链的深度上限（默认 8）、每个成员的重试上限（同一父任务下 FAIL→修复 默认 3 轮），超过就自动 `block` 给 owner。
- `hc send` 只在目标空闲时投递，不打断正在工作的 agent（hc 本身也会等人停止输入约 3 秒）。
- 用户随时可以在 GUI 里暂停整个项目：只停派发，不杀进程。

---

## 8. 阻塞与人工介入

阻塞分两类，来源不同，但都汇总到 GUI 的 **Inbox**：

| 类型 | 来源 | 例子 | 处理 |
|---|---|---|---|
| **终端阻塞** | OSC 7501 `blocked`，`kind=permission/question/auth` | Claude Code 等你批准一条命令 | Inbox 显示"测试工程师在等待权限"，点击直接打开它的终端去操作。不走 Owner，因为只有你能按这个键。 |
| **工作阻塞** | `lila task block` → 指派人 / Owner | "缺少支付网关的测试 key" | Owner 能自己解决就解决（重新分派、补充说明）；需要你的才调 `lila ask` |
| **决策请求** | Owner 调 `lila ask` | "支付用 Stripe 还是 Adyen？" | Inbox 里回答（可选项或自由文本）→ lilad 用 `<lila answer q="Q3">...</lila>` 发回给 Owner |

所以你的日常是：**在 Owner 终端里聊需求 + 处理 Inbox**。其他成员的终端可以看，但不用盯。

---

## 9. 数据存储

SQLite（`~/.local/share/lila/lila.db`，WAL 模式），只有 lilad 写入：

```sql
projects (id TEXT PK, name, cwd, config_toml, paused BOOL, created_at)
members  (project_id, id, role_id, terminal TEXT NULL,   -- hc 地址，如 t1n
          PRIMARY KEY (project_id, id))
tasks    (id TEXT PK,                    -- T12（项目内自增）
          project_id, title, body_path, state, priority,
          from_member, to_member,        -- from 可以是 'human'
          parent_id NULL, attempt INT,
          created_at, delivered_at, started_at, finished_at,
          summary NULL)
task_events (id INTEGER PK, task_id, at, actor, kind, data_json)
                                         -- created/delivered/started/blocked/done/nudged/...
artifacts (task_id, path, note)
inbox    (id TEXT PK,                    -- Q3
          project_id, kind,              -- question | terminal_blocked | stalled
          member, task_id NULL, text, options_json, answer NULL,
          created_at, resolved_at)
```

- 角色和项目的**配置**以 TOML 文件为准（可手改、可版本管理），DB 只缓存当前生效的配置。
- 任务正文写入 `<cwd>/.lila/tasks/T12.md`：agent 用普通读文件就能看，也方便 git 留档（可加进 `.gitignore`）。
- `task_events` 是 GUI "动态流"的数据源。

---

## 10. GUI

### 10.1 主界面

```
┌────────────┬─────────────────────────────────┬──────────────────────┐
│ 项目         │ Owner · claude-opus-5-5 · 工作中   │ 需要你  (2)            │
│ ▸ shop  ●2 │                                 │ ? Q3 支付用哪家？       │
│   lila     │   ← 完整可交互终端（libghostty）      │ ⚠ tester 等待权限      │
│────────────│     你在这里和 Owner 对话             ├──────────────────────┤
│ 团队         │                                 │ 任务                  │
│ ● owner    │                                 │ 排队 进行中 阻塞 完成     │
│   T1 规划   │                                 │ T14  T12   T9   T3   │
│ ● architect│                                 │      T13        T5   │
│   空闲      ├─────────────────────────────────┤                      │
│ ● engineer │ [architect] [engineer] [tester] │──────────────────────│
│   T12 登录  │   成员终端缩略图，点击放大/接管       │ 动态                  │
│ ◐ tester   │                                 │ 10:42 owner→engineer │
│   等待权限   │                                 │   T12 实现登录接口     │
└────────────┴─────────────────────────────────┴──────────────────────┘
```

- **侧栏**：项目列表（徽标 = 需要你的数量）；团队成员，每人显示状态点（working / blocked / idle / done / error，来自 7501）、当前任务，以及 7501 的 `msg`。M0 验证过：Claude Code 工作时 `msg` 就是当前动作的描述，比如 "Printing tick 1-5 with one-second pauses"。终端 `title` 事件很频繁（Codex 约每 0.1 秒一次），要节流。
- **中间**：Owner 终端占主区域；下方是其他成员的终端缩略图，点击切换成主视图，可以直接接管输入。
- **右侧**：Inbox（最上面，最重要）→ 任务看板 → 动态流。
- **任务详情抽屉**：正文、父子链、事件时间线、交付物链接、"重新指派 / 取消 / 回复"。

### 10.2 其他界面

- **角色库**：增删改角色，下拉选择 runtime / model / effort，内嵌提示词编辑器，"预览最终提示词"（包含协议段）。
- **项目设置**：工作目录、成员、指派矩阵（勾选表格）。
- **新建项目向导**：选目录 → 选角色 → 写项目简介 → 启动。

### 10.3 数据来源

| 界面元素 | 来源 |
|---|---|
| 终端画面 | hc `terminal.attach`（framed 模式）→ libghostty-vt → GPUI |
| 成员状态 | hc `events.subscribe`（按 `lila.project` 标签过滤） |
| 任务、Inbox、动态 | lilad 的 `events.subscribe`（同样风格：快照 + seq） |

---

## 11. 代码结构

```
lila-agent/
├── Cargo.toml                # workspace
├── crates/
│   ├── lila-core/            # 领域模型、状态机、指派校验、提示词渲染、RuntimeAdapter
│   ├── lila-store/           # SQLite（rusqlite）、迁移
│   ├── hc-client/            # herdr-core JSON-RPC 客户端（plain + framed）
│   ├── lila-cli/             # bin `lila`：daemon / task / project / ask / --skill
│   ├── ghostty-sys/          # 来自 herdr-app（Apache-2.0）
│   └── lila-app/             # GPUI 桌面应用
├── vendor/gpui-pre*/         # 与 herdr-app 一致的 GPUI 补丁版本
├── defaults/roles/           # 4 个默认角色 toml + md
└── docs/
```

`lila-core` 不依赖任何 IO，可以完整做单元测试（状态机、派发决策、权限校验）；lilad 的派发逻辑写成"输入事件 → 输出动作"的纯函数，hc 调用放在最外层。

---

## 12. 里程碑

| 阶段 | 内容 | 验收 |
|---|---|---|
| **M0 技术验证** ✅ 已完成，见 `spikes.md` | ① 装 Zig 0.16，从源码编译 herdr-app；② 确认 codex 是否上报 7501、`developer_instructions` 能否生效；③ 向正在工作的 Claude Code 执行 `hc send` 的实际行为；④ 最小 GPUI 窗口 + 一个 libghostty 终端 attach 到 hc | 四项都有结论，写进 `docs/spikes.md` |
| **M1 编排核心，无 GUI** ✅ 已完成，见 `m1.md` | lila-core / store / hc-client / lila CLI + lilad；默认角色；`lila project up` | 用 **herdr-app 本身**观察：你在 Owner 终端提需求 → Owner 拆任务 → engineer 实现 → tester 验证/退回 → 阻塞时 `lila inbox` 能看到 Owner 的提问 |
| **M2 GUI** ✅ 已完成第一版，见 `m2.md` | lila-app：侧栏、Owner 主终端、成员缩略图、看板、Inbox、动态流 | M1 的流程全程在 Lila 窗口里完成 |
| **M3 增强** | 角色/项目编辑器 GUI、每成员独立 git worktree、每任务新会话选项、用量统计、远程机器（hc machine） | 按需排序 |

M1 先不做 GUI 是刻意的：编排协议能不能跑通，是整个产品最大的不确定性，而 herdr-app 已经能显示终端和状态，可以白拿来做观察窗口。

---

## 13. 风险与应对

| 风险 | 应对 |
|---|---|
| `gpui-pre` 不是 Zed 官方发行，API 会变 | 锁定 `=0.3.8` 并 vendor，与 herdr-app 同步升级 |
| libghostty-vt 在 1.0 之前，构建需要 Zig | 复用 herdr-app 的 `ghostty-sys` 及其固定版本的子模块 |
| herdr-core API 是 preview 0.x | 官方承诺只增不删（有 `released.openrpc.json` 兼容性测试）；hc-client 只用必需的方法，启动时检查 `hello.features` |
| agent 忘记调用 `lila task done` | §7.3 的空闲提醒 + `stalled` 上报；协议段放在系统提示词里，上下文压缩后不丢 |
| 长期会话上下文膨胀、跑偏 | v1：任务完成后 lilad 可以给成员发 `/compact`（可配置）；M3：每个任务开新会话 |
| 多个 agent 同时改同一个仓库 | v1：只有 engineer 写代码，tester 只加测试文件（沿用你现有提示词的约束）；M3：每成员 worktree |
| agent 之间互相退回、死循环 | §7.4 的深度和重试上限 |
| 订阅用量上限：多个成员同时跑（尤其两个 Opus high）会更快触顶 | 默认只让 1 个成员同时 `working`，其他排队（项目级并发上限可调）；M0 验证触顶时 CLI 是否上报 `error` / `blocked`，lilad 据此暂停派发并在 Inbox 提示；角色可以随时降低 model 或 effort |
| 某个运行时不上报 7501 | 适配器标记 `expects_status_reports=false`，退回 hc 的 `ready: quiet` 并降级显示"未知状态" |

---

## 14. 待你决定

1. **工作区隔离**：v1 所有成员共用一个目录（简单，靠提示词约束谁能改什么），还是一开始就给每个写代码的成员开 git worktree？——建议 v1 共用，M3 再加。
2. **成员之间直接指派**：默认按指派矩阵放行（如 engineer → tester 直接交接），还是所有任务都必须经过 Owner？——建议按矩阵，Owner 能看到所有流转。
3. **项目配置放哪**：`~/.config/lila/projects/`（个人配置），还是仓库内的 `.lila/project.toml`（可以提交、团队共享）？——建议仓库内，角色仍放在用户目录。
4. **备选路线**：herdr-app 支持 JS mod 绘制面板，也能运行程序。Lila 也可以做成"`lila` CLI + herdr-app mod"，不写自己的 GUI，工作量会小很多，但界面受 mod API 限制。M1 的产出两条路线都通用，M2 之前再决定也来得及。你倾向于坚持自研 GPUI 应用吗？
5. 产品名用 **Lila** 吗？
