# Lila 架构 v2：重新评估（草案）

> 2026-11-06 · §9 的决定：全部按建议。阶段 A–D 已完成，见 §10–§13。参考资料是 README 末尾的四篇：
> ① Claude Code 文档《Subagents》；② Thariq Shihipar《A harness for every task: dynamic workflows in Claude Code》；
> ③ DeepSeek Harness《Exploring Super-agent Design》；④ Addy Osmani《Critical thinking when agents do the work》。
>
> 一句话结论：Lila 的底子（lilad 是唯一写入方、CLI 协议、按链换会话、完成通知只给在等的人）和这几篇文章的方向一致，不用推倒重来。
> 要改的是三件事：**把流程从提示词里搬进 lilad**、**让 PM 在结构上只做路由**、**让"做完"由证据来定**。

---

## 1. 四篇文章里和 Lila 有关的要点

| 文章 | 核心观点 | 对 Lila 的含义 |
|---|---|---|
| ① Subagents | 子代理有独立上下文，只拿到委派消息，只把摘要交回来。要靠的规则必须写进委派消息里。工具限制靠配置强制，不靠提示词。范围清楚的活交给便宜的模型。可以续接（resume）原会话，也可以开一个新的。 | 成员 ≈ 长期存在的子代理。任务正文就是委派消息，质量要靠结构来保证。角色的工具权限应该落到 CLI 参数上。 |
| ② Dynamic workflows | 长时间待在一个上下文里会出三种问题：**偷懒**（做了 20 项就说做完了 50 项）、**偏爱自己的结果**（自己验自己）、**目标漂移**（尤其是压缩之后，"不要做 X"这类约束会丢）。解法是让**确定性的循环掌握流程**，每一步交给一个独立上下文的 agent。常见模式有分类后执行、扇出再汇总、对抗式验证、循环直到满足停止条件。不要滥用：大多数编码任务不需要 5 个审阅者。 | Lila 现在"engineer 做完交给 tester、tester 失败退回"这条流程写在角色提示词里，全靠模型记得做。应该由 lilad 按模板推进。PM 不要靠 `/compact` 续命。 |
| ③ Super-agent | 两层结构：**Router**（长期在线，独自对接人，只派发、不执行，可以用较轻的模型）+ **Worker**（上下文隔离，一个任务一个会话，用最强的模型，不直接和人说话）。同一条线程（Thread）上的后续事件由代码**直通**给订阅它的 Worker，不经过 Router（Router 的 token 少了 37%）。上下文当缓存用：先放 ID 加摘要（Inline），细节放在外面留个指针（Spill），需要时按 ID 一跳一跳往回取（Chain of Recall）。Watchdog 不只看进程活没活着，还看有没有真进展。实验结果：只写在提示词里的"请委派"不可靠，中性提示词下 5 次里 0 次委派。 | Lila 的 PM 现在既是 Router 又是计划者，而且"不要自己写代码"只是提示词。链就是 Thread，链上的成员就是订阅者，lilad 的 `route` 已经实现了一半。lilad 的沉默提醒只检查活着没有。 |
| ④ Critical thinking | 瓶颈从写代码移到了"确定自己是对的"。**事先定义什么算验证过了**，终点要是看得见摸得着的东西（测试通过、截图、日志）。绿勾只证明了一件事：要问它证明了什么，又没证明什么。要求 agent 列出它替你做的决定和假设。**警惕摘要套摘要**：第三个 agent 往往根本没读过原文。产出比你能审阅的多时就要放慢（编排税）。被打回的原因要写进配置或测试，经验才能积累。 | tester 现在只看到 engineer 写的交接说明，看不到用户的原话。任务没有验收标准字段。done 只有一句摘要，没有证据。没有给人的验收关口。 |

---

## 2. 现状诊断

### 2.1 已经做对、保留的

- **lilad 是唯一写入方，状态存在 SQLite 里**：相当于 ③ 的"状态和计算分开"，只是在本机这个尺度上。
- **成员走官方 CLI，通过 `lila` 命令参与协议**：不依赖 API key，也不用给每种 CLI 写适配。
- **换链时开新会话（`fresh_session`）**：就是 ① 的"子代理默认用新上下文"，以及 ② 的"每一步一个干净的上下文"。
- **完成通知只给在等的人（`lila_core::route`）**：就是 ③ 的订阅加直通。PM 一条链只被叫醒一次。
- **一个成员攒着的多条 note 合成一条发**：就是 ③ 里 Router 的批处理。
- **tester 默认是 Codex，和 engineer 不是同一家模型**：跨模型验证能天然减少"偏爱自己的结果"。这一点要写成原则。
- **并行的结果做完一件通知一件**（你否决了攒着一起发）：和 ③ 的"随到随处理"一致。扇出汇总的屏障只在显式声明时才用（见 §4.3）。

### 2.2 问题（附证据）

| # | 问题 | 证据 | 对应的文章 |
|---|---|---|---|
| P1 | **PM 不执行，只靠提示词约束**。它能跑测试、读 diff、改代码。 | `docs/m3.md`「省 token」：24 次 note 轮里有约 6 次是 tester 已经 PASS 之后 PM 又自己验一遍。 | ③ 消融实验，① 的工具限制 |
| P2 | **流程写在提示词里**：交给 tester、FAIL 时退回、轮数，都靠模型记得做。 | M1 第 1、2 轮：通知漏给 Owner、Owner 派重复的验证任务，都是靠事后打补丁修的。engineer 和 tester 的提示词里一半是流程说明。 | ② 的确定性循环 |
| P3 | **摘要套摘要**：用户原话 → PM 写成任务 → engineer 写交接 → tester 只看到交接说明。 | `defaults/roles/engineer.md`："hand it to the tester … name the files you changed … what behaviour to verify"。验证标准是被验的人自己写的。 | ④，以及 ② 的偏爱自己的结果 |
| P4 | **"做完"没有客观终点**：done 只有一句摘要，没有验收标准、没有证据。 | `lila task done --summary` 是唯一的必填项。 | ④ 事先定义验证，② 偷懒 |
| P5 | **PM 的上下文只增不减**，原计划是给它发 `/compact`。 | 每一轮的输入成本约等于当前上下文的长度（缓存读取占 95% 以上，`m3.md` 用量一节）。PM 永不重启，越用越贵。② 指出压缩有损，约束会丢。 | ② 目标漂移，③ Chain of Recall |
| P6 | **会话和成员绑死**：`count = 2` 时任务指定发给 `engineer-2`，不能发给"任意一个空闲的 engineer"。返工如果回到一个中间做过别的链的成员，原来的上下文已经没了。 | `task new --to` 只认具体编号。`fresh_session` 的"上一条链"只记在内存里。 | ① resume，③ 会话是实现细节 |
| P7 | **监控只看活没活着**：沉默提醒只判断空闲加没调协议。转圈、超预算、额度用完都发现不了。 | `dispatch::stall_check`。额度用完时成员的表现至今没观察到（`m1.md` 待办 5）。 | ③ Watchdog |
| P8 | **没有给人的验收关口**：链结束后 PM 关单，你在收件箱里看到一条 report，里面只有摘要。 | `inbox.kind = report`。 | ④ 问责在人 |

---

## 3. 新架构总览

```
                 你（只在两端：提需求 + 验收）
                   │ 对话        ▲ 验收卡片（证据 / 决定 / 没验到的）
                   ▼             │
          ┌──────────────────────┴───────┐
          │  PM = Router（工具受限）       │  只能：读代码、lila 命令、只读 git
          │  长期在线，可随时换新会话        │  换会话时由 lilad 注入「项目摘要」
          └──────────┬───────────────────┘
            lila task new --flow code     ▲ 一条链只汇报一次
                     ▼                    │
   ┌──────────────── lilad（确定性编排，唯一写入方）─────────────────┐
   │ 链(Chain) = 线程：原话、验收标准、检查命令、预算、工作区、会话账本    │
   │ 流程模板：implement → check(lilad 自己跑) → verify → (fail→fix)×N │
   │ 直通：链上的后续消息（你的补充、返工）直接给链上的成员，不经过 PM     │
   │ Watchdog（快路径）：沉默、长时间工作、超预算、额度用完、崩溃循环       │
   └──┬───────────────────┬────────────────────┬──────────────────┘
      ▼                   ▼                    ▼
  engineer 池（slot×N）   tester（Codex）        designer / architect
  一条链一个会话           只看原话加验收标准       按需
  返工续接原会话(resume)   结论：pass / fail
      └─────── 同一条链共用工作区（v1 共用目录，后续每条链一个 worktree）───────┘
```

和现在相比，变化在三层：

1. **PM 从"团队负责人"变成"Router"**：对接人、建链、选流程、汇报。深入的技术拆解交给 architect 当作一个任务去做。
2. **lilad 从"派发加转发"变成"按模板推进的编排器"**：成员只需要做好自己那一步，然后说结论。
3. **链成为一等对象**：原话、验收、证据、会话、预算、工作区都挂在链上。

---

## 4. 各模块设计

### 4.1 PM 是 Router，靠结构而不是提示词

- **角色可以配置工具权限**，`runtime.rs` 把它渲染成 CLI 参数（这些参数本机 `claude 2.1.318`、`codex 0.166` 都已经核实存在）：

  ```toml
  # ~/.config/lila/roles/pm.toml
  [tools]
  allow = ["Read", "Grep", "Glob", "Bash(lila:*)", "Bash(git log:*)", "Bash(git diff --stat:*)", "Bash(git status:*)"]
  deny  = ["Edit", "Write", "NotebookEdit"]
  # 渲染为 claude --allowedTools … --disallowedTools …
  # codex 角色用 sandbox = "read-only" | "workspace-write"（渲染为 -s；read-only 会挡住 spool，PM 用 Codex 时要注意）
  ```

  PM 想跑 `cargo test` 时会弹权限确认，收件箱里出现「PM 想运行 cargo test」。越界会被你看见，而不会悄悄花掉 token。
- **模型**：③ 的经验是 Router 的活比想象的简单，较轻的模型就够了。建议 PM 默认改成 `claude-sonnet-5-5`，需要深度规划时由 PM 派一个 `plan` 任务给 architect（Opus）。这一条需要你决定，见 §8。
- **PM 的提示词变短**：流程、核对、轮数都由 lilad 负责，PM 只需要知道建链、选流程、读汇报、问人、验收。

### 4.2 链（Chain）是一等对象

PM 收到一个需求就建一条链，链的根任务就是现在的「根任务」，只是带上更多的字段：

```
lila task new --to engineer --flow code \
  --title "登录接口" \
  --quote "<你的原话，一字不改>" \
  --accept "POST /login 成功返回 token" --accept "密码错误返回 401" \
  --check "cargo test -p api" \
  [--out-of-scope "不做注册"] [--budget 300k]
```

| 字段 | 作用 | 来源 |
|---|---|---|
| `quote` | 用户原话，原样传到链上的每一步，包括 tester。避免摘要套摘要。 | ④ |
| `accept` | 验收标准，事先定好。tester 按它来验，而不是按 engineer 的交接说明。 | ④ 事先定义验证 |
| `check` | 一条命令，由 lilad 自己运行，看退出码。项目里可以设默认值。 | ④ 客观终点，② 循环直到满足条件 |
| `out_of_scope` | 不做什么。压缩或换会话时最容易丢的就是这类约束。 | ② 目标漂移 |
| `budget` | 这条链最多花多少 token，由 `lila::usage` 统计。 | ② token 预算 |

PM 拿不到 `accept` 时，应该先问你（④ 的意图债：意图只能来自你）。新建项目向导的「简介」一步也相应分成三栏：要什么、不做什么、怎样算完成。

**直通**：你在任务详情里写的补充（现在的 `task edit --append`），直接送给链上正在做的成员，不叫醒 PM（③ 的订阅直通）。PM 只在链结束、阻塞或超预算时收到一条汇总。

**成员池**：`--to engineer` 指的是 engineer 这个池，lilad 挑一个空闲的 slot。同一条链的返工优先回到原来那个 slot（会话亲和，见 4.5）。

### 4.3 流程模板：由 lilad 推进

模板写在项目配置里，PM 建链时选一个（默认 `code`）：

```toml
# .lila/project.toml
[flows.code]                       # 默认流程
steps = [
  { do = "implement", by = "engineer" },
  { do = "check" },                                 # lilad 自己跑 check 命令，不花模型的 token
  { do = "verify", by = "tester", on_fail = "implement", max_rounds = 3 },
]

[flows.quick]                      # 小改动：只做实现和检查
steps = [{ do = "implement", by = "engineer" }, { do = "check" }]

[flows.design]
steps = [{ do = "design", by = "designer" }, { do = "implement", by = "engineer" }, { do = "check" }, { do = "verify", by = "tester", on_fail = "implement" }]
```

lilad 每一步做什么：

1. **implement 做完** → engineer 只需要 `lila task done T12 --summary … [--decision "…"]`，不用再自己建交给 tester 的任务。
2. **check**：lilad 在链的工作区里跑 `check` 命令（放进一个后台 hc 终端，有超时），记下退出码和最后 50 行输出。失败就直接把输出发回给同一个 engineer，这一步**不叫醒 tester，也不叫醒 PM**。
3. **verify**：lilad 生成验证任务，内容有：原话、验收标准、check 的结果、engineer 的摘要和它做的决定、改了哪些文件（`git diff --stat`）。tester 用 `lila task done T13 --verdict pass|fail --summary … [--unverified "…"]` 给出结论。
4. **fail** → lilad 把失败的用例原文退回同一个 engineer slot，并计一轮。超过 `max_rounds` 就阻塞给 PM。
5. **全部通过** → 链进入 `review`：PM 收到一条汇总（不再需要它自己去核实），你的收件箱里出现一张验收卡片（见 4.7）。

这样改的好处：

- 成员提示词里关于流程的部分大约能删掉一半，模型能漏掉的步骤少了（解决 P2）。
- 验证标准来自原话和验收标准，不来自被验的人（解决 P3）。
- 明显的失败由 `check` 当场拦下，tester 只在 check 通过后才被叫醒（省 token）。
- 成员仍然可以用 `lila task new` 临时派活。模板是默认路径，不是唯一路径。
- 文章 ② 里的其他模式以后可以作为模板加进来：扇出汇总（`fan_out`，**只在模板里写明时才等齐再汇总**，默认不攒）、对抗式审阅（`review`，用另一家的模型）、做方案时的多方案比较。都是可选的，默认不开（② 也说了：大多数任务不需要）。

### 4.4 上下文：Inline 和 Spill，以及 PM 换会话

**每种消息都有内联上限**（③ 的 Inline 和 Spill）：

| 消息 | 内联放什么 | 放在外面（只给路径或 ID） |
|---|---|---|
| 投递任务 | 原话、验收标准、标题、正文（不超过约 1.5k 字符）、相关 ID（父任务、链根、上一步） | 长正文、上一步的完整报告、check 的完整输出 |
| 完成通知 | 每一步一行摘要，加上还剩什么 | 报告文件 |
| `lila task show` | 本任务，以及下一跳对象的 ID 和一行摘要 | 再下一跳需要再查一次 |

**PM 换会话，替代 `/compact`**（解决 P5）。由于事实都在 lilad 里，PM 的会话可以随时换：

- 触发条件：PM 的上下文超过阈值（`lila::usage` 已经能从会话记录里读出最近一轮的输入大小），**并且** PM 空闲，**并且**你最近 N 分钟没在 PM 终端里输入。窗口里也提供一个手动的「整理 PM 上下文」按钮。
- 新会话的第一条消息是 lilad 生成的「项目摘要」（确定性生成，不由模型总结，不会有损）：
  - 项目目标；
  - 你回答过的问题和做过的决定（收件箱里的 answer）；
  - 进行中的链：ID、状态、一行摘要、在等什么；
  - 最近关闭的若干条链：ID 加一行摘要。
  - 上限约 2k token。需要细节时，PM 用 `lila task show` 按 ID 往回查（Chain of Recall）。
- PM 被要求把你在终端里提出的新需求马上建成链（带 `--quote` 原话），所以换会话不会丢掉意图。
- 代价：PM 终端的滚动历史会清空。动态里记一条「PM 已换新会话（上下文 180k → 3k）」。

### 4.5 会话账本：按（成员, 链）记，可以续接

- 新表 `sessions(project, member, chain, runtime, session_id, started_ms, last_ms, tokens)`。
- Claude 启动时由 lilad 指定 `--session-id <uuid>`，不需要事后去猜。Codex 从 `~/.codex/sessions` 里按工作目录和提示词里的身份认出来，`lila::usage` 已经有这套认人的逻辑。
- 返工回到一个中间做过别的链的 slot 时，用 `claude --resume <id>` 或 `codex resume <id>` 续接这条链原来的会话，而不是开一个空会话（① 的 resume）。
- "上一条链"从内存移到数据库里，lilad 重启也不会丢（顺带修掉 `m3.md` 第 1 项里的已知限制）。
- 换链时也不一定要重启：上下文还小，就沿用当前会话。只有换链**并且**上下文超过阈值时才开新会话（就是 `m3.md` 里计划要做的那项）。

### 4.6 Watchdog：先用确定性的快路径

在 `lilad tick` 里加一组检查（③ 的 Fast Path：用代码判断，不调用模型）：

| 情况 | 判断方法 | 动作 |
|---|---|---|
| 沉默（已有） | 空闲，并且有没关的任务 | 提醒，然后上报 |
| 长时间工作 | `working` 超过 N 分钟，期间没调用过协议，7501 的 `msg` 一直重复 | 给成员发一次提醒；再没有进展就进收件箱 |
| 超预算 | 这条链的 token 超过 `budget` | 暂停这条链，进收件箱：继续、加预算，还是取消 |
| 额度用完 | 状态是 `error`/`blocked`，或者会话记录里 `used_percent ≥ 100` | 暂停这个 CLI 的所有派发，直到重置时间（Codex 的重置时间已经能读到），收件箱提示 |
| 崩溃循环 | 一个成员 10 分钟内退出 3 次 | 停止重启，上报 |

用模型来判断"有没有跑偏"（慢路径）先不做。以后可以作为可选项：用 `claude -p --model haiku` 单次判断，默认关闭，因为它花的是你的额度。

### 4.7 人只在两端：验收关口

链通过 verify 后，你的收件箱里出现一张**验收卡片**（④）：

```
T1 登录接口 · 已通过验证 · 等你验收
原话：…
证据：check `cargo test -p api` 通过（12 个测试）· tester PASS
它替你做的决定：token 有效期定为 24 小时；错误统一返回 JSON
没有验到的：并发登录；真实数据库（测试用的是内存库）
改动：src/api/login.rs +84 −3，tests/login.rs +40
[通过]  [打回并留言]  [查看 diff]
```

- 卡片的内容由 lilad 从各步的 `--decision` 和 `--unverified` 拼起来，不靠 PM 再写一遍。
- **打回**：留言作为补充直通给链上的 engineer（同一条链，续接原会话），PM 只收到一行记录。
- **积压上限**（④ 的编排税）：`max_unreviewed`。没验收的链超过这个数时，新链先排队、不开工，因为产出已经超过你能审阅的速度了。建议默认值 3，0 表示不限。
- **以后再做**：打回的理由和 FAIL 的用例可以一键「记为规则」，写进 `.lila/rules.md`，注入所有成员的提示词（④：被打回的原因要写进配置，经验才能积累）。

### 4.8 工作区：一条链一个 worktree（M3 第 6 项的建议答案）

原来的计划是每个成员一个 worktree。按 ①（`isolation: worktree`）和 ②（"每个修复一个 worktree，审过再合并"）的做法，**按链分**更顺：

- 同一条链上的 engineer 和 tester 共用这个 worktree，tester 测的正好就是 engineer 的改动。
- 并行的链互不干扰，`count > 1` 的 engineer 不会在同一个目录里打架。
- `check` 在链的 worktree 里跑。
- 你点「通过」时合并回主分支（快进或 squash，二选一，需要你定）。合并冲突就阻塞给 PM，让它派一个 rebase 任务。
- v1 先保持共用一个目录。这一节和 §4.3 之间没有依赖，可以单独上。

---

## 5. 数据模型的改动

阶段 A、B 已经落地的（`lila-store`；旧数据库打开时自动加列）：

```sql
-- tasks 新列
quote, accept, out_of_scope      -- 简报（A）
decisions, unverified, verdict   -- done 时报告的（A、B）
run, run_step                    -- 属于哪个流程运行、第几步（B）

-- 新表：一次流程运行，锚点是它的第一个任务
runs (project, anchor, flow, step, round,
      state,                     -- running | checking | done | failed | cancelled
      created_ms, updated_ms)
```

原计划里的 `chains` 表拆开了：原话和验收标准直接放在任务上，沿父子链往上找；流程的进度放在 `runs` 里。一条链上可以有多次运行，比如 PM 派一次返工，就会再走一遍流程。

阶段 C 加的：

```sql
sessions       (project, id, member, runtime, file, started_ms)   -- Lila 启动的每个 CLI 会话
session_chains (project, session, chain, last_ms)                 -- 哪个会话做过哪条链
members.session                                                   -- 成员终端里正在跑的会话
```

阶段 D 加的：`runs.budget`、`runs.hold`（暂停时记下的下一步），`inbox.kind` 增加了 `review`、`budget`、`quota`、`runaway`。

后面阶段还要加的：

```sql
runs.worktree                                                                        -- E
```

检查的结果没有单独建表，记在 `task_events` 里（`kind = check`），失败的输出写进退回的 Fix 任务正文。

`lila-core` 加上 `flow.rs`：输入（链状态、刚发生的事件）→ 输出（下一步的动作），是纯函数，和 `dispatch.rs`、`route.rs` 是同一种写法，可以完整地做单元测试。

---

## 6. token 账

你在意的是 Lila 自己造成的消耗。逐项估计：

| 改动 | 对 token 的影响 |
|---|---|
| PM 工具受限，信任流程结论 | **省**：去掉 PM 的重复验证（实测约占 note 轮的 1/4） |
| PM 换会话，替代 `/compact` | **大幅省**：每一轮的输入是上下文长度。PM 从 150k 以上降回约 3k |
| PM 用 Sonnet | **省额度**：Opus 的额度消耗明显更高 |
| `check` 由 lilad 来跑 | **省**：明显的失败不叫醒 tester |
| 流程由 lilad 推进 | **省**：成员提示词变短，少了"该交给谁"的来回 |
| 你的补充直通链上的成员 | **省**：不经过 PM 转发（③ 的实验：Router 少 37%） |
| 投递时带上原话和验收标准 | **略增**：每个任务多约 200 token |
| 会话续接 | **有省有增**：返工不用重新读代码，但续接的上下文比空会话大 |
| 并行链、按链分 worktree | 对 token 没影响，额度消耗得更快一些（并行跑） |

---

## 7. 不做 / 推迟

- **外部事件触发**（GitHub 评论、CI、聊天群）：③ 的大头，但 Lila 现在是本机工具。lilad 的事件入口按"来源 + 快路径过滤"来设计，以后再接。
- **云端运行、跨机器**：留给 M3 第 7 项（hc machine），③ 的"计算和执行环境分开"正好对得上，不提前做。
- **提前执行（speculative execution）、默认开启的多方案比较或多人审阅**：花额度，不默认开。
- **用模型判断的 Watchdog**：先做确定性的那部分。

---

## 8. 落地顺序

每一步都单独可用，都用 daemon 的假终端测试覆盖，最后用 Haiku 和 codex low 各真跑一次。界面上的改动先做进 HTML 原型。

| 阶段 | 内容 | 主要涉及 | 风险 |
|---|---|---|---|
| **A** ✅ | 角色工具权限（`[tools]` → CLI 参数），收紧 PM；`--quote`、`--accept`、`--out-of-scope` 字段，投递时原样带上；done 加 `--decision`、`--unverified`；成员池寻址 | `config.rs`、`runtime.rs`、`prompt.rs`、`daemon.rs`、默认角色 | 低 |
| **B** ✅ | 运行表、流程模板、`check` 由 lilad 运行、`--verdict` 和自动返工；删掉角色提示词里的流程说明 | 新的 `flow.rs`，`daemon.rs`，store | 中：要改写交接相关的测试 |
| **C** ✅ | 会话账本（`--session-id`、resume），换链按上下文大小决定；PM 摘要换会话 | `daemon.rs`、`usage.rs`、`message.rs` | 中：需要真跑一次确认 resume |
| **D** ✅ | Watchdog 快路径（长时间工作、预算、额度、崩溃循环）；验收卡片；积压上限 | `dispatch.rs`、收件箱、窗口 | 低 |
| **E** | 一条链一个 worktree，验收时合并 | 新的 `worktree.rs`，`check` 和成员启动时的 `cwd` | 中：要定合并方式 |
| 以后 | `.lila/rules.md` 记规则、可选的模板（扇出、审阅、多方案比较）、外部事件 | | |

---

## 9. 需要你决定的（2026-11-06：全部按建议）

1. **PM 默认模型**：从 Opus 5.5 改成 Sonnet 5.5，深度规划交给 architect？（建议改）
2. **PM 的工具限制**：只读，加上 `lila` 和只读的 git，越界时弹权限确认？还是更严，完全不给 Bash 以外的工具？（建议前者）
3. **流程模板接管交接**：engineer 不再自己建交给 tester 的任务，由 lilad 按模板生成。这改变了你之前定的"成员直接互相派活"的默认路径（临时派活仍然可以）。可以吗？
4. **验收关口**：链结束后要不要你点「通过」才算完？积压上限默认 3 可以吗？
5. **PM 换会话**：接受 PM 终端的滚动历史被清空吗？阈值建议 120k。
6. **worktree**：按链分可以吗？验收时用 squash 还是快进合并？

---

## 10. 阶段 A 完成记录（2026-11-06）

**角色工具权限**
- 角色文件可以写 `[tools]`，有四个键：
  - `only`：渲染成 claude 的 `--tools`；
  - `allow`：渲染成 `--allowedTools`；
  - `deny`：渲染成 `--disallowedTools`；
  - `sandbox`：渲染成 codex 的 `--sandbox`。
- 实现在 `lila_core::Tools` 和 `runtime::launch`。
- 没写 `[tools]` 的角色，`launch_key` 和以前一样，升级后正在运行的成员不会被标「待重启」。

**默认 PM**
- 改成 `claude-sonnet-5-5`。
- `only = Read, Grep, Glob, Bash`；`allow` 是 `lila` 和只读的 git 命令（status、log、diff --stat、show --stat）。别的 Bash 命令要你确认。
- 没改过的旧 `pm.toml` 会自动升级，旧版本存在 `defaults/roles/previous/pm.3.toml`。
- 真跑验证过一次（Haiku，`-p`，临时目录）：
  - 内置工具只剩 Bash、Glob、Grep、Read，没有 Edit 和 Write；
  - `git status` 不问就执行了；
  - 用 shell 写文件时停下来等确认。

**原话和验收标准**
- `lila task new` 加了三个参数：`--quote`、`--accept`（可重复）、`--out-of-scope`（可重复）。存在 `tasks` 表的新列里，旧数据库打开时自动加列（`lila-store` 的 `ADDED_COLUMNS`）。
- 投递任务时，lilad 往上找最靠近链根、带有简报的那个任务，把它的原话、「Done when」和「Out of scope」附在后面（`message::ChainBrief`）。
  - 你派的任务本身就是原话，所以 T1 下面的每一步，包括 tester，都能看到你写的简介。
  - 原话在投递消息里超过 1200 个字符会截断，并指向链根的任务文件。任务文件里有完整内容。

**done 带上决定和没验到的**
- `lila task done` 加了 `--decision` 和 `--unverified`，都可以重复。
- 它们出现在三个地方：
  - 完成通知：每一步下面多两行「Decided / Not checked」；
  - 你的收件箱 report；
  - `task show`。
- PM 关你的任务时，被要求把这些转述给你。

**成员池**
- `count > 1` 的成员（`engineer`、`engineer-2`）用 `--to engineer` 来派。lilad 按这个顺序选：
  1. 父任务那条链上最近做过的那个；
  2. 否则选没关的任务最少、并且空闲的那个。
- 写具体编号（`--to engineer-2`）仍然只发给它。
- 提示词里的团队列表和「可以派给」都写成池的编号。

**提示词**
- 协议段：说明原话和验收标准是要达到的标准；交接时不用自己抄原话；done 时报告决定和没验到的。
- PM 段：建链时带 `--quote` / `--accept`；说明自己没有改文件和跑构建的工具。
- `tester.md`：按原话和「Done when」验，不只按交接说明；PASS 时列出没验到的。旧版存在 `previous/tester.1.md`。
- `skill.md` 同步更新。

**验证**
- 新增测试：
  - `lila-core`：工具渲染成参数；池的选择；池在提示词里只写一次；你派的任务就是原话；投递消息里的简报。
  - `lila-store`：简报和结果读写；旧数据库自动加列。
  - `lila`：默认角色升级到 Sonnet 和工具限制；原话传到每一步；决定传到 PM 和收件箱；池的分派和返工回到原来的人。
- 工作区 158 个测试通过，Clippy 零警告。

**已知限制和下一步**
- 池在建任务时就选定成员。如果当时所有成员都忙，之后另一个先空出来，任务不会改派给它。阶段 B 有了链以后再处理。
- 窗口还没显示原话、验收标准、决定和没验到的，`task show` 和 JSON 里已经有了。界面要先在 HTML 原型里定，放进阶段 D 的验收卡片。
- 用全局设置「新建角色」从 PM 复制出来的角色，不会带上 `[tools]`。
- PM 想跑别的命令时会弹权限确认，成员状态变成 blocked，收件箱里会有提示。PM 实际会不会这样做，要真跑一轮才知道。

---

## 11. 阶段 B 完成记录（2026-11-06）

**流程**（`lila_core::flow`）
- 项目配置里可以写 `[flows.<名字>] steps = [...]`。每一步有三个键：
  - `do`：做什么，`check` 表示由 lilad 运行检查命令；
  - `by`：谁来做，成员或成员池；
  - `on_fail`：这一步给出结论，失败时退回到哪一步。
- 自带 `code`（implement → check → verify）和 `quick`（implement → check）：
  - 团队里没有 tester 时，`code` 去掉 verify；
  - 没有 engineer 时，两个都没有；
  - 项目自己定义了同名流程时，用项目的。
- 加载项目时校验：
  - 第一步必须是成员的步骤；
  - `by` 必须是团队里的成员或成员池；
  - `on_fail` 必须指向前面的某个成员步骤。
- `flow::after` 是纯函数：输入哪一步结束了、通过没有、第几轮，输出下一步做什么。
- **新的 `[settings]`**：
  - `flow`：默认流程，默认 `code`；
  - `check`：检查命令，默认为空，为空时跳过检查步骤；
  - `check_timeout_secs`：检查超时，默认 600 秒。

**哪些任务会走流程**
- 默认流程只用于 PM 或你派的任务，而且接活的人正是流程第一步的成员（或成员池里的一个）。
- `--flow <名字>` 指定流程，`--flow none` 不走流程。
- 成员之间的临时派活不走流程。

**lilad 怎么推进**
- 一步做完后，按 `flow::after` 的结果做下面几件事之一：
  - 在后台跑检查命令（`crates/lila/src/check.rs`）：
    - 在项目目录里用 `/bin/sh -c` 运行，stdout 和 stderr 合在一起，保留最后 40 行；
    - 命令有自己的进程组，超时时连它启动的子进程一起停掉；
    - 它留下的后台进程不会卡住结果。
  - 建下一步的任务：`from = lilad`，父任务是上一步。任务正文里写着上一步的摘要、做的决定、交付的文件，以及检查已通过。
  - 退回：新建一个「Fix:」任务，给上次做这一步的那个人。正文是 tester 找到的失败用例，或者检查命令的输出。轮数加一。
  - 超过 `max_rounds`：运行标记为 failed，通知启动这次运行的人和 PM，交给他们决定。
  - 全部通过：这条链照常汇报一次。你直接派的任务，结果进收件箱：「T4 done, code flow passed: …」。
- 检查失败时直接退回给 engineer，**不叫醒 tester，也不叫醒 PM**。
- 检查进行中，等着这次运行的人（比如 PM）不会收到沉默提醒。
- lilad 重启后，还在 checking 的运行会重新跑一遍检查。
- `lila status` 显示进行中的运行：流程名、当前步骤、第几轮。

**成员这一侧**
- 投递消息会告诉成员下一步发生什么，比如「When you finish, Lila runs `cargo test` and sends it to `tester` to verify with your summary. Don't hand it on yourself.」
- 验证步骤的投递消息要求给出结论，结束命令里带着 `--verdict pass|fail`。不带 `--verdict` 不能结束；不是验证步骤的任务带了也会被拒绝。
- 流程里的成员想自己交接给流程里的人，会被拒绝，并告诉它直接 `done`（或 `--verdict fail`）就行。叫流程以外的人帮忙不受影响。
- 简报合并：原话取最靠近链根的那个；「Done when」和「Out of scope」把一路上设过的都合在一起。PM 在中间任务上加的验收标准，tester 也能看到。

**提示词**
- PM 段后面多了「Flows」一节：按项目配置生成，写着默认流程会做什么，以及 `--flow` 怎么用。
- 协议段：要求给出结论的任务加 `--verdict`；任务里写着 Lila 会接着交接时，不要自己交接。
- `engineer.md`：直接 `done`，加上 `--file`；只有不在流程里时才自己交给 tester。
- `tester.md`：流程里的验证用 `--verdict`，流程以外的照旧手动退回。
- 旧版本存在 `previous/engineer.1.md` 和 `previous/tester.2.md`，没改过的会自动升级。
- `skill.md` 加了「Flows」一节；`lila init` 的模板加上了 `flow = "code"`，以及注释掉的 `check`。

**验证**
- `flow` 5 个单元测试；`config`、`message`、`prompt`、`store` 各加了流程相关的测试；`check` 4 个测试，真的运行子进程，包括超时和留下的后台进程。
- daemon 新增 5 个测试：
  - 完整走一遍 code 流程，PM 只收到一条通知；
  - 检查失败退回、验证失败退回、轮数用完后放弃；
  - 流程里不能手动交接；
  - 哪些任务走流程，包括 quick，以及你直接派的任务进收件箱；
  - 重启后重新跑检查，其中一次真跑了检查命令。
- 原来那些测试手动交接的测试，现在用 `flow = "none"` 跑。
- 工作区全部测试通过，Clippy 零警告。没有真跑模型 CLI。

**已知限制**
- 检查命令和成员共用一个工作目录（阶段 E 才有 worktree）。检查运行时，engineer 如果又在改别的东西，检查看到的是改了一半的代码。
- 运行只记流程的名字。运行中途改了流程配置，步骤编号可能对不上，lilad 会在那一步停下并照常汇报。
- 检查命令用的是 lilad 启动时的 PATH，不是角色里的 `env`。
- 窗口里还看不到流程的进度（只有 `lila status`），要先在 HTML 原型里设计。

---

## 12. 阶段 C 完成记录（2026-11-06）

**会话账本**（`lila-store` 的 `sessions`、`session_chains`；`crates/lila/src/sessions.rs`）
- **Claude 成员**：启动时由 lilad 生成一个 UUID，用 `--session-id` 传进去，所以一开始就知道它的记录在 `~/.claude/projects/<目录>/<id>.jsonl`。
- **Codex 成员**：会话 id 由 Codex 自己定。lilad 在 tick 里到 `~/.codex/sessions` 找：同一个工作目录，系统提示词里写着这个成员，而且是在它启动之后才出现的那个记录文件。只在成员启动后的半小时内找。
- 每次给一个会话投递任务，都记下「这个会话做过这条链」。lilad 重启后，从数据库恢复每个成员当前的会话和上一条链（顺带修掉 `m3.md` 第 1 项「只记在内存里」的限制）。
- **上下文大小**：读会话记录的最后 512KB，取最近一轮的输入。Claude 是 input、cache write、cache read 三项之和；Codex 是 `last_token_usage.input_tokens`。文件没变就不重读。

**换链时怎么办**（`switch_plan`，前提是 `fresh_session` 开着，PM 除外）
1. 这个成员以前有会话做过这条链，而且记录文件还在：**续接那个会话**（claude `--resume <id>`，codex `codex resume … <id>`）。返工回到写代码的那段上下文里。
2. 否则，当前会话的上下文小于 `fresh_after_tokens`（默认 60k）：**沿用当前会话**，不重启。
3. 否则，或者读不到上下文大小：**开新会话**（就是原来的做法）。
- 动态里会分别记成 `resumed_session` 和 `fresh_session`。

**PM 换会话，替代原计划的 `/compact`**
- 换的条件（三个都要满足）：
  - PM 的上下文 ≥ `pm_refresh_tokens`（默认 120k）；
  - 已经空闲了 `pm_refresh_idle_minutes`（默认 10 分钟），因为你可能正在 PM 终端里打字；
  - 没有等着发给它的消息。
- 满足后，lilad 关掉 PM 的终端，开一个新会话，第一条消息是 `<lila digest>` 项目摘要。
- 摘要由 lilad 从数据库直接生成，不经过模型总结，内容有：
  - 你回答过的问题；
  - 每个还没结束的根任务，以及它下面还没做完的任务和进行中的流程（当前步骤、第几轮返工）；
  - 最近完成的 8 条；
  - 收件箱里还有几条等你。
  - 细节让 PM 用 `lila task show` 去查。
- 你在窗口里手动重启 PM（`member.restart`）时，也会收到这份摘要。
- 动态里记一条 `pm_refreshed`，比如「its context reached 150k tokens …」。
- 代价：PM 终端的滚动历史会清空（你已经同意）。

**新的 `[settings]`**：`fresh_after_tokens = 60000`（0 表示换链一律开新会话）、`pm_refresh_tokens = 120000`（0 表示不自动换）、`pm_refresh_idle_minutes = 10`。

**真跑验证过一次**（Haiku，`-p`，临时目录）：
- `--session-id` 指定的 id 就是记录文件名；
- `--resume` 接着写同一个文件，从 31 行变成 44 行，模型也记得前面说过的话；
- 读出最近一轮的上下文是 28.9k。
- 一个空会话本身就有 2–3 万 token，所以 60k 的阈值大约相当于会话里实际的工作内容超过 3 万时才换。

**验证**
- 新增测试：
  - `sessions` 4 个：UUID 格式、两种 CLI 的上下文、从文件末尾读、找 Codex 会话；
  - `runtime` 1 个：新会话和续接时的参数；
  - `store` 1 个：会话账本；
  - `message` 1 个：摘要的格式；
  - daemon 3 个：
    - 换链时沿用、新开、续接三种情况，以及 lilad 重启后还记得；
    - Codex 会话被认出来，并读出上下文；
    - PM 上下文大时，等空闲够久才换，新会话收到含决定和进行中工作的摘要，之后不再重复换。
- 工作区全部测试通过，Clippy 零警告。

**已知限制**
- 续接时，用的是成员当前的设置（模型、提示词）。如果中途改过设置，续接的会话会用上新的设置。
- Codex 续接用的是 `codex resume <id>`，没有真跑验证过。Codex 这几天的周额度已经用完（会话记录里 `used_percent = 100`）。
- 摘要里没有你在 PM 终端里说过、但还没变成任务的话。PM 的提示词已经要求它及时用 `--quote` 建任务。
- 窗口里还没有「整理 PM 上下文」按钮，也没有显示每个成员的上下文大小，要先在 HTML 原型里设计。

---

## 13. 阶段 D 完成记录（2026-11-06）

**验收卡片**（`inbox.kind = review`）
- 你派的工作完成时，收件箱里不再是一条 report，而是一张验收卡片，「完成」包括两种情况：
  - PM 关掉你给的任务；
  - 你直接派给 engineer 的任务，流程全部通过。
- 卡片内容：
  - 第一行：任务的摘要，以及它做的决定和没验到的；
  - 接着按时间顺序列出每一步：成员的摘要、交付的文件、决定、没验到的，以及 lilad 跑过的每次检查和结果。
- 你的两种回答：
  - **通过**（不写内容，或者写「通过 / accept / ok」）：记一条 `accepted`。
  - **其他任何文字**算打回：lilad 新建一个你派给 PM 的任务「Rework: …」，挂在被打回的任务下面，正文是你的意见。它在同一条链上，所以原话会带下去，返工给 engineer 时也会回到原来的会话。原计划是把意见直接交给 engineer、不经过 PM；现在改成先交给 PM，因为打回的意见可能要换人或重新规划，由 PM 来判断更稳。
- 窗口：卡片标题是「等你验收」，下面是「通过」按钮、写返工意见的输入框和「打回」按钮。先改了 HTML 原型，再搬进 GPUI，两边都截图看过。

**积压上限**（`[settings] max_unreviewed`，默认 3，0 表示不限）
- 等你验收的卡片达到上限时，PM 新开的链（没有父任务）先排队不派发。`lila task new` 会提示「N results wait for the person's review…」，`lila status` 也会显示有几个在等。
- 你自己派的活、链里面的后续工作不受影响。
- 你验收一张，排队的就开始派发。

**Watchdog**（在 `tick` 里，全部用代码判断，不调用模型）
- **长时间没有进展**（`runaway_minutes`，默认 30）：成员一直在工作、超过这个时间没有调用过 `lila`，先给它发一次提醒（可以 block，也可以继续）；再过同样长的时间还是没有调用，收件箱里出现「可能在打转」，带「打开终端」按钮。它停下来空闲时，这条提醒自动消失。
- **预算**（`lila task new … --budget 300k`，只能用在走流程的任务上）：
  - 统计方法：流程每一步的成员，从任务送达到完成这段时间里，会话记录中用了多少 token。
  - 只在步骤之间检查：超了就不开下一步，运行进入 `held`，记下下一步要做什么。收件箱里出现「超出预算」，有「继续」「取消」两个按钮，PM 也会收到一条通知。
  - 选「继续」：预算翻倍（至少是已用的量再加原预算），然后接着走；选「取消」：这次运行取消。
- **Codex 额度**：每分钟读一次项目里 Codex 会话记录中的额度。用到 100%、还没到重置时间时，给 Codex 成员的任务先排队，Claude 成员照常。收件箱提醒一次（「额度用完」，写着多久后重置，以及可以先把它换成 Claude 模型）。同一次用完只提醒一次，你点「知道了」后不会再冒出来。额度恢复后提醒自动关掉，排队的任务开始派发。
- **崩溃循环**：没有做。lilad 不会自动重启意外退出的成员，所以不会出现循环重启。

**提示词**：PM 段说明了验收卡片、「Rework:」任务、积压时新链要排队，以及 `--budget` 的用法；`skill.md` 同步更新。

**验证**
- 新增测试：
  - `message`：卡片和提醒的文字、token 的简写；
  - `store`：预算和暂停状态的读写；
  - CLI：`--budget` 的写法，比如 `300k`、`1.5M`。
  - daemon 6 个：
    - 验收卡片的证据顺序，以及通过和打回；
    - 积压时 PM 的新链排队，你派的活和链里面的工作照常；
    - 长时间没有进展：提醒一次，再进收件箱，空闲后自动消失；
    - 预算：超了暂停，「继续」后预算翻倍接着走，「取消」后停止；
    - Codex 额度用完时排队，提醒只出一次，额度恢复后派发。
- 原来几个测试改成新的卡片格式。工作区全部测试通过，Clippy 零警告。

**已知限制**
- Claude 的额度：它的会话记录里没有额度信息，所以 Claude 用完时还发现不了。只有 7501 报 error 时，成员才会标红。
- 预算只在步骤之间检查。一个很长的步骤可能大大超出预算，这种情况靠「长时间没有进展」的提醒来兜底。
- 积压上限只挡 PM 新开的链。PM 如果把新工作挂到旧链下面，就挡不住。
- Codex 额度取的是这个项目的会话记录里最新的那条。你在别处用 Codex 更多时，这里会晚一点才知道。（后来改了：装了 magpie 时，改用 magpie 读到的当前登录账号的额度，见 `docs/HANDOFF.md`。）
