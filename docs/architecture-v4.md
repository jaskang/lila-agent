# Lila 架构 v4：项目与会话（lilad 的 socket 接口）

> 2026-11-10 · P1a（后端）和 P1b（窗口，见 §9）。需求见 `product-v4.md`；引擎驱动和对话事件模型见 `architecture-v3.md`（继续有效）。
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
  | `cwd` | 引擎运行的文件夹。现在就是项目文件夹（P2 的 worktree 会换掉它） |
  | `session` | 引擎自己的会话 id，续接用。Claude：新建时 lilad 选的 UUID；Codex、omp：host 报告 |
  | `terminal` | 当前的 hc 终端地址；结束后为空 |
  | `created_ms`、`active_ms`、`started_ms`、`ended_ms`、`archived` | 时间戳（Unix 毫秒）；`active_ms` 是最近一条消息或回复，列表按它倒序 |

- **状态**（`state`，由终端状态和待答的权限卡片算出）：

  | state | 含义 |
  |---|---|
  | `starting` | 引擎已启动，还没报过状态 |
  | `running` | 在处理一轮 |
  | `waiting` | 在等人。`waiting` 字段说等什么：`card`（有权限卡片，在 `thread.read` 的 `permissions` 里）或 `terminal`（终端里有对话框：Claude 的「信任这个文件夹？」、登录；或启动 20 秒还没报状态） |
  | `idle` | 在提示符前，能收消息 |
  | `ended` | 引擎进程没了。对话还在，可 `thread.resume` |

  另有 `status`：终端报的原始词（`idle`/`working`/`blocked`/`done`/`error`/`unknown`/`gone`）。

- **数据库**（`lila-store`，SQLite，`PRAGMA user_version = 4`）：`projects`、`threads`、`conv_items`（按会话递增的 `seq`）、`outbox`（引擎还不能收时排队的话）、`thread_events`（lilad 自己做过的事）。**旧库不迁移**：打开 v3 的库（没有版本号、有 `tasks` 表）时，文件改名为 `lila.db.v3.bak`，新库从空开始。

## 2. 进程与终端

每个会话的引擎跑在**自己的 hc 终端**里：lilad 重启时会话不断。

- 终端名 `lila-<会话 id>`，标签 `lila.project`、`lila.thread`，环境变量 `LILA_PROJECT`、`LILA_THREAD`、`LILA_ROOT`（文件夹）、`LILA_HOME`（若设了）。
- 状态由程序自己用 OSC 7501 报给 hc（Claude Code 自带；`lila host` 替 Codex 和 ACP 报），lilad 订阅 hc 的事件。lilad 重启后：数据库里有终端的会话先当作活着，hc 的快照到了，快照里没有的标成 `ended`；快照里有的按标签对回会话。
- 启动命令：
  - Claude：`claude --model M [--session-id UUID | --resume ID] [--effort E] --permission-mode manual --settings <state>/threads/<id>/settings.json`。settings 里五个 hook（PermissionRequest、MessageDisplay、UserPromptSubmit、Stop、StopFailure）都跑 `lila hook <事件>`。
  - Codex：`lila host codex --program <codex> --model M [--effort E] --sandbox S --approval A [--thread ID]`。
  - omp：`lila host omp --program <omp> [--model M] [--session ID]`。
  - `lila host` 的 `--prompt-file` 可选（现在不带系统提示词）。
- 对话条目：Claude 读会话记录文件（每 250ms 跟读），Codex 和 omp 由 host 通过 `host.attach` 连上 lilad 推事件。条目存数据库，格式见 `lila_agent::Item`（`architecture-v3.md` §5）。

## 3. 协议

unix socket：`$LILA_HOME/state/lilad.sock`（没设 `LILA_HOME` 时 `~/.local/state/lila/lilad.sock`；路径太长时落在临时目录，`lila_core`/`Paths::socket()` 给准）。每行一个 JSON。

请求：`{"method": "thread.list", "params": {...}}`；回复：`{"result": ...}` 或 `{"error": "文字"}`。一条连接可以连着发多个请求。窗口用 `lila::client::request(paths, method, params, autostart)`；`autostart` 为真时 lilad 没跑就先在 hc 里启动它。

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
{"id": "shop", "cwd": "/abs/path", "title": "商店"|null, "label": "商店", "error": null,
 "defaults": {"engine": "claude", "model": "claude-sonnet-5-5", "effort": null, "permission": "full"},
 "threads": 3, "states": {"idle": 2, "running": 1}, "waiting": 0}
```

`error`：project.toml 读不了（文件夹没了、文件坏了）的原因；这时项目还在列表里，用默认值。`threads` 不含归档的；`waiting` 是待答的权限卡片数。

### 会话

| 方法 | 参数 | 返回 |
|---|---|---|
| `thread.new` | `project`｜`cwd`, `text?`, `attachments?`, `title?`, `engine?`, `model?`, `effort?`, `permission?` | 会话。按项目默认值建，参数覆盖；启动引擎；第一条消息排队，引擎能收了就发。别的引擎不继承项目默认的 model/effort。引擎的程序不在 PATH 上、文件夹没了就报错，什么都不留 |
| `thread.list` | `project?`｜`cwd?`, `archived?` | 会话数组，`active_ms` 倒序；不给项目就是所有项目的；`archived: true` 含归档的 |
| `thread.show` | `thread` | 会话 |
| `thread.read` | `thread`, `after?`, `limit?`(400) | 见下 |
| `thread.send` | `thread`, `text`, `attachments?` | `{sent: true, thread}`。`attachments` 是绝对路径数组。引擎还不能收（starting）就排队；`waiting`（有卡片或终端对话框）时拒绝，因为话会变成对它的回答；`ended` 时拒绝，要先 resume |
| `thread.interrupt` | `thread` | `{interrupted: true}`。只在 `running` 时；Codex、omp 发协议的中断，Claude 在终端按 Esc |
| `thread.stop` | `thread` | 会话。杀终端，状态变 `ended`，对话保留 |
| `thread.resume` | `thread`, `text?`, `attachments?`, `model?`, `effort?`, `permission?` | 会话。在原来的 session 上重新启动引擎（Claude `--resume`；Codex `thread/resume`；ACP `session/load`）。Claude 会话在第一条消息前就结束了的，用同一个 id 新开。`text` 作为续接后的第一条消息 |
| `thread.archive` / `thread.unarchive` | `thread` | 会话。归档会先结束引擎 |
| `thread.rename` | `thread`, `title` | 会话 |
| `thread.permission` | `thread`, `permission` | 会话。立即生效，不用重启，见 §6 |
| `thread.answer` | `thread`, `card`｜`permission`, `option`｜`allow`｜`dismiss` | `{thread, card, answered: true}`。回答权限卡片：`option` 是卡片选项的 `id` 或 `label`；`allow: true/false` 选第一个允许或拒绝的选项；`dismiss: true`（只对 Claude）关掉卡片、让 Claude Code 在自己的终端里问。卡片已经没了（别处答过、引擎收回了）报错 |
| `thread.model` | `thread`, `model?`, `effort?` | `{set: true}`。Codex、omp 运行中立即换（omp 只能换它列出的）；Claude 运行中拒绝（它的 `/model` 会改用户的全局默认），要先 stop 再改再 resume；ended 的会话只改存的值，下次启动用 |
| `thread.run` | `thread`, `action` | `{ran: true}`。现在只有 Codex 的 `compact` |
| `thread.commands` | `thread` | `{commands: [{name, description, kind: builtin｜skill｜command, hint, insert, action}]}`，`/` 菜单的内容 |
| `thread.files` | `thread`｜`project`｜`cwd` | `{files: [相对路径], cwd}`，`@` 菜单的内容（git 跟踪的和没被忽略的新文件）。给 `thread` 是会话的文件夹；给 `project`（或 `cwd`）是项目的文件夹，用于还没创建会话的新会话输入框 |
| `thread.log` | `thread`, `limit?`(50) | `[{at_ms, actor, kind, detail}]`：lilad 自己做过的事，见下 |

会话：

```json
{"id": "t07b6b991", "project": "shop", "title": "…", "engine": "codex", "model": "gpt-6.1-sol", "effort": "low",
 "permission": "full", "cwd": "/abs/path", "session": "…"|null,
 "state": "idle", "status": "idle", "waiting": null, "cards": 0, "msg": null,
 "terminal": "t78"|null, "caps": {"tui": false, "gui": true, "steer": true},
 "created_ms": 0, "active_ms": 0, "started_ms": 0, "ended_ms": null, "archived": false}
```

`model`/`effort` 是引擎正在用的（Claude 取自最近一条回复的 `model`；Codex、omp 取自引擎报告），没有就是存的。`cards` 是待答卡片数。`msg` 是终端报的说明（比如 blocked 的原因）。`caps`：`tui` 能切到终端（只有 Claude），`steer` 运行中发的话能进当前这一轮（omp 不能，会排到下一轮）。

`thread.read`：

```json
{"thread": {会话}, "seq": 12, "items": [...], "permissions": [卡片], "writing": "正在写的回复"|null,
 "config": {"model": "…", "effort": "…", "default_model": "…", "default_effort": "…", "switch": true, "models": [{id,name}], "efforts": [{id,name}]}}
```

- `items`：`seq` 大于 `after` 的条目，旧到新；超过 `limit` 取最新的。同一个 `id` 的条目更新时整条覆盖并拿到新的 `seq`，所以客户端按 `id` 替换。`seq` 是返回的最大序号，下次当 `after`。条目格式是 `lila_agent::Item`：`{id, turn?, at_ms, state: running｜done｜failed, kind: user_message｜agent_message｜thought｜tool_call, …}`，用户消息带 `from: person｜local｜summary`，工具调用带 `tool`（`execute/read/edit/search/fetch/delete/…`）、`name`、`title`、`output?`、`diffs?`。
- `writing`：Claude 的 `MessageDisplay` 增量攒出的「正在写」，会话记录里有了回复就清掉。
- `permissions`：待答的卡片：

  ```json
  {"id": "p3", "thread": "t…", "request": "codex-12", "tool": "execute", "name": "shell", "title": "git push origin main",
   "detail": "", "command": "git push origin main"|null, "paths": [], "why": "全自动档仍需确认：推送或删除 git 分支",
   "danger": "git_push"|null, "options": [{"id": "accept", "label": "允许", "kind": "allow"}, …], "at_ms": 0}
  ```

  `danger`：`outside_workspace`｜`git_push`｜`external_action`｜`computer_use`，档位本身要问（`confirm`）时为空。选项的 `kind`：`allow`｜`allow_always`｜`reject`｜`reject_always`。
- `config.switch`：窗口能不能让用户换模型和努力程度（见 `thread.model`）。

`thread.log` 的 `kind`：`started`、`ended`、`resumed`、`auto_allowed`、`auto_denied`、`asked`（开了卡片）、`answered`、`dismissed`、`permission`（档位切换）、`turn_failed`、`notice`、`limit`。`auto_allowed`/`auto_denied` 记着档位和请求，是审计「lilad 替你答了什么」的地方。

### 其他

| 方法 | 说明 |
|---|---|
| `ping` | `{pong, pid}` |
| `status` | `{pid, projects, threads: {state: 数量}, cards, magpie}` |
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
| `{"event": "thread", "project": "shop", "thread": "t…", "state": "running", "waiting": null, "items_seq": 12, "seq": N}` | 这个会话变了：状态、条目、权限卡片、正在写的回复、标题、档位、模型。`items_seq` 是它的条目的最大 `seq`：比自己手里的大，就 `thread.read {after}` 取增量。会话被删（随项目删除）时不推 |

一次变化先推 `changed`，再推 `project`，再推各个 `thread`。事件只说「变了」，数据一律用请求取。「正在写」的逐字增量也走 `thread` 事件加 `thread.read` 的 `writing`，没有单独的增量事件。

## 6. 权限档

三档，每个会话各一档，随时 `thread.permission` 切换，项目默认 `full`：

| 档 | 含义 |
|---|---|
| `read_only` | 读、搜索、看网页可以；改文件、有副作用的命令一律拒绝（拒绝的文字会回到 agent：「This thread is read-only in Lila…」） |
| `confirm` | 读可以；其余（改文件、运行命令、写操作的 MCP 工具）都出卡片问人 |
| `full` | 都不问，除了四类（下）出卡片 |

**判定**（`crates/lila/src/policy.rs`，纯函数，有测试）：三种引擎的权限请求都转成 `PermissionRequest`（工具种类、名字、命令行、路径、工作目录、选项），lilad 先按会话的档位回答（`daemon/permissions.rs::ask_permission`），回答不了的才变成卡片；每次自动回答和开卡片都记进 `thread_events`。

全自动档下仍要问的四类，以及**能识别到什么程度**（识别靠请求里的文字，不是沙箱）：

1. **改动或删除工作区以外的文件**：编辑类请求（Claude 的 Edit/Write/NotebookEdit 的 `file_path`，Codex 的 apply_patch 的文件列表，ACP 的 `locations`）按路径判断，路径会展开 `~`、`..`、符号链接，临时目录（`/tmp`、`/var/folders`、`$TMPDIR`、`/dev/null`）不算。命令行：看重定向（`> >>`）、`rm mv touch mkdir tee chmod chown ln truncate…`、`cp rsync install` 的目的地、`sed -i`，跟着 `cd`，进 `sh -c '…'`。**识别不了的**：变量拼出来的路径、脚本或程序内部写的文件（`python x.py` 里写 `/etc`）、`find -delete`、`xargs rm`、`dd of=`、`git checkout/clean` 对工作区外的仓库、通过别名或函数。
2. **`git push`、强制推送、删分支**：命令行里的 `git [-C dir|-c k=v …] push`、`git branch -d/-D/--delete`（包括 `bash -c`、`;`、`&&`、括号里的）。**识别不了的**：用变量、别名、`git` 的包装脚本、`GIT_SSH` 之类绕过；以及不是 `git` 命令的推送（`jj git push`、`gh repo sync`）。
3. **对外发出的动作：提 PR、发评论、操作 Asana**：`gh pr/issue/release/repo/… create|comment|review|merge|close|edit|…`、`gh api` 的 POST/PUT/PATCH/DELETE 或带 `-f/-F/--input`；`npm|pnpm|yarn|cargo publish`、`docker push`、`twine upload`；`curl|wget` 的写操作（`-X POST`、`-d`、`-F`、`-T`……，指向 localhost 的除外）；MCP 工具（Claude 的名字 `mcp__<server>__<tool>`）里工具名以 create/add/update/delete/merge/push/post/send/comment/close/edit/fork/assign/submit/approve/publish/move/write/upload… 开头的。**识别不了的**：没有写动词的 MCP 工具名（读写看不出）、`ssh`/`scp`/邮件、用脚本语言直接调 HTTP、Codex 的 MCP 工具（它的审批是 MCP elicitation，目前直接拒绝，不问人）。
4. **computer use**：工具名含 `computer` 且是 MCP 工具或以 `computer` 开头的。**现在没有这样的工具**（Lila 还没提供，三种引擎自带的也没有），所以这条没有实测过。

### 各引擎怎么落实

- **Claude Code**：始终以 `--permission-mode manual` 启动，凡是它自己不放行的（改文件、跑命令）都会问，问就走 `PermissionRequest` hook 到 lilad；档位由 lilad 逐个请求判定，所以切档不用重启。hook 的答复是 allow 或 deny（deny 带文字）；要人答的请求，hook 每 0.5 秒问一次 `hook.answer`，最多 570 秒；卡片被 `dismiss`、lilad 重启丢了请求、超时：hook 什么都不输出，Claude Code 在终端里问（会话变 `waiting`/`terminal`）。**缺口**：用户自己的 `~/.claude/settings.json` 里的 allow 规则会让某些命令不问，lilad 看不到；`read_only` 档靠 lilad 拒绝，不靠 Claude 的 plan 模式。lilad 没起来时 hook 立即退出，由终端问。
- **Codex**（`codex app-server`）：按档位给 `thread/start` 的 `approvalPolicy` 和 `sandbox`，每次 `turn/start` 也带上（所以切档在下一轮生效，运行中的这一轮不变）：

  | 档 | approvalPolicy | sandbox |
  |---|---|---|
  | `read_only` | `on-request` | `read-only`（它请求越出沙箱的，lilad 拒绝） |
  | `confirm` | `untrusted` | `workspace-write`（实测：写文件的 shell 命令出卡片） |
  | `full` | `on-request` | `workspace-write`，不开网络 |

  `full` 的做法：沙箱本身不让写工作区外、不让联网，所以 `git push`、`gh pr create` 这些要靠「请求越出沙箱」才能执行，请求到 lilad 手里按上面四类判定（实测：`git push` 出卡片）。代价是装依赖、拉代码这类联网命令也会多一轮「请求越出沙箱」，由 lilad 自动允许。Codex 的 `item/permissions/requestApproval`（模型主动申请额外权限）目前直接回「不支持」，不问人。
- **ACP（omp）**：omp 对执行、编辑、删除、移动类工具发 `session/request_permission`，全部到 lilad，按档位判定，切档立即生效；读类不问。

## 7. 命令行

`lila` 是窗口之外的另一个客户端，只走同一个 socket：

```sh
lila up                              # 启动 lilad，把当前文件夹加成项目
lila project list|add [路径] [--name --title]|remove [项目]|show [项目]|set [--engine --model --effort --permission --title]
lila thread new "第一条消息" [--project --engine --model --effort --permission --title --file F]
lila thread list [--project|--all] [--archived]
lila thread show <id> [--after N]    # 状态、对话、待答的卡片
lila thread send <id> "话" [--file F]
lila thread interrupt|stop|archive|unarchive <id>
lila thread resume <id> ["话"] [--model --effort --permission]
lila thread rename <id> <标题>
lila thread permission <id> read_only|confirm|full
lila thread answer <id> <卡片> allow|deny|dismiss|<选项>
lila thread model <id> [--model --effort]
lila thread log <id>
lila daemon run|stop|status
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
| `lila/src/policy.rs` | 档位判定（§6） |
| `lila/src/project.rs` | project.toml 的读写 |
| 留着、没人调用的 | `worktree.rs`、`commit.rs`（P2 改成一个会话一个可选 worktree，现在是「一条链一个槽」的接口）、`github.rs`、`daemon/sources.rs`（P4）；它们不依赖任务和流程，能独立编译、有测试 |

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
| 项目设置（⇧⌘,） | `project.set {project, title, defaults}`；`model`/`effort` 传空字符串表示清掉 |
| 输入框的 `/` 菜单、会话内换模型 | `thread.commands`、`thread.run`、`thread.model` |
| 移除项目 | `project.remove` |

- 窗口记着：上次显示的会话和项目、哪些会话切成了终端视图（`state/app-tabs.json`、`app-views.json`）。折叠的项目、归档开关不记。
- 系统通知在窗口里发：对比前后两次 `thread.list`，会话变成 `waiting`、或从 `running` 变成 `idle`，而窗口不在前面时，用 `osascript` 发一条。
- 截图自查：`--features shots`，环境变量见 `docs/HANDOFF.md`；`LILA_APP_FAKE=<json>` 可以不起 lilad，用造好的项目、会话和对话画界面。
