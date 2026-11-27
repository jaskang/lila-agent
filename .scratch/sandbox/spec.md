# P0：OS 级沙箱调研与方案设计

Status: ready-for-agent

## 1. 背景与目标

Lila 当前已落地后台定时调度器（`packages/core/src/scheduler-service.ts`）与个人助理任务收件箱（`packages/core/src/task-service.ts`），同时在交互会话中支持三档审批模式（`ask-all` / `ask-exec` / `auto`）。

现有安全防线仅存在于工具调用前的静态检查（`packages/core/src/approval-policy.ts`）：
- 一旦某条 `bash` 命令获批（或命中白名单 / 处于 `auto` 模式），子进程直接以宿主用户完整权限执行；
- 脚本或构建工具内部的子进程调用（如 `pnpm test`、测试脚本、恶意依赖）仍可读取 `~/.ssh`、`~/.aws` 等敏感凭据，写入项目外目录，或向任意外部地址发起网络请求；
- 定时任务与无人值守场景缺乏操作系统内核级兜底，而日常交互中低风险脚本又容易带来审批疲劳。

**目标**：借鉴 DeepSeek Harness 的“沙箱与审批解耦”模型，为 Lila 增加一层**可选的、Fail-closed 的 OS 级进程沙箱**，同时遵守 Lila 的核心约束：
1. 复用社区成熟方案，不自写底层 Seatbelt/bwrap 规则引擎；
2. 保持与 `pi` CLI 的兼容，不破坏用户自装的外部沙箱扩展；
3. 兼顾 Lila `core` **单进程托管多项目并发会话**的架构特点。

---

## 2. 调研结论（一手验证）

### 2.1 宿主环境与依赖现状
- **macOS 原生隔离**：`/usr/bin/sandbox-exec`（Seatbelt）系统自带，无需 Docker/VM。
- **`ripgrep` 依赖隐患**：本机默认 `PATH` 无 `rg`，但 Pi 已在 `~/.pi/agent/bin/rg` 内置了一份 `rg`。`@anthropic-ai/sandbox-runtime` 部分版本初始化会检测 `rg`，因此必须显式注入 `rg` 路径或将其目录拼入检测环境，避免用户未装 Homebrew `rg` 时失败。
- **Node 版本**：Lila `core` 运行在 Electron 自带的 Node 24 上，满足 `@anthropic-ai/sandbox-runtime`（`>=22.12.0`）要求。

### 2.2 候选方案对比

| 维度 | 方案 A：直接集成 `@anthropic-ai/sandbox-runtime`（推荐） | 方案 B：内置 `@erichll/pi-sandbox` 扩展 | 方案 C：自写 macOS `sandbox-exec` 封装 |
| --- | --- | --- | --- |
| 成熟度 | Anthropic 开源，Claude Code 同源底层（npm `0.0.83`） | Pi 社区扩展（`0.29.4`），本身也基于 `sandbox-runtime` | 需自行维护 Seatbelt 规则、危险文件列表、网络代理与 Linux 兼容 |
| 依赖兼容性 | 轻量，仅依赖 `zod` / `commander` / `node-forge` / socks5 | peerDeps 强绑定 `pi-subagents >= 0.83`、`typebox >= 1.4.3`，且强依赖 `@erichll/pi-auto-review` | 无外部依赖 |
| 与 Lila 集成 | 可直接接入 Lila 的 `ApprovalMode`、`PendingRequest` 审批卡、定时任务和状态栏 | 自带独立审批代理和 TUI 逻辑，与 Lila UI/协议割裂 | 易集成但安全细节极易踩坑（`.git/hooks`、`~/.zshrc`、dyld 缓存、出网代理） |
| 多项目并发 | 需验证并妥善处理 `SandboxManager` 单例约束（见 3.3） | 同受底层 `SandboxManager` 约束 | 天然按命令独立 |

**结论**：选 **方案 A（集成 `@anthropic-ai/sandbox-runtime`）**；如果用户自己在 `~/.pi/agent` 安装了外部沙箱扩展（如 `@erichll/pi-sandbox`），参照 Lila 对 `pi-memory` / `pi-subagents` 的做法**自动检测并让位**（见 `packages/core/src/extensions-service.ts`）。

### 2.3 Pi SDK 挂载点与子 Agent 逃逸面核实
通过检查 `packages/core/node_modules/@earendil-works/pi-coding-agent` 与 `@tintinweb/pi-subagents` 源码确认：

1. **主会话 `bash` 覆盖非常干净**：
   - `pi-coding-agent` 的 `AgentSession._refreshToolRegistry()` 允许扩展调用 `pi.registerTool(createBashToolDefinition(cwd, { operations }))` 直接覆盖内置 `bash` 工具；
   - 覆盖后自动保留 Pi 原生的流式输出、超时、输出截断、结构化结果（供 `codemode` 使用）、`PI_*` 环境变量与进程树清理；
   - `pi.on("user_bash", () => ({ operations }))` 可同步覆盖用户手动触发的 `!cmd`。
2. **重要发现：`pi-subagents` 子会话默认不加载 `lila` 内联扩展**：
   - `@tintinweb/pi-subagents/src/agent-runner.ts` 在同进程内通过 `new DefaultResourceLoader({ extensionFactories: [codemode, tool-search, mcp, scopeGate] })` + `createAgentSession()` 创建子 Agent；
   - 这意味着仅在 `createLilaExtension`（作为内联 `extensionFactory`）里覆盖 `bash`，**无法自动覆盖子 Agent 内部执行的 `bash`**；
   - **规避/补齐路径**：
     - P0 阶段先明确记录该边界：对启用了沙箱的会话，检查 `Agent` 工具派发或给子 Agent 提供沙箱能力；
     - 后续/同阶段验证是否可通过**磁盘路径扩展**（`additionalExtensionPaths` 配合环境上下文）或 `pi-subagents` 暴露的扩展机制覆盖子 Agent 的 `bash`，或在沙箱严格模式下限制带 `bash` 的子 Agent 需经审批。

---

## 3. 核心方案设计

### 3.1 安全模型：Sandbox 与 Approval 解耦

保留现有 `ApprovalMode`（管“是否需要人点批准”），新增独立的 `SandboxMode`（管“OS 内核如何约束子进程”）：

```ts
export const SandboxMode = z.enum(["off", "workspace-write"]);
// 预留后续扩展："read-only"
```

| 维度 | `ApprovalMode`（审批层） | `SandboxMode`（OS 隔离层） |
| --- | --- | --- |
| 职责 | 调用前询问用户 / 匹配命令白名单 | 执行时内核强制边界，子进程树全继承 |
| 档位 | `ask-all` / `ask-exec` / `auto` | `off` / `workspace-write` |
| 失败语义 | 用户拒绝则 `block: true` | **Fail-closed**：开启沙箱但环境不支持/初始化失败时直接拒绝执行，绝不静默裸跑 |
| 适用范围 | 所有工具（文件、bash、MCP、记忆等） | 所有通过 `bash`（及 `user_bash`）启动的子进程树；Pi 内置 `read`/`write`/`edit` 已在 `approval-policy.ts` 中校验路径，沙箱开启时可与 `denyRead`/`denyWrite` 规则对齐 |

### 3.2 `workspace-write` 默认策略（面向日常编程的实用平衡）

参考 `@anthropic-ai/sandbox-runtime` 的 macOS Seatbelt 模型（默认禁止写入、按白名单放行写入；读取默认放行但显式屏蔽高危目录）：

1. **文件系统写入白名单（`allowWrite`）**：
   - 当前项目根目录 `projectRoot`（若为 Git Worktree，还需包含其真实 `.git` common dir，避免 `git commit`/`git status` 失败）；
   - 系统临时目录（`/tmp`、`/private/tmp`、`$TMPDIR`）；
   - 常见只影响本地缓存的工具缓存目录（可选配置，如 `~/.cache`、`~/Library/Caches/pnpm`、npm/cargo/uv 缓存，避免每次构建都卡死——或默认只开项目 + 临时目录，提供项目级可追加路径）。
2. **文件系统写入黑名单（`denyWrite`，优先生效）**：
   - `.git/hooks`、`.git/config`、`.env` / `.env.*`（防篡改钩子或覆盖密钥）；
   - `~/.zshrc`、`~/.bashrc`、`~/.profile` 等 shell 启动文件（`sandbox-runtime` 本身也内置危险文件保护）。
3. **文件系统读取黑名单（`denyRead`）**：
   - `~/.ssh`、`~/.aws`、`~/.gnupg`、`~/.kube`、`~/.npmrc`、`~/.netrc`；
   - Lila 自身敏感文件：`~/Library/Application Support/Lila/core-token`、`~/.pi/agent/auth.json`（防恶意脚本窃取 `core` 鉴权 token 或模型 API Key！）。
4. **网络策略（`network`）**：
   - 默认模式需用户拍板（见第 6 节待决问题）：
     - 选项 1（推荐起步）：默认禁止出网，预置开发常用域名白名单（或仅放行用户配置的域名），遇到被拦域名时给出清晰报错提示，支持一键加入项目允许域名；
     - 选项 2：P0 第一步先允许常规出网（或只拦私网/元服务），把网络代理审批放到 P0.2，降低初期摩擦。

### 3.3 解决 `SandboxManager` 单进程单例与多项目并发（已由 Ticket 01 Spike 验证）

Lila `core` 是常驻单进程，可能同时打开 Project A 和 Project B 的会话，或前台跑 Project A、后台定时任务跑 Project B。

**设计**：在 `packages/core/src/sandbox-service.ts` 中封装统一的 `SandboxService`：
1. **全局单次初始化 + 按命令编译策略**：
   - 进程内只对 `SandboxManager` 做一次受控初始化（惰性启动：仅当有会话/任务实际开启沙箱时启动），并在 `core` 退出时统一 `reset()`；
   - 初始化时 `network` 必须传 `{}`（保持 `allowedDomains: undefined`，从而不劫持网络流量），并预先 `mkdirSync("/tmp/claude", { recursive: true })` 确保 `os.tmpdir()` 可用；
   - 自动探测系统 `rg` 并在 Linux 上回退到 `join(getAgentDir(), "bin", "rg")`（macOS `0.0.83` 已零依赖）；
   - 每次执行命令通过 `wrapWithSandbox(command, shellPath, perProjectConfig, abortSignal, { commandId, commandText })` 传入**该项目专属的文件系统读写策略**（已实测并发多项目完全隔离，`allowWrite` 仅需 `[projectRoot, ...(worktreeGitCommonDir ? [worktreeGitCommonDir] : []), ...getDefaultWritePaths()]` 即可跑通 `git`/`pnpm`/`tsc`/`vitest`）；
2. **集成到 Pi `BashOperations`**：
   - 新增 `createSandboxedBashOperations(baseOps, resolveSandboxPolicy)`：
     - 当 `mode === "off"`：直接透传给 `createLocalBashOperations()`；
     - 当 `mode === "workspace-write"`：调用 `SandboxService.wrapCommand(...)` 包装后再交给 `baseOps.exec(...)`；若沙箱不可用则抛出明确的 `UserError`（Fail-closed）；
     - 执行失败时调用 `SandboxManager.annotateStderrWithSandboxFailures(commandId, stderr)`，将 Seatbelt 拦截的具体路径附加到输出中，减少模型和用户的困惑。
3. **`pi-subagents` 子会话逃逸兜底（Fail-closed）**：
   - 经 Ticket 01 核实，`@tintinweb/pi-subagents@0.26.1` 在同进程内自建 `DefaultResourceLoader`，不会加载内联 `lila` 扩展；
   - 当会话开启 `workspace-write` 且未检测到外部全局沙箱扩展时，`lila-extension.ts` 在 `tool_call` 中对 `Agent` / `SubagentWorkflow` 触发门控：无人值守场景（如定时任务）直接拒绝并提示模型在主会话执行，交互会话弹出 `PendingRequest` 明确告知用户子 Agent 的 `bash` 暂不受沙箱隔离。

### 3.4 与审批策略（`approval-policy.ts`）的联动

- 沙箱开启后，**项目内**的 `bash` 调用受内核约束，安全感显著提高：
  - `ask-all`：依然按原逻辑询问（适合高谨慎场景）；
  - `ask-exec`（帮我批准）：可考虑在沙箱开启且非破坏性/非逃逸命令时减少不必要的弹窗，或保持先独立、验证稳定后再放宽；
  - `auto`（完全访问）& **定时任务**：即使跳过人工审批，依然被 OS 沙箱兜底。
- **定时任务默认值**：新建定时任务（`ScheduledJob`）默认 `sandboxMode: "workspace-write"`，老数据反序列化时兼容默认值策略（可在 spec 确认）。

### 3.5 协议、状态与 UI 最小改动面

1. **`packages/protocol`**：
   - 新增 `SandboxMode`（`"off" | "workspace-write"`）与 `SandboxStatus`（可用性、后端名称如 `seatbelt` / `bubblewrap`、不可用原因）；
   - `SessionState` 增加 `sandboxMode`；`ScheduledJob` / `ScheduleCreateInput` / `ScheduleUpdatePatch` 增加 `sandboxMode`；
   - 新增命令 `session.setSandboxMode`（与现有 `session.setApprovalMode` 对称）。
2. **`packages/core/src/store.ts`**：
   - `state.json` 增加 `sandboxModes: Record<string, SandboxMode>`（按会话持久化）与项目级沙箱例外规则（如允许的额外写路径/域名，第二阶段可扩展）。
3. **`packages/app-desktop`**：
   - 复用现有 shadcn/ui 组件：在会话输入区底部审批模式选择器旁增加沙箱状态指示/切换器（盾牌图标 + 工具提示说明限制范围）；
   - 定时任务创建/编辑弹窗增加沙箱开关（默认开启）。

---

## 4. 风险与边界清单

1. **Git Worktree 写权限**：Worktree 项目的 `.git` 是指向主仓库 `.git/worktrees/<name>` 的文本文件，提交/切分支需要写主仓库的 `.git` 目录，规则生成器必须解析 `git rev-parse --git-common-dir` 放行必要路径（同时继续保护 `hooks`）。
2. **包管理器与构建工具缓存**：`pnpm`、`npm`、`cargo`、`uv` 会写全局 store/cache。若未放行缓存目录，可能导致构建失败；需在 Spike 中实测 `pnpm test`、`tsc`、`git status`、`git commit`。
3. **子 Agent 覆盖缺口**：`pi-subagents` 在同进程创建子 `AgentSession` 且不自动加载 `lila` 内联扩展，P0 需明确处理策略（阻止未沙箱子 Agent、审批提示，或通过扩展路径注入覆盖子 Agent 的 `bash`）。
4. **Electron 打包环境**：开发态和打包态下 `ELECTRON_RUN_AS_NODE=1` 运行 `core`，需确认 `@anthropic-ai/sandbox-runtime` 及其代理依赖能被正常解析运行。

---

## 5. 分阶段实施拆解（建议）

- **Ticket 01（Spike / 原型验证）**：安装 `@anthropic-ai/sandbox-runtime`，编写最小验证测试，在 macOS 上实测：
  1. 用 `~/.pi/agent/bin/rg` 初始化是否无报错；
  2. 并发多项目 `wrapWithSandbox(..., customConfig)` 是否互相隔离；
  3. 实测拦截写入项目外文件、拦截读取 `~/.ssh` 与 `core-token`、放行项目内 `pnpm test` / `git status` / Worktree 操作；
  4. 验证 `pi-subagents` 子 Agent 能否通过共享扩展路径一并接入沙箱。
- **Ticket 02（Core + Protocol）**：实现 `SandboxService`、`createSandboxedBashOperations`、外部沙箱扩展自动让位逻辑、协议与 `store.ts` / `scheduler-service.ts` 接入。
- **Ticket 03（Desktop UI）**：会话栏沙箱切换与状态展示、定时任务沙箱选项、沙箱拦截提示体验优化。

---

## 6. 需要确认的设计决策（Open Questions）

1. **默认开启范围**：
   - 普通交互会话默认 `off` 还是 `workspace-write`？
   - 后台定时任务（`ScheduledJob`）默认 `workspace-write` 是否符合预期？
2. **网络策略第一步怎么选**：
   - **A（推荐）**：P0.1 先聚焦文件系统隔离（屏蔽敏感凭据读取 + 限制仅项目目录可写），网络默认放行或仅做简单域名配置，避免开发时 `npm`/`git`/`curl` 频繁撞墙；P0.2 再做基于 `PendingRequest` 的动态域名审批。
   - **B**：一步到位默认断网 + 域名白名单。
3. **子 Agent 策略**：
   - P0 是先做到“主会话 `bash` + 定时任务强制沙箱，同时如果子 Agent 需要 `bash` 则走审批提示”，还是一并把 `pi-subagents` 子会话的 `bash` 也接入沙箱？
4. **构建工具缓存**：
   - `workspace-write` 是否默认放行常见包管理器缓存目录（如 `pnpm`/`npm`/`uv`/`cargo` 缓存），以保证日常跑构建和测试开箱即用？
