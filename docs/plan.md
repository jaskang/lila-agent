# Lila 开发计划

2026-11-13 定。架构见 `architecture.md`，开发和验证的方法见 `development.md`。

目标的顺序：先让 Lila 成为你每天用的个人助手（编程、待办跟进、调研和写文档、定时简报），用下来稳了再考虑给别人用。邮件和日历放到后面（见「以后」）。

这份文档是任务单：接手的人（或模型）从「下一个任务」开始，按顺序做，每做完一个任务在这里把它标成 ✅ 并写一行验证结果。

## 现在在哪

都在 `main` 上。

| 已完成 | 结果 |
|---|---|
| 精简成只用 omp | 删掉 Claude Code、Codex 两个引擎和终端、hook、账号用量等，约 2.8 万行 |
| omp 接入换成 RPC | 流式对话、工具卡片、权限卡片、提问卡片、会话续聊、模型菜单 |
| P1.1 omp 由服务核心直接启动 | 去掉 `lila host`、OSC 7501、`host.attach` 和 `Terminals`；库里去掉 `threads.terminal` 列 |

验证到的程度：测试和 clippy 全过；真 omp 加 magpie 跑过普通回复、出权限卡片并允许执行、停止后续聊。**没有验证**：窗口接真后台用鼠标点、agent 真的提一个问题、改文件和 MCP 工具的权限卡片。

**下一个任务：P1.2。**

## 每个任务都要守的

- 先读 `AGENTS.md`、`architecture.md`、`DESIGN.md`（改界面时）。
- 做完跑 `cargo build --workspace`、`cargo test --workspace`、`cargo clippy --workspace --all-targets`，全部干净。
- 改了界面就用 `--features shots` 截图自己看（`development.md`）。
- 真实模型请求每个任务最多三次，放在单独的 `LILA_HOME` 里；能用假 omp 测的不用真的。
- 删掉不用的代码，不留开关和兼容层；不加任务单以外的功能。
- 文档跟着改：`architecture.md` 里对应的「现状」段落、`README.md`。
- 汇报时分开写「实测过」和「没验证」。提交和推送等用户说。

## P1 一个程序

把后台的 lilad 并进窗口程序，去掉 herdr-core。**完成标志**：只装一个程序、只依赖 `omp`，就能开会话干活；关掉窗口后会话还在跑，重新打开能看到结果。

现在的样子：窗口 `lila-app` 经 socket 调 lilad（`crates/lila/src/daemon/`）；lilad 自己跑在 herdr-core 的终端里（`crates/hc-client`，`client::start_daemon`）；每个会话的 omp 是 lilad 的子进程（P1.1 做完）。

### P1.1 omp 由服务核心直接启动 ✅

lilad 里每个会话一个线程（`daemon/runner.rs` 的 `OmpAgents`）启动 omp 子进程、读写它的标准输入输出，用 `lila_agent::omp::Omp` 翻译；事件、状态、退出都作为 `Fact` 经通道回到 lilad 的主循环（`Input::Agent`）。服务核心通过 `Agents` trait 用它，测试用假的 `Fake`。

- 删掉：`lila host omp`、`crates/lila/src/host.rs`、OSC 7501（`HostStatus::osc`）、`host.attach`、`Terminals`、`HcTerminals`、`on_terminal`/`on_event`/`reconcile`、启动 20 秒没报状态的日志。
- 库升到版本 9：去掉 `threads.terminal` 列；一个会话「在跑」就是 `started_ms` 有值、`ended_ms` 没有。lilad 重启时这样的会话（它们的 omp 随旧 lilad 结束了）被标成已结束，事件里记「lilad was not running」。
- omp 退出：会话变为已结束，对话里出现带退出码和最后几行输出的错误（omp 的 stderr 写在 `state/omp/<会话 id>.log`）。放手一个会话（停止、归档、`lila daemon stop`）：关 omp 的标准输入，3 秒后还在就杀掉；`lila daemon stop` 等所有 omp 走完才退出。
- 事件带「第几次启动」，已经被放手的旧 omp 的迟到事件会被丢掉（停止后马上继续时不会串）。
- socket 协议升到 6：没有 `host.attach`，会话 JSON 没有 `terminal`。
- 测试：`tests/omp.rs`（假 omp 通过真的 `OmpAgents`：启动参数、`--continue`、分块帧、提问回复、崩溃信息、放手后离开）、`runner.rs` 里用 shell 脚本测退出信息、放手、杀掉；`daemon/tests.rs` 的假终端换成假 omp。

验收（实测，隔离的 `LILA_HOME`，真 omp 加 magpie，3 次模型请求）：新会话 → 回复 → 写文件出权限卡片 → 允许 → 文件写好 → 停止（`pgrep -x omp` 为 0）→ 继续（带 `--continue`，agent 记得上一轮写过 `hello.txt`）；`lila daemon stop` 后没有残留的 omp；`kill -9` lilad 后 3 秒内 omp 也走了（标准输入关了它自己退出）。**没验证**：窗口接这个 lilad 点鼠标（窗口没改，协议只是少了 `terminal` 字段）。

### P1.2 服务核心成为库

- 新 crate `lila-service`：把 `crates/lila/src/` 里的 `daemon/`、`policy.rs`、`rules.rs`、`checkpoint.rs`、`changes.rs`、`merge.rs`、`models.rs` 搬进去。对外一个入口：启动（给数据目录），返回一个句柄，句柄能发请求（方法名加参数，和现在 socket 上的一样）、订阅推送、关闭。
- socket 服务只是这个句柄外面的一层薄壳，留在库里，可开可关。
- `lila` crate 只剩命令行。

验收：行为不变，测试全过。

### P1.3 窗口在自己进程里跑服务核心

- `lila-app` 启动时启动服务核心，`crates/lila-app/src/data.rs` 改成直接用句柄，不连 socket。
- 同时打开 socket，命令行照样能用。程序没在运行时，命令行报「Lila 没有在运行」。
- 只允许一个实例：第二次启动时发现 socket 已被占用，就让已有的窗口到前面，然后退出。
- 删掉：`client::start_daemon`、`lila up`、`lila daemon status/restart/stop`、版本握手（`PROTOCOL` 的比较、侧栏的「lilad 版本不符」和重启按钮、关于页里的对应内容）。
- 删掉 `crates/hc-client` 和 README 里对 herdr-core 的要求。

验收：只运行窗口程序就能开会话；另开一个终端用 `lila thread list` 能看到同样的会话。

### P1.4 关窗口不退出

- 关掉窗口后程序继续运行，会话继续跑；点 Dock 图标重新打开窗口，状态和关之前一样。
- ⌘Q 真正退出：如果有会话正在跑，先问一次；退出时结束所有 omp 子进程。
- 先核实 GPUI 怎么做「最后一个窗口关闭后不退出」和「点 Dock 图标重开窗口」；菜单栏图标这次不做。

验收：发一条要跑一会儿的消息，关窗口，等它跑完，重开窗口看到结果；⌘Q 后 `pgrep omp` 没有残留。

### P1.5 整理

- crate 改名和合并，按 `architecture.md` §4：`lila-agent` → `lila-omp`；`lila-app` 并进 `lila`（不带参数启动窗口，带参数是命令行）。
- omp 的会话目录从 `data/omp/<会话 id>/` 挪到 `sessions/<会话 id>/`，已有的目录启动时搬过去。
- `README.md` 的安装、使用、开发各节重写；`architecture.md` 里 §2、§4、§8 的「现状」段落删掉。

## P2 能每天用

**完成标志**：你用 Lila 做完一天的编程工作，没有因为它的毛病回到终端。做完这一阶段开始自用，之后的阶段都在 Lila 里做。

| 任务 | 内容 | 验收 |
|---|---|---|
| P2.1 | **Lila 自己的 omp 配置目录**（`architecture.md` §6）：`LILA_HOME/omp/`，启动 omp 时设 `PI_CODING_AGENT_DIR`。第一次启动时：(1) 检查本机的 magpie（`127.0.0.1:3425`）是否在运行，在就自动在 `models.yml` 里写好 magpie 这个模型来源（`baseUrl`、`apiKey: local`、`api: anthropic-messages`，模型列表向 magpie 查），不需要密钥；(2) `~/.omp` 里还有别的模型来源时，问一次要不要复制过来，只复制模型来源的配置，不复制工具审批等其他设置。在 `~/.omp` 里登录过的账号不会带过来，要在 Lila 里重新登录（omp 的 RPC 有 `login`，设置页里的登录入口放到 P4.1 一起做）。先读 omp 的 `docs/settings.md`、`docs/models.md` 弄清楚是哪些文件。 | 全新的 `LILA_HOME` 加运行中的 magpie，不做任何配置就能选到模型并对话；在 `~/.omp` 里把 bash 设成总是允许，Lila 的「改动前确认」档仍然出卡片 |
| P2.2 | **原生通知**：会话在等你、一轮跑完，而且窗口不在前面时发系统通知，点击跳到那个会话。现在是用 `osascript` 发的（`data.rs::notify`），点了没反应。先查没打包成 `.app` 的程序能不能发可点击的通知；不能就做一个最小的 `.app` 打包。判断该不该发放在服务核心里，窗口只负责显示。 | 窗口在后面时跑完一轮，收到通知，点它回到会话 |
| P2.3 | **拒绝带理由**：权限卡片上拒绝时可以写一句话；omp 的回答带不了理由，所以拒绝之后把这句话作为附言排进下一条发给 agent 的消息（检查点的备注已经是这样做的，照它来）。 | 拒绝并写理由，agent 的下一步说明它看到了 |
| P2.4 | **提问卡片补全**：密码类输入打码（gpui-kit 没有就先查 gpui-component）；侧栏和通知对提问写「要你回答」而不是「要你允许」；`ask` 工具的选项说明显示出来。 | 截图；用真 omp 让 agent 提一个问题并回答 |
| P2.5 | **清理**：删除会话时删掉它的 omp 会话目录；旧的 Claude Code、Codex 会话和 `legacy` 列从库里清掉（一次迁移），相关的界面分支删掉；旧的 ACP 会话同样清掉。 | 迁移用造出来的旧库测 |
| P2.6 | **把没验证的走一遍**：窗口接真的服务核心，用 `LILA_APP_CLICK` 点完「新会话 → 发消息 → 权限卡片 → 允许 → 改文件 → 看改动 → 提交 → 回到某条消息」；改文件、写文件的权限卡片上工具名、路径对不对；中断；切模型。发现的问题当场修，修不了的记到这份文档末尾。 | 每一步的截图 |

## P3 看得见它在干什么

**完成标志**：一个跑了半小时的会话，你扫一眼就知道它做到哪了、卡在哪。

数据 omp 都有接口（omp 仓库的 `docs/rpc.md`），这一阶段主要是接上并画出来。每个任务先用不花额度的方式（直接和 `omp --mode rpc-ui` 对话）看清帧的样子，再写翻译和界面。

| 任务 | 内容 |
|---|---|
| P3.1 | **子 agent**：`set_subagent_subscription`、`get_subagents`、`get_subagent_messages`、`cancel_subagent`。对话里子 agent 是一张卡片：任务、状态；展开看它的对话；可取消。 |
| P3.2 | **待办和计划**：显示 agent 当前的清单（`todo` 工具的结果和相关事件），固定在对话上方或右面板，界面规范先写进 `DESIGN.md`。 |
| P3.3 | **上下文占用**：`get_session_stats`，输入框旁显示用了多少；快满时提示；菜单里加「压缩」（`compact`）。 |
| P3.4 | **标题和翻页**：会话标题在第一轮结束后自动生成（先看 omp 有没有现成的，`set_session_name` 相关）；长对话往前翻页。 |
| P3.5 | **回到这里同时回退对话**：现在只恢复文件（Lila 的检查点）；加上用 omp 的 `navigate_tree` 把对话也回到那一点，两者一起成功或一起不做。 |
| P3.6 | **代码块语法高亮**：先查 gpui-component 或 Zed 的现成做法，不自己写。 |

## P4 集成和待办跟进

**完成标志**：你说「看一下 Asana 上这周到期的事和我的 GitHub 通知，告诉我先做哪个」，它做到。

| 任务 | 内容 |
|---|---|
| P4.1 | **集成的开关**：设置里一页「集成」，每项对应 Lila 的 omp 配置里的一个 MCP server（omp 的 `docs/mcp-config.md`），可按工作区开关。需要登录的，登录流程用 omp 自己的。 |
| P4.2 | **Asana**：选一个现成的 Asana MCP server 接进来。 |
| P4.3 | **GitHub**：不接 MCP，用 omp 内置的 GitHub 工具（包着 `gh`）；确认它在 Lila 里可用，结果卡片能看。 |
| P4.4 | **对外动作的权限**：MCP 工具里会对外发出东西的（建任务、发评论），即使在全自动档也出卡片。`policy.rs` 里已有按 `mcp__server__tool` 认 MCP 工具的写法，用真的工具名核实。 |

## P5 定时任务和简报

**完成标志**：每个工作日早上 9 点，你收到一条通知，点开是 Asana 上到期的事、GitHub 上等你处理的事。

| 任务 | 内容 |
|---|---|
| P5.1 | **定时任务的数据和调度**：一句话加一个时间规则（cron 写法，用现成的 crate 解析），到点在指定工作区开一个会话跑。存在库里；命令行 `lila schedule add/list/remove/run`。 |
| P5.2 | **界面**：设置里一页「定时任务」，新建、开关、立刻跑一次；跑出来的会话在侧栏里有标记。 |
| P5.3 | **权限**：定时会话没有人在旁边看，对外发出的动作一律等用户点头，不管会话是哪一档；等的时候发通知。 |
| P5.4 | **登录时自动启动**（不开窗口），这样定时任务不依赖你手动打开程序。 |

## P6 调研和写文档

**完成标志**：你给一个题目，它查资料，交回一份带出处的文档。

| 任务 | 内容 |
|---|---|
| P6.1 | 实测 omp 内置的 `web_search` 和 `browser`：要配什么搜索来源，配置写进 Lila 的 omp 配置；结果卡片能看、链接能点。不够用再找 MCP server。 |
| P6.2 | 调研的技能：一份 omp 技能，说明怎么分头查、怎么记出处、文档写到工作区的哪里。 |

## 以后

顺序没定，等前面自用稳定后再排。

- **邮件**：要支持一个人有多个邮箱，而且是不同的服务（QQ 邮箱、Gmail 等）。这意味着不能绑死某一家的接口，多半要走通用的 IMAP/SMTP 加各家的授权方式，账号管理要单独设计。
- **日历**。
- 远程访问和手机端。
- 给别人用的版本。

## 待定事项

括号里是建议。用户没有另外说时按建议做。

1. **omp 配置归 Lila 单独管**：2026-11-13 用户已同意，做法见 P2.1。
2. **旧的 Claude Code、Codex 会话**（建议：清掉，见 P2.5。）
3. **命令行保留多少**（建议：保留现有的，测试和排错都靠它。）

## 遗留问题

做任务时发现、当时没修的，记在这里。

- omp 的设置里把某个工具设成总是允许，会绕过 Lila 的审批（P2.1 解决）。
- 第一条消息发出前会先向 omp 要模型列表，omp 找模型慢时第一条消息会等。
- `ask` 工具一次问多项时，每项是单独一张卡片。
