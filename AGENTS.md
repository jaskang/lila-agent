## 对话规则

- Think and respond in Chinese.

## 编码规则

- 优先考虑社区现成的非常成熟方案, 避免自行实现一套复杂的库或组件

## 提交和推送

- 提交信息遵循 Conventional Commits；AI 协作者署名按实际运行模型填写。

## 项目

- 开发前先读 `docs/architecture.md` 和 `docs/plan.md`（从「下一个任务」开始）。
- 界面优先用 shadcn/ui（Base UI 版本）的现成组件，包括它的 AI 组件，不要自己重写。

## Agent skills

### Issue tracker

Issues and specs for this repo live as Markdown files under `.scratch/<feature>/`. See `docs/agents/issue-tracker.md`.

### Triage labels

Use the default labels `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, and `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

This is a single-context repository. Read `GLOSSARY.md` and relevant ADRs under `docs/adr/` when they exist. See `docs/agents/domain.md`.
