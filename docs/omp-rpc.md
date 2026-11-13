# omp RPC 笔记

`core` 的 omp 适配层以这份笔记为依据。来源是 omp 18.20.0 的官方文档 [`docs/rpc.md`](https://github.com/can1357/oh-my-pi/blob/main/docs/rpc.md) 和本机实测。每条都标了是否实测过。

## 结论

- **RPC 模式可用，可以作为 Lila 的运行时。** 本机 omp 18.20.0 以 `--mode rpc-ui` 启动，Claude Code 通道（`claude/claude-sonnet-5-5`）和 Codex 通道（`codex/gpt-6-luna`）各跑通了真实对话和工具调用。（已实测）
- **本机的模型来自 `magpie` 提供方**，共 9 个，走 Claude Code、Codex、Cursor 三条通道。omp 自带的 `anthropic` 和 `openai-codex` 订阅登录都显示未登录。（已实测）
- **审批默认是关的。** 本机 omp 配置 `tools.approvalMode` 为 `yolo`，所有工具直接执行。Lila 启动 omp 时要用 `--approval-mode` 明确指定。（已实测）
- **额度扣在哪里没法从 RPC 确认。** 消息里的 `usage.cost` 是按公开价格算出来的名义值，不代表实际扣费。实际走的是 magpie 背后的订阅，需要在各家的用量页面核对。
- 实测用 `scripts/omp-probe.ts`，会消耗少量订阅额度。

## 启动

```bash
omp --mode rpc-ui --approval-mode yolo
```

Lila 默认用 `yolo`，每个会话可以切换档位。切换档位要重启该会话的 omp 进程，因为档位是启动参数。

- 协议是换行分隔的 JSON，走标准输入输出。它是 omp 自定义的协议，不是 JSON-RPC 2.0。
- **不要加 `--no-ui`。** 工具审批走扩展界面通道，加了 `--no-ui` 后需要审批的工具会直接失败。（文档）
- **`--approval-mode` 决定哪些工具要审批。** 工具分三级：读、写（改文件或会话状态）、执行（shell、代码、浏览器、子 Agent）。来源是官方文档 [`docs/approval-mode.md`](https://github.com/can1357/oh-my-pi/blob/main/docs/approval-mode.md)，`yolo`、`write`、`always-ask` 三档的行为已实测一致。

  | 档位 | 自动通过 | 要询问 |
  |---|---|---|
  | `always-ask` | 读 | 写、执行 |
  | `always-ask-cached` | 不涉及文件的读、已授权的文件和目录 | 读未授权的文件、写、执行 |
  | `write` | 读、写 | 执行 |
  | `yolo` | 全部 | 无 |

- **档位之外的控制**：`tools.approval.<工具名>`（`allow`、`deny`、`prompt`）在任何档位下都生效；`tools.restrictedFolders` 里的目录任何操作都要逐次同意，`yolo` 下也一样；危险命令（如 `rm -rf /`）在非 `yolo` 档位下强制询问，`yolo` 下不保护。
- **审批不是沙箱。** 命令被同意后拥有完整的文件、网络和子进程权限。
- 工作目录就是项目目录，omp 按工作目录存会话文件（`~/.omp/agent/sessions/<目录名>/<时间>_<会话 ID>.jsonl`）。（已实测）
- 启动后先输出一条 `ready`，之后才处理命令。（已实测）
- 关闭标准输入会让 omp 处理完已收命令后正常退出，退出码 0。关闭后仍要继续读标准输出直到结束。（已实测）

## 握手

`ready` 帧声明支持协议版本 1 和 2，单帧上限 1 MiB：

```json
{ "type": "ready", "protocolVersion": 1, "supportedProtocolVersions": [1, 2], "maxFrameBytes": 1048576, "maxReassembledFrameBytes": 67108864 }
```

收到后立刻协商到版本 2（已实测）：

```json
{ "id": "p", "type": "negotiate_protocol", "protocolVersion": 2 }
```

版本 2 下，超过 1 MiB 的帧会拆成连续的 `rpc_chunk` 帧，内容是 base64，需要按序拼回。版本 1 下超限内容会被截断或报错，所以必须用版本 2。

## 命令与响应

每条命令可以带 `id`，响应会带回同一个 `id`：

```json
{ "id": "s", "type": "get_state" }
{ "id": "s", "type": "response", "command": "get_state", "success": true, "data": { } }
```

失败时 `success` 为 `false`，带 `error` 字符串。大多数失败不会让进程退出。

第一版用到的命令：

| 用途 | 命令 | 实测 |
|---|---|---|
| 发消息 | `prompt`（`message`，可选 `images`） | 已实测 |
| 运行中追加消息 | `steer`、`follow_up` | 未测 |
| 中断 | `abort` | 未测 |
| 读当前状态 | `get_state` | 已实测 |
| 列出模型 | `get_available_models` | 已实测 |
| 切换模型 | `set_model`（`provider: "magpie"`、`modelId`） | 已实测 |
| 新会话 | `new_session` | 未测 |
| 恢复旧会话 | `switch_session`（`sessionPath`） | 未测 |
| 读历史消息 | `get_messages_page`（`cursor`、`limit`） | 未测 |
| 只收增量 | `set_event_filter`（`messageUpdates: "delta"`） | 未测 |
| 列出斜杠命令 | `get_available_commands` | 已实测 |
| 登录方式 | `get_login_providers`、`login` | 前者已实测 |

`get_state` 返回的字段里，适配层要用的有：`model`、`thinkingLevel`、`isStreaming`、`isSettled`、`sessionFile`、`sessionId`、`messageCount`、`contextUsage`、`queuedMessages`、`todoPhases`。

## 一轮对话的事件顺序

已实测。下面是「列出目录文件再回答」这一轮的实际序列，省略了内容：

```json
{ "id": "prompt", "type": "response", "command": "prompt", "success": true }
{ "type": "agent_start" }
{ "type": "turn_start" }
{ "type": "message_start", "messageId": "msg-1", "message": { "role": "user" } }
{ "type": "message_end", "messageId": "msg-1" }
{ "type": "message_start", "messageId": "msg-2", "message": { "role": "assistant" } }
{ "type": "message_update", "messageId": "msg-2", "assistantMessageEvent": { "type": "thinking_delta", "delta": "..." } }
{ "type": "message_update", "messageId": "msg-2", "assistantMessageEvent": { "type": "toolcall_delta" } }
{ "type": "message_end", "messageId": "msg-2", "message": { "stopReason": "toolUse" } }
{ "type": "tool_execution_start", "toolCallId": "toolu_...", "toolName": "bash", "args": { "command": "ls -la" }, "intent": "Listing directory files" }
{ "type": "tool_execution_update", "toolCallId": "toolu_...", "partialResult": { "content": [] } }
{ "type": "tool_execution_end", "toolCallId": "toolu_...", "result": { "content": [{ "type": "text", "text": "..." }] }, "isError": false }
{ "type": "message_start", "messageId": "msg-3", "message": { "role": "toolResult" } }
{ "type": "message_end", "messageId": "msg-3" }
{ "type": "turn_end" }
{ "type": "turn_start" }
{ "type": "message_start", "messageId": "msg-4", "message": { "role": "assistant" } }
{ "type": "message_update", "messageId": "msg-4", "assistantMessageEvent": { "type": "text_delta", "delta": "..." } }
{ "type": "message_end", "messageId": "msg-4", "message": { "stopReason": "stop" } }
{ "type": "turn_end" }
{ "type": "agent_end", "isTerminal": true, "yielded": true }
{ "type": "prompt_result", "id": "prompt", "agentInvoked": true, "status": "completed", "sessionSettled": true }
{ "type": "session_settled" }
```

- **`prompt` 的响应只表示消息已被接收**，不表示这一轮跑完。
- **`prompt_result` 表示这一轮结束**，`status` 是 `completed`、`aborted` 或 `error`。模型出错时 `prompt` 的响应仍然是成功，要看 `prompt_result.status`。
- **`session_settled` 表示会话彻底空闲**，后台工作也都结束了。回收进程前要等它。
- **一个 turn 是一次模型调用加它引发的工具执行。** 一轮对话里有几次模型调用就有几个 turn。
- **`message_update` 默认只带增量**，字段是 `messageId` 和 `assistantMessageEvent`。增量分三类，各有 `_start`、`_delta`、`_end`：`thinking`（思考）、`text`（正文）、`toolcall`（工具参数）。
- **`message_end` 带完整消息**，助手消息里有 `usage`（输入、输出、缓存读写的 token 数和名义费用）、`model`、`provider`、`duration`、`ttft`、`stopReason`。
- **工具调用用 `toolCallId` 串起来。** `tool_execution_start` 带 `toolName`、`args` 和一句 `intent`（模型自己写的意图说明，可以直接显示）。
- **编辑文件时有 `tool_stream_update`**，带 `files: [{ path, diff, op }]`，可以在工具执行过程中就显示 diff。
- `agent_end.messages` 在内容过大时不完整，适配层要自己保留流式收到的消息。
- 这一轮的日志约 137 KB，其中大部分是 `message_end` 里的完整消息。

其他事件：压缩（`auto_compaction_*`）、重试（`auto_retry_*`）、`model_changed`、`queue_update`、`notice`、`advisor_cost_changed`、`available_commands_update`。

## 审批和提问

omp 需要用户决定时，发 `extension_ui_request`，`core` 回 `extension_ui_response`，用 `id` 对应。

| `method` | 含义 | 回复 |
|---|---|---|
| `confirm` | 是或否 | `{ confirmed: boolean }` |
| `select` | 从 `options` 里选一个 | `{ value: string }` |
| `input`、`editor` | 填文本 | `{ value: string }` |
| `ask` | 一次问多个问题，带选项 | `{ answers: [{ id, selectedOptions, customInput? }] }` |
| `cancel` | omp 撤回某个请求（`targetId`） | 不用回复，关掉对应界面 |
| `notify`、`setStatus`、`setWidget`、`open_url` | 只是通知 | 不用回复 |

- 任何请求都可以回 `{ cancelled: true }` 表示用户取消。
- 请求可能带 `timeout`，超时后 omp 按默认值继续并发 `cancel`。
- `ask` 要先发 `set_ask_dialog`（`enabled: true`）才会出现；它和 shadcn 的 Questionnaire 组件形状一致。否则 omp 把问题拆成一连串 `select`。
- 启动时就会收到几条 `setStatus`、`notify`、`setWidget`，来自已装的扩展，可以忽略或显示在状态栏。（已实测）

### 工具审批的实际格式

已实测（`--approval-mode always-ask`，模型调用 bash）。omp 在 `tool_execution_start` 之后发一条 `select`：

```json
{
	"type": "extension_ui_request",
	"id": "15cc55f86478f5a6",
	"method": "select",
	"title": "Allow tool: bash\nCommand: rm hello.txt",
	"options": ["Approve", "Approve bash for this session", "Deny"]
}
```

- **回复选项的原文**：`{ "type": "extension_ui_response", "id": "...", "value": "Approve" }`。
- **请求里没有 `toolCallId`。** 适配层要把它和紧挨着的上一条 `tool_execution_start` 对应起来；工具名和参数从那条事件里取，不要解析 `title`。
- **拒绝或取消后**，工具以 `isError: true` 结束，结果文本是 `Tool call denied by user: bash`，模型会收到并继续回答。回 `{ cancelled: true }` 的效果和选 `Deny` 一样。
- 「Approve bash for this session」由 omp 自己记住，同一进程内该工具不再询问。
- 选择 `Approve` 后工具正常执行这一路还没跑过，同时有多个工具等待审批时的顺序也没测。

## 本机的模型

`get_available_models` 返回 9 个，全部在 `magpie` 提供方下（已实测）：

| 模型 ID | 名称 |
|---|---|
| `claude/claude-sonnet-5-5` | Claude Sonnet 5.5 · Claude Code |
| `claude/claude-opus-5-5` | Claude Opus 5.5 · Claude Code |
| `codex/gpt-6.1-sol` | GPT-6.1-Sol · Codex |
| `codex/gpt-6-astra` | GPT-6-Astra · Codex |
| `codex/gpt-6-luna` | GPT-6-Luna · Codex |
| `cursor/claude-opus-5-5@1m` | Claude Opus 5.5 @ 1m · Cursor |
| `cursor/claude-fable-5-2@1m` | Claude Fable 5.2 @ 1m · Cursor |
| `cursor/claude-sonnet-5-5@1m` | Claude Sonnet 5.5 @ 1m · Cursor |
| `cursor/grok-4.7@500k` | Grok 4.7 @ 500k · Cursor |

默认模型是 `magpie` 的 `claude/claude-sonnet-5-5`，思考级别 `high`。

## 可以直接用的现成东西

- **官方 TypeScript 客户端。** npm 包 `@oh-my-pi/pi-coding-agent`（18.20.0）导出 `RpcClient`，已实现起进程、请求对应、版本 2 协商、分片拼接、历史分页、`promptAndWait()`、`waitForSettled()` 和扩展界面请求。适配层优先用它，不自己写协议解析。它默认用 `bun` 启动，和 `core` 的运行时一致。（文档，未试用）
- **协议的 JSON Schema。** omp 仓库里有 `rpc-wire.schema.json` 和生成的 TypeScript 类型，可以用来校验。
- **宿主工具。** `set_host_tools` 让 `core` 向 omp 注册自己实现的工具，omp 调用时发 `host_tool_call`，`core` 回 `host_tool_result`。以后做个人助理的日历、待办等工具走这条路，不用写 omp 扩展。（文档）

## 还要实测的

1. 审批选 `Approve` 后的完整流程，以及多个工具同时等待审批的情况。
2. `always-ask-cached` 档的授权选项在 RPC 下的实际格式。
3. `abort`、`switch_session`、`get_messages_page` 的实际行为。
4. 试用官方 `RpcClient`，确认它在 Bun 下可以直接用。
5. 在 Claude 和 ChatGPT 的用量页面核对额度确实扣在订阅上。
