# 内核用 pi-coding-agent SDK，会话用 Pi 自己的文件

Lila 需要 MCP、技能、子 Agent、上下文文件和 Pi 扩展生态，这些都在 `pi-coding-agent` 的 `AgentSession` 这一层（`pi` 命令行本身也跑在这上面），而之前接入的 `pi-durable` 是标明实验性、API 随时会变的底层运行时，只有读写改和 bash 四个工具，上述能力都要自己实现。所以 core 直接嵌入 SDK，每个 Lila 会话一个 `AgentSession`，会话存成 Pi 的 JSONL 文件并与 `pi` 命令行共用配置目录，Lila 不再维护自己的事件库。

## Considered Options

- **继续用 pi-durable**：唯一优势是 core 崩溃后能接着跑完正在进行的一轮。代价是自己实现 MCP 桥、技能加载、上下文文件、子 Agent，且不兼容 Pi 扩展和 Pi 包。
- **每个会话一个 `pi --mode rpc` 子进程**：隔离好，但每个会话多一个进程，还要依赖系统装的 `pi`；Pi 文档也建议 Node/Bun 宿主直接用 SDK。

## Consequences

- core 崩溃时正在跑的一轮会中断，已完成的消息不丢。
- 待处理项（审批、提问）只在内存里。
- 升级 Pi 版本时要重新跑一遍真实模型、工具、审批、子 Agent 的冒烟。
