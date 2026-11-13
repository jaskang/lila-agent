# Lila 架构 v4：项目与会话（lilad 的 socket 接口）

> 2026-11-10 · P1a（后端）和 P1b（窗口，见 §9）；2026-11-11 · P2 后端（worktree、改动、提交、合并，见 §4、§10）；2026-11-11 · A1 后端（每轮检查点和回退，协议版本 2，见 §11）；2026-11-11 · A2 前半（卡片不过期、本会话总是允许、窗口里回答「信任此文件夹」，协议版本 3，见 §6、§12）。需求见 `product-v4.md`；引擎驱动和对话事件模型见 `architecture-v3.md`（继续有效）。
> **2026-11-13：Lila 是自己的 harness，引擎只有 omp，协议版本 4。** Claude Code、Codex 两个引擎已删（`claude.rs`、`codex.rs`、hook、信任对话框、终端视图、账号和用量）；下面凡是写到它们的地方都已改成 omp 的做法，只在谈旧数据时还提到。
> 本文和代码一致，窗口只看这份就能接。旧实现（任务、成员、请求）在标签 `v3-final`。

## 1. 模型

- **项目**：一个文件夹。注册在数据库的 `projects` 表（id、文件夹）；默认值在 `<文件夹>/.lila/project.toml`：

  ```toml
  name = "shop"            # 项目 id：1–24 个 a-z 0-9 - _，字母开头，注册后不变
  title = "商店"            # 可选，窗口显示的名字
  [defaults]
  model = ""               # 空：omp 自己的默认；或写 omp 认得的 provider/model
  permission = "full"      # read_only | confirm | full，默认 full
  permission = "full"      # read_only | confirm | full，默认 full
  ```

  v3 的文件（带 `[[members]]` 等）和选过引擎的文件（`engine`、`effort`）也能读，多余的键忽略；`engine` 不是 `omp` 的文件里的 `model` 是 Claude 或 Codex 的名字，omp 不认识，读的时候丢掉。`project.add` 在文件夹没有这个文件时写一个。lilad 是唯一的写入方：窗口改默认值用 `project.set`。
- **会话（thread）**：一次和 agent 的对话，属于一个项目。

  | 字段 | 说明 |
  |---|---|
  | `id` | `t` 加 8 位十六进制，如 `t07b6b991` |
  | `title` | 默认取第一条消息的首行（60 字内），`thread.rename` 可改 |
  | `legacy` | 只有 Claude Code 或 Codex 时代留下的会话有：`claude` 或 `codex`。它们已结束、不能 `thread.resume`，对话还能读，见下面「数据库」 |
  | `model`、`effort` | 存的值；运行中以 omp 报告的为准（见 `thread.show` 的 `model`）。`effort` 只在会话里换过才有，开会话时不传 |
  | `permission` | 权限档，见 §6 |
  | `cwd` | 引擎运行的文件夹：项目文件夹，或会话自己的 worktree（§10） |
  | `worktree` | 会话用独立 worktree（建会话时选的，之后不变）。合并之后仍为 `true`，看 `merged_ms` |
  | `base_branch`、`base_commit` | worktree 会话：建 worktree 时项目文件夹所在的分支和它的提交；合并回来就是放回这个分支。不是 worktree 的会话为 `null` |
  | `merged_ms` | worktree 的工作合并回 `base_branch` 的时间；合并后 worktree 和分支已删，会话已归档，`cwd` 这个文件夹不存在了。没合并为 `null` |
  | `merging` | 正在合并（`thread.merge` 还没回复）：这时不能发消息、续接、再合并 |
  | `changes` | 改动摘要 `{files, add, del}`：`cwd` 相对 HEAD 的改动文件数、加的行数、删的行数。`null`：还没数过，或 `cwd` 不在 git 仓库里；不是实时的，见 §5 |
  | `session` | omp 自己的会话 id，续接用（`session/load`）；host 报告 |
  | `terminal` | 当前的 hc 终端地址；结束后为空 |
  | `created_ms`、`active_ms`、`started_ms`、`ended_ms`、`archived` | 时间戳（Unix 毫秒）；`active_ms` 是最近一条消息或回复，列表按它倒序 |

- **状态**（`state`，由终端状态和待答的权限卡片算出）：

  | state | 含义 |
  |---|---|
  | `starting` | 引擎已启动，还没报过状态 |
  | `running` | 在处理一轮 |
  | `waiting` | 在等人：有权限卡片（在 `thread.read` 的 `permissions` 里），`waiting` 字段是 `card` |
  | `idle` | 在提示符前，能收消息 |
  | `ended` | 引擎进程没了。对话还在，可 `thread.resume` |

  另有 `status`：终端报的原始词（`idle`/`working`/`blocked`/`done`/`error`/`unknown`/`gone`）。

- **数据库**（`lila-store`，SQLite，`PRAGMA user_version = 8`）：`projects`、`threads`、`conv_items`（按会话递增的 `seq`）、`outbox`（引擎还不能收时排队的话；`hold = 1` 的是「随下一条消息一起发的备注」，自己不会被发出）、`thread_events`（lilad 自己做过的事）、`checkpoints`（§11：`thread, n, kind(turn|restore), at_ms, sha, tree, head, branch, text, item`，主键 `(thread, n)`）、`thread_rules`（§6「本会话总是允许」：`id, thread, kind(command|edit|tool), key, created_ms`，`(thread, kind, key)` 唯一；会话删除时一起删）。**v3 的库不迁移**：打开 v3 的库（没有版本号、有 `tasks` 表）时，文件改名为 `lila.db.v3.bak`，新库从空开始。**v4 的库平滑升级**：版本 5 给 `threads` 加了 `worktree`（默认 0）、`base_branch`、`base_commit`、`merged_ms` 四列；`migrate` 每次打开都按 `pragma_table_info` 查缺哪列就 `ALTER TABLE ADD COLUMN`，已有会话变成「不是 worktree」，再打开不变。以后只加列的改动照这样加到 `ADDED_COLUMNS`（现在是 `(表, 列, 定义)`，版本 6 加了 `outbox.hold`；新表用 `CREATE TABLE IF NOT EXISTS`，版本 6 加了 `checkpoints`，版本 7 加了 `thread_rules`：旧库打开后只多一张空表，原有的都照旧，有测试）。旧库打开后原有的会话、排队的话都照旧。
  **版本 8（只剩 omp）**：`threads.engine` 列删掉，加了 `legacy` 列。`migrate` 在库里还有 `engine` 列时：非 `omp` 的会话 `legacy` 记下旧引擎名，它们的 `terminal` 清空、`ended_ms` 补上（已结束，对话和检查点都留着）；然后 `DROP COLUMN engine`。窗口里它们照常列在侧栏、对话能看，输入框换成「Claude Code 会话，不能继续」，没有「继续」按钮；lilad 收到它们那个 hc 终端的事件时把终端杀掉（旧的 Claude Code、Codex 进程不会留在 hc 里）；`thread.resume` 报错。它们对话里的 `from: summary｜local｜interrupted` 条目窗口不再认，画成普通的话。
- `changes` 摘要**不进数据库**：lilad 在内存里记，启动时给每个没归档的会话数一遍（后台一个一个数）。

## 2. 进程与终端

每个会话的 omp 跑在**自己的 hc 终端**里，终端里运行的是 `lila host omp`：lilad 重启时会话不断。lilad 本身也在一个叫 `lilad` 的 hc 终端里（`lila up` 起的）。**herdr-core（`hc`）现在仍然是运行时必需的**：会话的进程托管、lilad 重启后找回会话、状态事件都靠它；换成 lilad 自己的子进程要同时改这几处（见 HANDOFF 2026-11-13 一节）。

- 终端名 `lila-<会话 id>`，标签 `lila.project`、`lila.thread`，环境变量 `LILA_PROJECT`、`LILA_THREAD`、`LILA_ROOT`（文件夹）、`LILA_HOME`（若设了）。
- 状态由 host 用 OSC 7501 报给 hc，lilad 订阅 hc 的事件。lilad 重启后：数据库里有终端的会话先当作活着，hc 的快照到了，快照里没有的标成 `ended`；快照里有的按标签对回会话。
- 启动命令：`lila host omp --program <omp> [--model M] --session-dir <目录>`（`lila_core::runtime::launch`，lilad 在 `host omp` 后面插入 `--program`）。host 起 `omp --mode rpc-ui --approval-mode always-ask [--model M] --session-dir <目录> [--continue]`，没有系统提示词。
  - `--mode rpc-ui` 而不是 `rpc`：只有它才给 omp 的 `ask` 工具（agent 向人提问）；`rpc` 里没有这个工具。除此之外两者对 Lila 没有区别（权限对话两种都发）。
  - `--approval-mode always-ask`（命令行覆盖配置）：读类工具 omp 自己放行，**写和执行类每次都问**，问的对话交给 lilad 判（§6）。用户在 omp 配置里写的 `tools.approval.<工具>: allow` 仍然生效（命令行覆盖不了它），那样的工具不会问 Lila。
  - **会话目录**：每个会话一个 `<数据目录>/omp/<会话 id>/`（`Paths::omp_sessions`；设了 `LILA_HOME` 就在它的 `data/` 下），omp 的会话文件（`*.jsonl`）都在里面。目录里已经有会话文件就加 `--continue`（omp 接着最后一个会话，恢复它存的模型）；没有就开新的。恢复时 omp 不重放历史（只有 `get_messages` 才给），host 也不去取，所以 lilad 存着的对话不会重复。条目 id 带这一次 host 运行的前缀（`<运行>-m<消息>-<块>`、`<运行>-<工具调用 id>`、`<运行>-t<轮>-said`），恢复后不会盖掉上次的条目。
  - **旧的 ACP 会话**（库里有 `session` 却没有会话目录）：恢复时目录是空的，omp 开一个新会话，对话在窗口里留着，但 omp 不记得之前说过什么（`thread_events` 里 `resumed` 写「no omp session in …」）。没有迁移。
  - omp 的 stderr 写到 `<state>/hosts/<会话 id>.log`；omp 意外退出时 host 把退出码和日志最后几行放进对话的 `error` 条目和 `notice`，然后自己退出（会话变 `ended`）。
- 对话条目：host 通过 `host.attach` 连上 lilad 推事件（`lila_agent::AgentEvent`），lilad 存数据库；host 自己也留一份，lilad 回来时重放。格式见 `lila_agent::Item`（`architecture-v3.md` §5）。
- 只有 `omp` 要在 PATH 上；lilad 给它的 PATH 是 lila 所在文件夹加启动 lilad 时的 PATH。omp 的模型和登录是它自己的配置（`~/.omp`），Lila 不管。


## 3. 协议

unix socket：`$LILA_HOME/state/lilad.sock`（没设 `LILA_HOME` 时 `~/.local/state/lila/lilad.sock`；路径太长时落在临时目录，`lila_core`/`Paths::socket()` 给准）。每行一个 JSON。

请求：`{"method": "thread.list", "params": {...}}`；回复：`{"result": ...}` 或 `{"error": "文字"}`。一条连接可以连着发多个请求，回复按请求的顺序一行一行回来。窗口用 `lila::client::request(paths, method, params, autostart)`；`autostart` 为真时 lilad 没跑就先在 hc 里启动它。

**请求 id**（可选）：请求里可以带 `"id"`（任意 JSON 字符串或数字），lilad 原样放进这条请求的回复：`{"id": "a-1", "result": ...}`、`{"id": 7, "error": "…"}`，`subscribe` 和 `host.attach` 的第一条回复也带。不带 id 的请求，回复和以前一模一样。`id` 是别的类型（对象、数组、布尔）回 `{"error": "bad request: id must be a string or a number"}`；`null` 当作没带；整行不是 JSON 时没有 id 可回。这只是给以后的远程客户端留的：lilad 不做多路复用，回复仍然按顺序，不会乱序。`lila::client` 现在不发 id。

**协议版本 2**（A1）：加了 `thread.checkpoints`、`thread.restore`，`thread.read` 多了 `checkpoints`，会话多了 `restoring`，条目多了 `from: "restored"`。窗口用这些就要求 `protocol >= 2`。

**协议版本 3**（A2）：会话多了 `waiting_for`、`trust_folder`；`thread.show`、`thread.read` 多了 `rules`；加了 `thread.trust`、`thread.rule.remove`；卡片选项多了 Lila 自己的 `always`（见 §6）。窗口都是读这些字段：旧 lilad 没有它们，窗口照旧画「在终端里等你」、没有「总是允许」、权限菜单里没有规则，什么都不坏（所以窗口不查协议号，只是侧栏会照常提示 lilad 版本不符）。

**协议版本 5**（omp 的 RPC 模式）：`thread.read` 多了 `questions`（agent 的提问卡片，见 §6 末尾「提问」）；`thread.answer` 能答提问（`value`｜`yes`｜`cancel`）；新方法 `models.list`；对话里有 `kind: "error"` 的条目；工具调用的 `name` 是 omp 的工具名（`bash`、`edit`、`mcp__server__tool`），权限卡片的 `request` 是 omp 的对话 id（不再是 `acp-N`）。

**协议版本 4**（只剩 omp）：会话没有了 `engine`、`caps`、`waiting_for`、`trust_folder`，多了 `legacy`；`waiting` 只有 `card`；`thread.new`、`thread.resume`、`project.set` 不再收 `engine`、`effort`（项目的 `defaults` 只有 `model`、`permission`）；`thread.answer` 没有 `dismiss`；`thread.read` 没有 `writing`，`thread.commands` 的条目没有 `action`；`status` 没有 `magpie`；删了 `thread.trust`、`thread.run`、`hook`、`hook.answer`。旧窗口读不到 `engine` 会画不出标题，所以协议不同时要重启 lilad（窗口会提示）。

**版本握手**：`lila::PROTOCOL`（`crates/lila/src/lib.rs`）是 socket 协议的版本号，方法、参数、回复有不兼容的改动就加一。`ping` 和 `status` 都带 `version`（`CARGO_PKG_VERSION`）和 `protocol`。窗口和命令行用 `lila::client::daemon_version` 比较：协议一样、不一样（带两边的数字；还没有这套握手的旧 lilad 不报协议，算不一样）、没有运行、连不上。重装或重新编译后，旧 lilad 会一直跑着（会话在各自的终端里，重启 lilad 是安全的），所以窗口和 `lila daemon status` 会提示，`lila daemon restart` 或窗口里的按钮重启它。

`params` 里 `project` 可以省略，改传 `cwd`（任意文件夹，lilad 往上找已注册的项目）。

## 4. 方法

约定：`?` 表示可选；时间都是 Unix 毫秒；错误是 `{"error": "…"}`。

### 项目

| 方法 | 参数 | 返回 |
|---|---|---|
| `project.list` | — | 项目数组（见下） |
| `project.show` | `project`｜`cwd` | 一个项目 |
| `project.add` | `cwd`, `name?`, `title?` | 项目，另加 `created_config`（这次写了 project.toml）。已有就返回已有的。会自动启动 lilad 的是客户端的 `autostart` |
| `project.remove` | `project` | `{project, removed, stopped: [会话 id]}`。结束它所有会话的引擎，删库里的记录，文件夹不动 |
| `project.reload` | `project` | 项目。手工改了 project.toml 之后 |
| `project.set` | `project`, `title?`, `defaults?: {model?, permission?}` | 项目。写 project.toml；空字符串的 `model` 表示清掉 |

项目：

```json
{"id": "shop", "cwd": "/abs/path", "title": "商店"|null, "label": "商店", "error": null, "repo": true,
 "defaults": {"model": "", "permission": "full"},
 "threads": 3, "states": {"idle": 2, "running": 1}, "waiting": 0}
```

`repo`：文件夹是 git 仓库的根目录（`worktree::is_repo_root`），即新会话可以选独立 worktree；project 加载或 `project.reload` 时问一次、缓存（`project.list` 在主循环里，不起 git），之后才 `git init` 的要 reload。`error`：project.toml 读不了（文件夹没了、文件坏了）的原因；这时项目还在列表里，用默认值。`threads` 不含归档的；`waiting` 是待答的权限卡片数。

### 会话

| 方法 | 参数 | 返回 |
|---|---|---|
| `thread.new` | `project`｜`cwd`, `text?`, `attachments?`, `title?`, `model?`, `permission?`, `worktree?: bool` | 会话。按项目默认值建，参数覆盖；启动引擎；第一条消息排队，引擎能收了就发。别的引擎不继承项目默认的 model/effort。引擎的程序不在 PATH 上、文件夹没了就报错，什么都不留。`worktree: true`：先给会话建独立 worktree（§10），引擎在里面启动，`cwd` 指向它；回复要等 worktree 建好（大仓库要几秒，lilad 的主循环不被占住）。项目文件夹不是 git 仓库的根目录、在 detached HEAD、还没有提交时报错，什么都不留（引擎起不来时 worktree 和分支也会被清掉） |
| `thread.list` | `project?`｜`cwd?`, `archived?` | 会话数组，`active_ms` 倒序；不给项目就是所有项目的；`archived: true` 含归档的 |
| `thread.show` | `thread` | 会话 |
| `thread.read` | `thread`, `after?`, `limit?`(400) | 见下 |
| `thread.send` | `thread`, `text`, `attachments?` | `{sent: true, thread}`。`attachments` 是绝对路径数组，在对话里显示成卡片。omp 没有自己的 `@`，话里提到的文件（`@相对路径`）由 lilad 当成附件一起发；omp 一次只收一轮，话在它空闲时才发出（先排在 `outbox`）；有卡片待答时拒绝  |
| `thread.interrupt` | `thread` | `{interrupted: true}`。只在 `running` 时，给 host 发 `abort`（omp 的 RPC 命令），开着的权限和提问对话一起收回 |
| `thread.stop` | `thread` | 会话。杀终端，状态变 `ended`，对话保留 |
| `thread.resume` | `thread`, `text?`, `attachments?`, `model?`, `permission?` | 会话。重新启动 omp，接着会话目录里最后一个 omp 会话（`--continue`；目录里没有会话文件就开新的，见 §2）。`legacy` 的会话报错。有话就排进 outbox |
| `thread.archive` / `thread.unarchive` | `thread` | 会话。归档会先结束引擎；worktree 会话归档时 worktree 和分支**保留**，取消归档后可以继续（`thread.resume`）。现在没有单独删除 worktree 的入口（合并是唯一会删的地方） |
| `thread.rename` | `thread`, `title` | 会话 |
| `thread.permission` | `thread`, `permission` | 会话。立即生效，不用重启，见 §6 |
| `thread.answer` | `thread`, `card`｜`permission`, `option`｜`allow`｜`value`｜`yes`｜`cancel` | `{thread, card, answered: true}`。回答提问卡片（`card` 是 `q…`）：`value`（选择题的选项原文或从 1 起的序号，或问答题的文字）、`yes`（确认题，也可以 `value` 写 yes/no）、`cancel: true`（不回答，agent 继续）；选择题答了没提供的选项、确认题答不是是否报错。回答权限卡片：`option` 是卡片选项的 `id` 或 `label`（`always` 见 §6：先把卡片说的规则加给会话，再用引擎的普通「允许」回答；这一刻不再提供就报错）；`allow: true/false` 选第一个允许或拒绝的选项。卡片已经没了（别处答过、omp 收回了）报错 |
| `thread.rule.remove` | `thread`, `rule` | `{thread, rule, removed: true, rules}`。收回一条「总是允许」（`rule` 是 `rules` 里的 `id`），它允许的以后又会问；没有这条规则报错 |
| `thread.model` | `thread`, `model?`, `effort?` | `{set: true}`。运行中立即换（给 host 发 `set_model`｜`set_thinking_level`，omp 只能换它列出的）；也存起来，下次启动用 |
| `models.list` | `refresh?` | `{models: [{id: "provider/model", name}]}`。lilad 起一个短命的 `omp --mode rpc --no-session --no-extensions …`，问它 `get_available_models`，在后台线程里跑，结果记 10 分钟（`refresh: true` 重问）；供新会话的输入框和工作区设置在会话有自己的 omp 之前选模型。会话的 omp 跑起来后 `config.models` 用它自己报的列表，没报之前用这份。omp 起不来或 30 秒没答就报错 |
| `thread.commands` | `thread` | `{commands: [{name, description, kind: builtin｜skill, hint, insert}]}`，`/` 菜单的内容：omp 为这个会话声明的命令和 skill（`available_commands_update`），没有声明就是空的 |
| `thread.files` | `thread`｜`project`｜`cwd` | `{files: [相对路径], cwd}`，`@` 菜单的内容（git 跟踪的和没被忽略的新文件）。给 `thread` 是会话的文件夹；给 `project`（或 `cwd`）是项目的文件夹，用于还没创建会话的新会话输入框 |
| `thread.changes` | `thread` | 会话工作目录相对 HEAD 的改动，见下 |
| `thread.diff` | `thread`, `path?` | `{diffs: [FileDiff…], truncated}`，见下 |
| `thread.file` | `thread`, `path` | `{path, text?, binary?, too_large?, size}`，见下 |
| `thread.commit` | `thread` | `{sent: true, thread}`。让 agent 提交，见下 |
| `thread.merge` | `thread` | 把 worktree 会话的工作合并回 `base_branch`，见下 |
| `thread.checkpoints` | `thread` | 每轮开始前的文件夹快照，新到旧，见 §11 |
| `thread.restore` | `thread`, `checkpoint`, `dry_run?` | 把文件夹里的文件放回到某个检查点，见 §11 |
| `thread.log` | `thread`, `limit?`(50) | `[{at_ms, actor, kind, detail}]`：lilad 自己做过的事，见下 |

会话：

```json
{"id": "t07b6b991", "project": "shop", "title": "…", "legacy": null, "model": "openai/gpt-6", "effort": null,
 "permission": "full", "cwd": "/abs/path", "session": "…"|null,
 "state": "idle", "status": "idle", "waiting": null, "cards": 0, "msg": null,
  "terminal": "t78"|null,
 "created_ms": 0, "active_ms": 0, "started_ms": 0, "ended_ms": null, "archived": false,
 "worktree": false, "base_branch": null, "base_commit": null, "merged_ms": null, "merging": false, "restoring": false,
  "changes": {"files": 3, "add": 40, "del": 2}|null}
```

`model`/`effort` 是引擎正在用的（取自 omp 的报告），没有就是存的。`cards` 是待答卡片数。`msg` 是终端报的说明（比如 blocked 的原因）。

`thread.read`：

```json
{"thread": {会话}, "seq": 12, "items": [...], "permissions": [卡片], "checkpoints": [检查点], "rules": [规则],
 "config": {"model": "…", "effort": "…", "default_model": "…", "default_effort": "…", "switch": true, "models": [{id,name}], "efforts": [{id,name}]}}
```

- `items`：`seq` 大于 `after` 的条目，旧到新；超过 `limit` 取最新的。同一个 `id` 的条目更新时整条覆盖并拿到新的 `seq`，所以客户端按 `id` 替换。`seq` 是返回的最大序号，下次当 `after`。条目格式是 `lila_agent::Item`：`{id, turn?, at_ms, state: running｜done｜failed, kind: user_message｜agent_message｜thought｜tool_call, …}`，用户消息带 `from: person｜lila｜restored`，工具调用带 `tool`（`execute/read/edit/search/fetch/delete/…`）、`name`、`title`、`output?`、`diffs?`。
- `checkpoints`：和 `thread.checkpoints` 的 `checkpoints` 一样（§11）。窗口用其中的 `item` 把「回到这里」挂到对应的用户消息上；因为 `item` 可能比消息晚才有，窗口每次读都按这份列表重新挂，不要只看新来的条目。
- `from: "restored"` 的条目：回退留下的安静标记，`{id: "restored-<检查点>-<毫秒>", kind: "user_message", from: "restored", checkpoint: 3, undo: 7, text: "…", at_ms}`；窗口自己画「已回到这里」，`undo` 是回退前状态的检查点（可以再回退一次来撤销）。
- `rules`（`thread.show` 里同样有）：会话的「总是允许」规则，旧到新：`{id, kind: command｜edit｜tool, key, label, at_ms}`，`label` 是给人看的（`cargo test`、`改动此文件夹里的文件`），见 §6。
- `permissions`：待答的卡片：

  ```json
  {"id": "p3", "thread": "t…", "request": "7a1…", "tool": "execute", "name": "bash", "title": "git push origin main",
   "detail": "", "command": "git push origin main"|null, "paths": [], "why": "全自动档仍需确认：推送或删除 git 分支",
   "danger": "git_push"|null, "options": [{"id": "accept", "label": "允许", "kind": "allow"}, …], "at_ms": 0}
  ```

  `danger`：`outside_workspace`｜`git_push`｜`external_action`｜`computer_use`，档位本身要问（`confirm`）时为空。选项的 `kind`：`allow`｜`allow_always`｜`reject`｜`reject_always`。**`allow_always` 只有 Lila 自己的那一个**（id `always`，`label` 写明会允许什么，如「本会话总是允许 cargo test」），见 §6；引擎自己的「总是」选项不出现在卡片上。
- `questions`：待答的提问卡片：`{id: "q2", thread, ask: "question", question: <omp 的对话 id>, kind: choice｜confirm｜text, title, detail, options: [文字], prefill, secret, at_ms}`。窗口把它和 `permissions` 放在一起画（有 `ask: "question"` 的是提问）。
- `config.switch`：窗口能不能让用户换模型和努力程度（见 `thread.model`）。

**改动、diff、文件、提交、合并**（`thread.changes`、`diff`、`file`、`commit`、`merge`；P2）：

- 这些方法里凡是跑 git 的（`thread.new` 带 `worktree`、`changes`、`diff`、`commit`、`merge`）都**放在 lilad 主循环外**，在各自的线程里跑，跑完回复；所以回复可能慢一点（合并会跑用户的 commit hook，大仓库建 worktree 是一次检出），但 lilad 不会卡住。客户端请求的读超时是 120 秒（`thread.merge` 和 `thread.new` 是 900 秒）。`thread.file` 只读一个小文件，直接在主循环里答。
- 用的是 git 命令行，走用户的 git 配置，不改用户的 index（`git status` 用 `GIT_OPTIONAL_LOCKS=0`；找改名用一份临时 index）。

`thread.changes {thread}` → 会话 `cwd` 相对 HEAD 的全部改动（已暂存、未暂存、未跟踪的都算；`.gitignore` 忽略的不算）：

```json
{"repo": true, "branch": "lila/t07b6b991"|null, "worktree": true, "base_branch": "main", "ahead": 2,
 "commits": [{"sha": "9d81c46", "subject": "Add b"}],
 "files": [{"path": "src/a.rs", "status": "modified", "add": 3, "del": 1},
           {"path": "new name.txt", "status": "renamed", "old_path": "old.txt", "add": 0, "del": 0},
           {"path": "pic.bin", "status": "untracked", "add": 0, "del": 0, "binary": true}],
 "add": 4, "del": 1, "truncated": false}
```

`status`：`added`（已 `git add` 的新文件）、`modified`、`deleted`、`renamed`（带 `old_path`；未暂存的改名也认得出）、`untracked`（git 还不认识，整个文件都算加的行）。`binary: true` 的文件行数记 0。路径相对 `cwd`（`cwd` 在大仓库的子文件夹里时只列那个子文件夹里的改动）。`files` 最多 2000 个，超出 `truncated: true`，`add`/`del` 仍是全部的合计。`branch` 为 `null` 是 detached HEAD。`base_branch`、`ahead`、`commits` 只在 worktree 会话里有：`ahead` 是分支上有而 `base_branch` 没有的提交数，`commits` 是这些提交（新到旧，最多 50）。**文件列表只含没提交的改动**；agent 提交之后列表是空的，提交看 `commits`。`cwd` 不是 git 仓库（或文件夹没了、会话已合并）：`{"repo": false, "branch": null, "worktree": …, "files": [], "add": 0, "del": 0, "truncated": false}`。没有 HEAD（仓库还没提交）时和空树比。

`thread.diff {thread, path?}` → `{"diffs": [...], "truncated": false}`。`diffs` 的每一项是对话里工具调用 `diffs` 用的 `lila_agent::FileDiff`（`{path, lines}`，`lines` 是统一 diff 从第一个 `@@` 起的行），所以窗口可以直接喂给同一个 diff 查看器；另外多了几个字段：`status`（同上）、`old_path?`、`binary?`（二进制文件：`lines` 为空）、`truncated?`（这个文件的 diff 被截断了：最多 3000 行或 30 万字节）。未跟踪的文件是整个文件的「全加」diff；纯改名没有 `lines`。不给 `path` 是所有改动文件（顺序和 `thread.changes` 一样），给了只返回那一个（`path` 可以是改名前或改名后的名字）；没有改动返回空数组。顶层的 `truncated` 是整份补丁超过 8MB、后面的文件没有给全。`path` 必须是相对路径、不含 `..`。

`thread.file {thread, path}` → `{"path": "src/a.rs", "text": "…", "size": 123}`；二进制（前 8000 字节有 NUL，或不是 UTF-8）是 `{path, binary: true, size}`，超过 1MB 是 `{path, too_large: true, size}`，都不带 `text`。读不了就报错：绝对路径、`..`、`.git` 里的、符号链接指到 `cwd` 外面的（先解析再比较）、不存在、是文件夹。文件树用已有的 `thread.files`（git 跟踪的和没被忽略的新文件，相对路径）。

`thread.commit {thread}`：给会话发一条 Lila 的消息（`<lila note about="commit">…</lila>`，对话里显示成安静的一行，`from: lila`，不是人发的气泡），让 agent 看 `git status`、`git diff`，按仓库习惯写提交信息、提交全部改动（含新文件），不推送、不改历史。提交本身由 agent 做，所以**权限档照常管它**（只读档的 `git commit` 会被拦）。报错：会话在跑、在等回答、已结束（先 `resume`，要 agent 在才能提交）、正在合并、已合并；`cwd` 不是 git 仓库；没有改动。成功后重新数一次摘要，agent 做完（一轮结束）时再数一次。

`thread.merge {thread}`：只对 worktree 会话。会话必须是 `idle` 或 `ended`（在跑、在等、刚启动都不行），没有排队没发出的话（已结束的不看这条）。成功返回 `{"merged": true, "thread": 会话, "commit": "短哈希", "commits": 2, "files": 3, "branch": "main"}`，会话已归档、`merged_ms` 有值、引擎已结束、worktree 和分支稍后在后台删掉（§10）。失败什么都不改，报错文字说原因：worktree 有未提交的改动（先 `thread.commit`）；分支上相对 `base_branch` 没有提交；项目文件夹不在 `base_branch` 上（或 detached）；项目文件夹有未提交的改动和这次合并要改的文件重名（列出文件名）；和 `base_branch` 这段时间的新提交冲突（rebase 失败，已恢复原状，worktree 保留）；提交被 commit hook 拒绝（hook 的输出在错误里）；worktree 的分支被切走了。每次失败在 `thread.log` 里记一条 `merge_failed`。

**改动摘要**（`changes` 字段）不是实时的：lilad 在这些时候重新数，**变了才**推送（`thread` 事件，`changes` 字段也带在事件里）：每一轮结束（终端状态从工作中/被拦转为空闲，host 报 turn 结束）、`thread.commit` 发出后、`thread.merge` 之后、会话建好时、取消归档时、lilad 启动时（所有没归档的会话）。一轮进行当中不数，更不是每 100ms 跑 git；用户自己在 worktree 里改文件要等下一次触发才反映，窗口要最新的就调 `thread.changes`（它也会更新摘要）。摘要在后台一个一个数，同一个会话数的时候又被要求就在数完后再数一次。

`thread.log` 的 `kind`：`checkpoint_failed`（这一轮没打成检查点，`detail` 是原因，轮照常进行）、`restored`（`detail`：`checkpoint 3 (首行): 1 put back, 1 made again, 1 deleted; undo is checkpoint 7`）、`restore_failed`、`started`、`ended`、`resumed`、`auto_allowed`、`auto_denied`、`asked`（开了卡片）、`answered`、`permission`（档位切换）、`turn_failed`、`notice`、`commit`（要 agent 提交）、`merged`（`detail`：`2 commit(s) into main as f898670`）、`merge_failed`、`rule_added`（人点了「总是允许」，`detail` 是加的规则）、`rule_removed`、`rules_dropped`（切到只读时规则被清掉）、`trusted`（`thread.trust`，`detail` 是文件夹）。`auto_allowed`/`auto_denied` 记着档位和请求，是审计「lilad 替你答了什么」的地方；规则答的那条以 `· rule: cargo test` 结尾，点名是哪条规则；

### 其他

| 方法 | 说明 |
|---|---|
| `ping` | `{pong, pid, version, protocol}` |
| `status` | `{pid, version, protocol, projects, threads: {state: 数量}, cards}` |
| `stop` | 停 lilad。会话照样运行（只有 socket 请求能发，spool 已经没有了） |
| `subscribe` | 见 §5 |
| `host.attach` `{thread}` | `lila host` 用：这条连接变成事件进、命令出的通道（`lila_agent::AgentEvent` / `Command`），窗口不要碰 |

## 5. 推送

`subscribe` 请求的回复是 `{"result": {"subscribed": true}}`，之后这条连接只有 lilad 推过来的行（`lila::client::subscribe` 封装了）：

| 事件 | 何时 |
|---|---|
| `{"event": "changed", "seq": N}` | 任何东西变了就推一条（兜底：收到就把项目列表和会话列表重拉一遍） |
| `{"event": "project", "project": "shop", "seq": N}` | 项目增、删、改默认值、重读 |
| `{"event": "thread", "project": "shop", "thread": "t…", "state": "running", "waiting": null, "items_seq": 12, "changes": {"files": 3, "add": 40, "del": 2}|null, "seq": N}` | 这个会话变了：状态、条目、权限卡片、正在写的回复、标题、档位、模型、改动摘要、合并。`items_seq` 是它的条目的最大 `seq`：比自己手里的大，就 `thread.read {after}` 取增量。会话被删（随项目删除）时不推 |

一次变化先推 `changed`，再推 `project`，再推各个 `thread`。事件只说「变了」，数据一律用请求取。「正在写」的逐字增量也走 `thread` 事件加 `thread.read` 的 `writing`，没有单独的增量事件。

## 6. 权限档

三档，每个会话各一档，随时 `thread.permission` 切换，项目默认 `full`：

| 档 | 含义 |
|---|---|
| `read_only` | 读、搜索、看网页可以；改文件、有副作用的命令一律拒绝（拒绝的文字会回到 agent：「This thread is read-only in Lila…」） |
| `confirm` | 读可以；其余（改文件、运行命令、写操作的 MCP 工具）都出卡片问人 |
| `full` | 都不问，除了四类（下）出卡片 |

**判定**（`crates/lila/src/policy.rs`，纯函数，有测试）：omp 的权限请求（ACP 的 `session/request_permission`）转成 `PermissionRequest`（工具种类、名字、命令行、路径、工作目录、选项），lilad 先按会话的档位回答（`daemon/permissions.rs::ask_permission`），回答不了的才变成卡片；每次自动回答和开卡片都记进 `thread_events`。

全自动档下仍要问的四类，以及**能识别到什么程度**（识别靠请求里的文字，不是沙箱；omp 线程没有沙箱）：

1. **改动或删除工作区以外的文件**：编辑类请求（对上的工具调用的 `path`，或 `edit` 对话里的 `File:` 行）按路径判断，路径会展开 `~`、`..`、符号链接，临时目录（`/tmp`、`/var/folders`、`$TMPDIR`、`/dev/null`）不算。命令行：看重定向（`> >>`）、`rm mv touch mkdir tee chmod chown ln truncate…`、`cp rsync install` 的目的地、`sed -i`，跟着 `cd`，进 `sh -c '…'`。**识别不了的**：变量拼出来的路径、脚本或程序内部写的文件（`python x.py` 里写 `/etc`）、`find -delete`、`xargs rm`、`dd of=`、`git checkout/clean` 对工作区外的仓库、通过别名或函数。
2. **`git push`、强制推送、删分支**：命令行里的 `git [-C dir|-c k=v …] push`、`git branch -d/-D/--delete`（包括 `bash -c`、`;`、`&&`、括号里的）。**识别不了的**：用变量、别名、`git` 的包装脚本、`GIT_SSH` 之类绕过；以及不是 `git` 命令的推送（`jj git push`、`gh repo sync`）。
3. **对外发出的动作：提 PR、发评论、操作 Asana**：`gh pr/issue/release/repo/… create|comment|review|merge|close|edit|…`、`gh api` 的 POST/PUT/PATCH/DELETE 或带 `-f/-F/--input`；`npm|pnpm|yarn|cargo publish`、`docker push`、`twine upload`；`curl|wget` 的写操作（`-X POST`、`-d`、`-F`、`-T`……，指向 localhost 的除外）；MCP 工具（按 `mcp__<server>__<tool>` 这个名字拆；omp 的对话标题里确实写 `Allow tool: mcp__…`、`Origin: MCP server tool`，但没有用真的 MCP 服务器跑过）里工具名以 create/add/update/delete/merge/push/post/send/comment/close/edit/fork/assign/submit/approve/publish/move/write/upload… 开头的。**识别不了的**：没有写动词的 MCP 工具名（读写看不出）、`ssh`/`scp`/邮件、用脚本语言直接调 HTTP。
4. **computer use**：工具名含 `computer` 且是 MCP 工具或以 `computer` 开头的。**现在没有这样的工具**（Lila 还没提供，omp 自带的也没有），所以这条没有实测过。

### omp 怎么落实

omp 在 `always-ask` 下对每个写或执行类工具调用（`bash`、`eval`、`edit`、`write`、`ast_edit`、`task`、MCP 工具……）发一个 `extension_ui_request`（`method: select`），标题是文字：`Allow tool: <工具名>`，后面可能有 `Origin: …`、`Reason: …`，然后是细节行；选项 `["Approve", "Approve <工具> for this session", "Deny"]`。读类 omp 自己放行，不问。全部到 lilad（`host_event` → `ask_permission`），按档位判定，切档立即生效。

**把对话对回工具调用**（`lila_agent::omp`）：对话本身没有结构化的内容，所以 host 一直记着还没结束的工具调用（来自 `toolcall_end`、`tool_execution_start`，带结构化的工具名和参数；`toolcall_end` 在模型消息结束前就有，一定早于执行，不依赖事件到达的先后）。对话标题第一行给出工具名，按名字找没结束、没被别的对话认领的调用：只有一个就是它；有几个（并行调用）时看哪个的命令/路径/代码的前 200 个字出现在对话标题里，**都对不上就当作对不上**。对上了：`name` 是稳定的工具名，`command`（bash）、`paths`（write 等）、`cwd`（bash 的 `cwd` 参数，相对的接在会话文件夹后面）取自参数——但**只在对话标题里也有这段话时才信**（扩展可以在调用之后改参数，对话标题是 omp 真正等着批准的东西）。`edit` 的参数是各种模式的补丁文本，读不出路径，改读 omp 自己为这个对话写的 `File:`、`Move: a → b` 行。对不上时：整个请求从对话标题里读（`Command:` 之后到结尾是命令，`File:`/`Path:`/`Move:` 是路径；`Content:`、`Code:`、`Arguments:` 之后是调用带的内容，不当行读），工具种类按名字分，所以 `git push` 之类仍然被 `policy` 认出来。工具名到 `ToolKind`：`bash eval task`→执行，`edit write ast_edit`→编辑，`read`→读，`grep glob find ast_grep lsp`→搜索，`web_search github`→网络，`mcp__…` 和不认识的→其他（`policy::reads` 按 `mcp__server__tool` 里的写动词判其他）。
lilad 的回答是 host 的 `Answer` 命令：host 只给卡片 omp 对话里的两个标签（普通的 `Approve` 和 `Deny`；`for this session` 和 `Always` 之类的不给，Lila 的规则取代它们），选项 id 就是这两个标签，别的一律当 `Deny`。`read_only` 档的拒绝没有文字回到 agent：RPC 里对话的回答只有选项文字，拒绝后 omp 自己给模型的错误是固定的 `Tool call denied by user: <工具>`（`wrapper.ts`），没有地方带原因，所以 `Verdict::Deny` 的文字不用（要送只能当备注排进下一条消息，没做）。没有沙箱：一切靠上面对请求文字的识别。


### 卡片不过期，lilad 重启也在

卡片开着，直到人答、omp 把请求收回（`PermissionResolved`，比如这一轮被中断），或会话结束（`thread_gone` 清掉它所有的卡片）。lilad 重启后 host 把没答的 `Permission` 事件重放，卡片回来。


### 本会话总是允许（A2，`crates/lila/src/rules.rs`）

`confirm` 档下每个命令和每次改动都是卡片，长任务没法用。卡片多一个选项，点了就把一条或几条**规则**记在会话上（`thread_rules`，lilad 重启、`thread.resume` 之后还在），之后同样的请求由 lilad 直接放行，不再出卡片。

**规则的形状**（简单、可预期）：

| 种类 | 内容 | 例 |
|---|---|---|
| `command` | 命令的**程序**，对按子命令用的工具（`git cargo npm pnpm yarn bun docker kubectl gh brew pip pip3 uv go make just rustup`）再加第一个子命令词 | `cargo test`、`git commit`、`npm run`、`ls`、`touch` |
| `edit` | 会话文件夹里的改动：改文件的工具（`edit`、`write`、`ast_edit`），所列的文件都在文件夹里（且列了文件） | 「改动此文件夹里的文件」 |
| `tool` | 一个 MCP 工具，名字完全一样（`mcp__…`） | `mcp__github__get_pull_request` |

- 程序按**写出来的词**比：`cargo` 不是 `/usr/bin/cargo`，`cargo-evil` 不是 `cargo`；没有「更宽」的规则（不按前缀、不按别名、不按路径）。子命令词必须是普通的小写词（`cargo +nightly test`、`git -C x status`、`git --no-pager log` 不成规则，因为那个词是选项）。
- **一行里串了几个命令**（`;`、`&&`、`||`、`|`、`&`、换行；用 `policy::split_commands` 切）：只有**每一段都有规则**才放行。卡片上的选项把**还没有规则的**那几段一起列出（`本会话总是允许 cargo test、tail`，最多 3 段，更多就不提供），点一下加几条规则。`cd`/`pushd`/`popd` 不算命令。
- **不提供、也永远不按规则放行**：包装器（`sudo env command time nohup nice timeout xargs watch exec eval source bash sh zsh …`，含 `bash -c`）、解释器和包运行器（`python node deno ruby perl php lua osascript awk npx bunx uvx pipx`）、`find ssh scp nc tmux screen open`、名字就说明会动文件的（`rm rmdir dd shred mkfs kill killall pkill chmod chown chgrp trash unlink`）、以 `VAR=值` 开头的行、命令替换（`$(…)`、反引号、`<(…)`）、here-document、子 shell 和 `if/for/while`、用变量或通配符当程序的、一行里有任何一段属于以上的；子命令里会跑任意东西的也不行（`npm exec`、`pnpm dlx`、`docker run/exec/compose`、`kubectl exec/run`、`bun x`、`uv tool`）。
- **危险永远不被规则吞掉，也不提供**：`rules::derive` 和 `rules::covers` 先问 `policy::danger`（工作区外、`git push`/删分支、对外动作、computer use），有就一律 `None`。这个检查是对**请求本身**做的，不依赖档位怎么问的：`confirm` 档里 `git push` 的卡片 `danger` 是空的（档位问的），但它照样没有「总是」选项、不会被 `git` 之类的规则放行（实测、有测试）。`full` 档的危险卡片（`danger` 非空）同样没有。只有 `confirm` 档、`danger` 为空、且规则能把「会允许什么」说准的卡片才有这个选项。
- **选项**：id `always`，`kind: allow_always`，标签写明从现在起允许什么（`本会话总是允许 cargo test`、`本会话总是允许改动此文件夹里的文件`）。放在「允许」之后、「拒绝」之前。点的时候 lilad 再算一遍（档位、规则可能变了），不再提供就报错。答的时候先加规则、记 `rule_added`，再用 omp 自己的普通「允许」选项回答；同一会话里这时还开着的、被新规则答得了的别的卡片一起答掉。
- **怎么合成「一个总是」**：omp 自己的 `Approve <工具> for this session` 的含义是 omp 自己的（范围 Lila 说不出，也收不回、重启就没了），所以**卡片上一律不出现**，由 Lila 的 `always` 取代；点了之后 omp 收到的是普通的 allow，下一次同样的请求还是先到 lilad，由规则答，日志里点名规则。omp 自己的「总是」会让它自己吞下以后的危险请求（比如 `git push`），所以危险卡片上也一并去掉。没有普通 allow 选项的请求保留 omp 的选项不动。只有假 host 的单元测试，没有真跑。
- **答的地方**：`ask_permission` 在档位判成 `Ask`、`danger` 为空、档位是 `confirm` 时先看规则；规则放行也记 `auto_allowed confirm · shell: cargo test --lib · rule: cargo test`。`full` 档里规则不起作用（保留着）；**切到 `read_only` 时规则全部清掉**（记 `rules_dropped`），再切回 `confirm` 也不回来。
- 窗口里规则列在权限档菜单里，每条一行点一下收回（`thread.rule.remove`）；命令行 `lila thread rules <id> [--remove N]`。
- **实际上还用不上的**：`tool` 规则现在不会被提供：档位在 `confirm` 下问的 MCP 工具只有名字带写动词的，它们本来就是「对外动作」的危险；不带写动词的 MCP 工具档位当作读，直接放行。规则本身有单元测试，等 P3 接了档位分不清的 MCP 工具才派上用场。

### 提问（agent 问人，不是权限）

omp 的 `ask` 工具（只在 `--mode rpc-ui` 里有）和任何扩展，会发 `extension_ui_request`：`select`（选项）、`confirm`（标题加说明）、`input`（占位文字，可能 `secret`）、`editor`（预填文字）。标题不是 `Allow tool: ` 开头、选项里没有 `Approve` 和 `Deny` 的，一律是提问（`lila_agent::Question {id, title, detail, kind: choice｜confirm｜text, options, prefill, secret}`），**不会静默挂着**：host 发 `AgentEvent::Question`，lilad 开一张 `q…` 卡片（`daemon/questions.rs`，在内存里，和权限卡片一样；host 留一份，lilad 回来时重放），会话状态是 `waiting`（`waiting: "card"`），侧栏和通知照旧；会话在等卡片时 `thread.send` 被拒绝。

- 答：`thread.answer {thread, card, value｜yes｜cancel}`（命令行 `lila thread answer <id> <q…> <选项原文或序号|yes|no|文字>`，`--cancel` 不回答），lilad 校验后发 `Command::Reply`，host 回 `extension_ui_response`（选择和文字是 `value`，确认是 `confirmed`，不回答是 `cancelled: true`），omp 把它作为 `ask` 工具的结果交给模型。
- omp 收回（超时，或这一轮被中断）发 `cancel`（`targetId`），host 发 `QuestionResolved`，卡片消失；`thread.interrupt` 时 host 自己把开着的提问回成 `cancelled`。
- `ask` 工具一张表单会按字段一个一个问（先 `select`，选「其他」再 `editor`）；每个对话一张卡片，不合并；多选的 `checkedIndices`、`optionDetails`（选项说明）、`initialIndex` 不用。`secret` 的输入在窗口里**没有遮盖**（gpui-kit 的输入框不支持），只是日志里不记。没打开 `set_ask_dialog`（一次问多题的 `ask` 方法），所以用不到它。

## 7. 命令行

`lila` 是窗口之外的另一个客户端，只走同一个 socket：

```sh
lila up                              # 启动 lilad，把当前文件夹加成项目
lila project list|add [路径] [--name --title]|remove [项目]|show [项目]|set [--model --permission --title]
lila thread new "第一条消息" [--project --model --permission --title --file F --worktree]
lila thread list [--project|--all] [--archived]
lila thread show <id> [--after N]    # 状态、对话、待答的卡片
lila thread send <id> "话" [--file F]
lila thread interrupt|stop|archive|unarchive <id>
lila thread resume <id> ["话"] [--model --permission]
lila thread rename <id> <标题>
lila thread permission <id> read_only|confirm|full
lila thread answer <id> <卡片> allow|deny|always|<选项>   # 提问卡片：<选项原文或序号>|yes|no|<文字>，或 --cancel
lila models [--refresh]               # omp 提供的模型（provider/model）
lila thread rules <id> [--remove N]    # 本会话「总是允许」的规则；--remove 收回一条
lila thread model <id> [--model --effort]
lila thread changes <id>             # 分支、提交（worktree）、改动的文件和行数
lila thread diff <id> [路径]          # 改动的 diff（一个文件或全部）
lila thread commit <id>              # 让 agent 提交（它写信息，不推送）
lila thread merge <id>               # worktree 会话合并回起点分支，删 worktree，归档
lila thread checkpoints <id>         # 检查点：编号、时间、HEAD、它之前那条消息的首行
lila thread restore <id> <编号> [--dry-run]   # 文件放回到那个检查点；--dry-run 只列出会改哪些文件
lila thread log <id>
lila daemon run|stop|status|restart  # status：协议和窗口的不一样时说怎么办，退出码 3 没有运行、4 协议不一样；restart：停、等 socket 没了、再启动（会话不断）
lila --json …                        # 任何命令都能输出原始 JSON
```

`LILA_HOME=/tmp/x lila …` 用独立的库、socket 和状态文件夹，真跑测试用。

## 8. 代码

| 位置 | 内容 |
|---|---|
| `lila-core` | `config.rs`（项目默认值、project.toml）、`thread.rs`（`Tier`、`ThreadState`、`AgentStatus`、`Thread`）、`runtime.rs`（omp 的启动参数） |
| `lila-store` | SQLite（§1） |
| `lila-agent` | 对话事件模型（`model.rs`）和 omp RPC 模式的翻译（`omp.rs`，纯函数，不做 IO：一行进，`Step {send, events}` 出；`lila/src/host.rs` 是它的 IO，`lila/tests/host.rs` 用一个假 omp 和假 lilad 整个走一遍） |
| `lila/src/daemon/` | `mod.rs`（状态、hc 事件、状态计算）、`threads.rs`（项目和会话的生命周期、投递）、`permissions.rs`（卡片、`thread.answer`）、`conversation.rs`（读对话、发消息、菜单）、`host.rs`、`questions.rs`（提问卡片）、`requests.rs`（方法分派）、`server.rs`（socket、推送）、`sources.rs`（留给 P4 的 GitHub 任务，没人调用） |
| `lila/src/daemon/jobs.rs` | 放在主循环外跑的活（§4 末尾、§10）：`Job`、`Daemon::begin`（请求要么直接答，要么给一个 job）、`finish`（job 的结果变成回复和后续）、改动摘要的刷新 |
| `lila/src/worktree.rs` | 会话的 worktree：建（`create`）、删（`remove`）、git 命令行的小工具 |
| `lila/src/changes.rs` | 改动列表、diff、读文件（`thread.changes/diff/file`） |
| `lila/src/merge.rs` | squash、放到起点分支最新提交之上、项目文件夹 `--ff-only`（`thread.merge`） |
| `lila/src/checkpoint.rs` | 检查点的 git 工作（§11）：打快照、放回、删 ref |
| `lila/src/daemon/checkpoints.rs` | 什么时候打、检查点怎么对上对话里的消息、回退的前置条件和后续 |
| `lila/src/policy.rs` | 档位判定（§6） |
| `lila/src/rules.rs` | 「本会话总是允许」的规则：怎么从请求算出规则、哪些请求被规则答、标签（§6） |
| `lila/src/project.rs` | project.toml 的读写 |
| 留着、没人调用的 | `github.rs`、`daemon/sources.rs`（P4）；它们不依赖任务和流程，能独立编译、有测试 |

## 9. 窗口

`lila-app` 只通过上面的方法和推送工作，不读数据库、不起引擎进程；界面规范见 `DESIGN.md`。

| 窗口里的操作 | 用到的接口 |
|---|---|
| 侧栏的项目和会话；颜色、转圈、红底 | `project.list`、`thread.list {archived: true}`（一次拿全，归档的在窗口里过滤；`state`、`waiting`、`active_ms`、`archived`），`subscribe` 的任何推送都触发重拉 |
| 添加项目 | `project.add {cwd}`（`autostart`），然后窗口显示它的新会话输入框 |
| 新会话：输入框、选模型和权限档、发出第一条消息 | 项目默认值来自 `project.list` 的 `defaults`；输入框里的 `@` 用 `thread.files {project}`；模型菜单（工具条右边）第一次显示时问一次 `models.list`（`LilaApp::ensure_models`），第一项「默认」是工作区的；发出时 `thread.new {project, text, attachments, model?, permission}`（选了才带 model）。成功后显示返回的会话 |
| 对话：消息、权限卡片、提问卡片 | `thread.read {after}`（`items`、`permissions`、`questions`、`config`），`thread.send`，`thread.answer`（权限用 `option`，提问用 `value`｜`yes`｜`cancel`）。提问卡片（`ui::question_card`）和权限卡片同一个壳：选择题一排胶囊、确认题「是」「否」、问答题一个输入框加「回答」，都有「不回答」。`error` 条目画成红色 CircleX 加一行字 |
| 标题行：权限档、中断、结束、继续 | `thread.permission`、`thread.interrupt`、`thread.stop`、`thread.resume` |
| 会话右键：重命名、结束或继续、归档或取消归档 | `thread.rename`、`thread.stop`/`resume`、`thread.archive`/`unarchive` |
| 旧 lilad 提示（侧栏最后一行，设置按钮右边） | `ping` 的 `version`/`protocol`（每次刷新都取）；协议和窗口的不一样时出一行「lilad 版本不符」加「重启」按钮：在后台线程里 `lila::client::restart_daemon`（`stop`、等 socket 没了、`start_daemon`），然后重取。重启后还不一样（磁盘上的 `lila` 本身是旧的）就改成「lila 命令需要重装」，不再给按钮。全局设置 → 关于的 lilad 一行写版本和协议 |
| 项目设置（⇧⌘,） | `project.set {project, title, defaults}`；`model` 传空字符串表示清掉；模型一行是 `models.list` 的下拉 |
| 输入框的 `/` 菜单、会话内换模型 | `thread.commands`、`thread.model` |
| 移除项目 | `project.remove` |
| 右面板：文件、改动（P2） | 标题行按钮和 ⇧⌘E / ⇧⌘G 开关，宽度、视图记在 `state/app-panel.json`。文件树 `thread.files`，预览 `thread.file`；改动 `thread.changes`、点开一行 `thread.diff {path}`；提交 `thread.commit`，合并 `thread.merge`（确认后；成功就离开会话）。重新读的时机：打开、切视图、换会话、推送里该会话的 `changes` 摘要或 `state` 变了、提交或合并之后 |
| 新会话选独立 worktree | `project.list` 的 `repo` 为真才有开关；发出时 `thread.new {…, worktree: true}`（回复要等 worktree 建好，窗口在后台等，发送按钮转圈） |
| 每轮的「回到这里」（A1，`restore.rs`） | `thread.read` 的 `checkpoints`（窗口每次读都整份换掉，按 `item` 对上用户消息）。一条 `from: person` 的消息有按钮，要同时：有 `kind: turn`、`item` 对得上、`restorable` 为真的检查点；会话 `idle`/`ended`、没在合并或回退、没合并过；`ping` 的 `protocol` ≥ 2。点了先 `thread.restore {dry_run: true}`（后台线程，按钮转圈），回复的 `counts`、清单、`truncated`、`head_moved` 放进确认框；全是 0 就只说「文件已经是这个样子」，没有可确认的。确认后 `thread.restore`（后台线程；窗口记在 `restore.going`，lilad 的 `restoring` 也算，这期间对话末尾转圈、输入框不发送）；回复后不论成败都 `refresh_now` 并重读右面板（失败也读：可能做到一半）。失败的文字原样放进窗口顶部的红色提示条（和别处操作失败一样，可关）。条目 `from: "restored"` 画成「已回到这里」一行；`undo` 对应的检查点还在列表里且 `restorable` 时带「撤销」，走同一条 dry run、确认、回退的路。窗口不调 `thread.checkpoints`，也不读 `can_restore`/`why_not`（`thread.read` 里没有；每行的 `restorable` 够用） |
| 权限卡片的「总是允许」（A2） | 卡片选项由 lilad 给，窗口照画（`ui::permission_card`）：第一个 allow 是主按钮，其余 outline，所以「本会话总是允许 cargo test」是第二个 outline 胶囊；标签超过 44 字截断、tooltip 是全文；窄时按钮折行。点了 `thread.answer {option: "always"}`。旧 lilad 不给这个选项，卡片和以前一样 |
| 会话的规则（A2） | 权限档菜单（输入框工具条里的权限按钮）：三档下面一条线，每条规则一行「不再总是允许 cargo test」加 × 图标，点一下 `thread.rule.remove`。数据来自 `thread.read` 的 `rules`（`Conv.rules`，每次读整份换）；新会话的输入框没有规则 |
| 已合并的会话 | `merged_ms` 有值：输入框换成「已合并到 <base_branch> · <sha>」，sha 从 `thread.log` 里 `merged` 那条的 `detail`（`… as f898670`）取 |

- 窗口记着：上次显示的会话和项目、哪些会话切成了终端视图（`state/app-tabs.json`、`app-views.json`）。折叠的项目、归档开关不记。
- 系统通知在窗口里发：对比前后两次 `thread.list`，会话变成 `waiting`、或从 `running` 变成 `idle`，而窗口不在前面时，用 `osascript` 发一条。
- 截图自查：`--features shots`，环境变量见 `docs/HANDOFF.md`；`LILA_APP_FAKE=<json>` 可以不起 lilad，用造好的项目、会话和对话画界面。

## 10. worktree 和合并（P2）

- **建**：`thread.new {worktree: true}`。项目文件夹必须是 git 仓库的**根目录**（`worktree::is_repo_root`：仓库里的子文件夹、整个 home 在 git 里都不行）、在某个分支上、已有提交。worktree 在仓库外：`<data>/worktrees/<项目>/<会话 id>`（`Paths::worktree`，`LILA_HOME` 下就在它的 `data/` 里），分支 `lila/<会话 id>`，从项目文件夹当前分支的 HEAD 建（`git worktree add -b`）；记下 `base_branch` 和 `base_commit`。引擎在里面启动：`cwd`、终端的工作目录、`LILA_ROOT` 都是它，权限档判定的「工作区」也是它（所以 worktree 会话动项目文件夹里的文件会被当成工作区外）。
- **留**：归档、结束（`thread.stop`）都不动 worktree；`thread.resume` 在同一个 worktree 里继续（worktree 的文件夹被手工删了会报错）。项目被移除（`project.remove`）也**不**删它的 worktree 和分支。没有单独删 worktree 的入口，`thread.merge` 是唯一会删的地方。
- **合并**（`merge::run`，在主循环外）：分支上相对 `base_branch` 的全部提交（`base_branch..HEAD`）压成一个提交——只有一个提交就原样用它；多个就用会话标题当第一行、正文逐条列各提交的标题（`- 标题`）——放到 `base_branch` 最新提交之上（`git rebase`），再在项目文件夹里 `git merge --ff-only`，所以项目文件夹的分支只是向前移动；然后会话的引擎结束、会话归档并记 `merged_ms`，worktree 和分支由另一个后台 job 删掉（失败记 `notice`，不影响合并的结果）。`git commit`（压缩时）和 rebase 照常跑用户的 hook 和 git 配置。开始改动 worktree 之前先查完所有会失败的条件（见 `thread.merge`）；rebase 冲突、commit hook 拒绝、ff 失败时 `git reset --hard` 回合并前的提交、`rebase --abort`，worktree 的分支和项目文件夹都是原样。ff 时 `base_branch` 刚好又前进了就重新 rebase 再试（最多重试 3 次）。
- **已知的局限**：worktree 里只有 git 跟踪的文件，被忽略的（`.env`、`node_modules/`、`target/`）不会带过去，omp 在里面可能跑不起来，目前不处理；`.lila/project.toml` 没提交的话 worktree 里没有它。

## 11. 检查点和回退（A1）

用户定的：**只有文件回去，对话不动**（不去回滚引擎的会话）。会话默认全自动，agent 改坏了要有退路。

### 怎么打

快照是一次不动用户仓库状态的提交（`checkpoint.rs`，git 命令行，不用 libgit2）：把用户的 index 复制一份（`GIT_INDEX_FILE`，复制是为了沿用它的 stat 缓存，大仓库快）→ 在副本上 `git add -A -- .` → `git write-tree` → `git commit-tree`（父提交是 HEAD，还没有提交就没有父提交；作者固定 `Lila <lila@localhost>`，关掉签名）→ `git update-ref refs/lila/checkpoints/<会话>/<n>` 留住它。**HEAD、分支、用户的 index、stash、工作区都不动**（测试里比较了 `git status --porcelain=v2`、HEAD、分支、stash、index 文件的字节）。

- **范围是会话的 `cwd`，不是整个仓库**：`cwd` 在大仓库的子文件夹里时，只存、也只回退那个子文件夹（`git add -A -- .`；回退时 `diff-tree --relative`）。理由：`cwd` 是权限判定的「工作区」，回退会删文件，不能越出它。树里 `cwd` 以外的部分是用户 index 里原有的，不会被回退碰。
- 内容：已跟踪文件当前的样子（含未提交、已暂存的改动）、没被忽略的新文件；已删除的不在里面。`.gitignore`（含全局的）忽略的文件不进快照，回退也不碰。
- 没有变化：和该会话上一个检查点的树、HEAD 都一样，就用上一个的提交；文件夹正好等于 HEAD 的树，就用 HEAD 本身（不写任何对象）。每一轮仍各有一行记录和各自的 ref，所以每轮都能回。
- `cwd` 不在 git 仓库里（或会话已合并）：不打，不记录，不拖慢（`maybe_repo` 往上找 `.git`，不起进程，没有就跳过）。
- **什么时候打**（要求：在引擎对这一轮的消息动文件之前；lilad 主循环不被 git 卡住；打不成不拦消息）：

  - **在 `deliver` 里**，话发给 host 之前：话留在 outbox 里，job 做完才发（`snapshotting`）。只在 omp 空闲（`idle`/`done`）时打，这些话会开始新一轮；omp 运行中收不了话，所以话留在 lilad 的 outbox 里，等它空闲再发，好让快照是下一轮之前的。
  - 失败（git 不能跑、index 有未解决的冲突……）：记 `thread_events`（`checkpoint_failed`），消息照发。
- 数量：每个会话保留编号在「最新编号 − 50」之后的，更老的删 ref **也删记录**（编号不重用）。会话合并后（worktree 删掉时）、项目被移除时，删它们所有的 ref（一个后台 job；ref 在共享的 `.git` 里，worktree 会话的也由主仓库的路径删）和记录。

### 检查点和对话里的消息怎么对上

条目的 id 是 host 在话发出去时才起的，所以检查点一开始没有 `item`。lilad 在每次 `thread.read` / `thread.checkpoints` 时（`resolve_checkpoints`）给还没对上的检查点找消息：按编号从旧到新，取**从它的时间（宽限 3 秒）起、还没被别的检查点用掉、说的话一样的第一条** `from` 为 `person` 或 `lila` 的用户消息；对上就写进记录，之后不变。「话一样」比较的是去掉 `<lila note>`、压缩空白之后的话（只有文件的消息没有话，也算一样）。同样的话说两遍，各自对上各自的。一个检查点的消息一直没出现（`/` 命令），等后面的检查点对上之后它就被放弃（`item` 记成空，对外是 `null`），它仍然能回退。窗口永远是拿 `thread.read` 里的 `checkpoints` 列表按 `item` 去对条目，不是在条目上找。

检查点的一行（`thread.checkpoints` 的 `checkpoints` 和 `thread.read` 的 `checkpoints` 一样）：

```json
{"id": 3, "kind": "turn"|"restore", "at_ms": 0, "title": "它之前那条消息的首行（100 字内）", "item": "u7"|null,
 "restorable": true, "head": "847b0c2"|null, "branch": "main"|null}
```

`kind: "restore"` 是回退之前自动存的当时状态，没有消息（`title` 是一句英文说明）。`restorable` 是「现在提交 `thread.restore` 会不会被接受」（和会话的状态有关，同一时刻每行一样）。

`thread.checkpoints {thread}` → `{"checkpoints": [...新到旧], "can_restore": true, "why_not": null|"文字"}`。

### 回退

`thread.restore {thread, checkpoint, dry_run?}`。只在会话 `idle` 或 `ended` 时，其余（在跑、等回答、正在启动、正在合并或回退、已合并）拒绝；没有排队没发的话（`ended` 的不看这条）；没有一条消息正等着快照；同一个文件夹里没有别的会话在跑（或在等权限卡片）——两个会话共用一个文件夹时，回退会连带撤销另一个会话的改动，所以先用 `dry_run` 看文件清单，别的不处理。`dry_run` 同样先过这些检查。

回复（`dry_run` 和真回退同一个形状；`dry_run` 时 `restored: false`、`undo: null`）：

```json
{"thread": "t…", "checkpoint": 3, "dry_run": false, "restored": true, "undo": 7,
 "head_moved": false, "head": {"then": "847b0c2", "now": "847b0c2"},
 "changed": ["src/a.rs"], "recreated": ["b.txt"], "deleted": ["new.txt"],
 "counts": {"changed": 1, "recreated": 1, "deleted": 1}, "truncated": false}
```

`changed`：现在和检查点里内容不同的文件，会放回去；`recreated`：检查点里有、现在没有的，会重新做出来；`deleted`：现在有、检查点里没有（且没被忽略）的，会删掉。路径相对 `cwd`；每个清单最多 500 个，`counts` 是全部，超出 `truncated: true`。`head_moved`：HEAD 和检查点那时不同（其间有提交）：**文件照样放回去，HEAD、分支、index 一律不动**，所以面板里这些提交的内容会显示成未提交的改动（暂时接受）。

真回退的步骤（job，在主循环外）：先给**现在的状态**打一个检查点（`kind: "restore"`，编号在 `undo` 里；这一步失败就整个不做）→ 删「现在有、检查点里没有」的文件，再删因此空了的文件夹（到 `cwd` 为止；里面还有被忽略的文件的文件夹留着；是仓库里的另一个仓库的文件夹不删）→ 用一份临时 index（`read-tree` 检查点的树）加 `git checkout-index -f` 把其余文件写回（权限位、符号链接、用户的换行设置都和 git 检出一样；用户的 index 不参与）。中途失败：报错说明「做到一半，检查点 N 是回退之前的状态」。之后（在主循环）：

1. `thread_events` 记 `restored`；
2. 对话里加一条标记条目（`from: "restored"`，存在 `conv_items`，lilad 重启后还在）；
3. 给 agent 排一条英文备注（`outbox` 里 `hold = 1`）：哪一轮之后的改动被撤销了、几个文件、HEAD 没动，让它再改文件前先重新读。**不开新的一轮，不花 token**：`hold` 的备注自己不会被发出，等人的下一条消息时放在它前面合成一条消息发出（`deliver_with`）。文件和检查点一模一样（没有差别）时不排备注。备注+人的话在 host 的回显（`acp.rs`）里被认出来，只显示人的话：`lila_agent::without_notes`；
4. 重新数改动摘要。

### 已知的局限

- 只管文件：对话、引擎的内部状态、已经执行过的命令的副作用（装的依赖、数据库、网络请求）都不回。
- 被忽略的文件不管：`node_modules/`、`target/`、`.env` 这类如果被 agent 改了，不会回来。一个文件在检查点时没被忽略、现在被忽略了（`.gitignore` 变了），会被检查点里的内容覆盖。
- 范围是会话的 `cwd`；`cwd` 以外的改动不回。仓库里嵌套的仓库（子模块、自己有 `.git` 的文件夹）在快照里只是一条记录，里面的文件不管。
- 打快照是 `git add -A`：大仓库首次要几秒（之后沿用 index 的 stat 缓存）；没被忽略的巨大文件夹（几 GB 的 `node_modules` 没写进 `.gitignore`）会变慢并占对象库。没有大小上限。
- `refs/lila/checkpoints/*` 会出现在 `git log --all`、gitk 里；`git push --mirror` 会把它们一起推（`git push`、`--all` 不会）。
- 回退只更新工作区的文件，**不更新用户的 index**：回退之前暂存的内容、回退后 index 里还是原来那份。
