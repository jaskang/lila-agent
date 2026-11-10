# Lila 产品方案 v4：有自己界面的 agent（草案）

> 2026-11-10 · 取代 `product-v3.md` 和 `review-2026-11-10.md` 的定位。
> v3 的「以请求为中心的 agent 团队」不再是需求：PM、流程、验收卡片、成员树全部废除。

## 1. 定位

Lila 是一个原生界面（GPUI）的 agent 桌面应用。你在项目里开会话让 agent 干活；agent 由官方引擎来跑，Lila 提供界面、工具和集成。

- **用户**：先是自己和开发者。界面可以出现终端、diff、Git、worktree。
- **场景**：编程为主。浏览器等能力用来辅助开发和测试。
- **和官方产品的区别**：一个界面里用多家引擎和订阅；工具层对所有引擎一致；一个会话可以把子任务委派给另一家引擎。

## 2. 已定的决定

| # | 问题 | 决定 |
|---|---|---|
| 1 | 用户 | 先自己和开发者 |
| 2 | 核心场景 | 编程为主，其他为辅 |
| 3 | 引擎 | 只做官方引擎的外壳：Claude Code、Codex、ACP agent。不自己写 agent loop，不特别依赖 dsh 或 pi（它们可以作为普通 ACP agent 接入） |
| 4 | 手机端 | 桌面版稳定后再做。现在只守一条：所有功能都走 lilad 的 socket |
| 5 | 工具层 | Lila 托管一套 MCP，注入每个会话，所有引擎共用。用现成的 MCP server，不自己写 |
| 6 | GitHub、Asana | agent 的工具，加界面上的面板；从一条 issue 或任务开会话。不自动触发 |
| 7 | 工作区 | 会话默认在项目文件夹原地工作，新建时可选独立 worktree |
| 8 | Claude Code 的驱动 | 保持终端加 hooks，不换 Agent SDK |
| 9 | subagent | 显示引擎自带的 subagent；Lila 另外提供跨引擎的「委派」工具 |
| 10 | 权限 | 每个会话三档：只读、改动前确认、全自动。**默认全自动**，只有危险操作才问 |
| 11 | 第一版范围 | 基础（项目和会话、对话、三种引擎、权限）加：diff 和文件面板、GitHub 面板和工具、浏览器工具、独立 worktree |
| 12 | 旧代码 | 先提交现状并打标签，再在 `main` 上原地删改 |

## 3. 模型：项目 → 会话

- **项目**：一个文件夹。保存默认引擎、默认权限档、启用哪些工具。
- **会话**：一次和 agent 的对话。新建时选引擎、模型、努力程度，以及是否用独立 worktree。会话对应引擎自己的一个 session，可以续接。
- **subagent**：显示在会话内部，不是侧栏里的独立条目。
- 没有成员、角色、任务、请求、收件箱。权限确认直接显示在会话里；侧栏的会话条目标出「在等你」。

窗口：

| 位置 | 内容 |
|---|---|
| 左 | 项目，以及每个项目下的会话（运行中、在等你、已结束） |
| 中 | 对话：消息、思考、工具调用、subagent、权限确认；底部是输入框。Claude Code 会话可以切到它的终端 |
| 右 | 这个会话改动的文件和 diff、提交；GitHub 面板 |

## 4. 引擎

| 引擎 | 怎么接 | 登录 |
|---|---|---|
| Claude Code | 跑它的终端，从会话记录和 hooks 画出对话（现有） | 订阅 |
| Codex | `codex app-server`（现有） | 订阅 |
| ACP agent | `lila host` 里的 ACP 驱动（现有，目前接了 omp） | 各 agent 自己管 |

- lilad 仍是唯一的写入方，引擎进程仍跑在 hc 终端里：lilad 重启时会话不断。
- 一个会话的引擎中途不能换。要换就新开一个会话，带上前一个的摘要。
- magpie 保持可选：切换账号；ACP agent 想用任意模型时可以指向它的网关。Lila 不读它的令牌。

## 5. 工具层

lilad 启动并管理一组 MCP server，开会话时注入引擎（Claude 的 `--mcp-config`、Codex 的 app-server 配置、ACP 的 `session/new`）。

| 工具 | 方案 | 阶段 |
|---|---|---|
| 浏览器 | 现成的浏览器 MCP（`chrome-devtools-mcp` 或 Playwright MCP，实测后选一个） | 第一版 |
| GitHub | 官方 `github-mcp-server`；界面面板用现有的 `github.rs`（`gh`） | 第一版 |
| 委派 | Lila 自己的 MCP 工具：把一个子任务交给另一种引擎，在子会话里跑，只把结果带回来 | 第一版之后的第一项 |
| Asana | 官方 MCP，加面板 | 之后 |
| computer use | 待调研 macOS 上成熟的方案 | 之后 |

引擎自带的同类工具不主动屏蔽；如果实测发现 agent 选错，再按引擎关掉重复的。

## 6. 权限

三档，每个会话可以随时切换，项目设默认值：

- **只读**：不能改文件、不能运行有副作用的命令。
- **改动前确认**：改文件和运行命令都问。
- **全自动**（默认）：不问。以下操作仍然问：
  - 改动或删除工作区以外的文件
  - `git push`、强制推送、删除分支
  - 对外发出的动作：提 PR、发评论、操作 Asana
  - computer use

实现沿用 architecture-v3 的 D4：三种引擎的权限请求先转成同一种 `PermissionRequest`，lilad 按这个会话的档位自动回答，剩下的才显示给人。

## 7. 代码去留

**留下**
- `lila-agent`（事件模型和三个驱动）、`lila host`、`daemon/conversation.rs`、`daemon/host.rs`
- `lila-app`：`conversation.rs`、`composer.rs`、`dialog.rs`、`style.rs`、`prefs.rs`、`usage.rs`、设置框架
- 账号、登录、magpie、用量；`lila-term`；`sessions.rs`（会话续接）；`commands.rs`
- hc 这一层

**删掉**
- `lila-core`：`flow`、`dispatch`、`route`、`task`、`request`、`message`、`inbox` 里的验收和预算、`prompt` 里的团队提示词
- `lila`：`daemon/requests.rs` 的任务部分、`daemon/review.rs`、`daemon/slots.rs`、`watch` 里的流程部分、`roles.rs`、`init.rs` 的团队部分、相应的命令和测试
- `lila-app`：`requests.rs`、`wizard.rs`、`tree.rs`、`detail.rs`、`compose.rs`

**改造**
- `config.rs`：`.lila/project.toml` 从「成员列表」改成项目默认值。角色降级为「预设」（引擎、模型、努力程度、附加提示词）。
- `lila-store`：新表 `threads`，对话条目改挂在会话上。旧表不迁移。
- `worktree.rs`、`commit.rs`：从「一条链一个槽」改成「一个会话一个可选的 worktree」。
- `github.rs`、`daemon/sources.rs`：去掉分拣，只保留读写 issue 和 PR。
- `lila` 命令行：去掉 `task`、`requests`、`triage`、`draft` 等，保留 `up`/`daemon`、`accounts`、`usage`、`host`、`hook`，新增 `thread`。

## 8. 路线图

| 阶段 | 内容 | 做完的标志 |
|---|---|---|
| P0 基线 | 提交当前未提交的改动，打标签 `v3-final` | 可以随时回看旧实现 |
| P1 骨架 | 删掉 PM 和流程；`threads` 数据模型；窗口改成项目 → 会话；新建会话时选引擎；三档权限 | 三种引擎各开一个会话，能对话、续接、回答权限确认 |
| P2 编程面板 | 右侧的改动文件、diff、提交；独立 worktree 以及合并回来 | 两个会话同时在一个项目里干活互不干扰 |
| P3 工具层 | lilad 托管 MCP 并注入三种引擎；浏览器工具；对话里显示截图 | 三种引擎都能打开网页并截图 |
| P4 GitHub | GitHub MCP；面板列出 issue 和 PR；从 issue 开会话；提 PR | 从一条 issue 到一个 PR 全程在 Lila 里完成 |
| — | **以上是第一版。自用一周，只修用出来的问题** | |
| P5 | 跨引擎委派 | Claude 会话让 Codex 审一遍并拿回结论 |
| P6 | Asana 工具和面板 | |
| P7 | computer use；定时任务 | |
| P8 | 远程访问和手机端（参考 t3code：服务端拥有执行，客户端只走 RPC，中继加配对码） | |

每个阶段照旧：真跑用 Haiku 和 Codex low，放在单独的 `LILA_HOME`；界面用 `--features shots` 自查，并列出需要亲手点的地方。

## 9. 还没定的（括号里是默认做法，没有异议就按它做）

1. ACP 默认带哪个 agent（保留 omp；dsh 和 pi 只要装了就能在设置里加）。
2. 浏览器 MCP 选哪个（P3 开始时两个都实测一次再定）。
3. 浏览器用用户自己的 Chrome 还是单独的干净实例（单独的实例，不碰用户的登录状态）。
4. 全自动档下还要问的操作清单（§6 那四类）。
5. 旧数据（任务、请求、对话记录）要不要迁移（不迁移，新库从空开始）。
6. `DESIGN.md` 里和成员树、请求列表有关的组件规范（P1 改窗口时一起改）。

## 10. 参考

- t3code（`pingdotgg/t3code`）：同一类产品，结构可以借鉴：适配器边界、按会话注入 MCP 桥、隐藏 Git ref 做检查点、远程连接。
- DeepSeek Harness：功能清单的参照（subagent、浏览器、computer use、定时任务）。
- `architecture-v3.md`：引擎驱动和对话事件模型，继续有效。`architecture-v2.md`、`product-v3.md`：只作历史记录。
