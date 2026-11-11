# Lila 架构 v4：项目与会话（lilad 的 socket 接口）

> 2026-11-10 · P1a（后端）和 P1b（窗口，见 §9）；2026-11-11 · P2 后端（worktree、改动、提交、合并，见 §4、§10）；2026-11-11 · A1 后端（每轮检查点和回退，协议版本 2，见 §11）；2026-11-11 · A2 前半（卡片不过期、本会话总是允许、窗口里回答「信任此文件夹」，协议版本 3，见 §6、§12）。需求见 `product-v4.md`；引擎驱动和对话事件模型见 `architecture-v3.md`（继续有效）。
> 本文和代码一致，窗口只看这份就能接。旧实现（任务、成员、请求）在标签 `v3-final`。

## 1. 模型

- **项目**：一个文件夹。注册在数据库的 `projects` 表（id、文件夹）；默认值在 `<文件夹>/.lila/project.toml`：

  ```toml
  name = "shop"            # 项目 id：1–24 个 a-z 0-9 - _，字母开头，注册后不变
  title = "商店"            # 可选，窗口显示的名字
  [defaults]
  engine = "claude"        # claude | codex | omp
  model = ""               # 空：该引擎的默认（claude-sonnet-5-5 / gpt-6.1-sol / omp 自己的）
  effort = "high"          # 可选
  permission = "full"      # read_only | confirm | full，默认 full
  ```

  v3 的文件（带 `[[members]]` 等）也能读，多余的键忽略。`project.add` 在文件夹没有这个文件时写一个。lilad 是唯一的写入方：窗口改默认值用 `project.set`。
- **会话（thread）**：一次和 agent 的对话，属于一个项目。

  | 字段 | 说明 |
  |---|---|
  | `id` | `t` 加 8 位十六进制，如 `t07b6b991` |
  | `title` | 默认取第一条消息的首行（60 字内），`thread.rename` 可改 |
  | `engine` | `claude`（Claude Code 终端加 hooks）、`codex`（`codex app-server`）、`omp`（ACP agent，`lila host omp`）。ACP 引擎目前只内置 omp；再接一个只要在 `lila_core::Runtime` 加一项、`runtime::launch` 加一支 |
  | `model`、`effort` | 存的值；运行中以引擎报告的为准（见 `thread.show` 的 `model`） |
  | `permission` | 权限档，见 §6 |
  | `cwd` | 引擎运行的文件夹：项目文件夹，或会话自己的 worktree（§10） |
  | `worktree` | 会话用独立 worktree（建会话时选的，之后不变）。合并之后仍为 `true`，看 `merged_ms` |
  | `base_branch`、`base_commit` | worktree 会话：建 worktree 时项目文件夹所在的分支和它的提交；合并回来就是放回这个分支。不是 worktree 的会话为 `null` |
  | `merged_ms` | worktree 的工作合并回 `base_branch` 的时间；合并后 worktree 和分支已删，会话已归档，`cwd` 这个文件夹不存在了。没合并为 `null` |
  | `merging` | 正在合并（`thread.merge` 还没回复）：这时不能发消息、续接、再合并 |
  | `changes` | 改动摘要 `{files, add, del}`：`cwd` 相对 HEAD 的改动文件数、加的行数、删的行数。`null`：还没数过，或 `cwd` 不在 git 仓库里；不是实时的，见 §5 |
  | `session` | 引擎自己的会话 id，续接用。Claude：新建时 lilad 选的 UUID；Codex、omp：host 报告 |
  | `terminal` | 当前的 hc 终端地址；结束后为空 |
  | `created_ms`、`active_ms`、`started_ms`、`ended_ms`、`archived` | 时间戳（Unix 毫秒）；`active_ms` 是最近一条消息或回复，列表按它倒序 |

- **状态**（`state`，由终端状态和待答的权限卡片算出）：

  | state | 含义 |
  |---|---|
  | `starting` | 引擎已启动，还没报过状态 |
  | `running` | 在处理一轮 |
  | `waiting` | 在等人。`waiting` 字段说等什么：`card`（有权限卡片，在 `thread.read` 的 `permissions` 里）或 `terminal`（终端里有对话框：Claude 的「信任这个文件夹？」、登录；或启动 20 秒还没报状态）。`terminal` 时 `waiting_for` 说是不是认得出的那一种：现在只有 `trust`（Claude Code 的「信任此文件夹」，见 §12），`trust_folder` 是它问的文件夹；认不出的（登录、别的）是 `null` |
  | `idle` | 在提示符前，能收消息 |
  | `ended` | 引擎进程没了。对话还在，可 `thread.resume` |

  另有 `status`：终端报的原始词（`idle`/`working`/`blocked`/`done`/`error`/`unknown`/`gone`）。

- **数据库**（`lila-store`，SQLite，`PRAGMA user_version = 7`）：`projects`、`threads`、`conv_items`（按会话递增的 `seq`）、`outbox`（引擎还不能收时排队的话；`hold = 1` 的是「随下一条消息一起发的备注」，自己不会被发出）、`thread_events`（lilad 自己做过的事）、`checkpoints`（§11：`thread, n, kind(turn|restore), at_ms, sha, tree, head, branch, text, item`，主键 `(thread, n)`）、`thread_rules`（§6「本会话总是允许」：`id, thread, kind(command|edit|tool), key, created_ms`，`(thread, kind, key)` 唯一；会话删除时一起删）。**v3 的库不迁移**：打开 v3 的库（没有版本号、有 `tasks` 表）时，文件改名为 `lila.db.v3.bak`，新库从空开始。**v4 的库平滑升级**：版本 5 给 `threads` 加了 `worktree`（默认 0）、`base_branch`、`base_commit`、`merged_ms` 四列；`migrate` 每次打开都按 `pragma_table_info` 查缺哪列就 `ALTER TABLE ADD COLUMN`，已有会话变成「不是 worktree」，再打开不变。以后只加列的改动照这样加到 `ADDED_COLUMNS`（现在是 `(表, 列, 定义)`，版本 6 加了 `outbox.hold`；新表用 `CREATE TABLE IF NOT EXISTS`，版本 6 加了 `checkpoints`，版本 7 加了 `thread_rules`：旧库打开后只多一张空表，原有的都照旧，有测试）。旧库打开后原有的会话、排队的话都照旧。
- `changes` 摘要**不进数据库**：lilad 在内存里记，启动时给每个没归档的会话数一遍（后台一个一个数）。

## 2. 进程与终端

每个会话的引擎跑在**自己的 hc 终端**里：lilad 重启时会话不断。

- 终端名 `lila-<会话 id>`，标签 `lila.project`、`lila.thread`，环境变量 `LILA_PROJECT`、`LILA_THREAD`、`LILA_ROOT`（文件夹）、`LILA_HOME`（若设了）。
- 状态由程序自己用 OSC 7501 报给 hc（Claude Code 自带；`lila host` 替 Codex 和 ACP 报），lilad 订阅 hc 的事件。lilad 重启后：数据库里有终端的会话先当作活着，hc 的快照到了，快照里没有的标成 `ended`；快照里有的按标签对回会话。
- 启动命令：
  - Claude：`claude --model M [--session-id UUID | --resume ID] [--effort E] --permission-mode manual --settings <state>/threads/<id>/settings.json`。settings 里六个 hook（PreToolUse、PermissionRequest、MessageDisplay、UserPromptSubmit、Stop、StopFailure）都跑 `lila hook <事件>`，matcher 都是 `*`。settings 在会话启动或恢复时写，**已经在跑的会话保持旧的 settings（没有 `PreToolUse`），要结束再恢复才换**。
  - Codex：`lila host codex --program <codex> --model M [--effort E] --sandbox S --approval A [--thread ID]`。
  - omp：`lila host omp --program <omp> [--model M] [--session ID]`。
  - `lila host` 的 `--prompt-file` 可选（现在不带系统提示词）。
- 对话条目：Claude 读会话记录文件（每 250ms 跟读），Codex 和 omp 由 host 通过 `host.attach` 连上 lilad 推事件。条目存数据库，格式见 `lila_agent::Item`（`architecture-v3.md` §5）。

## 3. 协议

unix socket：`$LILA_HOME/state/lilad.sock`（没设 `LILA_HOME` 时 `~/.local/state/lila/lilad.sock`；路径太长时落在临时目录，`lila_core`/`Paths::socket()` 给准）。每行一个 JSON。

请求：`{"method": "thread.list", "params": {...}}`；回复：`{"result": ...}` 或 `{"error": "文字"}`。一条连接可以连着发多个请求，回复按请求的顺序一行一行回来。窗口用 `lila::client::request(paths, method, params, autostart)`；`autostart` 为真时 lilad 没跑就先在 hc 里启动它。

**请求 id**（可选）：请求里可以带 `"id"`（任意 JSON 字符串或数字），lilad 原样放进这条请求的回复：`{"id": "a-1", "result": ...}`、`{"id": 7, "error": "…"}`，`subscribe` 和 `host.attach` 的第一条回复也带。不带 id 的请求，回复和以前一模一样。`id` 是别的类型（对象、数组、布尔）回 `{"error": "bad request: id must be a string or a number"}`；`null` 当作没带；整行不是 JSON 时没有 id 可回。这只是给以后的远程客户端留的：lilad 不做多路复用，回复仍然按顺序，不会乱序。`lila::client` 现在不发 id。

**协议版本 2**（A1）：加了 `thread.checkpoints`、`thread.restore`，`thread.read` 多了 `checkpoints`，会话多了 `restoring`，条目多了 `from: "restored"`。窗口用这些就要求 `protocol >= 2`。

**协议版本 3**（A2）：会话多了 `waiting_for`、`trust_folder`；`thread.show`、`thread.read` 多了 `rules`；加了 `thread.trust`、`thread.rule.remove`；卡片选项多了 Lila 自己的 `always`（见 §6）。窗口都是读这些字段：旧 lilad 没有它们，窗口照旧画「在终端里等你」、没有「总是允许」、权限菜单里没有规则，什么都不坏（所以窗口不查协议号，只是侧栏会照常提示 lilad 版本不符）。

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
| `project.set` | `project`, `title?`, `defaults?: {engine?, model?, effort?, permission?}` | 项目。写 project.toml；空字符串的 `model`/`effort` 表示清掉 |

项目：

```json
{"id": "shop", "cwd": "/abs/path", "title": "商店"|null, "label": "商店", "error": null, "repo": true,
 "defaults": {"engine": "claude", "model": "claude-sonnet-5-5", "effort": null, "permission": "full"},
 "threads": 3, "states": {"idle": 2, "running": 1}, "waiting": 0}
```

`repo`：文件夹是 git 仓库的根目录（`worktree::is_repo_root`），即新会话可以选独立 worktree；project 加载或 `project.reload` 时问一次、缓存（`project.list` 在主循环里，不起 git），之后才 `git init` 的要 reload。`error`：project.toml 读不了（文件夹没了、文件坏了）的原因；这时项目还在列表里，用默认值。`threads` 不含归档的；`waiting` 是待答的权限卡片数。

### 会话

| 方法 | 参数 | 返回 |
|---|---|---|
| `thread.new` | `project`｜`cwd`, `text?`, `attachments?`, `title?`, `engine?`, `model?`, `effort?`, `permission?`, `worktree?: bool` | 会话。按项目默认值建，参数覆盖；启动引擎；第一条消息排队，引擎能收了就发。别的引擎不继承项目默认的 model/effort。引擎的程序不在 PATH 上、文件夹没了就报错，什么都不留。`worktree: true`：先给会话建独立 worktree（§10），引擎在里面启动，`cwd` 指向它；回复要等 worktree 建好（大仓库要几秒，lilad 的主循环不被占住）。项目文件夹不是 git 仓库的根目录、在 detached HEAD、还没有提交时报错，什么都不留（引擎起不来时 worktree 和分支也会被清掉） |
| `thread.list` | `project?`｜`cwd?`, `archived?` | 会话数组，`active_ms` 倒序；不给项目就是所有项目的；`archived: true` 含归档的 |
| `thread.show` | `thread` | 会话 |
| `thread.read` | `thread`, `after?`, `limit?`(400) | 见下 |
| `thread.send` | `thread`, `text`, `attachments?` | `{sent: true, thread}`。`attachments` 是绝对路径数组，在对话里显示成卡片。Codex、omp 没有自己的 `@`：话里 `@相对路径` 指到的（文件夹里存在的）文件，lilad 也当文件交给引擎（`thread.new` 同样），但对话里只显示在话里，不再多一张卡片（`lila_agent::Attachment::is_mentioned_in`：引擎翻译成条目时去掉话里用 `@` 点了名的文件；窗口画条目时同样去掉，所以以前存下的重复也不显示了）。引擎还不能收（starting）就排队；`waiting`（有卡片或终端对话框）时拒绝，因为话会变成对它的回答；`ended` 时拒绝，要先 resume |
| `thread.interrupt` | `thread` | `{interrupted: true}`。只在 `running` 时；Codex、omp 发协议的中断，Claude 在终端按 Esc |
| `thread.stop` | `thread` | 会话。杀终端，状态变 `ended`，对话保留 |
| `thread.resume` | `thread`, `text?`, `attachments?`, `model?`, `effort?`, `permission?` | 会话。在原来的 session 上重新启动引擎（Claude `--resume`；Codex `thread/resume`；ACP `session/load`）。Claude 会话在第一条消息前就结束了的，用同一个 id 新开。`text` 作为续接后的第一条消息 |
| `thread.archive` / `thread.unarchive` | `thread` | 会话。归档会先结束引擎；worktree 会话归档时 worktree 和分支**保留**，取消归档后可以继续（`thread.resume`）。现在没有单独删除 worktree 的入口（合并是唯一会删的地方） |
| `thread.rename` | `thread`, `title` | 会话 |
| `thread.permission` | `thread`, `permission` | 会话。立即生效，不用重启，见 §6 |
| `thread.answer` | `thread`, `card`｜`permission`, `option`｜`allow`｜`dismiss` | `{thread, card, answered: true}`。回答权限卡片：`option` 是卡片选项的 `id` 或 `label`（`always` 见 §6：先把卡片说的规则加给会话，再用引擎的普通「允许」回答；这一刻不再提供就报错）；`allow: true/false` 选第一个允许或拒绝的选项；`dismiss: true`（只对 Claude）关掉卡片、让 Claude Code 在自己的终端里问。卡片已经没了（别处答过、引擎收回了）报错 |
| `thread.rule.remove` | `thread`, `rule` | `{thread, rule, removed: true, rules}`。收回一条「总是允许」（`rule` 是 `rules` 里的 `id`），它允许的以后又会问；没有这条规则报错 |
| `thread.trust` | `thread` | `{thread, trusted: true, folder}`。人信任 Claude Code 正在问的文件夹，见 §12。屏幕不是那个对话框就报错，什么键都不按 |
| `thread.model` | `thread`, `model?`, `effort?` | `{set: true}`。Codex、omp 运行中立即换（omp 只能换它列出的）；Claude 运行中拒绝（它的 `/model` 会改用户的全局默认），要先 stop 再改再 resume；ended 的会话只改存的值，下次启动用 |
| `thread.run` | `thread`, `action` | `{ran: true}`。现在只有 Codex 的 `compact` |
| `thread.commands` | `thread` | `{commands: [{name, description, kind: builtin｜skill｜command, hint, insert, action}]}`，`/` 菜单的内容 |
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
{"id": "t07b6b991", "project": "shop", "title": "…", "engine": "codex", "model": "gpt-6.1-sol", "effort": "low",
 "permission": "full", "cwd": "/abs/path", "session": "…"|null,
 "state": "idle", "status": "idle", "waiting": null, "cards": 0, "msg": null,
 "terminal": "t78"|null, "caps": {"tui": false, "gui": true, "steer": true},
 "created_ms": 0, "active_ms": 0, "started_ms": 0, "ended_ms": null, "archived": false,
 "worktree": false, "base_branch": null, "base_commit": null, "merged_ms": null, "merging": false, "restoring": false,
 "changes": {"files": 3, "add": 40, "del": 2}|null,
 "waiting_for": "trust"|null, "trust_folder": "/abs/path"|null}
```

`model`/`effort` 是引擎正在用的（Claude 取自最近一条回复的 `model`；Codex、omp 取自引擎报告），没有就是存的。`cards` 是待答卡片数。`msg` 是终端报的说明（比如 blocked 的原因）。`caps`：`tui` 能切到终端（只有 Claude），`steer` 运行中发的话能进当前这一轮（omp 不能，会排到下一轮）。

`thread.read`：

```json
{"thread": {会话}, "seq": 12, "items": [...], "permissions": [卡片], "writing": "正在写的回复"|null, "checkpoints": [检查点], "rules": [规则],
 "config": {"model": "…", "effort": "…", "default_model": "…", "default_effort": "…", "switch": true, "models": [{id,name}], "efforts": [{id,name}]}}
```

- `items`：`seq` 大于 `after` 的条目，旧到新；超过 `limit` 取最新的。同一个 `id` 的条目更新时整条覆盖并拿到新的 `seq`，所以客户端按 `id` 替换。`seq` 是返回的最大序号，下次当 `after`。条目格式是 `lila_agent::Item`：`{id, turn?, at_ms, state: running｜done｜failed, kind: user_message｜agent_message｜thought｜tool_call, …}`，用户消息带 `from: person｜local｜summary`，工具调用带 `tool`（`execute/read/edit/search/fetch/delete/…`）、`name`、`title`、`output?`、`diffs?`。
- `checkpoints`：和 `thread.checkpoints` 的 `checkpoints` 一样（§11）。窗口用其中的 `item` 把「回到这里」挂到对应的用户消息上；因为 `item` 可能比消息晚才有，窗口每次读都按这份列表重新挂，不要只看新来的条目。
- `from: "restored"` 的条目：回退留下的安静标记，`{id: "restored-<检查点>-<毫秒>", kind: "user_message", from: "restored", checkpoint: 3, undo: 7, text: "…", at_ms}`；窗口自己画「已回到这里」，`undo` 是回退前状态的检查点（可以再回退一次来撤销）。
- `rules`（`thread.show` 里同样有）：会话的「总是允许」规则，旧到新：`{id, kind: command｜edit｜tool, key, label, at_ms}`，`label` 是给人看的（`cargo test`、`改动此文件夹里的文件`），见 §6。
- `writing`：Claude 的 `MessageDisplay` 增量攒出的「正在写」，会话记录里有了回复就清掉。
- `permissions`：待答的卡片：

  ```json
  {"id": "p3", "thread": "t…", "request": "codex-12", "tool": "execute", "name": "shell", "title": "git push origin main",
   "detail": "", "command": "git push origin main"|null, "paths": [], "why": "全自动档仍需确认：推送或删除 git 分支",
   "danger": "git_push"|null, "options": [{"id": "accept", "label": "允许", "kind": "allow"}, …], "at_ms": 0}
  ```

  `danger`：`outside_workspace`｜`git_push`｜`external_action`｜`computer_use`，档位本身要问（`confirm`）时为空。选项的 `kind`：`allow`｜`allow_always`｜`reject`｜`reject_always`。**`allow_always` 只有 Lila 自己的那一个**（id `always`，`label` 写明会允许什么，如「本会话总是允许 cargo test」），见 §6；引擎自己的「总是」选项不出现在卡片上。
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

**改动摘要**（`changes` 字段）不是实时的：lilad 在这些时候重新数，**变了才**推送（`thread` 事件，`changes` 字段也带在事件里）：每一轮结束（Claude 的 Stop 和 StopFailure hook、终端状态从工作中/被拦转为空闲，Codex、omp 的 turn 结束）、`thread.commit` 发出后、`thread.merge` 之后、会话建好时、取消归档时、lilad 启动时（所有没归档的会话）。一轮进行当中不数，更不是每 100ms 跑 git；用户自己在 worktree 里改文件要等下一次触发才反映，窗口要最新的就调 `thread.changes`（它也会更新摘要）。摘要在后台一个一个数，同一个会话数的时候又被要求就在数完后再数一次。

`thread.log` 的 `kind`：`checkpoint_failed`（这一轮没打成检查点，`detail` 是原因，轮照常进行）、`restored`（`detail`：`checkpoint 3 (首行): 1 put back, 1 made again, 1 deleted; undo is checkpoint 7`）、`restore_failed`、`started`、`ended`、`resumed`、`auto_allowed`、`auto_denied`、`asked`（开了卡片）、`answered`、`dismissed`、`permission`（档位切换）、`turn_failed`、`notice`、`limit`、`commit`（要 agent 提交）、`merged`（`detail`：`2 commit(s) into main as f898670`）、`merge_failed`、`rule_added`（人点了「总是允许」，`detail` 是加的规则）、`rule_removed`、`rules_dropped`（切到只读时规则被清掉）、`trusted`（`thread.trust`，`detail` 是文件夹）。`auto_allowed`/`auto_denied` 记着档位和请求，是审计「lilad 替你答了什么」的地方；规则答的那条以 `· rule: cargo test` 结尾，点名是哪条规则；`dismissed` 也记「它的 hook 不再来问」（卡片被丢弃，见 §6）。

### 其他

| 方法 | 说明 |
|---|---|
| `ping` | `{pong, pid, version, protocol}` |
| `status` | `{pid, version, protocol, projects, threads: {state: 数量}, cards, magpie}` |
| `stop` | 停 lilad。会话照样运行（只有 socket 请求能发，spool 已经没有了） |
| `subscribe` | 见 §5 |
| `host.attach` `{thread}` | `lila host` 用：这条连接变成事件进、命令出的通道（`lila_agent::AgentEvent` / `Command`），窗口不要碰 |
| `hook` `{thread, event, payload}`、`hook.answer` `{thread, id}` | `lila hook` 用（Claude Code 的 hook），窗口不要碰 |

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

**判定**（`crates/lila/src/policy.rs`，纯函数，有测试）：三种引擎的权限请求都转成 `PermissionRequest`（工具种类、名字、命令行、路径、工作目录、选项），lilad 先按会话的档位回答（`daemon/permissions.rs::ask_permission`；Claude 的每个工具调用在 `PreToolUse` 里就判，见下），回答不了的才变成卡片；每次自动回答和开卡片都记进 `thread_events`。

全自动档下仍要问的四类，以及**能识别到什么程度**（识别靠请求里的文字，不是沙箱；Claude 线程现在没有沙箱，Codex 线程有）：

1. **改动或删除工作区以外的文件**：编辑类请求（Claude 的 Edit/Write/NotebookEdit 的 `file_path`，Codex 的 apply_patch 的文件列表，ACP 的 `locations`）按路径判断，路径会展开 `~`、`..`、符号链接，临时目录（`/tmp`、`/var/folders`、`$TMPDIR`、`/dev/null`）不算。命令行：看重定向（`> >>`）、`rm mv touch mkdir tee chmod chown ln truncate…`、`cp rsync install` 的目的地、`sed -i`，跟着 `cd`，进 `sh -c '…'`。**识别不了的**：变量拼出来的路径、脚本或程序内部写的文件（`python x.py` 里写 `/etc`）、`find -delete`、`xargs rm`、`dd of=`、`git checkout/clean` 对工作区外的仓库、通过别名或函数。
2. **`git push`、强制推送、删分支**：命令行里的 `git [-C dir|-c k=v …] push`、`git branch -d/-D/--delete`（包括 `bash -c`、`;`、`&&`、括号里的）。**识别不了的**：用变量、别名、`git` 的包装脚本、`GIT_SSH` 之类绕过；以及不是 `git` 命令的推送（`jj git push`、`gh repo sync`）。
3. **对外发出的动作：提 PR、发评论、操作 Asana**：`gh pr/issue/release/repo/… create|comment|review|merge|close|edit|…`、`gh api` 的 POST/PUT/PATCH/DELETE 或带 `-f/-F/--input`；`npm|pnpm|yarn|cargo publish`、`docker push`、`twine upload`；`curl|wget` 的写操作（`-X POST`、`-d`、`-F`、`-T`……，指向 localhost 的除外）；MCP 工具（Claude 的名字 `mcp__<server>__<tool>`）里工具名以 create/add/update/delete/merge/push/post/send/comment/close/edit/fork/assign/submit/approve/publish/move/write/upload… 开头的。**识别不了的**：没有写动词的 MCP 工具名（读写看不出）、`ssh`/`scp`/邮件、用脚本语言直接调 HTTP、Codex 的 MCP 工具（它的审批是 MCP elicitation，目前直接拒绝，不问人）。
4. **computer use**：工具名含 `computer` 且是 MCP 工具或以 `computer` 开头的。**现在没有这样的工具**（Lila 还没提供，三种引擎自带的也没有），所以这条没有实测过。

### 各引擎怎么落实

- **Claude Code**：始终以 `--permission-mode manual` 启动，**每个工具调用在跑之前**先走 `PreToolUse` hook（`daemon/hooks.rs::pre_tool_use`），lilad 按会话的档位（`policy::decide`）判定，所以切档不用重启（实测：`lila thread permission` 之后的下一个工具调用就按新档位）：
  - **allow**：Claude Code 不再问。这个 allow 也盖过 Claude Code 的「命令里有变量，没法事先检查」之类的确认，但盖不过它对「工作目录以外的路径」的检查（那个框照样弹，到 `PermissionRequest`，见下）。
  - **deny**（带文字）：工具不跑，文字回到 agent（Claude Code 在对话里把它显示成「PreToolUse:Bash hook error: …」）。
  - **ask**：Claude Code 弹自己的确认框，弹框触发 `PermissionRequest` hook，lilad 在那里开卡片；人的回答走原来的 `hook.answer`（hook 每 0.5 秒问一次，**不设上限**地等，卡片不过期，见下「卡片不过期」；只有卡片被 `dismiss` 时 hook 什么都不输出，Claude Code 在终端里问，会话变 `waiting`/`terminal`）。`PreToolUse` 里不等人。
  - 用户自己配的 allow 规则（实测：项目 `.claude/settings.json` 里的 `Bash(touch:*)`、`Edit`、`Write`）**不再绕过 lilad**：hook 的 deny 和 ask 都压过 allow 规则（`read_only` 档里 `touch x` 被拒；`confirm` 档里照样出卡片）。在手写 hook 加开着沙箱的会话里也验证过 ask、deny 压过 `autoAllowBashIfSandboxed`。
  - 两个 hook 的负载（2.1.322 实测）：`PreToolUse` 有 `session_id, transcript_path, cwd, scratchpad_dir, prompt_id, permission_mode, effort, hook_event_name, tool_name, tool_input, tool_use_id`；`PermissionRequest` 有同样的，**没有 `tool_use_id`**，而且 `permission_suggestions` 有时有（空数组）有时没有。所以两个 hook 靠 `tool_name` 加 `tool_input` 的哈希对上：`PreToolUse` 把判定记下（每个会话最近 64 个、10 分钟内），后到的 `PermissionRequest` 取走：判定是 ask 就直接开卡片（不再按档位判一次，日志里只有一条 `asked`）；判定是 allow（Claude Code 自己还要问，比如工作目录以外的路径）就再放行一次，不再记日志；对不上（没有 `PreToolUse`：旧会话、lilad 当时没起来；输入被别的 hook 改过）就照旧按档位判，行为和以前一样。
  - 日志（`thread_events`）：看东西的工具（`policy::reads`：读、搜索、取网页，Claude 自己的待办、提问、`ToolSearch` 这类 `INERT` 里的）放行不记；其他自动回答每个调用记一条（`auto_allowed`/`auto_denied`），出卡片时记 `asked`。`PreToolUse` 看得到所有调用，包括 Claude Code 本来不问的，所以 `confirm` 档下不在 `reads` 里的新工具会出卡片、`read_only` 档下会被拒；Claude Code 新增了无害的内部工具时，要加进 `policy::INERT`。
  - 速度：`lila hook PreToolUse` 一次往返（含起进程）约 4.6 毫秒，`lila --version` 本身 3.7 毫秒；200 KB 的 Write 约 8 毫秒。lilad 这一侧没有 git、没有读盘，只查一次会话、必要时写一条日志。
  - lilad 没起来或者回复读不了：hook 什么都不输出、退出码 0，Claude Code 自己决定（这时用户的 allow 规则又起作用）。
- **Codex**（`codex app-server`）：按档位给 `thread/start` 的 `approvalPolicy` 和 `sandbox`，每次 `turn/start` 也带上（所以切档在下一轮生效，运行中的这一轮不变）：

  | 档 | approvalPolicy | sandbox |
  |---|---|---|
  | `read_only` | `on-request` | `read-only`（它请求越出沙箱的，lilad 拒绝） |
  | `confirm` | `untrusted` | `workspace-write`（实测：写文件的 shell 命令出卡片） |
  | `full` | `on-request` | `workspace-write`，不开网络 |

  `full` 的做法：沙箱本身不让写工作区外、不让联网，所以 `git push`、`gh pr create` 这些要靠「请求越出沙箱」才能执行，请求到 lilad 手里按上面四类判定（实测：`git push` 出卡片）。代价是装依赖、拉代码这类联网命令也会多一轮「请求越出沙箱」，由 lilad 自动允许。Codex 的 `item/permissions/requestApproval`（模型主动申请额外权限）目前直接回「不支持」，不问人。
- **ACP（omp）**：omp 对执行、编辑、删除、移动类工具发 `session/request_permission`，全部到 lilad，按档位判定，切档立即生效；读类不问。

### 卡片不过期，lilad 重启也在（A2）

- Claude Code 2.1.322 对 hook 的 `timeout`（设置里的秒数）取 `min(timeout × 1000, 2^31−1)` 毫秒（二进制里的 `Math.min(…, 2147483647)`，约 24.8 天，计时器能放下的最大值），默认 10 分钟。实测：`timeout: 99999999` 不会被立刻杀（没有溢出），一个 20 秒的 hook 照常跑完。所以 `PermissionRequest` 的 `timeout` 写 **2 000 000 秒**（`hooks::PERMISSION_WAIT_SECS`，约 23 天），`lila hook` 自己没有等待上限，等到人答、卡片被 `dismiss` 或 Claude Code 把它杀掉。端到端实测：`confirm` 档下一张卡片放了 11 分钟再答，hook 还在，命令照常执行。不需要「到点先回 deny」的办法。
- 只有新启动或 `resume` 的会话有新的 `timeout`；已经在跑的会话的 settings 文件里还是 600 秒，要结束再恢复。
- **lilad 重启**：等着的 hook 进程还活着，继续每 0.5 秒问。新的 lilad 不认得这个请求 id，`hook.answer` 回 `{"decision": null, "unknown": true}`；hook 于是带上原来的 `payload` 再问一次（只在 lilad 说不认得时才带，平时不传几百 KB 的写入内容），lilad 把它当作新的 `PermissionRequest` 重新判（`permission_hook(…, again: Some(id))`）：按档位重新答或重开卡片（卡片又出现，判的是现在的档位，所以重启期间切了档就按新档位）。会话已结束就不重开。hook 的请求 id 带 lilad 的启动时刻（`hook-<boot>-<n>`），重启前后的不会撞。lilad 不在时 hook 的请求失败，它继续问到 lilad 回来为止。旧版 `lila hook`（没有这套）遇到 `unknown` 照旧退到终端。实测：卡片开着时 `lila daemon restart`，6 秒内卡片带着同样的内容回来，答了之后命令执行。
- **卡片跟着 hook 走**：每次 `hook.answer` 记下这张卡片最后被问的时间；超过 30 秒没人问（Claude Code 因为人在终端里答了、或被中断而杀了 hook），卡片就丢掉并记 `dismissed`；这一轮结束（`Stop`、`StopFailure`）时 Claude 线程的卡片也直接丢。线程结束（`thread_gone`）照旧清掉所有卡片。

### 本会话总是允许（A2，`crates/lila/src/rules.rs`）

`confirm` 档下每个命令和每次改动都是卡片，长任务没法用。卡片多一个选项，点了就把一条或几条**规则**记在会话上（`thread_rules`，lilad 重启、`thread.resume` 之后还在），之后同样的请求由 lilad 直接放行，不再出卡片。

**规则的形状**（简单、可预期）：

| 种类 | 内容 | 例 |
|---|---|---|
| `command` | 命令的**程序**，对按子命令用的工具（`git cargo npm pnpm yarn bun docker kubectl gh brew pip pip3 uv go make just rustup`）再加第一个子命令词 | `cargo test`、`git commit`、`npm run`、`ls`、`touch` |
| `edit` | 会话文件夹里的改动：改文件的工具（Claude 的 Edit/Write/NotebookEdit，Codex 的补丁，ACP 的编辑），所列的文件都在文件夹里（且列了文件） | 「改动此文件夹里的文件」 |
| `tool` | 一个 MCP 工具，名字完全一样（`mcp__…`） | `mcp__github__get_pull_request` |

- 程序按**写出来的词**比：`cargo` 不是 `/usr/bin/cargo`，`cargo-evil` 不是 `cargo`；没有「更宽」的规则（不按前缀、不按别名、不按路径）。子命令词必须是普通的小写词（`cargo +nightly test`、`git -C x status`、`git --no-pager log` 不成规则，因为那个词是选项）。
- **一行里串了几个命令**（`;`、`&&`、`||`、`|`、`&`、换行；用 `policy::split_commands` 切）：只有**每一段都有规则**才放行。卡片上的选项把**还没有规则的**那几段一起列出（`本会话总是允许 cargo test、tail`，最多 3 段，更多就不提供），点一下加几条规则。`cd`/`pushd`/`popd` 不算命令。
- **不提供、也永远不按规则放行**：包装器（`sudo env command time nohup nice timeout xargs watch exec eval source bash sh zsh …`，含 `bash -c`）、解释器和包运行器（`python node deno ruby perl php lua osascript awk npx bunx uvx pipx`）、`find ssh scp nc tmux screen open`、名字就说明会动文件的（`rm rmdir dd shred mkfs kill killall pkill chmod chown chgrp trash unlink`）、以 `VAR=值` 开头的行、命令替换（`$(…)`、反引号、`<(…)`）、here-document、子 shell 和 `if/for/while`、用变量或通配符当程序的、一行里有任何一段属于以上的；子命令里会跑任意东西的也不行（`npm exec`、`pnpm dlx`、`docker run/exec/compose`、`kubectl exec/run`、`bun x`、`uv tool`）。
- **危险永远不被规则吞掉，也不提供**：`rules::derive` 和 `rules::covers` 先问 `policy::danger`（工作区外、`git push`/删分支、对外动作、computer use），有就一律 `None`。这个检查是对**请求本身**做的，不依赖档位怎么问的：`confirm` 档里 `git push` 的卡片 `danger` 是空的（档位问的），但它照样没有「总是」选项、不会被 `git` 之类的规则放行（实测、有测试）。`full` 档的危险卡片（`danger` 非空）同样没有。只有 `confirm` 档、`danger` 为空、且规则能把「会允许什么」说准的卡片才有这个选项。
- **选项**：id `always`，`kind: allow_always`，标签写明从现在起允许什么（`本会话总是允许 cargo test`、`本会话总是允许改动此文件夹里的文件`）。放在「允许」之后、「拒绝」之前。点的时候 lilad 再算一遍（档位、规则可能变了），不再提供就报错。答的时候先加规则、记 `rule_added`，再用引擎自己的普通「允许」选项回答，所以对 Claude 就是一个 allow；同一会话里这时还开着的、被新规则答得了的别的卡片一起答掉（Claude 常一次发好几个调用）。
- **三个引擎怎么合成「一个总是」**：Codex 的 `acceptForSession`、ACP 的 `allow_always`/`reject_always` 的含义是引擎自己的（范围 Lila 说不出，也收不回、重启就没了），所以**卡片上一律不出现**，由 Lila 的 `always` 取代；点了之后引擎收到的是普通的 allow，下一次同样的请求还是先到 lilad，由规则答，日志里点名规则。引擎自己的「总是」会让引擎自己吞下以后的危险请求（比如 `git push`），所以危险卡片上也一并去掉。没有普通 allow 选项的请求（理论上有）保留引擎的选项不动。Codex、omp 只有假 host 的单元测试，没有真跑。
- **答的地方**：`ask_permission`（Codex、ACP、Claude 的 `PermissionRequest`）和 `pre_tool_use`（Claude 每个调用）在档位判成 `Ask`、`danger` 为空、档位是 `confirm` 时先看规则；规则放行也记 `auto_allowed confirm · Bash: cargo test --lib · rule: cargo test`。`full` 档里规则不起作用（保留着）；**切到 `read_only` 时规则全部清掉**（记 `rules_dropped`），再切回 `confirm` 也不回来。
- 窗口里规则列在权限档菜单里，每条一行点一下收回（`thread.rule.remove`）；命令行 `lila thread rules <id> [--remove N]`。
- **实际上还用不上的**：`tool` 规则现在不会被提供：档位在 `confirm` 下问的 MCP 工具只有名字带写动词的，它们本来就是「对外动作」的危险；不带写动词的 MCP 工具档位当作读，直接放行。规则本身有单元测试，等 P3 接了档位分不清的 MCP 工具才派上用场。

### Claude Code 自带的沙箱（A1 调查，2026-11-11，**没有启用**）

打算给 Claude 线程开它的 Bash 沙箱（写只限会话文件夹和临时文件夹，网络不限）。对着 2.1.322 实测的结果：**「网络不限」用设置表达不了，所以停在这里，没有改代码**。下面的实验用的是手写的 hook 脚本（记负载、按文件回判定）和手写的 `--settings`，不是 lilad 的 hook；用的是临时 git 项目和 Haiku。

- 设置：`--settings` 文件里 `sandbox.enabled: true` 生效（`claude --settings f sandbox status` 能看到，不花额度）。文件系统规则（`sandbox.filesystem.allowWrite`）和代理端口**只在会话启动时读**，改了文件对运行中的会话不起作用（所以「运行中的线程保持旧设置」成立）。
- **网络**：沙箱里的命令走 Claude Code 自己的出口代理，按域名放行。`sandbox.network` 里没有「全部放开」的开关（键只有 `allowedDomains`、`deniedDomains`、`strictAllowlist`、`allowManagedDomainsOnly`、`allowUnixSockets`、`allowAllUnixSockets`、`allowLocalBinding`、`allowMachLookup`、`httpProxyPort`、`socksProxyPort`、`tlsTerminate` 等）。`allowedDomains` 不接受 `*`、`*.com` 这类过宽的写法（二进制里的校验文字：「Overly broad patterns like "*.com" or "*" are not allowed」；实测 `["*"]` 和 `["*.org","*.com","*.io"]` 都不起作用，也不报错），`permissions.allow` 里的 `WebFetch`、`WebFetch(domain:*)` 同样不行。没列出的域名第一次访问时，Claude Code 在终端里弹「Network request outside of sandbox」框（选「是」「是并且不再问这个域名」「否」）。**这个框不是工具权限框，不触发 `PermissionRequest` hook**，lilad 看不到：会话变成 `blocked`，窗口只会显示「在终端里等你」。
- 实测唯一做到网络不限的办法：`sandbox.network.httpProxyPort` 指向一个自己跑的、放行一切的本地 HTTP 代理（会话启动时读）。之后不再弹框，`curl`、`cargo build`（取 index.crates.io、static.crates.io）都通。代价：lilad 要自己托管一个代理进程和端口，这是新增的组件，也是个本机上谁都能用的开放代理，所以没有做。其他选择：列一个域名清单（不是「不限」）；沙箱只管写文件，联网的命令走 `excludedCommands`；不启用沙箱。等用户定。
- 文件：沙箱开着、没有别的配置时，Bash 只能写会话文件夹和 `$TMPDIR`（`/tmp/claude-501`）；`/tmp` 本身和 HOME 下别处写入被拒（`Operation not permitted`）。`allowWrite: ["/tmp","/private/tmp",…]` 重启后 `/tmp` 可写；`CARGO_HOME=/tmp/x cargo build` 取依赖并编译一个带依赖的 hello-world 通过（取依赖要代理放行）。worktree 里的 `git commit` **不用额外放行主仓库的 `.git`**（Claude Code 自己认得 worktree），提交到了主仓库。
- **Edit/Write 工具不经过 OS 沙箱**：手写 hook 放行时，`Write` 写会话文件夹以外的文件成功。所以这类写入只靠 lilad 的路径规则（全自动档出 `outside_workspace` 卡片，用 lilad 实测过）。
- 沙箱和 hook：`PreToolUse` 的 ask、deny 都压过 `autoAllowBashIfSandboxed`（默认 true）；allow 让沙箱里的命令不再有任何确认框。
- 沙箱拦下命令后：Claude 先在沙箱里跑，得到 `Operation not permitted`；要它「被拦就关沙箱重试」时，它用 `dangerouslyDisableSandbox: true` 再调一次 Bash（没这么说时它只是告诉人被拦了，不自己重试）。`PreToolUse` 看得到这个字段（在 `tool_input` 里，和 `command`、`description` 并列）。**即使 `PreToolUse` 回 allow，Claude Code 照样弹确认框**（触发 `PermissionRequest`），所以人的答复能走现有的卡片；答 allow 之后命令在沙箱外跑成功，答 deny 走的是和别的请求一样的通道（没有单独验证）。

## 7. 命令行

`lila` 是窗口之外的另一个客户端，只走同一个 socket：

```sh
lila up                              # 启动 lilad，把当前文件夹加成项目
lila project list|add [路径] [--name --title]|remove [项目]|show [项目]|set [--engine --model --effort --permission --title]
lila thread new "第一条消息" [--project --engine --model --effort --permission --title --file F --worktree]
lila thread list [--project|--all] [--archived]
lila thread show <id> [--after N]    # 状态、对话、待答的卡片
lila thread send <id> "话" [--file F]
lila thread interrupt|stop|archive|unarchive <id>
lila thread resume <id> ["话"] [--model --effort --permission]
lila thread rename <id> <标题>
lila thread permission <id> read_only|confirm|full
lila thread answer <id> <卡片> allow|deny|always|dismiss|<选项>
lila thread rules <id> [--remove N]    # 本会话「总是允许」的规则；--remove 收回一条
lila thread trust <id>               # 信任 Claude Code 在终端里问的文件夹（只在屏幕上正是那个对话框时按键）
lila thread model <id> [--model --effort]
lila thread changes <id>             # 分支、提交（worktree）、改动的文件和行数
lila thread diff <id> [路径]          # 改动的 diff（一个文件或全部）
lila thread commit <id>              # 让 agent 提交（它写信息，不推送）
lila thread merge <id>               # worktree 会话合并回起点分支，删 worktree，归档
lila thread checkpoints <id>         # 检查点：编号、时间、HEAD、它之前那条消息的首行
lila thread restore <id> <编号> [--dry-run]   # 文件放回到那个检查点；--dry-run 只列出会改哪些文件
lila thread log <id>
lila daemon run|stop|status|restart  # status：协议和窗口的不一样时说怎么办，退出码 3 没有运行、4 协议不一样；restart：停、等 socket 没了、再启动（会话不断）
lila usage [--days N]                # 按会话统计 token（用数据库里的 session id 对会话记录文件）
lila accounts                        # 订阅账号和额度
lila --json …                        # 任何命令都能输出原始 JSON
```

`LILA_HOME=/tmp/x lila …` 用独立的库、socket 和状态文件夹，真跑测试用。

## 8. 代码

| 位置 | 内容 |
|---|---|
| `lila-core` | `config.rs`（`Runtime`、项目默认值、project.toml）、`thread.rs`（`Tier`、`ThreadState`、`AgentStatus`、`Thread`）、`runtime.rs`（各引擎的启动参数、`codex_settings`） |
| `lila-store` | SQLite（§1） |
| `lila-agent` | 对话事件模型和三个驱动（不变），`PermissionRequest` 多了 `name/command/paths/cwd`，选项多了 `kind`，命令多了 `SetPermission` |
| `lila/src/daemon/` | `mod.rs`（状态、hc 事件、状态计算）、`threads.rs`（项目和会话的生命周期、投递）、`permissions.rs`（卡片、`thread.answer`）、`conversation.rs`（读对话、发消息、菜单）、`host.rs`、`hooks.rs`、`requests.rs`（方法分派）、`server.rs`（socket、推送）、`sources.rs`（留给 P4 的 GitHub 任务，没人调用） |
| `lila/src/daemon/jobs.rs` | 放在主循环外跑的活（§4 末尾、§10）：`Job`、`Daemon::begin`（请求要么直接答，要么给一个 job）、`finish`（job 的结果变成回复和后续）、改动摘要的刷新 |
| `lila/src/worktree.rs` | 会话的 worktree：建（`create`）、删（`remove`）、git 命令行的小工具 |
| `lila/src/changes.rs` | 改动列表、diff、读文件（`thread.changes/diff/file`） |
| `lila/src/merge.rs` | squash、放到起点分支最新提交之上、项目文件夹 `--ff-only`（`thread.merge`） |
| `lila/src/checkpoint.rs` | 检查点的 git 工作（§11）：打快照、放回、删 ref |
| `lila/src/daemon/checkpoints.rs` | 什么时候打、检查点怎么对上对话里的消息、回退的前置条件和后续 |
| `lila/src/policy.rs` | 档位判定（§6） |
| `lila/src/rules.rs` | 「本会话总是允许」的规则：怎么从请求算出规则、哪些请求被规则答、标签（§6） |
| `lila/src/daemon/trust.rs` | 认出 Claude Code 的「信任此文件夹」对话框，`thread.trust`，每 2 秒看一次屏幕（§12）；`daemon/fixtures/` 是 2.1.322 真实屏幕文字 |
| `lila/src/project.rs` | project.toml 的读写 |
| 留着、没人调用的 | `github.rs`、`daemon/sources.rs`（P4）；它们不依赖任务和流程，能独立编译、有测试 |

## 9. 窗口

`lila-app` 只通过上面的方法和推送工作，不读数据库、不起引擎进程；界面规范见 `DESIGN.md`。

| 窗口里的操作 | 用到的接口 |
|---|---|
| 侧栏的项目和会话；颜色、转圈、红底 | `project.list`、`thread.list {archived: true}`（一次拿全，归档的在窗口里过滤；`state`、`waiting`、`active_ms`、`archived`），`subscribe` 的任何推送都触发重拉 |
| 添加项目 | `project.add {cwd}`（`autostart`），然后窗口显示它的新会话输入框 |
| 新会话：输入框、选引擎模型努力程度权限档、发出第一条消息 | 项目默认值来自 `project.list` 的 `defaults`；输入框里的 `@` 用 `thread.files {project}`；发出时 `thread.new {project, text, attachments, engine, model?, effort, permission}`（窗口总是传 engine、effort、permission；model 为空就不传，由 lilad 补引擎的默认）。成功后显示返回的会话 |
| 对话：消息、权限卡片 | `thread.read {after}`（`items`、`permissions`、`writing`、`config`），`thread.send`，`thread.answer`（`option`；Claude 另有 `dismiss`：「在终端里回答」） |
| 引擎卡在终端对话框 | `thread.show` 的 `waiting == "terminal"`：输入框上方出现「打开终端」，切到 Claude 的 hc 终端（窗口直接连 hc） |
| 标题行：权限档、中断、结束、继续 | `thread.permission`、`thread.interrupt`、`thread.stop`、`thread.resume` |
| 会话右键：重命名、结束或继续、归档或取消归档 | `thread.rename`、`thread.stop`/`resume`、`thread.archive`/`unarchive` |
| 旧 lilad 提示（侧栏最后一行，设置按钮右边） | `ping` 的 `version`/`protocol`（每次刷新都取）；协议和窗口的不一样时出一行「lilad 版本不符」加「重启」按钮：在后台线程里 `lila::client::restart_daemon`（`stop`、等 socket 没了、`start_daemon`），然后重取。重启后还不一样（磁盘上的 `lila` 本身是旧的）就改成「lila 命令需要重装」，不再给按钮。全局设置 → 关于的 lilad 一行写版本和协议 |
| 项目设置（⇧⌘,） | `project.set {project, title, defaults}`；`model`/`effort` 传空字符串表示清掉 |
| 输入框的 `/` 菜单、会话内换模型 | `thread.commands`、`thread.run`、`thread.model` |
| 移除项目 | `project.remove` |
| 右面板：文件、改动（P2） | 标题行按钮和 ⇧⌘E / ⇧⌘G 开关，宽度、视图记在 `state/app-panel.json`。文件树 `thread.files`，预览 `thread.file`；改动 `thread.changes`、点开一行 `thread.diff {path}`；提交 `thread.commit`，合并 `thread.merge`（确认后；成功就离开会话）。重新读的时机：打开、切视图、换会话、推送里该会话的 `changes` 摘要或 `state` 变了、提交或合并之后 |
| 新会话选独立 worktree | `project.list` 的 `repo` 为真才有开关；发出时 `thread.new {…, worktree: true}`（回复要等 worktree 建好，窗口在后台等，发送按钮转圈） |
| 每轮的「回到这里」（A1，`restore.rs`） | `thread.read` 的 `checkpoints`（窗口每次读都整份换掉，按 `item` 对上用户消息）。一条 `from: person` 的消息有按钮，要同时：有 `kind: turn`、`item` 对得上、`restorable` 为真的检查点；会话 `idle`/`ended`、没在合并或回退、没合并过；`ping` 的 `protocol` ≥ 2。点了先 `thread.restore {dry_run: true}`（后台线程，按钮转圈），回复的 `counts`、清单、`truncated`、`head_moved` 放进确认框；全是 0 就只说「文件已经是这个样子」，没有可确认的。确认后 `thread.restore`（后台线程；窗口记在 `restore.going`，lilad 的 `restoring` 也算，这期间对话末尾转圈、输入框不发送）；回复后不论成败都 `refresh_now` 并重读右面板（失败也读：可能做到一半）。失败的文字原样放进窗口顶部的红色提示条（和别处操作失败一样，可关）。条目 `from: "restored"` 画成「已回到这里」一行；`undo` 对应的检查点还在列表里且 `restorable` 时带「撤销」，走同一条 dry run、确认、回退的路。窗口不调 `thread.checkpoints`，也不读 `can_restore`/`why_not`（`thread.read` 里没有；每行的 `restorable` 够用） |
| 权限卡片的「总是允许」（A2） | 卡片选项由 lilad 给，窗口照画（`ui::permission_card`）：第一个 allow 是主按钮，其余 outline，所以「本会话总是允许 cargo test」是第二个 outline 胶囊；标签超过 44 字截断、tooltip 是全文；窄时按钮折行。点了 `thread.answer {option: "always"}`。旧 lilad 不给这个选项，卡片和以前一样 |
| 会话的规则（A2） | 权限档菜单（输入框工具条里的权限按钮）：三档下面一条线，每条规则一行「不再总是允许 cargo test」加 × 图标，点一下 `thread.rule.remove`。数据来自 `thread.read` 的 `rules`（`Conv.rules`，每次读整份换）；新会话的输入框没有规则 |
| 引擎在终端里等「信任此文件夹」（A2） | `thread.show` 的 `waiting == "terminal"` 且 `waiting_for == "trust"`：输入框上方那一行写「信任此文件夹」和文件夹（`trust_folder`，长了留结尾），右边「打开终端」（outline）和主按钮「信任并继续」→ `thread.trust`（后台，按钮转圈；失败的话是顶部红色提示条）。别的终端等待、旧 lilad：仍是「Claude Code 在终端里等你」加主按钮「打开终端」 |
| 已合并的会话 | `merged_ms` 有值：输入框换成「已合并到 <base_branch> · <sha>」，sha 从 `thread.log` 里 `merged` 那条的 `detail`（`… as f898670`）取 |

- 窗口记着：上次显示的会话和项目、哪些会话切成了终端视图（`state/app-tabs.json`、`app-views.json`）。折叠的项目、归档开关不记。
- 系统通知在窗口里发：对比前后两次 `thread.list`，会话变成 `waiting`、或从 `running` 变成 `idle`，而窗口不在前面时，用 `osascript` 发一条。
- 截图自查：`--features shots`，环境变量见 `docs/HANDOFF.md`；`LILA_APP_FAKE=<json>` 可以不起 lilad，用造好的项目、会话和对话画界面。

## 10. worktree 和合并（P2）

- **建**：`thread.new {worktree: true}`。项目文件夹必须是 git 仓库的**根目录**（`worktree::is_repo_root`：仓库里的子文件夹、整个 home 在 git 里都不行）、在某个分支上、已有提交。worktree 在仓库外：`<data>/worktrees/<项目>/<会话 id>`（`Paths::worktree`，`LILA_HOME` 下就在它的 `data/` 里），分支 `lila/<会话 id>`，从项目文件夹当前分支的 HEAD 建（`git worktree add -b`）；记下 `base_branch` 和 `base_commit`。引擎在里面启动：`cwd`、终端的工作目录、`LILA_ROOT` 都是它，权限档判定的「工作区」也是它（所以 worktree 会话动项目文件夹里的文件会被当成工作区外）。
- **留**：归档、结束（`thread.stop`）都不动 worktree；`thread.resume` 在同一个 worktree 里继续（worktree 的文件夹被手工删了会报错）。项目被移除（`project.remove`）也**不**删它的 worktree 和分支。没有单独删 worktree 的入口，`thread.merge` 是唯一会删的地方。
- **合并**（`merge::run`，在主循环外）：分支上相对 `base_branch` 的全部提交（`base_branch..HEAD`）压成一个提交——只有一个提交就原样用它；多个就用会话标题当第一行、正文逐条列各提交的标题（`- 标题`）——放到 `base_branch` 最新提交之上（`git rebase`），再在项目文件夹里 `git merge --ff-only`，所以项目文件夹的分支只是向前移动；然后会话的引擎结束、会话归档并记 `merged_ms`，worktree 和分支由另一个后台 job 删掉（失败记 `notice`，不影响合并的结果）。`git commit`（压缩时）和 rebase 照常跑用户的 hook 和 git 配置。开始改动 worktree 之前先查完所有会失败的条件（见 `thread.merge`）；rebase 冲突、commit hook 拒绝、ff 失败时 `git reset --hard` 回合并前的提交、`rebase --abort`，worktree 的分支和项目文件夹都是原样。ff 时 `base_branch` 刚好又前进了就重新 rebase 再试（最多重试 3 次）。
- **已知的局限**：worktree 里只有 git 跟踪的文件，被忽略的（`.env`、`node_modules/`、`target/`）不会带过去，引擎在里面可能跑不起来，目前不处理；Claude Code 在没信任过的仓库里第一次开 worktree 会话可能先问「信任这个文件夹」（现在窗口里可以答，§12；项目文件夹已经信任过的，实测 worktree 不再问）；`.lila/project.toml` 没提交的话 worktree 里没有它。

## 11. 检查点和回退（A1）

用户定的：**只有文件回去，对话不动**（不去回滚引擎的会话）。会话默认全自动，agent 改坏了要有退路。

### 怎么打

快照是一次不动用户仓库状态的提交（`checkpoint.rs`，git 命令行，不用 libgit2）：把用户的 index 复制一份（`GIT_INDEX_FILE`，复制是为了沿用它的 stat 缓存，大仓库快）→ 在副本上 `git add -A -- .` → `git write-tree` → `git commit-tree`（父提交是 HEAD，还没有提交就没有父提交；作者固定 `Lila <lila@localhost>`，关掉签名）→ `git update-ref refs/lila/checkpoints/<会话>/<n>` 留住它。**HEAD、分支、用户的 index、stash、工作区都不动**（测试里比较了 `git status --porcelain=v2`、HEAD、分支、stash、index 文件的字节）。

- **范围是会话的 `cwd`，不是整个仓库**：`cwd` 在大仓库的子文件夹里时，只存、也只回退那个子文件夹（`git add -A -- .`；回退时 `diff-tree --relative`）。理由：`cwd` 是权限判定的「工作区」，回退会删文件，不能越出它。树里 `cwd` 以外的部分是用户 index 里原有的，不会被回退碰。
- 内容：已跟踪文件当前的样子（含未提交、已暂存的改动）、没被忽略的新文件；已删除的不在里面。`.gitignore`（含全局的）忽略的文件不进快照，回退也不碰。
- 没有变化：和该会话上一个检查点的树、HEAD 都一样，就用上一个的提交；文件夹正好等于 HEAD 的树，就用 HEAD 本身（不写任何对象）。每一轮仍各有一行记录和各自的 ref，所以每轮都能回。
- `cwd` 不在 git 仓库里（或会话已合并）：不打，不记录，不拖慢（`maybe_repo` 往上找 `.git`，不起进程，没有就跳过）。
- **什么时候打**（要求：在引擎对这一轮的消息动文件之前；lilad 主循环不被 git 卡住；打不成不拦消息）：
  - **Claude Code：在 `UserPromptSubmit` hook 里**（`hooks.rs` → `prompt_begin`）。Claude Code 会等 hook 返回才开始这一轮，所以快照一定在它动文件之前；窗口发的、Lila 自己发的（提交的备注），或用户在 Claude 自己的终端里打的，都走同一个点，不会打两次。`deliver` 对 Claude 不打。hook 的答复要等 job 做完才回（和 `thread.merge` 一样，job 在线程里跑）；hook 超时从 10 秒改成 30 秒（只对新启动或 `resume` 的会话生效，旧会话的 settings 文件里仍是 10 秒，超时了 Claude 照常往下走，快照可能含这一轮开头的一点改动）。
  - **Codex、omp：在 `deliver` 里**，话发给 host 之前：话留在 outbox 里，job 做完才发（`snapshotting`）。只在引擎空闲（`idle`/`done`）、这些话会开始新一轮时打；Codex 在跑的时候发的话进当前这一轮（steer），不打。omp 运行中收不了话，所以 omp 的话现在留在 lilad 的 outbox 里，等它空闲再发（以前是先交给 host 排队），好让快照是下一轮之前的。
  - 失败（git 不能跑、index 有未解决的冲突……）：记 `thread_events`（`checkpoint_failed`），消息照发。
- 数量：每个会话保留编号在「最新编号 − 50」之后的，更老的删 ref **也删记录**（编号不重用）。会话合并后（worktree 删掉时）、项目被移除时，删它们所有的 ref（一个后台 job；ref 在共享的 `.git` 里，worktree 会话的也由主仓库的路径删）和记录。

### 检查点和对话里的消息怎么对上

引擎给条目的 id 各不相同，Claude 的还要等 0.4 秒后从会话记录里读到，所以检查点一开始没有 `item`。lilad 在每次 `thread.read` / `thread.checkpoints` 时（`resolve_checkpoints`）给还没对上的检查点找消息：按编号从旧到新，取**从它的时间（宽限 3 秒）起、还没被别的检查点用掉、说的话一样的第一条** `from` 为 `person` 或 `lila` 的用户消息；对上就写进记录，之后不变。「话一样」比较的是去掉 `<lila note>` 和粘贴包装、压缩空白之后的开头（Claude 会把附件当 `@路径` 加在后面，所以允许一边是另一边的开头）。同样的话说两遍，各自对上各自的。一个检查点的消息一直没出现（Claude 拒绝了那条、`/` 命令），等后面的检查点对上之后它就被放弃（`item` 记成空，对外是 `null`），它仍然能回退。窗口永远是拿 `thread.read` 里的 `checkpoints` 列表按 `item` 去对条目，不是在条目上找。

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
3. 给 agent 排一条英文备注（`outbox` 里 `hold = 1`）：哪一轮之后的改动被撤销了、几个文件、HEAD 没动，让它再改文件前先重新读。**不开新的一轮，不花 token**：`hold` 的备注自己不会被发出，等人的下一条消息时放在它前面合成一条消息发出（`deliver_with`）。文件和检查点一模一样（没有差别）时不排备注。备注+人的话在引擎的回显里会被各个翻译器（Claude 会话记录、Codex、ACP）认出来，只显示人的话：`lila_agent::without_notes`；
4. 重新数改动摘要。

### 已知的局限

- 只管文件：对话、引擎的内部状态、已经执行过的命令的副作用（装的依赖、数据库、网络请求）都不回。
- 被忽略的文件不管：`node_modules/`、`target/`、`.env` 这类如果被 agent 改了，不会回来。一个文件在检查点时没被忽略、现在被忽略了（`.gitignore` 变了），会被检查点里的内容覆盖。
- 范围是会话的 `cwd`；`cwd` 以外的改动不回。仓库里嵌套的仓库（子模块、自己有 `.git` 的文件夹）在快照里只是一条记录，里面的文件不管。
- 打快照是 `git add -A`：大仓库首次要几秒（之后沿用 index 的 stat 缓存）；没被忽略的巨大文件夹（几 GB 的 `node_modules` 没写进 `.gitignore`）会变慢并占对象库。没有大小上限。
- `refs/lila/checkpoints/*` 会出现在 `git log --all`、gitk 里；`git push --mirror` 会把它们一起推（`git push`、`--all` 不会）。
- 回退只更新工作区的文件，**不更新用户的 index**：回退之前暂存的内容、回退后 index 里还是原来那份。
- Claude 的 hook 超时 30 秒（旧会话 10 秒）：快照超时时 Claude 不等，这一轮可能已经开始动文件。Claude 运行中排队的消息会不会也触发 `UserPromptSubmit` 没有验证；如果会，那只是多一个检查点（无害，没变化就复用同一个提交）。
- 回退进行中（几秒）用户在 Claude 自己的终端里提交了消息，这一轮不打检查点（记 `checkpoint_failed`）。

## 12. 在窗口里回答「信任此文件夹」（A2）

Claude Code 在没信任过的文件夹里（新项目）先在终端里问「信任此文件夹」，这时它不报任何状态（`hc` 的 `status` 为空、`reported: false`），会话以前要等 20 秒才变成「在终端里等你」。

- **认出**：`trust::recognise` 读 `terminal.read` 的屏幕文字（2.1.322 的真实文字存在 `daemon/fixtures/claude-trust-2.1.322.txt`，登录屏幕的存在 `claude-login-2.1.322.txt` 当反例）。要求同时有：一行 `Accessing workspace:`、一行 `Quick safety check…`、紧挨着的两行选项 `No, exit` 和 `Yes, I trust this folder`（光标 `❯` 在其中恰好一个上，启动时在 `No, exit`）、一行以 `Enter to confirm` 开头；文件夹是前两行之间的文字（路径长了会折行，拼回去）。缺一样、顺序不对、多出别的字，都认不出，那时一切照旧。
- **看的时机**：每 2 秒（`Input::Look` → `Daemon::look`）读一次**状态是 `starting` 或 `waiting`/`terminal` 的 Claude 线程**的屏幕（刚启动 1 秒内不看；在工作的线程从不看）。认出了：`trust_seen[线程]=文件夹`，会话立刻是 `waiting`/`terminal`、`waiting_for: "trust"`、`trust_folder`，不再等 20 秒（实测：启动后约 3 秒）；屏幕不再是这个对话框（人自己在终端里答了，Claude 开始报状态）就去掉。
- **`thread.trust {thread}`**：只对在跑的 Claude 线程。**先读屏幕**，不是这个对话框就报错、什么键都不按。光标在 `No, exit` 上时按 `down`，再读屏幕，**看到光标在 `Yes` 上**（最多等 1.5 秒）才继续，否则报错（那时不会按 Enter：Enter 在 `No, exit` 上会让 Claude Code 退出）；按 `enter` 之前**再读一次**屏幕确认。然后记 `trusted`，会话继续，排队的第一条消息在 Claude 报 `idle` 后照常发出（实测）。lilad 自己从不信任：没有「总是信任」的设置，窗口里是一个按钮，命令行是 `lila thread trust`。对话框还在时发的消息和别的排队的话一起等着，不会打进对话框。
- **实测**（Claude Code 2.1.322，临时文件夹）：对话框约 1 秒内被认出；`thread.trust` 按 `down`、`enter` 后 Claude 在 2 秒内开始工作并回答；已经没有对话框时再 `thread.trust` 报「not at Claude Code's trust dialog」；**项目文件夹被信任之后，它的 worktree 不再问**（worktree 在 `<data>/worktrees/…`，不在项目文件夹里面，Claude Code 仍认得它属于已信任的仓库）；信任过的文件夹的子文件夹也不问。一个还没信任过的仓库里第一次开 worktree 会话，它问的是仓库还是 worktree 的文件夹，没有单独验证。
- **局限**：只认 2.1.322 的文字；Claude Code 改了措辞就退回「在终端里等你」（失败安全）。`hc keys` 有「3 秒内没人打字」的规矩，人正在那个终端里打字时按键会晚一点。屏幕每 2 秒读一次，最多比对话框出现晚 2 秒。
