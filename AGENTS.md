
## Agent skills

### Issue tracker

Issues and specs for this repo live as Markdown files under `.scratch/<feature>/`. See `docs/agents/issue-tracker.md`.

### Triage labels

Use the default labels `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, and `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

This is a single-context repository. Read `GLOSSARY.md` and relevant ADRs under `docs/adr/` when they exist. See `docs/agents/domain.md`.

## 对话规则

- Think and respond in Chinese.

## 编码规则

- 开发前先读 `docs/architecture.md` 和 `docs/plan.md`（从「下一个任务」开始）。
- 优先考虑社区现成的非常成熟方案，避免自行实现一套复杂的库或组件。
- 桌面端基于 Rust + `gpui-kit`（`packages/app-native`），界面优先复用 `gpui-component` 现成组件，颜色与圆角统一走 `packages/app-native/themes/lila.json` 与 `ui/theme_setup.rs` 语义 Token，禁止在 UI 组件里散落硬编码 `rgb()`/`rgba()`。
- 视觉与交互规范见根目录 `DESIGN.md`。

## 提交和推送

- 提交信息遵循 Conventional Commits；AI 协作者署名按实际运行模型填写。

## 测试

- 保持克制，禁止编写一些与业务功能无关的测试。

