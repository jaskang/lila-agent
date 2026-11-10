# Lila

一个有自己界面的 agent。在项目里开会话，agent 由官方引擎（Claude Code、Codex、ACP agent）跑，用你的订阅登录，不需要 API key。

![Lila](docs/images/window.png)

## 安装

需要 Rust（版本见 `rust-toolchain.toml`）、[herdr-core](https://github.com/herdrdev/herdr-core) 的 `hc`，以及 `claude` 和/或 `codex`。终端用的 libghostty-vt 1.0 是预编译好放在仓库里的（`crates/ghostty-sys/libghostty-vt`），目前只有 Apple Silicon 的。

```sh
cargo install --path crates/lila
cargo install --path crates/lila-app
lila-app
```

`lila-app` 用 `lila` 命令启动后台的 lilad，两个都要装。

## 使用

Lila 是一个有自己界面的 agent：在项目（一个文件夹）里开会话，会话由官方引擎跑（Claude Code、Codex、ACP agent），Lila 提供界面和权限。

- **添加项目**：侧栏「项目」旁的 `+`，选一个文件夹。
- **新会话**：⌘N，或侧栏的「新会话」、项目旁的 `+`。直接出现输入框，光标已经在里面；发出第一条消息时才真正创建会话。发出之前可以在输入框下面改引擎、模型、努力程度和权限档，默认值是项目默认值。
- **侧栏**：每个项目下是它的会话。转圈是在工作，整行红底是在等你（要你允许一个操作，或 Claude Code 卡在终端里的对话框，比如「信任这个文件夹」），灰字是已结束，其余是空闲。右键一个会话：重命名、结束或继续、归档。归档的会话默认不显示，有归档时「项目」旁多一个归档按钮，点开列出来，右键取消归档。
- **对话**：消息、思考、工具调用，底下是输入框。权限确认以卡片出现在对话里，在卡片上选；Claude Code 卡在终端里时，输入框上方有「打开终端」。标题行右边：对话和终端的切换（Claude Code）、权限档、中断、结束或继续。
- **权限档**：每个会话三档，随时切换：只读、改动前确认、全自动（默认）。全自动下，改工作区以外的文件、`git push`、对外发出的动作仍然会问。
- **项目设置**：⇧⌘,（或项目右键「项目设置…」），设名称和新会话的默认引擎、模型、努力程度、权限档，存在项目的 `.lila/project.toml`。⌘, 是全局设置：用量和关于。
- **通知**：窗口不在前面时，会话开始等你、或一轮跑完，发系统通知。

| 快捷键 | |
|---|---|
| ⌘N | 新会话（在当前项目里） |
| ⌘1–9 | 切到第 n 个项目的最近会话（没有会话就是它的新会话） |
| ⌘. | 中断正在跑的这一轮 |
| ⌘[ | Claude Code 的终端切回对话 |
| ⌘, / ⇧⌘, | 全局设置 / 项目设置 |
| ↩ / ⇧↩ | 发送 / 换行（输入框里） |
| ⌘F | 搜索（终端里） |
| Esc | 关闭对话框 |

输入框里：`@` 选项目里的文件，`/`（Codex 是 `$`）选命令和 skill，回形针、拖进来或粘贴来添加附件；框是空的时候 ↑ 取回上一条话。

右侧的 diff、文件和 GitHub 面板是后面的阶段，还没有。接口见 [docs/architecture-v4.md](docs/architecture-v4.md)。

## 命令行

窗口能做的，命令行都能做，走同一个 socket：

```sh
lila up                                  # 启动 lilad，把当前文件夹加成项目
lila thread new "做一个登录页" --engine codex --effort low
lila thread list
lila thread show t07b6b991               # 对话和待答的权限卡片
lila thread send t07b6b991 "再加个测试"
lila thread answer t07b6b991 p1 allow
lila thread stop t07b6b991 / resume t07b6b991
lila thread permission t07b6b991 confirm
lila project set --engine claude --permission full
```

## 配置

- **项目**：`.lila/project.toml`，里面是项目的 id、标题和新会话的默认值（引擎、模型、努力程度、权限档）。
- `LILA_HOME=<目录>` 把配置、数据库和 socket 都放进这个目录。

## 开发

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
```

| crate | |
|---|---|
| `lila-core` | 引擎、权限档、会话、项目默认值、启动参数。不做 IO。 |
| `lila-store` | SQLite |
| `lila` | CLI 和 lilad |
| `hc-client` | herdr-core 客户端 |
| `ghostty-sys`、`lila-term` | libghostty 绑定和终端视图 |
| `lila-app` | 桌面应用（GPUI） |

架构见 [docs/architecture-v4.md](docs/architecture-v4.md)，界面见 [DESIGN.md](DESIGN.md)，接手开发先读 [docs/HANDOFF.md](docs/HANDOFF.md)。第三方代码见 [THIRD-PARTY.md](THIRD-PARTY.md)。

## 参考

- https://code.claude.com/docs/en/sub-agents
- https://x.com/DeepSeekHarness/status/2116857257245987236
- https://x.com/trq212/status/2061907337154367865
- https://x.com/addyosmani/status/2117512471930167377
