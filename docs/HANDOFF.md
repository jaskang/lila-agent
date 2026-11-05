# 交接说明（给接手的模型）

更新于 2026-11-05 晚。用户用中文交流，回复也用中文。用户会换一台电脑继续，新电脑上没有上一台的 Claude 记忆，所以该知道的都写在这里。

先读这份。需要细节时再读：
- `docs/design.md`：总体设计
- `docs/m1.md`：编排层的发现和待办
- `docs/m2.md`：GUI 第一版
- `docs/m3.md`：M3 的顺序、做了什么，以及用户提出的界面意见

## 这是什么

Lila 是一个管理编程 agent 团队的桌面工具。

- **角色**：每个角色有 CLI、模型、努力程度和预设提示词，放在 `~/.config/lila/roles/`。默认五个：
  - `pm`：PM（项目经理），Claude Opus 5.5 high。负责拆任务、分派、跟进、向用户汇报。每个项目有且只有一个，这个角色不能删。
  - `designer`：设计师，Opus 5.5 high。负责流程、界面状态、文案、交互，做完交给开发。
  - `architect`：架构师，Opus 5.5 high。
  - `engineer`：开发工程师，Sonnet 5.5 high。
  - `tester`：测试工程师，Codex `gpt-6.1-sol` high。
- **项目**：一个 PM 加若干成员。成员之间按指派规则互相派任务、交付。
  - 每个成员可以设 `count`，同时开几个：`engineer`、`engineer-2`……
- **用户只和 PM 对话**。PM 负责规划和分派，遇到阻塞或需要人做决定时报告给用户。
- **界面仿照 Herdr app**：能看到每个成员在做什么，可以直接操作成员的终端。

## 硬性约束（用户明确说过）

- **用户只有订阅，没有 API key。** Claude 是 Pro 订阅，ChatGPT 用登录态。所有模型调用都必须通过官方 CLI（`claude`、`codex`），成员就是跑在终端里的 CLI。不要提出需要 key 的方案。
- **真跑 agent 的测试要省额度**：Claude 用 Haiku，codex 加 `-c model_reasoning_effort=low`，放在单独的 `LILA_HOME` 里。
- **不要参考 tern 的设计。**
- **v1 的约定**：
  - 成员共用一个工作目录，以后再上 worktree；
  - 成员可以直接互相派任务；
  - 项目配置放在仓库里的 `.lila/project.toml`。
- **工作方式**：
  - 提交信息用英文，末尾加 `Co-Authored-By` 署名行。
  - 只在用户要求时提交。默认在 `main` 上开发，用户已同意直接推送到 `main`。
  - 用户给界面意见时，改完要截图自查，再说明哪些点不了、需要用户亲手试。

## 新电脑上的准备

```sh
git clone git@github.com:jaskang/lila-agent.git && cd lila-agent
git submodule update --init --depth 1 vendor/ghostty
brew install zig@0.16                             # keg-only，不在 PATH 上
export ZIG=$(brew --prefix zig@0.16)/bin/zig      # libghostty 要 Zig 0.16
cargo test --workspace                            # 首次会编译 libghostty，要几分钟
cargo install --path crates/lila                  # 窗口通过 lila 命令启动 lilad
cargo run -p lila-app
```

- Rust 版本由 `rust-toolchain.toml` 固定为 1.99.1，rustup 会自动装。
- 还需要 herdr-core 的 `hc` 0.1.18（上一台装在 `~/.local/bin/hc`；也可以用 `LILA_HC` 指定路径），以及已经登录的 `claude` 和 `codex`。`herdr-app` 只用来对比样式，可以不装。
- **不随仓库走的东西**：
  - 角色目录 `~/.config/lila/roles/`：新电脑第一次 `lila init` 或 `lila up` 时会装上默认的五个角色。
  - 数据库 `~/.local/share/lila/lila.db`：任务、收件箱、动态都在里面，所以新电脑上是空的。
  - 窗口标签 `~/.local/state/lila/app-tabs.json`。
- 上一台电脑上用来看效果的项目是 `~/temp`，以及 `creator-studio`（它的 `.lila/` 被删了，打开时会自动重建）。新电脑上要用 `lila init` 或窗口里的「新建项目」另建。

## 技术栈

- 窗口：GPUI，通过 `gpui-kit 0.9.0`。它锁定 crates.io 上的 `gpui-pre =0.3.11`，不是 herdr-app 打过补丁的 0.3.8。
  - gpui-kit 的组件源码在 `~/.cargo/registry/src/*/gpui-component-0.9.0/src/`。查 API 时直接读这里，比如 select、menu、checkbox、switch、button_group。
  - 图标只有 gpui-kit 打包的那一部分能显示，名单见 `target/debug/build/gpui-kit-assets-*/out/icon_name.rs` 里 `__component_icon_names` 之后的部分。比如 `Trash` 不在其中，用了会显示空白。
- 终端渲染：libghostty-vt，作为子模块放在 `vendor/ghostty`。
- 终端进程：herdr-core（`hc` 0.1.18）。
- 从 herdr-app（Apache-2.0）移植了一部分代码，来源记在 `THIRD-PARTY.md`。

## 代码地图

| crate | 内容 |
|---|---|
| `lila-core` | 纯逻辑。`config.rs`：角色、ProjectConfig、MemberSpec 覆盖、`expand_ids`、Team::build、launch_key。`task.rs`：状态机。`prompt.rs`：系统提示词和协议。`runtime.rs`：claude/codex 启动参数。`dispatch.rs`：派发决策，`max_working` 为 0 表示不限。`message.rs`。 |
| `lila-store` | SQLite（rusqlite bundled）：projects、members、tasks、task_events、artifacts、inbox、outbox |
| `hc-client` | herdr-core JSON-RPC 客户端 |
| `lila` | CLI 和 lilad，详见下一行 |
| ↳ `daemon.rs` | `Daemon::handle` 处理所有请求；测试在 `daemon_tests.rs`，用 Fake 终端 |
| ↳ `settings.rs` | 项目设置读写：成员、count、指派规则、`[settings]`，用 toml_edit 保留注释 |
| ↳ `roles.rs` | 角色读写删、默认角色、旧版默认文件升级 |
| ↳ 其他 | `init.rs`（默认项目模板）、`client.rs`、`paths.rs`、`skill.md`（agent 协议说明） |
| `ghostty-sys`、`lila-term` | libghostty 绑定和终端视图（移植自 herdr-app）。`view.rs` 处理按键、IME、选择、复制粘贴、滚动、搜索、链接 |
| `lila-app` | 窗口，详见下一行 |
| ↳ 主体 | `app.rs`：状态、刷新循环、快捷键、对话框位置、缺配置自动重建。`ui.rs`：侧栏、项目右键菜单、标签栏、终端卡片、右侧概览、状态栏。`data.rs`：拉数据、标签持久化、`restore_config` |
| ↳ 设置 | `dialog.rs`：设置对话框外壳（导航、分组行、分段控件、步进器、拖动）。`settings.rs`：项目设置四页，以及分组的模型 Select。`prefs.rs`：全局设置，即角色库和关于 |
| ↳ 其他 | `detail.rs`（任务详情）、`compose.rs`（新建/改写任务）、`build.rs`（链接成 15.5 SDK） |

### 关键机制

- **lilad 是唯一写状态的进程。**
  - 它跑在一个名叫 `lilad` 的 hc 终端里，启动时带 `no_parent`。
  - 和 CLI、窗口之间用 unix socket 通信，每行一个 JSON：`{"method":…,"params":{…,"caller":{"cwd","project","member"}}}`。
  - 订阅接口 `subscribe` 会推送变化；窗口收到后合并 80ms 再刷新，另外每 5 秒兜底刷新一次。
- **成员状态**来自 OSC 7501：idle / working / done / blocked / error，外加 `msg`。
- **codex 沙箱进不了 socket**，所以走文件通道 `.lila/spool/`。
- **信任对话框不报状态**：成员启动后超过 `startup_timeout_secs`（默认 20 秒）还没报告状态，就提醒用户。
- **hc 的 `terminal.send` 的 `if` 参数只接受 `idle` 或 `working`。**
- **换任务时开新会话**（`[settings] fresh_session`，默认开）：
  - 非 PM 成员拿到另一条链的任务前，先重启它的 CLI。链的根是没有父任务的那个任务。
  - 同一条链上的返工沿用原会话。
  - 刚打进去的通知要先读完才重启。
  - 「上一条链」只记在内存里。
- **成员覆盖角色设置**：`[[members]]` 里可以写 `runtime`、`model`、`effort`、`prompt_file`、`count`。
  - `project.reload` 重新读取配置：
    - 还在用旧设置运行的成员标 `stale`；
    - 配置里没有了的成员会被停掉，它没做完的任务取消，并通知派活的人和 PM；
    - 已登记、但 lilad 没载入的项目，这时会被载入，不启动成员。
  - `member.restart` 用新设置重启一个成员。
- **`max_working`**：默认改成 0，表示不限。同时工作多少由各成员的 `count` 决定。窗口保存项目设置时，会删掉旧配置里的 `max_working`。
- **默认角色升级**：`defaults/roles/previous/` 存着旧版默认文件。用户角色目录里的文件如果和某个旧版一模一样，`install_defaults` 就把它换成新版；改过的不动。
- **macOS 红绿灯**：`build.rs` 用 `-platform_version macos 11.0 15.5` 链接，红绿灯位置是 `(12, 14)`，侧栏标志从 84px 开始。
- **gpui-kit 的 Root 会抢 Tab、Shift-Tab 和 ⌘C**：终端用 `Terminal` 按键上下文把它们绑定成 `NoAction`。

## 界面现状（用户的意见都已落实）

- **侧栏**：
  - 「项目」小标题右边的 `+` 用来新建项目；红绿灯那一行没有按钮。
  - 项目行右键菜单：新建任务…、项目设置…、在访达中显示、关闭标签。顶部标签也有同样的右键菜单。
  - 侧栏底部是「启动团队」「暂停/恢复派发」和「设置」，其中「设置」打开全局设置。
  - 缺少配置的项目标「缺少配置」，打开时自动重建。
- **标签栏**：只有打开的项目标签，没有按钮。
- **设置是两个分开的居中对话框**，用户确认过保持两个：
  - **全局设置**（侧栏「设置」或 ⌘,）：
    - 角色：增删改。PM 角色不能删，有项目在用的角色不能删。
    - 关于。
  - **项目设置**（右键或 ⇧⌘,）：
    - 通用；
    - 成员：列表加编辑区，有模型、努力程度、同时工作（count，PM 固定 1）、提示词、移出团队、添加成员（没有 PM 角色可选）；
    - 指派规则：勾选表；
    - 运行规则：换任务开新会话、无回应提醒、返工上限、子任务层数、启动等待。
  - 对话框表面在深色主题下比背景亮一档，带阴影。关闭按钮在右上角，可以拖动标题区移动对话框。
  - 模型选择是一个分组的 Select，分「Claude Code」「ChatGPT · Codex」两组，组决定 CLI。
  - 分段控件（努力程度）选中项用主色填满。
- **快捷键**：⌘N 新建任务、⌘1–9 切换标签、⌘W 关闭标签、⌘, 全局设置、⇧⌘, 项目设置、⌘I 右侧面板、Esc 关闭对话框或面板。

## 无法点鼠标时怎么验证

上一台电脑的 shell 没有屏幕录制权限，也不能点击窗口。`cargo build -p lila-app --features shots` 之后可以用这些环境变量：

- `LILA_APP_SHOT=/path.png`：每 2 秒把窗口画面存成 PNG，画面不含红绿灯。
- `LILA_APP_SWITCH=a,b,...`：每 3 秒切换到下一个项目。
- `LILA_APP_SETTINGS=general|members|assign|rules`：打开项目设置的那一页；再加 `LILA_APP_SETTINGS_ADD=<角色>` 会加一个新成员。
- `LILA_APP_PREFS=roles|about`：打开全局设置的那一页。
- `LILA_APP_OPEN_TASK=T3`、`LILA_APP_COMPOSE=<成员>`、`LILA_APP_DEBUG=1`。

截图时用临时的 `LILA_HOME`，不碰用户的真实数据：
1. 新建一个目录，在里面 `git init`；
2. 运行 `LILA_HOME=<home> target/debug/lila init`；
3. 用 sqlite3 往 `<home>/data/lila.db` 的 `projects` 表插入一行 `(id, cwd, created_ms)`，表结构在 `lila-store` 的 SCHEMA 里；
4. 运行窗口截图。

macOS 没有 `timeout` 命令，可以这样让程序跑一会儿再停：`prog & P=$!; perl -e 'select(undef,undef,undef,6)'; kill $P`。截图可以用 `sips -c H W --cropOffset Y X` 裁剪后再看。

## 分支

**默认在 `main` 上开发并直接推送**（用户 2026-11-05 晚定的）。M0 到 M3 的分支 `docs/design-and-m0`、`m1-orchestration`、`m1-followups`、`m2-gui`、`m3` 都已经合进 `main`，只是作为历史留在远端。

## 等用户亲手验证的

下面这些都过了测试和截图，但要用鼠标、键盘或真实环境确认：

- **终端**：Tab 和 Shift-Tab、平滑滚动、中文输入法、复制粘贴、⌘点击打开链接。
- **窗口**：
  - 拖动面板宽度；
  - 标签的打开、关闭和启动时恢复；
  - 项目和标签上的右键菜单。
- **设置对话框**：
  - 拖动对话框、模型 Select 的弹出列表、勾选表、− / +、添加或移出成员、保存；
  - 全局设置里新建和删除角色。
- **lilad 相关**，前提是先重装 lila，再 `lila daemon stop` 换成新的 lilad：
  - 换任务时开新会话；
  - 移出成员时停终端、取消任务；
  - 缺配置的项目重新载入。

## 待办

- **M3 剩下的**（顺序见 `docs/m3.md`）：
  - 第 3 项：新建项目向导，选目录 → 选角色 → 写简介 → 启动；
  - 第 5 项：用量统计；
  - 第 6 项：每成员独立 worktree，要先和用户确定合并方式；
  - 第 7 项：远程机器。
- **角色库**：还缺「预览最终提示词」。
- **未观察到的情况**：订阅额度用完时成员会怎样（见 `docs/m1.md`）。
- **终端**：Option 当 Alt 用、kitty 图片。
- **项目编号改名**：现在只能改显示名，改编号会牵涉数据库和终端名。
- **从列表移除项目**：已经提议放进右键菜单，用户还没表态。
