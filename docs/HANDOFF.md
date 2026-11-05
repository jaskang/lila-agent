# 交接说明（给接手的模型）

写于 2026-11-05。用户用中文交流，回复也用中文。先读这份，再按需读 `docs/design.md`（总体设计）、`docs/m1.md`（编排层的发现和待办）、`docs/m2.md`（GUI 的说明）。

## 这是什么

Lila 是一个管理编程 agent 团队的桌面工具。

- **角色**：每个角色有模型、努力程度和预设提示词。默认五个（另有设计师 `designer`，Opus 5.5 high，M3 加入；PM 不再兼设计）：
  - `pm`：产品经理，Claude Opus 5.5 high。每个项目有且只有一个，角色不能删。
  - `architect`：Claude Opus 5.5 high。
  - `engineer`：Claude Sonnet 5.5 high。
  - `tester`：Codex `gpt-6.1-sol` high。
- **项目**：一个 PM 加若干成员。成员之间可以按指派规则互相派任务、交付。
- **用户只和 PM 对话**。PM 负责规划和分派，遇到阻塞或需要人做决定时报告给用户。
- **界面仿照 Herdr app**：能看到每个成员在做什么，可以直接操作成员的终端。

## 硬性约束

- **用户只有订阅，没有 API key。** 用户是 Claude Pro，ChatGPT 用登录态。所有模型调用都必须通过官方 CLI（`claude`、`codex`），成员就是跑在终端里的 CLI。
- **真跑 agent 的测试要省额度**：Claude 用 Haiku，codex 加 `-c model_reasoning_effort=low`。
- **不要参考 tern 的设计**，用户明确说过。
- **v1 的约定**：成员共用一个工作目录，以后再上 worktree；成员可以直接互相派任务；项目配置放在仓库里的 `.lila/project.toml`。

## 技术栈

- Rust 1.99.1（`rust-toolchain.toml`）。
- 窗口：GPUI，通过 `gpui-kit 0.9.0`。它锁定 crates.io 上的 `gpui-pre =0.3.11`，不是 herdr-app 打过补丁的 0.3.8。
- 终端渲染：libghostty-vt，作为子模块放在 `vendor/ghostty`。
- 终端进程：herdr-core（`hc` 0.1.18）。
- 从 herdr-app（Apache-2.0）移植了一部分代码，来源记在 `THIRD-PARTY.md`。herdr-app 源码副本在 `/private/tmp/claude-501/-Users-jarax-codes-lila-agent/bfe302d2-da84-4c2c-a019-9ece5695916c/scratchpad/ext/app`（临时目录，可能已被清掉）。

## 构建和运行

```sh
git submodule update --init --depth 1 vendor/ghostty
export ZIG=$(brew --prefix zig@0.16)/bin/zig      # libghostty 要 Zig 0.16（brew 的 keg-only 包）
cargo test --workspace
cargo clippy --workspace --all-targets            # 保持零警告
cargo clippy -p lila-app --features shots         # shots 特性也要过
cargo install --path crates/lila                  # 窗口通过 lila 命令启动 lilad
cargo run -p lila-app
```

- 改了 `crates/lila` 以后，要重新 `cargo build -p lila` 或 `cargo install`。正在运行的 lilad 还是旧版本，要先 `lila daemon stop` 再启动团队。成员终端不受影响，新的 lilad 起来后会重新接上它们。
- 在本机 shell 里 `lila` 可能不在 PATH 上，可以用 `target/debug/lila`。

## 代码地图

| crate | 内容 |
|---|---|
| `lila-core` | 纯逻辑：`config.rs`（角色、ProjectConfig、MemberSpec 覆盖、Team::build、Member::launch_key）、`task.rs`（状态机）、`prompt.rs`（系统提示词 + 协议）、`runtime.rs`（claude/codex 启动参数）、`dispatch.rs`、`message.rs` |
| `lila-store` | SQLite（rusqlite bundled）：projects、members、tasks、task_events、artifacts、inbox、outbox |
| `hc-client` | herdr-core JSON-RPC 客户端 |
| `lila` | CLI + lilad：`daemon.rs`（`Daemon::handle` 处理所有请求，测试在 `daemon_tests.rs`，用 Fake 终端）、`client.rs`、`settings.rs`（项目设置读写，toml_edit 保留注释）、`roles.rs`、`init.rs`、`paths.rs`、`skill.md`（agent 协议说明） |
| `ghostty-sys`、`lila-term` | libghostty 绑定和终端视图（从 herdr-app 移植）。`view.rs` 处理按键、IME、选择、复制粘贴、滚动。`tests/keys.rs` 用无界面 GPUI 测 Tab 能到达终端 |
| `lila-app` | 窗口：`app.rs`（状态、刷新循环、动作、快捷键）、`ui.rs`（侧栏、标签栏、终端卡片、右侧概览、状态栏）、`detail.rs`（任务详情）、`compose.rs`（新建/改写任务）、`settings.rs`（项目设置面板）、`data.rs`（从 lilad 拉数据、标签持久化）、`build.rs`（链接成 15.5 SDK，见下） |

### 关键机制

- **lilad 是唯一写状态的进程。**
  - 它跑在一个名叫 `lilad` 的 hc 终端里，启动时带 `no_parent`。
  - 和 CLI、窗口之间用 unix socket 通信，每行一个 JSON。路径太长时，socket 退到 `$TMPDIR/lila-<fnv>.sock`。
  - 订阅接口 `subscribe` 会推送 `{"event":"changed"}`。窗口收到推送后合并 80ms 再刷新，另外每 5 秒兜底刷新一次。
- **成员状态**来自 herdr 的 OSC 7501 状态协议：idle / working / done / blocked / error，外加 `msg`。
- **codex 沙箱进不了 socket**（EPERM），所以改走文件通道 `.lila/spool/`。
- **信任对话框不报状态**。成员启动 20 秒还没报告状态，就在收件箱里提醒用户。
- **hc 的 `terminal.send` 的 `if` 参数只接受 `idle` 或 `working`**。Fake 终端也做了同样的校验。
- **成员可以覆盖角色设置**：`[[members]]` 里可以写 `runtime`、`model`、`effort`、`prompt_file`（相对 `.lila/` 的路径）。`project.toml` 还可以写显示名 `title`，项目编号 `name` 固定不变。
  - `project.reload` 重新读取配置。还在用旧设置运行的成员，在 `status` 里标记 `stale: true`。
  - `member.restart` 杀掉成员再用新设置重启，它手上的任务回到队列，再重新派给它。
  - 判断"旧设置"靠 `Live.launched`，它只存在内存里。lilad 重启前就在运行的成员，这个值未知，所以不会被标 stale。
- **macOS 红绿灯**：`build.rs` 加了链接参数 `-platform_version macos 11.0 15.5`。用新 SDK 链接时，AppKit 会画更大、间距更宽的新版按钮，和 Herdr（15.5 SDK）不一致。红绿灯位置是 `(12, 14)`，侧栏标志从 84px 开始（`ui::TRAFFIC_LIGHTS`）。
- **gpui-kit 的 Root 会抢 Tab、Shift-Tab 和 ⌘C**。终端用 `Terminal` 按键上下文把它们绑定成 `NoAction`，见 `lila_term::view::init`。

## 无法点鼠标时怎么验证

这个 shell 没有屏幕录制权限，也不能点击窗口。`cargo build -p lila-app --features shots` 之后可以用这些环境变量：

- `LILA_APP_SHOT=/path.png`：每 2 秒把窗口画面存成 PNG。画面不包含系统的红绿灯；红绿灯的尺寸会打在 stderr 里（`close button WxH at (x, y)`）。
- `LILA_APP_SWITCH=a,b,...`：每 3 秒切换到下一个项目。
- `LILA_APP_SETTINGS=general|members|assign|rules`：启动 1.5 秒后打开项目设置的那一页。再加 `LILA_APP_SETTINGS_ADD=<角色>` 会顺便加一个新成员。
- `LILA_APP_PREFS=roles|about`：打开全局设置的那一页。
- `LILA_APP_OPEN_TASK=T3`：打开任务详情。
- `LILA_APP_COMPOSE=<成员>`：打开新建任务表单。
- `LILA_APP_DEBUG=1`：每次刷新打一行日志。
- `herdr-app shot file.png`：截 Herdr 的窗口，用来对比样式。

macOS 上没有 `timeout` 命令。要让程序在后台跑一会儿再停，可以这样：
`prog & P=$!; perl -e 'select(undef,undef,undef,5)'; kill $P`。

本机有两个项目可以用来看效果：`temp`（`~/temp`，成员 id 还是旧的 `owner`）和 `creator-studio`。不要乱改它们的配置。测试时生成的 `~/.local/state/lila/app-tabs.json` 记得删掉。

## 分支和提交

所有分支都还没推送。分支依次叠加：

- `main` → `docs/design-and-m0` → `m1-orchestration` → `m1-followups` → `m2-gui` → `m3`（当前分支）。M3 的顺序和进度见 `docs/m3.md`。
- `m2-gui` 最新的提交是 `61302f9`，加了项目设置、左侧项目列表和顶部打开的标签。
- 提交信息用英文，末尾加 `Co-Authored-By` 署名行。只在用户要求时提交；推送前要先问。

## 最近完成，等用户实际试用

下面这些都过了测试和截图，但需要用户在窗口里亲手点一下确认：

- 终端里的 Tab 和 Shift-Tab。
- 平滑滚动。
- 标题栏拖动窗口，红绿灯的大小和位置。
- 切换项目不再等待：切换时立即拉取，并缓存每个项目上次的快照。
- 左侧项目列表；顶部标签的打开、关闭（⌘W）和启动时恢复。
- 右侧概览纵向排列：需要你 / 任务 / 动态。
- 项目设置的保存、`project.reload`，以及成员「待重启」和「重启」按钮。前提是用户先重装 lila 并重启 lilad。
- 还没在真实窗口里验证过：中文输入法、复制粘贴、拖动面板宽度。
- 终端搜索和链接已接上视图：标题行放大镜 / ⌘F 搜索，Enter / ⌘G 下一处，⇧Enter / ⇧⌘G 上一处，Esc 关闭；⌘悬停预览链接，⌘点击用系统默认应用打开，相对文件路径以项目目录为准。GPUI 输入分发测试和真实窗口搜索截图通过；最终打开浏览器/文件和真实输入法还待用户验证。当前工作区测试共 118 个，见 `docs/m2.md`。

## 待办（用户还没要求做的）

- 订阅额度用完时成员会怎样：还没观察到过（见 `docs/m1.md`）。
- 终端：Option 当 Alt 用（herdr 是靠它打过补丁的 gpui_macos 实现的）、kitty 图片。
- 新建项目向导（设计 §10.2，M3 第 3 项）。角色库已在全局设置里，还缺「预览最终提示词」。
- 项目编号本身的改名：现在只能改显示名，改编号会牵涉数据库和终端名。
