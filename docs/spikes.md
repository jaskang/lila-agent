# M0 技术验证记录

> 2026-11-05 · 环境：macOS arm64 · hc 0.1.18 · Claude Code 2.1.318 · codex-cli 0.166.0 · cargo 1.99.1 · zig 0.16.0（`brew install zig@0.16`，keg-only）
> 为节省订阅额度，测试中 Claude 用 `claude-haiku-5-5`，Codex 用 `gpt-6.1-sol` + `model_reasoning_effort=low`。

## 结论速览

| # | 问题 | 结论 |
|---|---|---|
| 1 | Codex 交互模式会上报 OSC 7501 吗？ | ✅ 会。`app=codex`，`idle → working → done`，turn 计数正常 |
| 2 | Codex 的 `-c developer_instructions=...` 有效吗？ | ✅ 有效，`exec` 和交互模式都生效 |
| 3 | Claude 的 `--append-system-prompt-file` 有效吗？ | ✅ 有效 |
| 4 | 向正在 `working` 的 agent `hc send` 会怎样？ | ✅ 两者都**排队并在同一轮内处理**，不打断当前工具调用 |
| 5 | 权限确认时的状态？ | ✅ Claude 上报 `blocked`，`kind=permission`，`msg="approve Bash: touch perm-test.txt"`；此时 `hc send` 被拒 |
| 6 | 编译 herdr-app（GPUI + libghostty）| ✅ `cargo build --locked` 5 分 25 秒（debug，10 核），Rust 1.99 可用，不必退回 1.95 |
| 7 | 最小 GPUI 窗口 + libghostty 终端 attach 到 hc | ✅ 约 400 行，一次编译通过；颜色、粗体、方框线、实时刷新、状态栏、键盘输入都正常 |
| 8 | 订阅额度用完时 CLI 的状态 | ⏸ 未验证：无法在不耗尽额度的情况下触发，M1 期间遇到时记录 |

## 详细记录与对设计的影响

### 1–2. Codex

```sh
codex --no-daemon -m gpt-6.1-sol -c model_reasoning_effort="low" \
  -c developer_instructions="\"You are the TESTER. Secret word PAPAYA-42.\""
```

- 问 "你的角色和暗号" → `My role is TESTER, and the secret word is PAPAYA-42.`
- `developer_instructions` 的值是 **TOML 字符串**，需要自己加引号和转义。lilad 直接拼 argv（不经过 shell），用 TOML 序列化器生成这个值即可，长提示词也没问题。
- 干活时 Codex 每约 0.1 秒用旋转字符更新一次终端标题（`⠏ sandbox`、`⠋ sandbox`……），一个 20 秒的任务产生了约 50 条 `title` 事件。**GUI 要对 title 事件节流**；lilad 的派发逻辑只看 `status` 和 `turn` 事件。

### 3. Claude

```sh
claude --model claude-haiku-5-5 --append-system-prompt-file prompt.md
```

回答出了提示词文件里的暗号 `MANGO-7`。

### 4. 向忙碌的 agent 发消息

| | Claude Code | Codex |
|---|---|---|
| `hc send` 返回 | 立即返回 `started: true, status: working` | 同左 |
| 界面表现 | 消息显示为排队（"Press up to edit queued messages"） | 消息插入当前轮次 |
| 结果 | 当前工具调用跑完后，在**同一轮**里一起回应两条消息 | 同左（`DONE-A GRAPE`） |

**影响**：投递到忙碌的 agent 不会丢、也不会打断。但两个任务会被混在一轮里处理，agent 可能先做完 A 再顺带做 B，也可能把两者混在一起。设计里"每个成员同一时刻只有一个 active 任务、空闲时才投递"的规则保持不变；新任务的通知（如"T12 已完成"回传给 Owner）可以随时投递，因为排队是安全的。

### 5. 权限确认与信任对话框

- `--permission-mode default` 下运行 Bash → 状态立刻变成 `{"state":"blocked","kind":"permission","msg":"approve Bash: touch perm-test.txt","app":"claude-code"}`。`msg` 可以原样显示在 Inbox。
- 此时 `hc send` 返回错误（`unavailable`），Lila 不会误把任务文本打进权限对话框。
- **新坑：工作目录信任对话框。** Claude Code 第一次进入某个目录时会弹 "Is this a project you trust?"，**这时不上报任何 7501 状态**，默认选中的是 "No, exit"。
  - 影响：`lila project up` 启动成员后，如果 N 秒（建议 20 秒）内没有收到首次状态上报，就在 Inbox 生成"需要你在终端里确认"，并提供"打开终端"按钮。
  - Lila **不自动替你点"信任"**：这是一个安全决策。新建项目向导里可以提示"首次启动需要在每个 Claude 成员终端里确认信任一次"。
- 你的默认权限模式是 auto，所以平时成员很少会因为权限进入 `blocked`。

### 其他发现

- **PATH**：`hc run` 启动的程序 PATH 不一定包含 pnpm / fnm 的目录（第一次启动 Codex 报 `exec: codex: not found`，退出码 127）。**角色配置里要能填 `env`，lilad 启动前把命令解析成绝对路径**，解析失败时直接报错，不要等终端退出。
- **子终端通知会作为消息插入父 agent 的当前轮次**：测试中 Codex 启动失败退出后，hc 自动把 `<hc>t1p m0-codex exited (exit 127)...</hc>` 发给了父终端（这次是我自己所在的 Claude Code）。这验证了"Owner 收到成员回报"会以同样的方式出现在 Owner 的对话里。由 lilad 启动的成员不是 Owner 的子终端，回报要由 lilad 主动 `hc send` 给 Owner。

### 6. 编译 herdr-app

```sh
brew install zig@0.16                      # keg-only，不影响其他 zig
git clone --depth 1 https://github.com/herdrdev/herdr-app && cd herdr-app
git submodule update --init --depth 1      # vendor/ghostty，约 107 MB
ZIG=$(brew --prefix zig@0.16)/bin/zig cargo build --locked
```

- 首次 debug 构建 5 分 25 秒，产物 98 MB；增量构建十几秒。
- 用的是本机 stable Rust 1.99.1，虽然 herdr-app CI 固定在 1.95.0，也能编过。Lila 仍然要用 `rust-toolchain.toml` 固定一个版本。
- `ghostty-sys` 的 build.rs 用 Zig 编译 libghostty-vt 静态库，再用 bindgen 生成绑定；macOS 上需要 Xcode SDK（本机已有）。

### 7. 最小终端窗口（term-spike）

代码在 scratchpad：`/private/tmp/claude-501/-Users-jarax-codes-lila-agent/bfe302d2-da84-4c2c-a019-9ece5695916c/scratchpad/m0/term-spike/src/main.rs`（一次性代码，不进仓库）。

做了什么：
- `hc connect` → `hello {framed:true}` → `terminal.attach` → 把 DATA 帧写进 libghostty-vt → 每帧读 render state 的行和格子 → 按样式合并成 `TextRun` → GPUI `StyledText` 渲染。
- 收到多少字节就立即回多少 CREDIT。
- 另开 `hc events --terminal X` 读状态显示在标题栏。
- 键盘：可打印字符 + Enter / Backspace / Tab / Esc / 方向键 / Ctrl+字母，以 DATA 帧发给 hc。

结果（截图见 scratchpad `m0/spike.png`、`m0/spike-working.png`）：
- Claude Code 界面完整还原：橙色图标、粗体、灰色提示、表格方框线、输入框分隔线。
- 发任务后画面实时滚动，标题栏从 `claude-code · done` 变成 `claude-code · working · Printing tick 1-5 with one-second pauses`。

**发现：7501 的 `msg` 里是 agent 当前在做的事**（Claude Code 工作时会填当前工具调用的描述）。这正好满足"在界面中能看到每个角色在做的事情"：侧栏每个成员下面显示 `msg` 即可，不用解析屏幕。

对 M2 的影响：
- **GPUI 补丁必须一起带上**：spike 直接引用了 herdr-app 的 `vendor/gpui-pre*` 和 `vendor/taffy`（它们修了 macOS 空闲时每帧唤醒、flex 测量等问题）。Lila 要么把这些目录 vendor 进仓库（Apache-2.0，保留 NOTICE），要么把 herdr-app 作为 git submodule 引用。建议 vendor：方便按需打自己的补丁。
- **截图测试可行**：本机 shell 没有屏幕录制权限，`screencapture` 失败；但 GPUI 的 `render-to-image` 能让窗口给自己截图，不依赖系统权限。Lila 的 UI 测试和自动验收可以用这个方法。
- herdr-app 完整的终端实现（`vt.rs` 2.5k 行 + `terminal.rs` 3.6k 行 + `hc.rs` 0.7k 行）覆盖了选区、滚动、鼠标、IME、kitty 图片、搜索、链接。Lila 的成员终端直接移植这些代码，不要在 spike 基础上重写。

**键盘输入**：已由你在 spike 窗口里亲手验证。你打字和 agent 对话，最后输入退出命令，Claude Code 正常退出（exit 0）；spike 正确收到 CLOSE 帧并在标题栏显示退出原因。

**没有验证的**：中文输入法、窗口缩放、滚动历史。这些 herdr-app 都已经实现，移植时一起带上。

## M0 结论

技术路线成立，没有发现阻塞问题。需要修改设计的地方：

1. 角色配置支持 `env`，启动前把命令解析成绝对路径（PATH 问题）。
2. 成员启动后 20 秒内没有状态上报 → Inbox 提示"需要在终端里确认"（信任对话框）。
3. 侧栏显示 7501 的 `msg` 作为"正在做什么"；title 事件要节流。
4. Codex 的系统提示词走 `-c developer_instructions=<TOML 字符串>`，不需要第一条消息兜底。
5. vendor herdr-app 的 GPUI 补丁和 `ghostty-sys`，移植它的终端视图。
