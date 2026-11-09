# 交接说明（给接手的模型）

更新于 2026-11-07。用户用中文交流，回复也用中文。用户会换一台电脑继续，那台电脑上没有这一台的 Claude 记忆，所以该知道的都写在这里。

先读这份。需要细节时再读：
- `docs/product-v3.md`：下一阶段的产品方案（请求为中心、多来源、worktree 槽），M1 起按它做。
- `docs/architecture-v2.md`：参考四篇文章（README 末尾）重新评估后的架构，分阶段落地。阶段 A–E 已完成，记录在 §10–§14；§15 是第一版上线前的精简。
- `DESIGN.md`（仓库根目录）：界面规范。第一条规则：不写说明文字，看不懂就改布局。
- 各阶段的旧记录（m1–m3、spikes、最初的 design.md）和 HTML 原型已经从仓库里去掉，要查就看 git 历史。
- `AGENTS.md`（仓库根目录）：项目规则，包括用中文交流，以及优先用社区成熟的库、不自己造复杂的轮子。

## 这是什么

Lila 是一个管理编程 agent 团队的桌面工具。

- **角色**：每个角色有 CLI、模型、努力程度和预设提示词，放在 `~/.config/lila/roles/`。默认五个：
  - `pm`：PM（项目经理），Claude Sonnet 5.5 high。负责对接用户、拆任务、分派、汇报。工具由 CLI 限制（角色文件里的 `[tools]`）：只能读代码、运行 `lila` 和只读的 git 命令，不能改文件。每个项目有且只有一个，这个角色不能删。
  - `designer`：设计师，Opus 5.5 high。
  - `architect`：架构师，Opus 5.5 high。
  - `engineer`：开发工程师，Sonnet 5.5 high。
  - `tester`：测试工程师，Codex `gpt-6.1-sol` high。
- **项目**：一个 PM 加若干成员。成员可以设 `count`，同时开几个（`engineer`、`engineer-2`……），派活时写 `--to engineer`，由 Lila 挑一个。
- **用户只和 PM 对话**。PM 建任务，lilad 按流程推进（实现 → 检查 → 验证 → 返工），做完后给用户一张验收卡片。
- **界面仿照 Herdr app**：能看到每个成员在做什么，可以直接操作成员的终端。

## 硬性约束和用户的偏好

- **用户只有订阅，没有 API key。** 所有模型调用都必须通过官方 CLI（`claude`、`codex`），成员就是跑在终端里的 CLI。不要提出需要 key 的方案。
- **真跑 agent 的测试要省额度**：Claude 用 Haiku（`claude -p --model claude-haiku-5-5`），codex 加 `-c model_reasoning_effort=low`，放在单独的 `LILA_HOME` 或临时目录里。
- **不要参考 tern 的设计。**
- **省 token 只看 Lila 自己造成的消耗**：不算 CLI 自带的工具、MCP 和 skills。**并行的结果不要攒着一起发**：用户明确否决过，做完一件就通知一件；只有已经在排队的消息可以合成一条。
- **不复制 magpie 或任何别的工具的登录令牌**：两家的刷新令牌都会轮换，复制后总会有一边被登出（见下文「magpie」）。
- **代码规则**（`AGENTS.md`）：能用成熟的库就用，比如时间用 `chrono`、UUID 用 `uuid`、遍历目录用 `walkdir`、在 PATH 上找命令用 `which`、进程超时用 `wait-timeout`、去终端转义用 `strip-ansi-escapes`、找链接用 `linkify`。2026-11-06 已经按这条规则梳理过一遍。
- **界面直接改 GPUI**：HTML 原型在 2026-11-07 去掉了，窗口本身就是设计稿。改完用 `--features shots` 截图自查，并说明哪些需要用户亲手点。
- **不写说明文字**（2026-11-07 用户定的目标：像 Linear 官网、苹果备忘录一样干净）：不加标语、提示、次要标签；看不懂就改布局。每个决定都要同时让用户、开发者、agent 更好用。
- **工作方式**：
  - 提交信息用英文，末尾加 `Co-Authored-By` 署名行。
  - 只在用户要求时提交。在 `main` 上开发，用户已同意直接推送到 `main`。
- **v2 架构里用户做过的决定**（`docs/architecture-v2.md` §9，全部按建议）：
  - PM 用 Sonnet，并由 CLI 限制工具；
  - 交接由 lilad 按流程模板推进；
  - 有验收关口，积压上限 3；
  - PM 上下文到约 12 万 token 时换新会话（接受滚动历史被清空）；
  - worktree 按链分，验收时用 squash 合并：后来决定第一版不做 worktree，阶段 E 改成同一时间一条链、验收时提交（§14）。

## 家里电脑上的准备

```sh
git clone git@github.com:jaskang/lila-agent.git && cd lila-agent
# Apple Silicon 不用 Zig 和子模块：libghostty-vt 预编译在 crates/ghostty-sys/libghostty-vt。
# 其他平台：git submodule update --init --depth 1 vendor/ghostty，再装 Zig 0.17（ZIG=…/zig）
cargo test --workspace                            # 应该是 247 个测试全过
cargo install --path crates/lila                  # 窗口通过 lila 命令启动 lilad
cargo run -p lila-app                             # 或者：cargo install --path crates/lila-app
```

- Rust 版本由 `rust-toolchain.toml` 固定为 1.99.1，rustup 会自动装。
- 还需要：
  - herdr-core 的 `hc` 0.1.18（这一台装在 `~/.local/bin/hc`；也可以用 `LILA_HC` 指定路径）；
  - 已经登录的 `claude` 和 `codex`； 
  - 可选：magpie（`/Applications/magpie.app`），装了的话用量页会读它的账号。
- 已经在跑旧版 lilad 的话，装新版后要 `lila daemon stop`，让窗口或 `lila up` 再启动新的。数据库打开时会自动加上新的列和表。
- **不随仓库走的东西**（家里电脑上要重新生成，或者是空的）：
  - 角色 `~/.config/lila/roles/`：第一次 `lila init` / `lila up` 时装上默认的五个角色。
  - 账号 `~/.config/lila/accounts.toml` 和 `~/.config/lila/accounts/`：没有时每个 CLI 只有用它自己目录的「默认」账号。
  - 数据库 `~/.local/share/lila/lila.db`：任务、收件箱、动态、流程运行、会话账本。
  - 窗口上次打开的项目 `~/.local/state/lila/app-tabs.json`。
- 这一台用来看效果的项目是 `~/temp` 和 `creator-studio`；家里电脑上用窗口里的新建项目向导（侧栏「项目」右边的 `+`）或 `lila init` 另建。

## 技术栈

- 窗口：GPUI，通过 `gpui-kit 0.9.0`，它锁定 crates.io 上的 `gpui-pre =0.3.11`，不是 herdr-app 打过补丁的 0.3.8。
  - gpui-kit 的组件源码：`~/.cargo/registry/src/*/gpui-component-0.9.0/src/`；输入框 `InputState` 在 `gpui-base-0.9.0/src/input/`。
  - 图标只有 gpui-kit 打包的那一部分能显示，名单见 `target/debug/build/gpui-kit-assets-*/out/icon_name.rs` 里 `__component_icon_names` 之后的部分（比如没有 `Trash`）。缺的图标放进 `crates/lila-app/assets/lila/`，再在 `style::Assets` 里登记（垃圾桶就是这样加的）。
  - **测试模块里不要 `use super::*`**：会把 `gpui::*` 里的 `test` 宏引进来，盖住标准库的 `#[test]`，报的错是「recursion limit」。
- 终端渲染：libghostty-vt 1.0。macOS arm64 用仓库里的预编译库（`crates/ghostty-sys/libghostty-vt/`：官方 xcframework 取 arm64、去掉调试信息，加头文件；升级跑 `crates/ghostty-sys/update-prebuilt.sh <版本>`，并把子模块切到同一个 tag `vt-<版本>`）。Linux、Windows 还没有预编译库，从子模块 `vendor/ghostty` 用 Zig 0.17 编译。注意 `*_get`/`*_set` 的输出是 `void*`，1.0 改了的类型（如 GRAPHEMES_BUF 要 `GhosttyCodepointBuffer`）编译器查不出来，升级时要对照头文件逐个核对。Kitty 图片的位置直接取渲染状态的图片列表（`Vt::placements`，占位符也由库解析）；视图还没有画图片，`placements`/`image_bgra` 目前只有测试在用。终端进程：herdr-core（`hc`）。
- 从 herdr-app（Apache-2.0）移植的代码记在 `THIRD-PARTY.md`。

## 代码地图

| crate | 内容 |
|---|---|
| `lila-core` | 纯逻辑，不做 IO。`config.rs`：角色（含 `[tools]`）、ProjectConfig、`[settings]`、成员池、Team::build、`flow_for`。`flow.rs`：流程模板和 `after`（下一步做什么）。`task.rs`：状态机、Brief（原话、验收标准、不做什么）、Outcome（决定、没验到的、verdict）。`prompt.rs`：系统提示词和协议，PM 还有 Flows 一节。`runtime.rs`：claude/codex 启动参数（工具限制、`--session-id`、`--resume`）。`dispatch.rs`：派发、沉默判定、`pick_slot`。`route.rs`：完成通知给谁。`message.rs`：所有打进终端的文字（投递、通知、摘要等，英文）。`inbox.rs`：给人看的收件箱文字（验收卡片等，中文）。 |
| `lila-store` | SQLite：projects、members（含 `session`）、tasks（含简报、结果、run）、task_events、artifacts、inbox、outbox、runs（流程运行、预算、暂停）、sessions、session_chains。新列在 `ADDED_COLUMNS` 里，打开旧数据库时自动加上。 |
| `hc-client` | herdr-core JSON-RPC 客户端 |
| `lila` | CLI 和 lilad，详见下面几行 |
| ↳ `daemon/` | lilad。`mod.rs`：状态、herdr-core 的事件、pump（投递和通知）、spool。`requests.rs`：`Daemon::handle` 和每个请求。`flows.rs`：流程推进、检查命令。`session.rs`：换链时的会话、PM 摘要。`review.rs`：验收卡片、一条链占目录、验收时提交、预算。`watch.rs`：Watchdog 和 `tick`。`server.rs`：socket、事件流和后台线程。`tests.rs` 用 Fake 终端，CLI 的会话记录目录指向临时目录 |
| ↳ `check.rs` | 后台跑流程的检查命令（进程组、超时） |
| ↳ `worktree.rs`、`commit.rs` | worktree 槽的 git 命令（占槽、放槽、取消时保留）；验收时提交，从槽里合成一个提交、rebase、快进合并（`docs/product-v3.md` §7.1） |
| ↳ `github.rs`、`daemon/sources.rs` | GitHub 来源（只用 `gh`）：定时拉 issue 建成给 PM 的任务、`lila triage` 和「要修吗？」卡片、`lila draft` 和评论草稿、发评论；验收后开 PR 在 `commit.rs`（`docs/product-v3.md` §8） |
| ↳ `daemon/slots.rs` | 哪条链在哪个槽、排队占槽、链结束后放槽或给验收卡片、合并失败的 `merge` 条目 |
| ↳ `sessions.rs` | Claude 会话记录的位置、找 Codex 的会话、读上下文大小 |
| ↳ `accounts.rs` | 订阅账号（`accounts.toml`）、成员用哪个账号、读每个账号的额度 |
| ↳ `signin.rs` | 添加账号时在浏览器里登录（后台运行 CLI 自己的登录） |
| ↳ `magpie.rs` | 读 magpie 的账号、通过 magpie 切换 |
| ↳ `usage.rs` | 按成员统计 token（读两个 CLI 的会话记录）、时间解析 |
| ↳ 其他 | `settings.rs`（项目设置读写，用 toml_edit 保留注释）、`roles.rs`（角色和默认角色升级）、`init.rs`、`client.rs`、`paths.rs`、`skill.md`（给 agent 的协议说明） |
| `ghostty-sys`、`lila-term` | libghostty 绑定和终端视图（移植自 herdr-app） |
| `lila-app` | 窗口。`app.rs`：状态、刷新、快捷键、窗口自己的焦点、选中的请求、离开期间和通知。`ui.rs`：侧栏卡片、中间卡片（标题行、终端）、收件箱卡片（含验收、预算、合并）。`requests.rs`：请求列表和右边的请求详情。`style.rs`：配色和小部件。`tree.rs`：成员树动画。`data.rs`：拉数据、本地时间。`dialog.rs`：对话框外壳（`footer: false` 不显示保存栏）。`settings.rs`：项目设置。`prefs.rs`：全局设置（角色 / 用量 / 关于）。`usage.rs`：用量页（账号、浏览器登录、magpie）。`wizard.rs`（新建项目）、`detail.rs`（任务详情）、`compose.rs`（新建任务）、`build.rs`。 |

## 关键机制（v2 之后）

- **lilad 是唯一写状态的进程**：跑在名叫 `lilad` 的 hc 终端里；和 CLI、窗口之间用 unix socket 通信，每行一个 JSON；`subscribe` 推送变化。codex 沙箱进不了 socket，走 `.lila/spool/` 文件通道。成员状态来自 OSC 7501。
- **PM 的工具由 CLI 限制**（§10）：默认 PM 只有 Read、Grep、Glob、Bash；`lila` 和只读 git 不用确认，其他 Bash 命令要用户确认。
- **原话和验收标准**（§10）：`lila task new --quote/--accept/--out-of-scope`。投递时，lilad 把链根的原话和一路上设过的验收标准合在一起附上，tester 看到的是用户原话。用户派的任务本身就是原话。`done` 可以带 `--decision`、`--unverified`。
- **流程**（§11，`lila_core::flow`）：
  - PM 或用户派给 engineer 的活默认走 `code` 流程：实现 → `[settings] check` 检查命令（lilad 自己跑）→ tester 验证（`--verdict pass|fail`）。
  - 失败时由 lilad 退回给写代码的人；超过 `max_rounds` 交给 PM。
  - `quick` 流程不经过 tester；`--flow none` 不走流程；项目可以在 `[flows.<名字>]` 里自定义流程。
  - 流程里的成员不能自己交接给流程里的人。进度存在 `runs` 表里。
- **会话**（§12）：
  - Claude 成员启动时由 lilad 指定 `--session-id`；Codex 的会话从会话记录目录里认出来。
  - 换链时：这条链以前有会话就续接（`--resume`）；上下文小于 `fresh_after_tokens`（6 万）就沿用当前会话；否则开新会话。
  - PM 上下文 ≥ `pm_refresh_tokens`（12 万）并且空闲 10 分钟后换新会话，第一条消息是 lilad 从数据库生成的 `<lila digest>` 项目摘要；用户手动重启 PM 时也发。
- **验收和 Watchdog**（§13）：
  - 用户派的工作完成后，收件箱里出现「等你验收」卡片，按时间列出每一步的证据。「通过」就结束；写意见就作为「Rework:」任务交给 PM。
  - 等验收的达到 `max_unreviewed`（3）时，PM 新开的链先排队。
  - 成员工作超过 `runaway_minutes`（30）没调用 `lila`：先提醒，再进收件箱。
  - `--budget 300k`：流程超预算时在步骤之间暂停，等用户选「继续」（预算翻倍）或「取消」。
  - Codex 额度用完时，给 Codex 成员的任务排队到重置时间；装了 magpie 时用 magpie 读到的当前账号额度。
- **一条链占目录、验收时提交**（§14）：
  - `max_chains`（1）：一条链从成员拿到第一个任务到你验收为止占着项目目录，别的链排队。
  - `commit_on_accept`（开）：点「通过」时 lilad `git add -A` 并提交一次，失败进收件箱。
- **账号和用量**（全局设置 → 用量）：
  - 账号写在 `~/.config/lila/accounts.toml`，每个账号一个 CLI 配置目录；成员用当前账号启动（`CLAUDE_CONFIG_DIR` / `CODEX_HOME`）。
  - 额度：Claude 读 `.claude.json` 的 `cachedUsageUtilization`，Codex 读会话记录里的 `rate_limits`。命令行是 `lila accounts`。
  - **添加账号**（做法照 magpie）：后台运行 CLI 自己的登录，在浏览器里打开登录页，不开终端。
    - Claude 是 `claude auth login --claudeai`，用 `$BROWSER` 截下链接，由 Lila 打开；
    - Codex 是 `codex login`，它不认 `$BROWSER`，自己打开浏览器；
    - CLI 正常退出才写进 `accounts.toml`；失败、取消或 10 分钟没完成就删掉目录。
  - **magpie**：装了时，用 `magpie accounts --json` 列出它的账号，代替「默认」卡片；「设为当前」调用 `magpie accounts switch`，让 magpie 换 CLI 自己的登录。Lila 从不复制 magpie 的令牌。代价：切换会影响用户在终端里开的 claude / codex。
- **完成通知只给在等的人**（`lila_core::route`）：一条链只叫醒 PM 一次；已经在排队的通知合成一条发。
- **成员覆盖角色设置**：`[[members]]` 里可以写 `runtime`、`model`、`effort`、`prompt_file`、`count`；`project.reload` 重新读配置；`member.restart` 用新设置重启。
- **默认角色升级**：`defaults/roles/previous/` 存着旧版默认文件，用户没改过的会自动换成新版。
- **macOS 红绿灯**：`build.rs` 用 `-platform_version macos 11.0 15.5` 链接，红绿灯在 `(24, 22)`，在侧栏卡片的第一行里，旁边是「Lila」。
- **Claude Code 的全屏模式**：Claude Code（2.1.x）默认是行内渲染，输入框跟在输出后面，滚轮滚的是整个终端，和终端类型无关（试过 `TERM_PROGRAM` 和 XTVERSION 的各种回答）。`CLAUDE_CODE_NO_FLICKER=1` 让它用备用屏幕并接收鼠标事件：输入框固定在底部，滚轮交给它只滚输出区。lilad 给 Claude 成员默认设上，角色的 `env` 里写 `CLAUDE_CODE_NO_FLICKER = "0"` 可以关掉。Codex 本来就是全屏。
- **gpui-kit 的 Root 会抢 Tab、Shift-Tab 和 ⌘C**：终端用 `Terminal` 按键上下文把它们绑定成 `NoAction`。
- **窗口自己有焦点**：没有别的元素有焦点时，焦点落在窗口上，这样团队没启动时 ⌘, 也能用。

## 界面现状

- **风格**（`style.rs` 是唯一来源）：
  - 石墨色毛玻璃窗口，内边距 8，卡片之间的缝是 8，也是拖动改宽度的把手。
  - 12px 圆角卡片，靠填充色区分层次。左、中、右三栏都是卡片，侧栏卡片的第一行是红绿灯、「Lila」和设置按钮。
  - 字号：正文 13、次要 12、说明 11。
- **布局**（2026-11-09 起以请求为中心，`docs/product-v3.md` §7.3）：
  - 侧栏是项目列表，加上两层的成员树（PM 在根上，界面上叫「助理」）。暂停派发在项目右键菜单里。
  - 中间一张卡片：一行标题和请求列表（`requests.rs`，按「下一步该谁动」分组）。点成员时换成它的终端，标题行是项目 › 成员、状态胶囊、上下文、整理上下文、搜索、项目设置、面板开关；点项目名或 ⌘[ 回到列表。
  - 右边：选中的请求（要你处理的卡片、决定、没验证、步骤、经过）。
  - 离开超过 10 分钟回来，列表顶部有一行回顾；窗口在后面时新的「需要你」发系统通知。
- **收件箱卡片**：PM 在问你、终端在等你、等你验收（通过 / 写意见打回）、超出预算（继续 / 取消）、额度用完、可能在打转等。文字来自 `lila_core::inbox`（中文）；给成员的消息在 `lila_core::message`（英文，协议）。
- **设置**：
  - 全局设置（⌘,）：角色、用量、关于。
  - 项目设置（⇧⌘,）：通用（名称、目录、运行规则）、成员。指派规则只在 `.lila/project.toml` 的 `[assign]` 里。
  - 新建项目：一页（文件夹、名称、团队、简介）；编号由文件夹名生成，不是 git 仓库就自动 `git init`，没装 CLI 的角色不能勾。
  - 都是居中对话框，用户确认过全局和项目设置保持两个。
- **快捷键**：⌘N 新建任务、⌘1–9 按侧栏顺序切换项目、⌘, 全局设置、⇧⌘, 项目设置、⌘I 右侧面板、Esc 关闭。

## 无法点鼠标时怎么验证

这一台的 shell 没有屏幕录制权限，也不能点击窗口。`cargo build -p lila-app --features shots` 之后可以用这些环境变量：

- `LILA_APP_SHOT=/path.png`：每 2 秒把窗口画面存成 PNG（不含红绿灯，也截不到真实的毛玻璃）。
- `LILA_APP_KEYS="cmd-, cmd-i"`：启动后按这些键，经过窗口自己的按键分发，用来验证快捷键。
- `LILA_APP_PREFS=roles|usage|about`、`LILA_APP_SETTINGS=general|members`（加 `LILA_APP_SETTINGS_ADD=<角色>`）。
- `LILA_APP_SIGNIN=claude:<名称>`：直接开始添加账号；`LILA_APP_NO_OPEN=1` 不打开浏览器。截完图要 `pkill -f "claude auth login"`。
- `LILA_APP_REQUEST=T3`（选中请求）、`LILA_APP_MEMBER=pm`（中间显示终端）、`LILA_APP_AWAY=360`（像离开了 6 小时回来）。
- `LILA_APP_FAKE_STATUS=designer:working,tester:blocked`、`LILA_APP_OPEN_TASK=T5`、`LILA_APP_COMPOSE=<成员>`、`LILA_APP_NEW=<文件夹>`（空：还没选文件夹）、`LILA_APP_SWITCH=a,b`、`LILA_APP_GUTTERS=1`、`LILA_APP_DEBUG=1`。

截图用临时的 `LILA_HOME`，不碰用户的真实数据：
1. 建一个目录并 `git init`，在里面运行 `LILA_HOME=<home> target/debug/lila init`；
2. 用 `lila-store` 源码里的 SCHEMA 建 `<home>/data/lila.db`，再插入 `projects` 行 `(id, cwd, created_ms)`；要看收件箱就插 `inbox` 行；
3. 窗口的数据来自 lilad，所以要先 `LILA_HOME=<home> target/debug/lila daemon run &`，截完 `lila daemon stop`；
4. 收件箱的 `options` 用 `char(31)` 分隔，正文的换行用 `char(10)`。

**显示器睡着时截不到新画面**：窗口不重绘，PNG 一直是第一帧（多半是空的）。先看 `pmset -g assertions` 或 `system_profiler SPDisplaysDataType | grep Asleep`。刚编译出的二进制第一次启动慢，至少等 8 秒再截。

macOS 没有 `timeout`：`prog & P=$!; perl -e 'select(undef,undef,undef,6)'; kill $P`；裁剪截图用 `sips -c H W --cropOffset Y X`。

## 等用户亲手验证的

都过了测试和截图，但要用鼠标、键盘或真实环境确认：

- **v2 的流程真跑一轮**（2026-11-07 用 Haiku 跑通了 PM → engineer → 检查 → tester 验证 → 验收 → 提交，tester 收到了原话和验收标准；检查失败退回给 engineer、Codex 成员当 tester 也跑通了，见 architecture §15）：先 `cargo install --path crates/lila`、`lila daemon stop`，在设了 `[settings] check = "…"` 的项目里让 PM 做个小需求。要看的是：
  - PM 有没有试图跑被拦下的命令；
  - tester 收到的任务里有没有用户原话；
  - 检查失败时是否退回给 engineer；
  - 最后收件箱里是否出现验收卡片。
- **会话**：换链时的续接或沿用；PM 上下文变大后，换新会话并收到摘要；`codex resume` 没真跑过。
- **账号**：
  - 在浏览器里添加 Claude 和 Codex 账号，走完整个登录；
  - magpie 账号的「设为当前」，会真的切换 CLI 的登录；
  - 已经在跑的成员要重启后才会换过去。
- **窗口**：
  - 团队没启动时按 ⌘,；
  - 验收卡片的通过和打回、预算卡片；
  - 拖动面板和右键菜单；红绿灯的位置（截图里没有红绿灯）；
  - 设置对话框里的各种控件；
  - 新建项目的「创建」（会 `git init`、写配置、启动团队）。
- **终端**：Tab 和 Shift-Tab、平滑滚动、中文输入法、复制粘贴、⌘点击链接。

## 待办

- **产品方案 v3**（`docs/product-v3.md`，2026-11-09 草案）：主界面改成以请求为中心，来源是手动、GitHub、Asana；M1 先做 worktree 槽和请求列表，不接外部渠道。取代下面这条。
- **阶段 E 的 worktree**（`docs/architecture-v2.md` §4.8）：第一版没做，改成一条链占目录、验收时提交（§14）。以后要真正并行时再做每条链一个 worktree。
- PM 自己开的链（终端里提的需求）没有验收卡片，也就不会提交（§14 已知限制）。
- **2026-11-07 的精简**见 `docs/architecture-v2.md` §15。
- **窗口里已经补上的**（2026-11-06）：任务详情里的流程进度、原话、验收标准、不做、决定和没验到的；任务列表里的当前步骤；终端卡片头的上下文大小；PM 的「整理上下文」（就是 `member.restart`，新会话收到摘要）；角色页的「预览最终提示词」（`lila::roles::preview`）；项目右键「从列表移除」（`project.remove`：停团队、删数据库记录，目录不动）。布局改成三栏卡片，去掉了标签页。
- **已知限制**：
  - Claude 额度用完时 lilad 还发现不了，它的会话记录里没有额度信息；可以考虑在 lilad 里读 `.claude.json` 或 magpie。
  - 积压上限只挡 PM 新开的链，挂到旧链下面的新工作挡不住。
  - 预算只在步骤之间检查。
  - 成员池在建任务时就选定成员，不会改派给后来空出来的那个。
- **更早的**：
  - Option 当 Alt 用（下一个要做）；
  - kitty 图片（用户决定先不做）；
  - 项目编号改名：不做，用户认为现有的显示名（`title`）就够了；
  - 远程机器（M3 第 7 项）。
