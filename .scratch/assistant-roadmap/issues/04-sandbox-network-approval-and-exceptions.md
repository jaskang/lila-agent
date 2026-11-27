# 04：OS 沙箱闭环补全（P0.2：出网域名审批卡与项目级例外路径）

Status: needs-triage

## 背景与目标

P0.1 已落地基于 `@anthropic-ai/sandbox-runtime` 的文件系统隔离（`workspace-write`），出网与项目级自定义可写例外目录预留到 P0.2 推进（详见 `.scratch/sandbox/spec.md`）。

## 交付范围

1. **动态出网域名审批卡**
   - 接入 `@anthropic-ai/sandbox-runtime` 的 `SandboxAskCallback` 与 `registerCommandNetworkLists(commandId, ...)`；
   - 当沙箱内命令访问未知外网域名时，自动转为 Lila 现有 `PendingRequest` 审批卡，支持「允许本次 / 本会话允许该域名 / 项目永久允许 / 拒绝」并复用规则管理面板。
2. **项目级沙箱例外路径**
   - 允许按项目配置额外的可写路径（如特殊构建工具/语言缓存目录），持久化在 `state.json` 并在设置/项目配置中可管理。
3. **更友好的沙箱拦截反馈**
   - 在工具调用卡片中高亮识别 Seatbelt/bwrap 拦截提示，给出更易懂的中文原因与一键调整建议（如加例外路径或临时切换沙箱档位）。
