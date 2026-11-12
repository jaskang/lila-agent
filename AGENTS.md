## 对话规则

- Think and respond in Chinese.

## 编码规则

- 优先考虑社区现成的非常成熟方案, 避免自行实现一套复杂的库或组件

## 提交和推送

- 提交信息遵循 Conventional Commits
## 项目

- 结构和命令见 `README.md`。提交前跑 `bun run check`。
- `apps/web/src/components/ui/` 是 shadcn 生成的，用 `bunx shadcn@latest add <组件>` 添加，不手改。
- Bun 独有的接口只放在 `apps/server/src/db/index.ts` 和 `apps/server/src/index.ts`。
