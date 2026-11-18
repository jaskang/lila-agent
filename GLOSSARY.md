# Lila

Lila 是以 Pi 为内核的个人 Agent 桌面应用。本词表定义客户端和 core 共享的领域语言。

## Language

**Project**:
用户添加的本地目录。会话在这个目录里工作；添加即表示信任它的 `.pi` 资源。
_Avoid_: Workspace（指仓库结构时使用）。

**Session**:
Pi 的一个会话文件及其在 Lila 里的显示。Lila 和 `pi` 命令行看到的是同一个会话。
_Avoid_: Thread、Conversation。

**Run**:
从一条用户消息开始，到 Pi 不再自动继续（`agent_settled`）为止的一轮执行。
_Avoid_: Task（用于规划或 issue）。

**Transcript item**:
core 把 Pi 会话条目投影成的界面条目：用户消息、助手消息、工具调用、扩展消息、提示。Lila 不单独存储它们。
_Avoid_: Event（旧版自存的事件日志已取消）。

**Pending request**:
等待用户回答的请求：工具审批、Pi 扩展的 select/confirm/input/editor 对话框、`ask_user` 提问。只在 core 内存里。
_Avoid_: Prompt、Dialog。

**Approval mode**:
会话级配置，决定哪些工具调用要用户确认：`auto`（不问）、`ask-exec`（命令、MCP、子 Agent、脚本前问）、`ask-all`（除只读工具外都问）。
_Avoid_: Permission level、Security mode。

**Steer / Follow-up**:
Run 进行中发送的消息。Steer 在当前工具轮次后插入，Follow-up 等 Run 结束后开始下一轮。

**Sub-agent**:
由 `Agent` 工具派出、在独立上下文里工作的子会话（pi-subagents）。
