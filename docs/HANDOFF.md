# 交接说明（给接手的模型）

## 2026-11-13（晚）：omp 走 RPC 模式，不再走 ACP（分支 `omp-harness`，没提交）

`acp.rs` 和 `agent-client-protocol-schema` 删了，换成 `crates/lila-agent/src/omp.rs`（纯翻译，约 800 行含测试：一行进，`Step {send, events}` 出）；`crates/lila/src/host.rs` 起 `omp --mode rpc-ui --approval-mode always-ask --session-dir <目录> [--continue]`；`lila-core::runtime::Session` 没了。细节在 `architecture-v4.md` §2（启动、会话目录）、§6（权限对话怎么对回工具调用、提问）。要点：

- **权限还是 Lila 的**：`always-ask` 让 omp 对每个写和执行类调用发 `select` 对话；host 用 `toolcall_end` / `tool_execution_start` 里结构化的工具名和参数对出 `tool`、`name`（现在是 `bash`、`edit`、`mcp__server__tool` 这种稳定的名字）、`command`、`paths`、`cwd`，只在对话标题里也有同样的话时才信；对不上就从标题的文字读。`edit` 的参数是补丁文本，路径读 omp 自己写的 `File:` 行。不给 omp 自己的「for this session」。**拒绝不能带原因**：RPC 的回答只有选项文字，omp 给模型的拒绝错误是固定的（`Verdict::Deny` 的文字仍然没人用）。
- **agent 的提问成了卡片**（A2 后半的第一件事）：`ask` 工具要 `--mode rpc-ui`（`rpc` 里没有这个工具，所以没按任务单写的 `rpc`）；所有不是权限的 `select/confirm/input/editor` 对话是 `Question`，lilad 开 `q…` 卡片，`thread.answer {value｜yes｜cancel}`，命令行 `lila thread answer <id> <q…> <答案>`（`--cancel`），窗口 `question_card`。
- **会话目录** `<data>/omp/<会话 id>/`，里面有 `*.jsonl` 就 `--continue`；恢复不重放历史，所以对话不重复。**旧的 ACP 会话**没有目录，恢复时开新的 omp 会话（对话留着，omp 不记得）。没有数据库迁移（版本还是 8）。
- **模型**：`models.list`（短命的 `omp --mode rpc --no-session …` 问 `get_available_models`，记 10 分钟）；新会话的模型菜单和工作区设置的模型一行回来了；会话里的菜单用它自己的 omp 报的列表（`set_model`、`set_thinking_level`）。
- 失败的模型请求、omp 退出（带退出码和 stderr 最后几行）成了对话里的 `error` 条目；协议版本 5；`lila models`。
- 验证：`cargo build/test/clippy --workspace` 过；`crates/lila/tests/host.rs` 用假 omp 和假 lilad 跑真的 `lila host omp`（参数、`--continue`、分块的帧、提问往返、omp 崩溃）；真 omp（隔离的 `LILA_HOME` 和 `HC_SESSION`，模型 `magpie/claude/claude-sonnet-5-5`）跑了三句：回一个词、`confirm` 档的 `echo ok`（出卡片、允许、执行）、停止后恢复再问上下文（答对，对话没重复）。**没验证**：真的提问卡片（`ask` 工具）、真 MCP 工具的名字、`edit`/`write` 的真实对话文字（只有按 omp 源码写的单元测试）、并行的同名调用、窗口接真 lilad 点（只有 `--features shots` 的假数据截图）。
- **留给下一步（去掉 hc、lilad 进窗口进程）**：host 现在和 lilad 的唯一接口是 socket 上的 `AgentEvent`/`Command`，状态走 OSC 7501（`HostStatus`）；换掉 hc 时 host 可以保持独立进程（会话要在 lilad 重启时不断），或者把 `Omp` 直接放进 lilad 进程里用线程跑 omp 子进程（`Omp` 是纯的，`host.rs` 里的 IO 循环约 150 行可以搬）。omp 的 `--session-dir` 已经按会话分，不依赖 hc。会话的 omp 进程崩溃时 host 退出、会话变 `ended`，这个语义要保留。

## 2026-11-13：定位改了，Lila 是自己的 harness，只跑 omp

用户把定位改成「自己的 harness，建在 oh-my-pi（omp）上，omp 是唯一的引擎」。Claude 订阅仍可用，但要走 omp 自己的 provider 配置，不是 Lila 的事。这一轮（分支 `omp-harness`，从 `main` 切出，没提交）先做了删减；换 omp 的 RPC 在上面一节。

**删掉的**（`cargo build/test/clippy --workspace` 都过；测试从 281 个降到 164 个，删的是被删功能的测试，其余改成了 omp 的）：
- 引擎：`lila-agent` 的 `claude.rs`（读会话记录）、`codex.rs`（app-server）；`lila host codex`；`Runtime` 枚举（`lila-core`）、`Thread.engine`、项目默认值里的 `engine`、`effort`；`codex_settings`、`SetPermission`、`Compact`、`Caps`、`AgentEvent::Limits`、`AttachmentKind::Skill`、`Origin::Summary|Local|Interrupted`。
- Claude 的 hook 和信任对话框：`lila hook`、`daemon/hooks.rs`（`PreToolUse`、`PermissionRequest` 等六个 hook、`judged`、`hook.answer`）、`daemon/trust.rs` 和屏幕文字样本、`thread.trust`、`waiting: "terminal"`、`waiting_for`、`trust_folder`、`writing`（MessageDisplay）、会话记录跟读（`follow_transcripts`）、`Terminals` 里的 `send/keys/read`。
- 账号、用量、magpie：`accounts.rs`、`signin.rs`、`magpie.rs`、`usage.rs`、`sessions.rs`、`lila usage`、`lila accounts`、`status` 的 `magpie`；窗口的用量页和添加账号。
- `commands.rs`（找 Claude、Codex 的 skill 和自定义命令；omp 的命令由它自己通过 ACP 声明）、`thread.run`、`Paths.config`（只放过 `accounts.toml`）。
- 终端栈：crate `lila-term`、`ghostty-sys`（含预编译的 libghostty-vt）、窗口里的终端视图、对话/终端切换、「打开终端」、「在终端里回答」、`app-views.json`。
- 窗口：引擎选择（新会话的模型按钮、工作区设置的引擎、模型、努力程度行）、`$` 命令、总结行、「已中断」行、`⌘[`、`⌘F`。
- 依赖：`strip-ansi-escapes`、`linkify`、`wait-timeout`、`yaml-rust2`（lila 和 lila-agent）。

**留着的**（任务单里的候选，核实后没删）：
- **`hc-client` 和 herdr-core**：omp 的 host 也跑在 hc 终端里，lilad 自己也是；会话在 lilad 重启时不断、状态走 OSC 7501 事件、重启后按标签找回会话，都靠它。换成 lilad 的子进程要同时改：`Terminals` 和 `HcTerminals`、`start_thread`、`on_terminal`/`on_event`/`reconcile`（hc 事件）、host 的 OSC 状态上报（改成 host 与 lilad 之间的消息）、lilad 自己的启动（`client::start_daemon`）、重启后找回；而且会话会随 lilad 一起死，除非 host 仍是独立进程。远超 300 行，所以没做。
- `lila host omp`、`host.attach`、`policy.rs`、`rules.rs`、检查点、改动、worktree、通知：照旧。
- `github.rs`、`daemon/sources.rs`：早就没人调用（P4），和这次无关。

**旧数据**：库的版本 8 把 `threads.engine` 删了；`claude`、`codex` 的会话记在 `legacy`，已结束，对话、检查点留着，窗口里输入框换成「Claude Code 会话，不能继续」；lilad 在 hc 里看到它们的终端就杀掉。`.lila/project.toml` 里的 `engine`、`effort` 读时忽略，`engine` 不是 `omp` 时 `model` 也丢掉。细节在 `architecture-v4.md` §1。协议版本 4。

**（已做，见上一节）ACP 换成 omp 的 RPC 模式。** 下面是当时的任务描述： 要换的是 `crates/lila-agent/src/acp.rs`（纯翻译，`Acp::on_message/command/tick/status`，输出 `Step {send, events}`）和 `crates/lila/src/host.rs`（现在直接用 `Acp`，没有 trait）；`lila_agent::model` 的 `AgentEvent`、`Command`、`PermissionRequest` 是 lilad 和窗口看到的接口，尽量不动。权限判定（`policy.rs`、`rules.rs`）和卡片依赖 `PermissionRequest` 里的 `tool`（种类）、`name`（工具名）、`command`、`paths`、`cwd`：ACP 里这些是从 `rawInput`、`locations`、`title` 里拼的，`name` 实际是 title，不是稳定的工具名；换 RPC 时要拿到结构化的工具名和参数，`policy` 里按 `mcp__server__tool` 认 MCP 的写法没核实过 omp 的真实名字。ACP 的拒绝不能带文字，RPC 如果能带，`Verdict::Deny` 的文字就有用了。新会话开始前 omp 还没报告模型列表，所以窗口新会话和工作区设置里选不了模型（只能写 `.lila/project.toml` 或 `lila project set --model`）；RPC 能先列模型的话这里要补。

**以下各节是 2026-11-11 的状态，写到 Claude Code、Codex、终端、账号、信任对话框、hook 的地方已经不是现状**；现状看 `architecture-v4.md`（已改）、`DESIGN.md`（已改）和上面这一节。

---

更新于 2026-11-11。用户用中文交流，回复也用中文。用户在两台电脑上轮流工作，另一台电脑上没有这一台的 Claude 记忆，所以该知道的都写在这里。

先读这份。需要细节时再读：
- `docs/product-v4.md`：**现在的产品方案**。定位、12 个已定的决定、路线图 P0–P8、还没定的小问题（§9，用户已说「都按建议」，即按括号里的默认做法）。
- `docs/architecture-v4.md`：**现在的架构**。项目和会话的数据模型、lilad 的每个 socket 方法、推送、三档权限和各引擎的落实、命令行、代码位置。和代码一致。
- `docs/architecture-v3.md`：引擎驱动和对话事件模型（Claude 终端加 hooks、Codex app-server、ACP），继续有效。里面讲「成员」的地方现在读作「会话」。
- `DESIGN.md`（仓库根目录）：界面规范。第一条规则：不写说明文字，看不懂就改布局。
- `AGENTS.md`（仓库根目录）：项目规则，包括用中文交流，以及优先用社区成熟的库、不自己造复杂的轮子。
- `docs/product-v3.md`、`docs/architecture-v2.md`、`docs/review-2026-11-10.md`：只作历史记录。旧实现（PM、任务、请求、成员、角色、流程）的最后状态在 git 标签 `v3-final`，要回看用 `git show v3-final:<路径>`。

## 这是什么

Lila 是一个原生界面（GPUI）的 agent 桌面应用。模型是 **项目 → 会话**：你在项目（一个文件夹）里开会话让 agent 干活；agent 由官方引擎来跑，Lila 提供界面、工具和集成。

2026-11-10 用户改了定位：原来的「agent 团队调度台」（PM 分派、流程、验收卡片、请求列表）**不再是需求，已经删除**。当天和用户逐条定下的决定（完整的表在 `product-v4.md` §2）：

- 先给用户自己和开发者用；编程为主。
- **只做官方引擎的外壳**：Claude Code、Codex、ACP agent（现在内置 omp）。不自己写 agent loop；DeepSeek Harness、pi 只当普通 ACP agent，不特别依赖。
- Claude Code 保持「跑它的终端，从会话记录和 hooks 画出对话」，不换 Agent SDK。
- Lila 登记一份现成的 MCP server 清单，开会话时注入，所有引擎共用；只给没有命令行的能力用（浏览器），按项目开关，不默认注入每个会话（2026-11-11 修订）。
- GitHub：agent 直接用 `gh`，**不接 GitHub MCP**（2026-11-11 用户定）；面板也用 `gh`，从一条 issue 开会话；不自动触发。Asana 不单独做，归入「添加任意 MCP server」。
- **computer use 不做**，除非自用时证明非常必要（2026-11-11 用户定）。
- 会话默认在项目文件夹原地工作，新建时可选独立 worktree。
- subagent：显示引擎自带的；Lila 另外提供跨引擎的「委派」工具。
- 权限：每个会话三档（只读、改动前确认、全自动），**默认全自动**，四类危险操作仍然问。
- 手机端等桌面版稳定后再做；现在只守一条：所有功能都走 lilad 的 socket。
- 讨论过但没采用的：自己写 harness、以 pi 或 DeepSeek Harness 为内核、变成 dsh 的发行版。参考过的同类产品是 t3code（`pingdotgg/t3code`）：服务端拥有执行、客户端只走 RPC、按会话注入 MCP 桥、中继加配对码做远程。

## 进度和下一步

**从这里接着做（2026-11-11 晚停在这里）**：A0、A1、A2 前半都已提交，工作区干净。下一步是 **A2 后半**，两件事，顺序做：

1. **agent 的提问成为卡片**，三种引擎归一成一种。要先实验的：Claude 的 `AskUserQuestion` 在 `PreToolUse`/`PermissionRequest` 里是什么样、hook 能不能把答案交回去（`updatedInput`）；Codex 的 `item/tool/requestUserInput` 现在回的是不支持（`codex.rs` 的测试里有）；ACP 有没有对应的请求。`policy.rs` 的 `INERT` 名单里现在把 `AskUserQuestion` 当成无害的直接放行，它放行后是在终端里问的。
2. **通知由 lilad 判断、窗口只显示**，换成原生通知，点击跳到会话。现在是窗口里对比两次 `thread.list` 再用 `osascript` 发（`data.rs::notify`、`app::notify_changes`）。没打包成 .app 的二进制能不能发可点击的原生通知要先查；信任对话框出现时通知和侧栏还写「终端在等你」，一起改。

做完 A2 就按路线图开始自用。用户的要求照旧：编码交给 subagent（Sonnet 5.5、high），主会话拆任务、验收、提交、汇报；这一轮每个阶段验收后都直接提交到本地 `main`，**推送要用户说了才做**。

在另一台电脑上接手前：`git pull`；`cargo install --path crates/lila`；`lila daemon restart`（协议版本现在是 3，旧 lilad 会被窗口认出来）；已经在跑的 Claude 会话要结束再继续才有新的 hook（`PreToolUse`、不过期的卡片）。

这一轮定下、还没做的：
- **Claude 的沙箱先不启用**（用户定）。实测它的联网只能按域名放行，清单外的域名在终端里弹框、lilad 看不到；要「联网不限」只能由 lilad 托管一个本地代理。记录在 `architecture-v4.md` §6 末尾。启用时再加「在沙箱外运行一律问」这一类（用户已同意）。
- 回退的两处界面问题，等用户用过再定：「已回到这里」的标记在对话末尾，看不出回到了哪条消息；旧的标记也带「撤销」。
- 「本会话总是允许」不给 `rm`、`python`、`node` 这类，是实现时定的，用户还没表态。


| 阶段 | 内容 | 状态 |
|---|---|---|
| P0 | 提交旧实现，打标签 `v3-final` | ✅ |
| P1 | 删掉 PM 和流程；会话模型；窗口改成项目 → 会话；三种引擎；三档权限 | ✅ 2026-11-10，等用户亲手试（见下） |
| P2 | 右侧的改动文件、diff、提交；独立 worktree 以及合并回来 | 后端 ✅ 2026-11-11（lilad、命令行、数据模型，`architecture-v4.md` §4 和 §10；测试和 clippy 过，真引擎没跑）；窗口 ✅ 2026-11-11（`panel.rs`：右面板的文件、改动，提交和合并按钮，新会话的 worktree 开关，已合并会话的标识；`--features shots` 截图和假数据看过，没接真 lilad 点过） |
| A0 | 收口：提交 P2；协议加请求 id 和版本握手；修 `@文件` 重复显示 | ✅ 2026-11-11（`a099ffb`）。请求 id：回复原样带回，原始 socket 实测过 ping/status/错误/subscribe/host.attach；版本握手：`lila::PROTOCOL`、`ping`/`status`、`lila daemon status/restart`、侧栏最后一行的提示和重启按钮、关于页，在隔离的 `LILA_HOME` 里用真旧 lilad（装在 `~/.cargo/bin` 的）加真窗口点过按钮（`--features shots`，没有鼠标）；`@文件`：Codex 回显和 ACP 条目、窗口画条目都不再多一张卡片，单元测试，没有真引擎跑过 |
| A1 | 安全网：每轮的 git 检查点和回退；Claude 用 `PreToolUse` 加自带沙箱 | **检查点和回退的后端 ✅ 2026-11-11**（`9de57b1`）（`architecture-v4.md` §11，协议版本 2）：每轮开始前用 git 打快照（Claude 在 `UserPromptSubmit` hook，Codex/omp 在 `deliver`），`thread.checkpoints`、`thread.restore {dry_run?}`、`lila thread checkpoints/restore`，对话里的 `restored` 标记，给 agent 的备注随下一条消息发出。验证过：`cargo test --workspace` 243 个全过、clippy 零警告；用临时 git 仓库测了快照不动 index/HEAD/分支/stash（比字节）、被忽略的文件不碰、子文件夹、中日文和空格路径、没有提交的仓库、dry run 什么都不改、各种拒绝、合并/移除项目后 ref 清掉、50 个上限、Claude 和 Codex/omp 的条目对应；隔离的 `LILA_HOME` 里用真 lilad 加假 `claude`（`sleep`）和真 `lila hook UserPromptSubmit`、命令行走了一遍。**没验证**：真 Claude Code 的 `UserPromptSubmit` 里 `prompt` 的实际样子（粘贴多行、带附件时和会话记录里的是否一致；对不上时检查点的 `item` 会是 `null`，仍可回退）、运行中排队的消息会不会也触发这个 hook、hook 等 job 做完这段时间 Claude 是否真的不开始；真 Codex/omp 的回显；窗口还没做。**窗口那一半要做的**见下 |
| A1 窗口 | 对话里每条用户消息的「回到这里」、确认框（dry run 的文件清单）、`restored` 标记「已回到这里」加「撤销」 | 代码 ✅ 2026-11-11（`37cf9cc`；`restore.rs`，加 `conversation.rs`、`composer.rs`、`data.rs` 的小改动；界面规范在 `DESIGN.md` 对话一节，接口在 `architecture-v4.md` §9）。验证过：`cargo test --workspace` 249 个全过（窗口这边新增 6 个：谁有按钮、什么时候藏、检查点怎么挑、清单文字、标记成行、回退时末尾转圈）、clippy 零警告；`--features shots` 加假数据截图看过悬停按钮、确认框（几个文件、很多文件时清单滚动、有提交的一行、什么都不变；「…还有 N 个」只有单元测试，没截到）、回退进行中（对话末尾转圈、输入框不发）、回退失败（红色提示条）、dry run 失败、两种标记（有和没有「撤销」）、旧协议、在跑、已合并的会话都没有按钮、窄窗口（1000×680）。确认后「回到这里」的整个路径只在假文件上走过。**没验证**：没有接真 lilad 点过，没有用真鼠标，没有真引擎（见「等用户亲手验证的」） |
| A1 判定 | Claude 的每个工具调用先过 `PreToolUse` hook，按档位判；自带沙箱 | `PreToolUse` ✅ 2026-11-11（`434f91f`；`hooks.rs`、`permissions.rs`、`client.rs`、`policy.rs`；`architecture-v4.md` §2、§6）。验证过：`cargo test --workspace` 253 个全过（新增 4 个：各档的判定、ask 交给 `PermissionRequest` 开卡片不判两次不记两次、保留的判定数量和过期、hook 输出）、clippy 零警告；隔离 `LILA_HOME` 里真 lilad、真 Haiku，项目的 `.claude/settings.json` 放 `Bash(touch:*)`、`Edit`、`Write` 的 allow 规则：以前 `read_only` 档里 `touch` 照样成功（缺口复现），现在被拒；`confirm` 档出卡片，答允许文件生成、答拒绝没有；`full` 档不弹框；`Write` 到文件夹外出 `outside_workspace` 卡片；运行中切档下一个调用就生效；每次往返约 4.6 毫秒。**沙箱没做**：「网络不限」设置表达不了（只有放行一切的本地代理 `httpProxyPort` 一个办法，要 lilad 托管代理）。**用户 2026-11-11 定：先不启用沙箱，记为待办，自用时确实遇到工作区外被改坏再做**；实验记录在 `architecture-v4.md` §6 末尾。**没验证**：subagent 里的工具调用、MCP 工具、`Edit`/`NotebookEdit` 没有在 lilad 里真跑（判定同一个函数，`Write` 跑过）；已经在跑的会话要结束再恢复才有 `PreToolUse` |
| A2 前半 | 卡片不过期、本会话总是允许、窗口里回答「信任此文件夹」 | ✅ 2026-11-11（`4e57f89`；协议版本 3；`architecture-v4.md` §6 和 §12，界面规范在 `DESIGN.md`）。(1) **卡片不过期**：Claude Code 把 hook 的 `timeout` 夹在 2^31−1 毫秒（约 24.8 天），所以 `PermissionRequest` 写 2 000 000 秒，`lila hook` 不设等待上限；lilad 重启时等着的 hook 带 `payload` 再问一次，卡片回来；hook 被杀（人在终端里答了）卡片 30 秒后丢，一轮结束也丢。(2) **本会话总是允许**（`rules.rs`、表 `thread_rules`、`thread.rule.remove`、`lila thread rules`）：`confirm` 档的卡片多一个 Lila 自己的选项，标签写明允许什么；规则 = 程序加子命令词 / 文件夹里的改动 / 一个 MCP 工具；一行里串几个命令要每段都有规则；包装器、解释器、危险一律不提供也不放行；切到只读清掉；Codex、ACP 自己的「总是」被它取代；窗口在权限档菜单里列出并可收回。(3) **信任对话框**（`daemon/trust.rs`、`thread.trust`、`lila thread trust`）：每 2 秒读终端屏幕认出对话框，`waiting_for: "trust"`，`thread.trust` 先读屏幕、移光标、再读、再按 Enter；窗口「信任并继续」。验证过：`cargo test --workspace` 281 个全过、clippy 零警告；隔离 `LILA_HOME` 里真 lilad 加真 Haiku：卡片放 11 分钟再答（一）、卡片开着重启 lilad 后答（二）、`touch` 的总是规则（第二个 `touch` 不出卡片、`mkdir` 出卡片、`full` 档的 `git push` 出 `git_push` 卡片且没有总是、`confirm` 档的 `git push` 卡片也没有总是）、信任对话框被认出并由 `thread.trust` 答掉、排队的第一条消息发出了；`--features shots` 截图看过卡片（短、长、窄）、规则列表、信任行（长路径、窄）。**没验证**：Codex、omp 的「总是」只有假 host 的单元测试；真鼠标；窗口没接真 lilad 点过。**A2 后半没做**：agent 的提问成卡片、原生通知 |
| — | 从这里开始自用，之后每个阶段都在 Lila 里做 | |
| A3 | 可观测：subagent、待办和计划、上下文占用、往前翻页、自动标题 | |
| P3 | MCP 清单和注入；Codex 的 MCP 审批；浏览器工具；图片条目 | |
| A4 | worktree 补全：`.worktreeinclude` 和 setup、丢弃和清理、diff 行内意见 | |
| P4 | GitHub 面板（`gh`，不接 MCP）；从 issue 开会话；提 PR | |
| P5 起 | 只读委派、定时任务、远程访问和手机端、任意 MCP server | |

2026-11-11 做了一次产品和 harness 的评审，路线图按它重排：P3 之前插入 A0–A3，每个阶段的内容、完成标志和不排顺序的 A5（回放测试、CLI 版本检查、`lila doctor`、Codex 卡住的根因）在 `product-v4.md` §8。A1、P5 里标了「先核实」的几条是凭对引擎的了解写的，没有对着装的版本验证过。

- 用户要求：**编码交给 subagent，用 Sonnet 5.5、努力程度 high**；主会话负责拆任务、写清楚要求、检查结果（重跑编译、测试、clippy，看截图）、向用户汇报。P1 就是这样分成后端和窗口两步做的。
- P2 开工前先听用户试用 P1 的反馈，有问题先改。
- P2 后端已经做完：会话有 `worktree`/`base_branch`/`base_commit`/`merged_ms`/`changes`；`thread.new {worktree}`、`thread.changes/diff/file/commit/merge`；每轮结束重新数改动摘要并推送。窗口那步只调这些，细节和返回格式在 `architecture-v4.md` §4（「改动、diff、文件、提交、合并」）。窗口要注意：`thread.merge` 和带 `worktree` 的 `thread.new` 可能要几秒到几十秒才回复，要在后台线程里调；`changes` 摘要是 `null` 时（还没数过、不在 git 仓库）不要显示「0 个改动」。
- **A1 后端留给窗口的**（细节和返回格式在 `architecture-v4.md` §11）：
  - 每个用户消息条目的「回到这里」：用 `thread.read` 的 `checkpoints`（`item` 是条目 id）去对，不要在条目上找（`item` 可能比条目晚几百毫秒才有，条目不会因此重发）。`item: null` 的检查点没有对应的消息，不显示按钮。`from: "lila"`（比如提交的备注）的条目也可能有检查点，要不要给按钮由窗口定。
  - 点了先 `thread.restore {thread, checkpoint, dry_run: true}`，用返回的 `changed`/`recreated`/`deleted`（和 `counts`）做确认框；`head_moved: true` 时说明中间有提交（文件会回去，提交不会，面板里会显示成未提交的改动）；两个会话共用一个文件夹时这份清单也会含对方的改动。确认后 `thread.restore` 不带 `dry_run`，在后台线程里调（要几秒），回复里的 `undo` 是撤销用的检查点编号。
  - 被拒绝时错误文字就是原因（在跑、等回答、有排队的话、同一文件夹里别的会话在跑、已合并），也可以先看 `thread.checkpoints` 的 `can_restore`/`why_not`，或每行的 `restorable`，把按钮置灰。
  - 条目 `from: "restored"`（带 `checkpoint`、`undo`）画成安静的一行「已回到这里」，`undo` 有值就给「撤销」。旧的窗口会把它当成人说的话画成气泡，所以窗口要认 `protocol >= 2`。
  - `thread` 的 `restoring: true` 期间不能发消息。回退后给 agent 的备注排在 outbox 里，下一条消息发出时在它前面一起出去；回显里备注已被去掉，对话里不会多出一条。
- P2 后端留下的：没有单独删除 worktree 的入口（只有合并会删），项目被移除也不删它的 worktree；worktree 里没有 `.env`、`node_modules/` 这类被忽略的文件，引擎进去可能跑不起来；worktree 里的 Codex 沙箱（`workspace-write`）写不了主仓库的 `.git`，提交可能要走「越出沙箱」的请求，由 lilad 按档位放行——这两条都没有真跑过。

## 硬性约束和用户的偏好

- **用户只有订阅，没有 API key。** 所有模型调用都必须通过官方 CLI（`claude`、`codex`）。不要提出需要 key 的方案，不要用 `claude -p` 或 Agent SDK 驱动 Claude。
- **lilad 是唯一的写入方**，窗口和命令行都只通过 socket 和它说话。以后的手机端靠这一条。
- **真跑 agent 的测试要省额度**：Claude 用 Haiku（`claude-haiku-5-5`），Codex 用 low 努力程度，放在单独的 `LILA_HOME` 和临时项目目录里，做完停掉自己启动的 daemon 和会话。
- **不要参考 tern 的设计。**
- **不复制 magpie 或任何别的工具的登录令牌**：两家的刷新令牌都会轮换，复制后总会有一边被登出。
- **代码规则**（`AGENTS.md`）：能用成熟的库就用，比如时间用 `chrono`、UUID 用 `uuid`、遍历目录用 `walkdir`、在 PATH 上找命令用 `which`、进程超时用 `wait-timeout`、去终端转义用 `strip-ansi-escapes`、找链接用 `linkify`。
- **界面直接改 GPUI**：窗口本身就是设计稿。改完用 `--features shots` 截图自查，并说明哪些需要用户亲手点。
- **不写说明文字**（用户定的目标：像 Linear 官网、苹果备忘录一样干净）：不加标语、提示、次要标签；看不懂就改布局。
- **工作方式**：
  - 提交信息用英文，遵循 Conventional Commits，末尾加 `Co-Authored-By` 署名行。
  - 只在用户要求时提交和推送。在 `main` 上开发，直接推送到 `main`。
  - 需要用户做决定时，用选择框一次问几个问题，每个选项带说明，推荐的放第一个。

## 另一台电脑上的准备

```sh
git clone git@github.com:jaskang/lila-agent.git && cd lila-agent   # 或 git pull
# libghostty-vt 预编译在 crates/ghostty-sys/libghostty-vt（目前只有 Apple Silicon）
cargo test --workspace                            # 应该是 281 个测试全过
lila daemon stop                                  # 如果有旧版 lilad 在跑
cargo install --path crates/lila                  # 窗口通过 lila 命令启动 lilad
cargo run -p lila-app                             # 或者：cargo install --path crates/lila-app
```

- Rust 版本由 `rust-toolchain.toml` 固定，rustup 会自动装。
- 还需要：
  - herdr-core 的 `hc`（装在 `~/.local/bin/hc`；也可以用 `LILA_HC` 指定路径）；
  - 已经登录的 `claude` 和 `codex`；可选 `omp`；
  - 可选：magpie（`/Applications/magpie.app`），装了的话用量页会读它的账号。
- **旧数据不迁移**：新版第一次打开 v3 的数据库（`~/.local/share/lila/lila.db`）时，把它改名为 `lila.db.v3.bak`，新库从空开始。工作区要在窗口里重新添加（侧栏「工作区」右边的文件夹加号）或 `lila project add <文件夹>`。
- 不随仓库走的东西：账号（`~/.config/lila/accounts.toml` 和 `accounts/`）、数据库、窗口上次显示的会话（`~/.local/state/lila/app-tabs.json`）。`~/.config/lila/roles/` 是旧版留下的，现在没人读。

## 技术栈


- 窗口：GPUI，通过 `gpui-kit 0.9.0`，它锁定 crates.io 上的 `gpui-pre =0.3.11`，不是 herdr-app 打过补丁的 0.3.8。
  - gpui-kit 的组件源码：`~/.cargo/registry/src/*/gpui-component-0.9.0/src/`；输入框 `InputState` 在 `gpui-base-0.9.0/src/input/`。
  - 图标只有 gpui-kit 打包的那一部分能显示，名单见 `target/debug/build/gpui-kit-assets-*/out/icon_name.rs` 里 `__component_icon_names` 之后的部分（比如没有 `Trash`）。缺的图标放进 `crates/lila-app/assets/lila/`，再在 `style::Assets` 里登记（垃圾桶就是这样加的）。
  - **测试模块里不要 `use super::*`**：会把 `gpui::*` 里的 `test` 宏引进来，盖住标准库的 `#[test]`，报的错是「recursion limit」。
- 终端渲染：libghostty-vt 1.0。macOS arm64 用仓库里的预编译库（`crates/ghostty-sys/libghostty-vt/`：官方 xcframework 取 arm64、去掉调试信息，加头文件；升级跑 `crates/ghostty-sys/update-prebuilt.sh <版本>`）。Linux、Windows 还没有预编译库，要放到 `libghostty-vt/<target>/`（可以用 Zig 0.17 从 ghostty 的 `vt-<版本>` tag 交叉编译：`zig build -Demit-lib-vt -Doptimize=ReleaseFast -Dtarget=…`）。注意 `*_get`/`*_set` 的输出是 `void*`，1.0 改了的类型（如 GRAPHEMES_BUF 要 `GhosttyCodepointBuffer`）编译器查不出来，升级时要对照头文件逐个核对。Kitty 图片的位置直接取渲染状态的图片列表（`Vt::placements`，占位符也由库解析）；视图还没有画图片，`placements`/`image_bgra` 目前只有测试在用。终端进程：herdr-core（`hc`）。
- 从 herdr-app（Apache-2.0）移植的代码记在 `THIRD-PARTY.md`。

## 代码地图

完整的表在 `docs/architecture-v4.md` §8，这里是概览：

| crate | 内容 |
|---|---|
| `lila-core` | 纯逻辑，不做 IO：`config.rs`（引擎、项目默认值、project.toml）、`thread.rs`（权限档、会话状态）、`runtime.rs`（各引擎的启动参数） |
| `lila-store` | SQLite：`projects`、`threads`、`conv_items`、`outbox`、`thread_events` |
| `lila-agent` | 对话事件模型和三个驱动的协议翻译：`model.rs`、`claude.rs`（读会话记录）、`codex.rs`（app-server）、`acp.rs`（ACP，用官方类型 crate `agent-client-protocol-schema`） |
| `hc-client` | herdr-core JSON-RPC 客户端 |
| `lila` | 命令行和 lilad。`daemon/`：`mod.rs`（状态、hc 事件）、`threads.rs`（项目和会话的生命周期）、`permissions.rs`（权限卡片）、`conversation.rs`（读对话、发消息、`/` 和 `@` 菜单、`thread.file`）、`host.rs`、`hooks.rs`、`requests.rs`（方法分派）、`jobs.rs`（放在主循环外跑的 git 活：建 worktree、改动、diff、提交检查、合并，以及改动摘要的刷新）、`server.rs`（socket、推送、跑 job 的线程）、`tests.rs`（用假终端；`p2` 模块用临时 git 仓库）。`checkpoint.rs`（每轮的 git 快照、放回、删 ref）、`daemon/checkpoints.rs`（什么时候打、对上消息、回退的前置条件和后续）、`worktree.rs`（会话的 worktree）、`changes.rs`（改动列表、diff、读文件）、`merge.rs`（squash 并合并回起点分支）：都用 git 命令行，有真 git 的测试。`policy.rs`：档位判定。`rules.rs`：「本会话总是允许」的规则。`daemon/trust.rs`：认出并回答 Claude Code 的信任对话框（`daemon/fixtures/` 是真实屏幕文字）。`project.rs`：project.toml 读写。`host.rs`：`lila host codex|omp`。`accounts.rs`、`signin.rs`、`magpie.rs`、`usage.rs`、`sessions.rs`、`commands.rs`。留着没人调用的：`github.rs`、`daemon/sources.rs`（P4） |
| `ghostty-sys`、`lila-term` | libghostty 绑定和终端视图（移植自 herdr-app） |
| `lila-app` | 窗口。`app.rs`：状态、刷新、快捷键、选中的会话或新会话、系统通知。`sidebar.rs`：侧栏。`ui.rs`：标题行（`header_actions`）、中间卡片、权限卡片、终端对话框的提示。`composer.rs`：输入区。`panel.rs`：右面板（文件树和预览、改动和 diff、提交和合并、合并确认框）。`restore.rs`：回到某条消息（气泡上的按钮、dry run 的确认框、「已回到这里」标记和「撤销」）。`conversation.rs`：对话视图、对话/终端切换。`settings.rs`：工作区设置。`prefs.rs`、`usage.rs`：全局设置。`dialog.rs`、`engines.rs`、`style.rs`、`data.rs` |

## 关键机制

- **lilad**：跑在名叫 `lilad` 的 hc 终端里；和命令行、窗口之间用 unix socket，每行一个 JSON；`subscribe` 推送变化。每个会话的引擎跑在自己的 hc 终端里，所以 lilad 重启时会话不断，重启后按终端标签对回会话。
- **三种引擎**：Claude Code 直接跑它的终端界面，lilad 跟读会话记录文件，再加六个 hook（`lila hook`）；Codex 和 omp 由 `lila host` 托管，通过 `host.attach` 把事件推给 lilad。细节见 `architecture-v3.md` 和 `architecture-v4.md` §2。
- **权限**（`architecture-v4.md` §6）：三种引擎的权限请求都转成同一种请求，lilad 先按会话的档位自动回答，答不了的才出卡片；每次自动回答都记在 `thread_events`（`lila thread log`）。
  - **全自动档的危险操作拦截靠识别命令文字，不是沙箱**（Codex 线程有沙箱，Claude 线程没有）：常见写法能拦下（`git push`、删分支、`gh` 的写操作、动工作区外的文件），别名、变量拼出的命令、脚本内部做的事拦不住。已如实告诉用户：它拦常见的误操作，不是安全边界。用户自己配的 allow 规则不再是缺口（见下一条）。Claude 自带沙箱的调查结果在 `architecture-v4.md` §6 末尾：网络不限设置表达不了，所以没启用。
  - Claude 始终以 `--permission-mode manual` 启动，档位由 lilad 的 hook 判定，切档不用重启。**每个工具调用先走 `PreToolUse`**：lilad 回 allow、deny 或 ask；ask 让 Claude Code 弹自己的框，弹框走 `PermissionRequest` hook 变成卡片（两个 hook 靠 `tool_name` 加 `tool_input` 的哈希对上，`PermissionRequest` 没有 `tool_use_id`），所以用户 allow 规则绕不过 lilad。已经在跑的会话没有 `PreToolUse`，结束再恢复才有。lilad 没起来时 hook 什么都不输出，Claude Code 自己决定。
  - Codex 的全自动档是「只能写工作区、不联网」的沙箱加 `on-request`：联网命令会多一轮申请，由 lilad 自动放行；切档在下一轮生效。
- **新文件夹里 Claude Code 先问「信任这个文件夹」**（`architecture-v4.md` §12）：lilad 每 2 秒读这类会话的终端屏幕（`terminal.read`），认得 2.1.322 的这个对话框就标 `waiting_for: "trust"` 和文件夹，窗口那一行写出文件夹，给「信任并继续」（`thread.trust`：先读屏幕，光标移到 Yes 并再读到，再按 Enter；不是这个对话框一个键都不按）。认不出的（登录、别的版本的措辞）仍是「在终端里等你」加「打开终端」。lilad 不会替用户信任，没有「总是信任」的设置。已信任的项目的 worktree 实测不再问。
- **权限卡片不过期、lilad 重启也在**（`architecture-v4.md` §6）：hook 的 timeout 是 2 000 000 秒（Claude Code 的上限约 24.8 天），hook 不设等待上限，lilad 重启后它带请求再问一次、卡片重开；hook 被杀（人在终端里答了）30 秒后卡片丢。已经在跑的会话要结束再恢复才换上新的 timeout。
- **本会话总是允许**（`architecture-v4.md` §6）：只在 `confirm` 档、`danger` 为空、`policy::danger` 也没认出危险的卡片上出现，`rules::derive`/`covers` 每次都先问 `policy::danger`，所以规则吞不掉危险，`confirm` 档里的 `git push` 也没有这个选项。引擎自己的「总是」不再出现在卡片上。
- **会话不注入系统提示词**；用量按会话记录的 session id 归属到会话。
- **账号和用量**（全局设置 → 用量）：
  - 账号写在 `~/.config/lila/accounts.toml`，每个账号一个 CLI 配置目录；会话用当前账号启动（`CLAUDE_CONFIG_DIR` / `CODEX_HOME`）。
  - 额度：Claude 读 `.claude.json` 的 `cachedUsageUtilization`，Codex 读会话记录里的 `rate_limits`。命令行是 `lila accounts`。
  - **添加账号**（做法照 magpie）：后台运行 CLI 自己的登录，在浏览器里打开登录页，不开终端。
    - Claude 是 `claude auth login --claudeai`，用 `$BROWSER` 截下链接，由 Lila 打开；
    - Codex 是 `codex login`，它不认 `$BROWSER`，自己打开浏览器；
    - CLI 正常退出才写进 `accounts.toml`；失败、取消或 10 分钟没完成就删掉目录。
  - **magpie**：装了时，用 `magpie accounts --json` 列出它的账号，代替「默认」卡片；「设为当前」调用 `magpie accounts switch`，让 magpie 换 CLI 自己的登录。Lila 从不复制 magpie 的令牌。代价：切换会影响用户在终端里开的 claude / codex。
- **macOS 红绿灯**：`build.rs` 用 `-platform_version macos 11.0 15.5` 链接，红绿灯在 `(24, 22)`，在侧栏卡片的第一行里，旁边是「Lila」。
- **Claude Code 的全屏模式**：Claude Code（2.1.x）默认是行内渲染，输入框跟在输出后面，滚轮滚的是整个终端，和终端类型无关（试过 `TERM_PROGRAM` 和 XTVERSION 的各种回答）。`CLAUDE_CODE_NO_FLICKER=1` 让它用备用屏幕并接收鼠标事件：输入框固定在底部，滚轮交给它只滚输出区。lilad 给 Claude 会话默认设上。Codex 本来就是全屏。
- **gpui-kit 的 Root 会抢 Tab、Shift-Tab 和 ⌘C**：终端用 `Terminal` 按键上下文把它们绑定成 `NoAction`。
- **窗口自己有焦点**：没有别的元素有焦点时，焦点落在窗口上，这样没有会话时 ⌘, 也能用。

## 界面现状


规范在 `DESIGN.md`（它是唯一的界面规范），接口在 `docs/architecture-v4.md` §9。这里只记怎么接手：

- **结构**：左边侧栏（新会话按钮、工作区、会话，状态一眼能看出：转圈在工作、红点在等你；⌘B 收起），中间标题行（会话标题和「引擎 · 模型 · 努力程度」，右端是在访达中显示等按钮，之后的文件、改动按钮加在 `ui::header_actions`）加对话（Claude Code 会话可切到它的终端）或新会话的输入框；右边的面板（`panel.rs`）：文件、改动，从标题行的两个按钮或 ⇧⌘E、⇧⌘G 打开；GitHub 面板是 P4。
- **新会话**：没有会话被选中时，中间就是新会话的输入框（composer 的 key 是 `new:<项目>`），发出第一条消息才 `thread.new`。引擎、模型、努力程度、权限档的选择在 `app::Draft`，默认值取工作区的 `defaults`。输入框工具条：`+`、权限档、「模型 · 引擎 努力程度」、发送（在跑且框空时是停止）。
- **设置**：全局设置（⌘,）：用量、关于；工作区设置（⇧⌘,）：名称和新会话的默认值（`project.set`）。
- **快捷键**：⌘N 新会话、⌘B 收起或展开侧栏、⌘1–9 第 n 个工作区的最近会话、⌘, 全局设置、⇧⌘, 工作区设置、⌘. 中断、⌘[ 终端切回对话、Esc 关对话框。
- **通知**：`app::notify_changes` 对比两次 `thread.list`；窗口不在前面时，会话开始等你或一轮跑完发 `osascript` 通知。

## 无法点鼠标时怎么验证


这一台的 shell 没有屏幕录制权限，也不能真的点击窗口，但 `--features shots` 的窗口可以截自己、也可以收模拟的键盘和鼠标事件（经过窗口自己的事件分发）。`cargo build -p lila-app --features shots` 之后：

- `LILA_APP_SHOT=/path.png`：每 2 秒把窗口画面存成 PNG（2882 × 1802，不含红绿灯，也截不到真实的毛玻璃）。
- `LILA_APP_FAKE=/path.json`：**不起 lilad**，用造好的数据画界面。JSON 是 `{projects, threads, conversations}`（可选 `daemon`：造 `ping` 的 `{version, protocol}`，协议不是 `lila::PROTOCOL` 或没有就出现「lilad 版本不符」和重启按钮；`restart: "fixed"`：点了重启后恢复正常，否则变成「lila 命令需要重装」，点击不碰真 lilad）：前两个和 `project.list`、`thread.list` 的返回一样，`conversations` 是会话 id 到 `thread.read` 返回的 `items`、`permissions`、`writing`、`config`。任何状态（在等你、归档、已结束、权限卡片、终端里的对话框……）都能这样造；这时窗口里的操作发不出去（没有 lilad）。
- `LILA_APP_THREAD=<id>`：显示这个会话；`LILA_APP_SIZE=1000x680`：窗口这么大（看窄窗口）；右面板的状态在 `<LILA_HOME>/state/app-panel.json`（`{"open":true,"view":"changes","width":380}`）；假数据文件里还可以有 `changes`、`diffs`、`file`、`commit`、`merge`、`log`、`new`（见 `data::fake_act`），`LILA_APP_FAKE_READ_MS` 让面板的读和回退的 dry run 变慢（`LILA_APP_FAKE_SEND_MS` 让提交、合并和真回退变慢，截「回退进行中」就用它），`trust`（`thread.trust` 的回复，或 `{"error": "…"}`；会话要 `waiting: "terminal"`、`waiting_for: "trust"`、`trust_folder`，`LILA_APP_FAKE_SEND_MS` 让按钮转圈更久），`conversations` 里的 `rules`（`thread.read` 的规则列表，点权限档按钮就看得到：`LILA_APP_CLICK`），`restore`（`{"dry_run": …, "restore": …}`，是 `thread.restore` 带和不带 `dry_run` 的回复，或 `{"error": "…"}`；按检查点编号分开写 `{"3": {"dry_run": …}}`；会话的 `restoring: true` 就是「正在放回」，对话里的 `checkpoints` 和 `from: "restored"` 条目直接写在 `conversations` 里，见 `data::fake_act`），`LILA_APP_NEW=<项目>,engine=codex,model=…,effort=…,permission=confirm`：打开新会话的输入框并选好；`LILA_APP_ADD=<文件夹>`：像选了文件夹那样新建工作区。
- `LILA_APP_CLICK="x,y;right:x,y;move:x,y;keys:cmd-n h i enter;wait:ms"`：启动 3 秒后依次点击（窗口里的点，不是截图的像素：截图是 2 倍，再缩放过的话自己换算）、右击、移动、按键（打字也行）、等待。点下拉菜单、右键菜单、对话框里的控件都靠它；真的 lilad 加它可以走完「新会话 → 发消息 → 回答 → 结束」。
- `LILA_APP_KEYS="cmd-, cmd-shift-,"`：启动后 1.5 秒按这些键，用来验证快捷键。
- `LILA_APP_INACTIVE=1` 加 `LILA_APP_NOTIFY_LOG=<文件>`：窗口当作不在前面，通知写进文件而不是真的发。
- `LILA_APP_PREFS=usage|about`；`LILA_APP_SIGNIN=claude:<名称>`（直接开始添加账号，`LILA_APP_NO_OPEN=1` 不打开浏览器，截完图要 `pkill -f "claude auth login"`）；`LILA_APP_CONV=top,row:60,open:25,openall:2,focus,key:h,key:enter,fill:<话>,attach:<路径>,menu,show:<会话>`（对话里的状态，见 `conversation::dev_conv`、`composer::dev_composer`）；`LILA_APP_GUTTERS=1`、`LILA_APP_DEBUG=1`。

真跑要省额度，放在单独的 `LILA_HOME`：

1. `LILA_HOME=/tmp/x target/debug/lila project add <一个 git 文件夹>`（这会在 hc 里起一个 lilad）；`lila project set --model claude-haiku-5-5 --effort low`；
2. `LILA_HOME=/tmp/x LILA_APP_SHOT=… LILA_APP_CLICK="keys:cmd-n;wait:300;keys:h i enter;wait:12000" target/debug/lila-app`；
3. 新文件夹里 Claude Code 先问「信任这个文件夹」：几秒内会话那一行变成「信任此文件夹」，点「信任并继续」（或 `lila thread trust <id>`）；
4. 收尾：`lila thread stop <id>`、`lila daemon stop`（会话的终端不随 lilad 结束，要逐个 stop）。

**显示器睡着时截不到新画面**：窗口不重绘，PNG 一直是第一帧（多半是空的）。先看 `pmset -g assertions` 或 `system_profiler SPDisplaysDataType | grep Asleep`。刚编译出的二进制第一次启动慢，至少等 8 秒再截。

macOS 没有 `timeout`：`prog & P=$!; perl -e 'select(undef,undef,undef,6)'; kill $P`；裁剪截图用 `sips -c H W --cropOffset Y X`（单位是截图的像素）。

## 等用户亲手验证的

都过了测试和截图，但要用鼠标、键盘或真实环境确认：

- **P1b 的窗口（项目 → 会话，2026-11-10）**：模拟的点击和按键、真跑一个 Haiku 会话都过了，下面这些要用真的鼠标和键盘点一遍：
  - 新建工作区的系统文件夹选择框；
  - 中文输入法在输入框、重命名、工作区名称里；复制粘贴（含粘贴图片）、拖文件进对话区；
  - 点击和右键的手感：侧栏会话行、工作区行（折叠，悬停出现 `+`）、菜单、对话框外点一下关闭、拖动窗口、红绿灯；
  - 系统通知：窗口放到后面，让一个会话等权限或跑完一轮（`osascript` 的通知在系统设置里要允许「脚本编辑器」之类的来源）；
  - 权限确认卡片在真引擎上的完整一轮（Claude 的 `confirm` 档、Codex 的 `git push`）；Codex 和 omp 的新会话没真跑过；
  - 窗口缩到很窄时侧栏、标题行、输入工具条的挤压。
- **omp 会话**：P1 里没有真跑过，只有启动参数的单元测试。
- **账号**：在浏览器里添加 Claude 和 Codex 账号，走完整个登录；magpie 账号的「设为当前」（会真的切换 CLI 的登录）；已经在跑的会话要结束再继续才会换过去。
- **终端**（Claude 会话切到终端后）：Tab 和 Shift-Tab、平滑滚动、中文输入法、复制粘贴、⌘点击链接。

- **A1 窗口（2026-11-11）**，要真鼠标和真引擎才能确认的：
  - 在真 lilad 上点一遍：悬停出按钮的手感（整行都算悬停，按钮在气泡左边一直占着位置）、tooltip、点了转圈到确认框出来的时间（大仓库 dry run 要几秒）、确认后「回到这里」到文件真的变回来、右面板（改动、文件）跟着刷新、红字失败提示；撤销一次，再撤销回去；
  - 真文件夹里：回退后对话末尾转圈多久、输入框是否真的发不出去（lilad 也拒绝）；
  - Claude 会话里用户消息和检查点的 `item` 对不上时（见 A1 后端「没验证」）按钮就不出现，这是窗口的预期行为，但要看真实对话里多常发生；
  - 回退后给 agent 的备注在下一条消息里的效果（后端那边的问题，要真引擎才看得出）；
  - 真键盘：确认框里 Esc 取消（模拟的 Esc 在 `--features shots` 里关掉了框）。

- **A2 前半（2026-11-11）**，要在真实使用里确认：
  - 重启 lilad 并把在跑的 Claude 会话结束再恢复，让它带上 2 000 000 秒的 timeout 和会重发请求的 hook；之后走开几个小时再回来答一张卡片；
  - 窗口里真点：卡片上的「本会话总是允许 …」、权限档菜单里的规则行和 ×、新项目里的「信任并继续」（点了之后排队的第一条消息发出）；
  - 真 Codex 和 omp 上的「总是」：卡片上只剩一个「总是」，点了之后引擎收到的是普通允许，下一次同样的命令由规则答（`lila thread log` 里看 `· rule:`）；
  - 日常用中规则有没有太窄或太宽：`confirm` 档下 `cargo test 2>&1 | tail` 这类一行要两条规则（`cargo test`、`tail`）；`rm`、`python`、`bash -c` 永远要问；
  - 第一次在没信任过的仓库里开 worktree 会话时 Claude Code 问的是哪个文件夹；
  - 通知文字：信任对话框出现时侧栏和系统通知仍写「终端在等你」。

- **A1 判定（2026-11-11）**，要在真实使用里确认：
  - 重启 lilad（`lila daemon restart`）后，把一个在跑的 Claude 会话结束再恢复，让它带上 `PreToolUse`；平时的编码里看 `lila thread log <会话>`：每个改动和命令一条 `auto_allowed`，没有重复；看东西的工具没有记录；
  - 自己 `~/.claude/settings.json` 里的 allow 规则下，`confirm` 档还是出卡片、`read_only` 档还是拒；
  - Claude Code 以后新增的内部工具：`confirm` 档下它会出卡片、`read_only` 档下被拒，到时加进 `policy::INERT`；
  - 在 `full` 档里让它读工作目录以外的文件、跑带变量的命令，看有没有多余的确认框。

## 待办和已知问题

- **路线图**见上面「进度和下一步」和 `product-v4.md` §8。
- **P1 留下的**：
  - （A2 已改）在 Claude Code 没信任过的文件夹里开会话，窗口现在认出「信任此文件夹」并能在原处答（只认 2.1.322 的文字，措辞变了就退回「在终端里等你」）；登录等别的终端对话框仍只显示「在终端里等你」。权限卡片 570 秒后退回终端的问题也已改（不过期，见上）。
  - Claude 会话的用户消息约 0.4 秒后才进会话记录（hc 粘贴 0.24 秒 + 跟读），窗口靠乐观显示盖住；回复靠 `MessageDisplay` hook 先显示，Stop 后保留到会话记录里有了为止。
  - Codex 偶尔停在工具结果之后不收尾（v3 就有的老问题）：120 秒的催促兜底没解决，发一条消息才继续。根因没查清。
  - Codex 的 MCP 工具审批和 `item/permissions/requestApproval`（模型主动申请额外权限）目前直接回「不支持」，不问人。P3 接 MCP 工具层时必须处理。
  - 角色预设（引擎、模型、努力程度、附加提示词的组合）整个推迟了，没留数据结构。
  - ACP 引擎只内置 omp；再接一个要在 `lila_core::Runtime` 加一项。
  - 工作区按 id 排序；折叠的工作区和「显示已归档」不记住（侧栏收起记住，`app-sidebar.json`）；会话标题行不显示项目名。
  - `README.md` 里的窗口截图（`docs/images/window.png`）还是旧窗口，等用户满意后换成真实截图。
  - Claude 会话的终端视图由窗口直接连 hc（和以前一样），是「只走 lilad 的 socket」的唯一例外，做远程访问时要处理。
- **更早留下的**：
  - Claude 额度用完时 lilad 还发现不了（会话记录里没有额度信息）；可以考虑读 `.claude.json` 或 magpie。
  - Option 当 Alt 用；kitty 图片（用户决定先不做）；Linux、Windows 的 libghostty 预编译库。
