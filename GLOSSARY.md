# Lila

Lila 是连接桌面界面、独立 core 和 Agent 运行时的个人编程工作台。本词表定义第一版编程闭环中跨客户端和 core 共享的领域语言。

## Language

**Project**:
用户选择的本地代码目录及其在 Lila 中的配置。
_Avoid_: Workspace（指仓库结构时使用）。

**Session**:
围绕一个 Project 持续保存对话、运行记录和事件的工作上下文。
_Avoid_: Thread、Conversation（需要指 Lila 的持久化工作上下文时使用 Session）。

**Run**:
Session 中由一条用户消息触发、直到完成、中断或失败的一轮 Agent 执行。
_Avoid_: Task（Task 用于规划或 issue 类型，不表示一次 Agent 执行）。

**Event**:
由 core 持久化并按 Session 内序号排列、供客户端恢复和渲染的一条事实记录。
_Avoid_: Message（Message 只是 Event 的一种载荷）。

**Pending item**:
等待用户回答或审批、并在 Session 中保持可恢复状态的交互请求。
_Avoid_: Prompt、Dialog（这些描述界面或输入形式，不描述持久化对象）。

**Approval mode**:
决定工具执行是否需要用户确认，以及确认粒度的 Session 运行配置；第一版支持 `yolo`、`write` 和 `always-ask`。
_Avoid_: Permission level、Security mode（它是 Agent 运行配置，不是通用权限模型）。
