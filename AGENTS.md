## 对话规则

- Think and respond in Chinese.

## 编码规则

- 优先考虑社区现成的非常成熟方案, 避免自行实现一套复杂的库或组件

## 提交和推送

- 提交信息遵循 Conventional Commits

## 项目

- 开发前先读 `docs/plan.md`（任务单，从「下一个任务」开始）和 `docs/architecture.md`；验证界面和用真 omp 跑的方法在 `docs/development.md`。
- 提交前跑 `cargo build --workspace`、`cargo test --workspace`、`cargo clippy --workspace --all-targets`。
- 提交和推送都等用户说。
