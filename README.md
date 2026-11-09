# Lila

一支编码 agent 团队。你只和 PM 说话，PM 拆任务、分派给团队；Lila 投递任务、传回结果，需要你的事放进收件箱。

每个成员都是官方 CLI（`claude` / `codex`）的交互会话，用你的订阅登录，不需要 API key。

![Lila](docs/images/window.png)

## 安装

需要 Rust（版本见 `rust-toolchain.toml`）、[herdr-core](https://github.com/herdrdev/herdr-core) 的 `hc`、Zig 0.16（编译 libghostty），以及 `claude` 和/或 `codex`。

```sh
git submodule update --init --depth 1
brew install zig@0.16
cargo install --path crates/lila
ZIG=$(brew --prefix zig@0.16)/bin/zig cargo install --path crates/lila-app
lila-app
```

`lila-app` 用 `lila` 命令启动后台的 lilad，两个都要装。

## 使用

点「新建项目」：选文件夹、勾团队、写一段要做什么。Lila 写好 `.lila/project.toml`，启动团队，把这段话作为第一个任务交给 PM。

- 左边：项目和团队。PM 在界面上叫「助理」。
- 中间：你的请求，按「下一步该谁动」分组：需要你、进行中、排队、已交付。点成员看它的终端，可以直接在里面打字（⌘F 搜索，⌘点击打开链接），⌘[ 回到列表。
- 右边：选中的请求。做完的工作会出现一张验收卡片：「通过」就合成一个提交落到当前分支，写意见就打回给助理。

| 快捷键 | |
|---|---|
| ⌘N | 新需求 |
| ⌘[ | 从终端回到请求列表 |
| ⌘1–9 | 切换项目 |
| ⌘I | 右侧面板 |
| ⌘, | 设置：角色、用量、关于 |
| ⇧⌘, | 项目设置：通用、成员 |

## 命令行

窗口能做的，命令行都能做：

```sh
lila init                       # 生成 .lila/project.toml
lila up --brief "做一个登录页"    # 启动团队，把这句话交给 PM
lila status                     # 谁在做什么
lila requests                   # 每个请求现在该谁动
lila inbox                      # 需要你的事
lila answer Q3 "用 Stripe"       # 回答；不带文字表示忽略
lila task ls --all              # 所有任务
lila pause / resume / down
```

成员用的协议：`lila --skill`。

## 配置

- **角色**：`~/.config/lila/roles/<id>.toml` 加 `<id>.md`（提示词）。`lila roles init` 补齐缺的默认角色。

  ```toml
  name = "开发工程师"
  runtime = "claude"          # claude | codex
  model = "claude-sonnet-5-5"
  effort = "high"
  can_assign = ["tester"]
  env = { PATH = "/opt/node/bin" }
  ```

- **项目**：`.lila/project.toml`，随仓库提交。成员（`[[members]]`，可以覆盖角色的 `runtime`、`model`、`effort`、`count`）、指派规则（`[assign]`）、自定义流程（`[flows.<名字>]`）和 `[settings]`（同时进行几件 `parallel`、检查命令 `check`、`max_rounds`、`stall_minutes` 等）。git 仓库里每件事在自己的 worktree 里做，验收后合成一个提交落到当前分支。
- **GitHub 来源**：在 `.lila/project.toml` 里加一段，issue 会自己进来，助理分拣，你点「修」才开工，做完开 PR：

  ```toml
  [[sources]]
  kind = "github"
  query = "is:open label:bug"   # gh 的搜索语法
  every = 10                    # 分钟
  max_per_day = 5
  ```

  需要装好并登录 `gh`。

- `LILA_HOME=<目录>` 把配置、数据库和 socket 都放进这个目录。

## 开发

```sh
export ZIG=$(brew --prefix zig@0.16)/bin/zig
cargo test --workspace
cargo clippy --workspace --all-targets
```

| crate | |
|---|---|
| `lila-core` | 角色、团队、任务状态机、流程、提示词、消息。不做 IO。 |
| `lila-store` | SQLite |
| `lila` | CLI 和 lilad |
| `hc-client` | herdr-core 客户端 |
| `ghostty-sys`、`lila-term` | libghostty 绑定和终端视图 |
| `lila-app` | 桌面应用（GPUI） |

架构见 [docs/architecture-v2.md](docs/architecture-v2.md)，界面见 [DESIGN.md](DESIGN.md)，接手开发先读 [docs/HANDOFF.md](docs/HANDOFF.md)。第三方代码见 [THIRD-PARTY.md](THIRD-PARTY.md)。

## 参考

- https://code.claude.com/docs/en/sub-agents
- https://x.com/trq212/status/2061907337154367865
- https://x.com/DeepSeekHarness/status/2116857257245987236
- https://x.com/addyosmani/status/2117512471930167377
