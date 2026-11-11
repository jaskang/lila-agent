# 交接说明（给接手的模型）

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

| 阶段 | 内容 | 状态 |
|---|---|---|
| P0 | 提交旧实现，打标签 `v3-final` | ✅ |
| P1 | 删掉 PM 和流程；会话模型；窗口改成项目 → 会话；三种引擎；三档权限 | ✅ 2026-11-10，等用户亲手试（见下） |
| P2 | 右侧的改动文件、diff、提交；独立 worktree 以及合并回来 | 后端 ✅ 2026-11-11（lilad、命令行、数据模型，`architecture-v4.md` §4 和 §10；测试和 clippy 过，真引擎没跑）；窗口 ✅ 2026-11-11（`panel.rs`：右面板的文件、改动，提交和合并按钮，新会话的 worktree 开关，已合并会话的标识；`--features shots` 截图和假数据看过，没接真 lilad 点过） |
| A0 | 收口：提交 P2；协议加请求 id 和版本握手；修 `@文件` 重复显示 | **下一步** |
| A1 | 安全网：每轮的 git 检查点和回退；Claude 用 `PreToolUse` 加自带沙箱 | |
| A2 | 等你闭环：提问卡片、卡片不过期、本会话总是允许、窗口里回答信任、原生通知 | |
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
cargo test --workspace                            # 应该是 199 个测试全过
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
| `lila` | 命令行和 lilad。`daemon/`：`mod.rs`（状态、hc 事件）、`threads.rs`（项目和会话的生命周期）、`permissions.rs`（权限卡片）、`conversation.rs`（读对话、发消息、`/` 和 `@` 菜单、`thread.file`）、`host.rs`、`hooks.rs`、`requests.rs`（方法分派）、`jobs.rs`（放在主循环外跑的 git 活：建 worktree、改动、diff、提交检查、合并，以及改动摘要的刷新）、`server.rs`（socket、推送、跑 job 的线程）、`tests.rs`（用假终端；`p2` 模块用临时 git 仓库）。`worktree.rs`（会话的 worktree）、`changes.rs`（改动列表、diff、读文件）、`merge.rs`（squash 并合并回起点分支）：都用 git 命令行，有真 git 的测试。`policy.rs`：档位判定。`project.rs`：project.toml 读写。`host.rs`：`lila host codex|omp`。`accounts.rs`、`signin.rs`、`magpie.rs`、`usage.rs`、`sessions.rs`、`commands.rs`。留着没人调用的：`github.rs`、`daemon/sources.rs`（P4） |
| `ghostty-sys`、`lila-term` | libghostty 绑定和终端视图（移植自 herdr-app） |
| `lila-app` | 窗口。`app.rs`：状态、刷新、快捷键、选中的会话或新会话、系统通知。`sidebar.rs`：侧栏。`ui.rs`：标题行（`header_actions`）、中间卡片、权限卡片、终端对话框的提示。`composer.rs`：输入区。`panel.rs`：右面板（文件树和预览、改动和 diff、提交和合并、合并确认框）。`conversation.rs`：对话视图、对话/终端切换。`settings.rs`：工作区设置。`prefs.rs`、`usage.rs`：全局设置。`dialog.rs`、`engines.rs`、`style.rs`、`data.rs` |

## 关键机制

- **lilad**：跑在名叫 `lilad` 的 hc 终端里；和命令行、窗口之间用 unix socket，每行一个 JSON；`subscribe` 推送变化。每个会话的引擎跑在自己的 hc 终端里，所以 lilad 重启时会话不断，重启后按终端标签对回会话。
- **三种引擎**：Claude Code 直接跑它的终端界面，lilad 跟读会话记录文件，再加五个 hook（`lila hook`）；Codex 和 omp 由 `lila host` 托管，通过 `host.attach` 把事件推给 lilad。细节见 `architecture-v3.md` 和 `architecture-v4.md` §2。
- **权限**（`architecture-v4.md` §6）：三种引擎的权限请求都转成同一种请求，lilad 先按会话的档位自动回答，答不了的才出卡片；每次自动回答都记在 `thread_events`（`lila thread log`）。
  - **全自动档的危险操作拦截靠识别命令文字，不是沙箱**：常见写法能拦下（`git push`、删分支、`gh` 的写操作、动工作区外的文件），别名、变量拼出的命令、脚本内部做的事拦不住。用户自己 `~/.claude/settings.json` 里的 allow 规则会让某些命令不问，lilad 看不到。已如实告诉用户：它拦常见的误操作，不是安全边界。
  - Claude 始终以 `--permission-mode manual` 启动，档位由 lilad 的 hook 判定，切档不用重启。
  - Codex 的全自动档是「只能写工作区、不联网」的沙箱加 `on-request`：联网命令会多一轮申请，由 lilad 自动放行；切档在下一轮生效。
- **新文件夹里 Claude Code 先问「信任这个文件夹」**：会话变成「终端在等你」，窗口提示打开终端去回答。lilad 不会替用户信任。
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
- `LILA_APP_FAKE=/path.json`：**不起 lilad**，用造好的数据画界面。JSON 是 `{projects, threads, conversations}`：前两个和 `project.list`、`thread.list` 的返回一样，`conversations` 是会话 id 到 `thread.read` 返回的 `items`、`permissions`、`writing`、`config`。任何状态（在等你、归档、已结束、权限卡片、终端里的对话框……）都能这样造；这时窗口里的操作发不出去（没有 lilad）。
- `LILA_APP_THREAD=<id>`：显示这个会话；`LILA_APP_SIZE=1000x680`：窗口这么大（看窄窗口）；右面板的状态在 `<LILA_HOME>/state/app-panel.json`（`{"open":true,"view":"changes","width":380}`）；假数据文件里还可以有 `changes`、`diffs`、`file`、`commit`、`merge`、`log`、`new`（见 `data::fake_act`），`LILA_APP_FAKE_READ_MS` 让面板的读变慢；`LILA_APP_NEW=<项目>,engine=codex,model=…,effort=…,permission=confirm`：打开新会话的输入框并选好；`LILA_APP_ADD=<文件夹>`：像选了文件夹那样新建工作区。
- `LILA_APP_CLICK="x,y;right:x,y;move:x,y;keys:cmd-n h i enter;wait:ms"`：启动 3 秒后依次点击（窗口里的点，不是截图的像素：截图是 2 倍，再缩放过的话自己换算）、右击、移动、按键（打字也行）、等待。点下拉菜单、右键菜单、对话框里的控件都靠它；真的 lilad 加它可以走完「新会话 → 发消息 → 回答 → 结束」。
- `LILA_APP_KEYS="cmd-, cmd-shift-,"`：启动后 1.5 秒按这些键，用来验证快捷键。
- `LILA_APP_INACTIVE=1` 加 `LILA_APP_NOTIFY_LOG=<文件>`：窗口当作不在前面，通知写进文件而不是真的发。
- `LILA_APP_PREFS=usage|about`；`LILA_APP_SIGNIN=claude:<名称>`（直接开始添加账号，`LILA_APP_NO_OPEN=1` 不打开浏览器，截完图要 `pkill -f "claude auth login"`）；`LILA_APP_CONV=top,row:60,open:25,openall:2,focus,key:h,key:enter,fill:<话>,attach:<路径>,menu,show:<会话>`（对话里的状态，见 `conversation::dev_conv`、`composer::dev_composer`）；`LILA_APP_GUTTERS=1`、`LILA_APP_DEBUG=1`。

真跑要省额度，放在单独的 `LILA_HOME`：

1. `LILA_HOME=/tmp/x target/debug/lila project add <一个 git 文件夹>`（这会在 hc 里起一个 lilad）；`lila project set --model claude-haiku-5-5 --effort low`；
2. `LILA_HOME=/tmp/x LILA_APP_SHOT=… LILA_APP_CLICK="keys:cmd-n;wait:300;keys:h i enter;wait:12000" target/debug/lila-app`；
3. 新文件夹里 Claude Code 先问「信任这个文件夹」：会话变成「终端在等你」，点「打开终端」，按 ↓ 和 Enter（`keys:down enter`），再 ⌘[ 回到对话；
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

## 待办和已知问题

- **路线图**见上面「进度和下一步」和 `product-v4.md` §8。
- **P1 留下的**：
  - 在 Claude Code 没信任过的文件夹里开会话，会停在终端里的「信任此文件夹」对话框，窗口只显示「在等你」，不说等什么。
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
