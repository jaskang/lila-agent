Type: wayfinder:grilling
Status: resolved

Blocked by: 01, 02

## Question

项目、模型和会话在第一版中的关系是什么？需要确定添加本地项目目录、新建/恢复会话、按会话选择模型、模型列表来源，以及会话默认 approval mode 和运行时配置的生命周期，确保“新建会话”能进入协议闭环。

## Answer

- Project 是本地目录，按路径去重，名称取目录名；添加时 core 校验目录存在。
- Session 属于一个 Project，保存模型、审批方式、状态和对应的 omp 会话文件；标题取第一条消息的前 60 个字符。
- 模型列表和默认模型来自 omp 的 `get_available_models` 与 `get_state`，core 缓存一次；ID 编码为 `provider::modelId`。读取失败时界面显示原因和重试按钮。
- 修改模型或审批方式时先停掉该会话的 omp，下一条消息用 `--resume <会话文件>` 重新启动，对话上下文保留。
- omp 进程空闲 10 分钟后回收，下次发消息同样用 `--resume` 恢复。
- 界面记住上次打开的会话，重启应用后直接回到那里。
