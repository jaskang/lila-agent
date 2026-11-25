
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
- 优先考虑社区现成的非常成熟方案, 避免自行实现一套复杂的库或组件
- 界面优先用 shadcn/ui（Base UI 版本）的现成组件，包括它的 AI 组件，不要自己重写。

## 提交和推送

- 提交信息遵循 Conventional Commits。
- 有 AI 参与时，在提交信息末尾空一行，为每个实际参与的模型各加一条 `Co-authored-by: 模型全名 <邮箱>`。按模型厂商选择已验证能关联到 GitHub 账号的邮箱，以显示厂商头像。通过 Antigravity 等工具调用模型时，也按实际模型选择邮箱。
  - OpenAI GPT · `noreply@openai.com`（[@codex](https://github.com/codex)）
  - Anthropic Claude · `noreply@anthropic.com`（[@claude](https://github.com/claude)）
  - Google Gemini · `gemini-code-assist@google.com`（[@gemini-code-assist](https://github.com/gemini-code-assist)）
  - 其他厂商先用 GitHub 上的已有提交核对邮箱与账号。查不到可验证的邮箱时，用 `Assisted-by: 模型全名` 保留署名，避免用猜测的邮箱把贡献归到他人名下。

## 测试

- 保持克制，禁止编写一些与业务功能无关的测试。

