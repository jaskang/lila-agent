Type: wayfinder:prototype
Status: resolved

Blocked by: 01

## Question

首版桌面会话界面如何让用户完成一次真实编程任务？用一个低成本交互原型确定项目/模型入口、消息列表、流式文本、思考/工具 trace、终端输出、diff、运行中断、错误和重连状态的排列及关键交互；只覆盖第一版闭环，不扩展到个人助理。

## Answer

直接在真实界面上迭代，没有单独做原型。

- 左侧 Sidebar（inset）：按项目分组列会话，显示标题、相对时间和状态图标（运行中、等待处理、失败）；底部「添加项目」用系统目录选择器。
- 右侧会话：顶栏显示标题、项目名、模型和审批方式下拉（运行中禁用）；中间 Message Scroller（`autoScroll`，打开时落在最后一条用户消息）；底部是待处理卡片和输入框。
- 用户消息用 Bubble；助手消息用 Typeset 渲染 Markdown，思考过程折叠在 Trace 里；工具调用是一行 Trace，显示模型写的意图和目标（命令、路径），展开后显示命令、输出和 diff。有 diff 或出错时默认展开。
- 运行状态、停止、失败、中断显示为 Marker；输入框 Enter 发送、Shift+Enter 换行、Esc 停止。
- 断线 0.8 秒后才显示提示条，避免快速重连时闪烁；请求失败用 sonner 提示。
- 2026-11-17 在 Electron 里用真实 omp 走通：修 bug、两次审批、diff 显示、杀掉 core 后自动恢复。
