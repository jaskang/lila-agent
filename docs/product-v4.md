# Lila 产品方案 v4：有自己界面的 agent（草案）

> 2026-11-10 · 取代 `product-v3.md` 和 `review-2026-11-10.md` 的定位。
> v3 的「以请求为中心的 agent 团队」不再是需求：PM、流程、验收卡片、成员树全部废除。
> 2026-11-11 修订：GitHub 改走 `gh`、不接 MCP；computer use 不做；路线图在 P3 之前插入 A0–A5 并重排（§5、§8）。

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
| 5 | 工具层 | Lila 登记一份 MCP server 清单，开会话时注入引擎，所有引擎共用。用现成的 MCP server，不自己写。只给没有命令行的能力用（浏览器）；按项目开关，不默认注入每个会话（2026-11-11 修订） |
| 6 | GitHub、Asana | GitHub：agent 直接用 `gh`，**不接 GitHub MCP**（2026-11-11 修订）；界面面板也用 `gh`；从一条 issue 开会话。Asana 不单独做，归入「添加任意 MCP server」。都不自动触发 |
| 7 | 工作区 | 会话默认在项目文件夹原地工作，新建时可选独立 worktree |
| 8 | Claude Code 的驱动 | 保持终端加 hooks，不换 Agent SDK |
| 9 | subagent | 显示引擎自带的 subagent；Lila 另外提供跨引擎的「委派」工具 |
| 10 | 权限 | 每个会话三档：只读、改动前确认、全自动。**默认全自动**，只有危险操作才问 |
| 11 | 第一版范围 | 基础（项目和会话、对话、三种引擎、权限）加：diff 和文件面板、GitHub 面板（`gh`）、浏览器工具、独立 worktree |
| 13 | computer use | 不做，除非自用时证明非常必要（2026-11-11） |
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

lilad 维护一份 MCP server 的清单，开会话时写进引擎的配置（Claude 的 `--mcp-config`、Codex 的 app-server 配置、ACP 的 `session/new`）。stdio 的 server 由引擎按会话自己启动，lilad 不管它们的进程；只有 Lila 自己的工具（委派）由 `lila mcp` 连回 lilad，和 `lila hook`、`lila host` 同一个模式。

| 工具 | 方案 | 阶段 |
|---|---|---|
| 浏览器 | 现成的浏览器 MCP（`chrome-devtools-mcp` 或 Playwright MCP，实测后选一个） | P3 |
| GitHub | 不接 MCP。agent 用 `gh`（`policy.rs` 已经识别它的写操作）；界面面板用现有的 `github.rs`（`gh`） | P4 |
| 委派 | Lila 自己的 MCP 工具：把一个子任务交给另一种引擎，在子会话里跑，只把结果带回来。第一版只读 | P5 |
| 任意 MCP server | 设置里添加，Asana 是其中一个；面板等自用时确实需要再做 | 最后 |
| computer use | 不做（决定 #13） | — |

规则：

- **不默认注入每个会话**：每个 server 的工具定义都占上下文。按项目开关，新会话可以临时勾选；浏览器默认关。
- **和用户自己配的去重**：Claude 和 Codex 会继承用户全局配置里的 MCP server，清单里同名的不再注入。
- **读写判断用 MCP 自带的标注**：登记时调一次 `tools/list`，按 `readOnlyHint`、`destructiveHint` 建表，工具名前缀的正则只做兜底。
- **Codex 的 MCP 审批是前置**：现在直接拒绝，要先接成权限卡片。
- 引擎自带的同类工具不主动屏蔽；如果实测发现 agent 选错，再按引擎关掉重复的。

## 6. 权限

三档，每个会话可以随时切换，项目设默认值：

- **只读**：不能改文件、不能运行有副作用的命令。
- **改动前确认**：改文件和运行命令都问。
- **全自动**（默认）：不问。以下操作仍然问：
  - 改动或删除工作区以外的文件
  - `git push`、强制推送、删除分支
  - 对外发出的动作：提 PR、发评论、写类的 MCP 工具
  - computer use（Lila 自己不提供；用户自己加的这类工具仍然问）

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
| A0 收口 | 提交 P2；协议加请求 id 和版本握手（`ping` 带版本，窗口发现不一致就提示重启 lilad）；修 Codex 会话里 `@文件` 同时显示附件卡片和文字气泡 | 工作区干净；旧 lilad 能被发现 |
| A1 安全网 | 每轮开始前用隐藏的 git ref 打检查点（t3code 的做法，纯 git 命令），对话里每轮可以「回到这里」；Claude 的档位判定改用 `PreToolUse` hook 并启用它自带的沙箱。2026-11-11 对着 Claude Code 2.1.322 的二进制查过：`PreToolUse` 的 `hookSpecificOutput.permissionDecision` 有 `allow`/`deny`/`ask`；`sandbox` 设置里有 `autoAllowBashIfSandboxed`、`allowUnsandboxedCommands`、`excludedCommands`、文件的 `allowWrite`/`denyWrite`、网络的 `allowedDomains`，可以放在 `--settings` 的文件里。2026-11-11 实测：`PreToolUse` 的 deny、ask 压过用户的 allow 规则，已经做了（Claude 的每个工具调用先过 `PreToolUse`，`architecture-v4.md` §6）；沙箱「网络不限」设置表达不了，**还没做，等用户定**（`architecture-v4.md` §6「Claude Code 自带的沙箱」） | 全自动档改坏了能一键回退 |
| A2 等你闭环 | agent 的提问成为卡片（三种引擎归一成一种）；权限卡片不过期（现在 570 秒后退回终端）；确认档有「本会话总是允许这类」；「信任此文件夹」在窗口里回答；通知由 lilad 判断、窗口只显示，换成原生通知，点击跳到会话 | 一个会话从头到尾不用切终端 |
| — | **从这里开始自用：之后每个阶段都在 Lila 里做，只修用出来的问题** | |
| A3 可观测 | Claude 的 subagent 折叠显示（组件给 P5 的子会话留好位置）；待办和计划固定在输入框上方；标题行显示上下文占用和 token；对话往前翻页（现在只有最新 400 条）；自动标题；侧栏会话行标出引擎和 worktree | 不开终端也知道它在干什么、还剩多少上下文 |
| P3 工具层 | MCP 清单和注入三种引擎（§5 的规则）；Codex 的 MCP 审批；浏览器工具；工具结果里的图片落成文件、条目里存引用；取文件和上传附件的 socket 方法（远程客户端也用） | 三种引擎都能打开网页并截图；Codex 上写类 MCP 工具出卡片 |
| A4 worktree 补全 | `.worktreeinclude` 和项目的 setup 命令（把 `.env`、依赖带进 worktree）；丢弃 worktree、清理孤儿；diff 行内意见发回 agent、还原单个文件 | 两个会话同时在一个真实项目里干活互不干扰 |
| P4 GitHub | 面板列出 issue 和 PR（`gh`）；从 issue 开会话（默认 worktree）；提 PR 是 worktree 会话在「合并到起点分支」之外的第二个出口，worktree 留到 PR 合并 | 从一条 issue 到一个 PR 全程在 Lila 里完成 |
| P5 委派 | 只读委派：`lila mcp` 的工具，子会话带 `parent`、不进侧栏、显示在父会话里，卡片冒到父会话，档位不高于父会话，用量算在父会话上，只允许一层；工具调用的超时要先核实（调大，或拆成发起和取结果）。「换引擎就新开会话并带上摘要」（§4）用同一套机制 | Claude 会话让 Codex 审一遍并拿回结论 |
| P6 定时任务 | lilad 按时开会话。无人值守的规则：卡片没人答就超时拒绝并通知；默认在 worktree 里跑；能发现额度用完（Claude 的现在发现不了，要先修） | 一个每天跑的任务连续几天不用人管 |
| P7 远程 | 远程访问和手机端（参考 t3code：服务端拥有执行，客户端只走 RPC，中继加配对码）。还剩的例外：Claude 的终端视图由窗口直接连 hc | |
| P8 任意 MCP | 设置里添加任意 MCP server（含 Asana） | |

A5（不排顺序，从自用开始随暴露的问题加）：录下真实的会话记录和 app-server 流做回放测试；启动时检查 CLI 版本；`lila doctor` 检查 `hc`、`claude`、`codex` 是否装好并登录；查 Codex「停在工具结果后不收尾」的根因。

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
- DeepSeek Harness：功能清单的参照（subagent、浏览器、定时任务）。
- `architecture-v3.md`：引擎驱动和对话事件模型，继续有效。`architecture-v2.md`、`product-v3.md`：只作历史记录。
