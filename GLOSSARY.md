# Lila

Lila 是以 Pi 为内核的个人 Agent 桌面应用。本词表定义客户端和 core 共享的领域语言。

## Language

**Project**:
用户添加的本地目录。会话在这个目录里工作；添加即表示信任它的 `.pi` 资源。
_Avoid_: Workspace（仅在指代仓库结构或项目级重载操作时使用）。

**Session**:
Pi 的一个会话文件及其在 Lila 里的显示。Lila 和 `pi` 命令行看到的是同一个会话。
_Avoid_: Thread、Conversation。

**Run**:
从一条用户消息开始，到 Pi 不再自动继续（`agent_settled`）为止的一轮执行。
_Avoid_: Task（用于规划或 issue）。

**Transcript item**:
core 把 Pi 会话条目投影成的界面条目：用户消息、助手消息、工具调用、扩展消息、提示以及 Run 结束状态标记。Lila 不单独存储它们。
_Avoid_: Event（旧版自存的事件日志已取消）。

**Pending request**:
等待用户回答的请求：工具审批、Pi 扩展的 select/confirm/input/editor 对话框、`ask_user` 提问。只在 core 内存里。
_Avoid_: Prompt、Dialog。

**Approval mode**:
会话级工具审批配置。请求批准（`ask-all`）审查项目外写入、联网和范围不明的调用；帮我批准（`ask-exec`）同时放行可识别的低风险 MCP 调用；完全访问权限（`auto`）跳过 Lila 审批。前两档允许内置文件工具进行只读访问及项目内写入。
_Avoid_: Permission level、Security mode。

**Steer / Follow-up**:
Run 进行中发送的消息。Steer 在当前工具轮次后插入，Follow-up 等 Run 结束后开始下一轮。

**Rewind / Fork**:
基于某条历史用户消息的两种回溯方式。Rewind 将当前会话回退到该消息之前并把原内容放回输入框编辑重发；Fork 从该消息（或已完成的子会话）分叉出一条新的独立会话。

**Sub-agent**:
由 `Agent` 工具派出、在独立上下文与独立 Pi 会话文件里工作的子会话（`pi-subagents`）。

**Memory**:
跨会话持久化的长期记忆（`@amaster.ai/pi-memory`），分为 `MEMORY.md`（Agent 笔记）和 `USER.md`（用户画像），存放在 `~/.pi/agent/memories/`，会话启动时冻结注入快照，并支持后台自动整理（Dreaming）。
