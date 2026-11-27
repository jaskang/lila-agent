# 05：macOS 应用打包、launchd 开机常驻与移动端轻审批控制台

Status: needs-triage

## 背景与目标

让 Lila 从本地开发态（`pnpm dev`）正式进化为每天开机不断电、离座也能随时拍板的个人主力工程助理。

## 交付范围

1. **macOS 应用打包分发（`.app` / `.dmg`）**
   - 完善 Electron 打包配置，并实测打包态下 `ELECTRON_RUN_AS_NODE=1` 运行 `core`、内置 Pi 扩展、`croner` 与 `@anthropic-ai/sandbox-runtime` 的路径解析，消除开发态与生产态差异。
2. **macOS `launchd` 用户级常驻服务**
   - 在设置中心提供「开机后台常驻（LaunchAgent）」开关，托管 `~/Library/LaunchAgents/com.lila.core.plist`，电脑重启或关闭桌面窗口后 `core` 与后台定时任务自动静默恢复。
3. **移动端轻审批控制台（复用协议 v20 + Tailscale）**
   - 经 Tailscale 仅向个人设备暴露轻量响应式 Web 控制台（复用现有 WebSocket + `core-token` 鉴权）；
   - 聚焦离座高频场景：查看「需要关注」、审批工具/域名请求、一键确认任务开工、查看验收对比图以及复制对外回复草稿。
