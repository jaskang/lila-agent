# Lila

个人助理：agent 由官方引擎来跑，Lila 提供界面、工具和监督（要人批、可查、可退）。

这是 TypeScript 重写的分支，目前只有骨架。之前的 Rust 实现在 `main` 分支。

## 结构

| 位置 | 内容 |
|---|---|
| `apps/server` | lilad：常驻的服务端。Bun、Hono、SQLite（Drizzle）。HTTP 在 `/api`，推送在 `/api/events`（WebSocket） |
| `apps/web` | 界面：React、Vite、Tailwind、shadcn/ui、TanStack Query |
| `packages/shared` | 两边共用的数据格式（zod）和协议版本号 |

约定：

- 推送只说「什么变了」，数据一律用 HTTP 取。
- 界面的接口类型直接来自服务端的路由（Hono RPC），改了路由界面会报类型错误。
- 只有 `apps/server/src/db/index.ts` 和 `src/index.ts` 用 Bun 独有的接口，其余写法 Node 也能跑。
- 服务端只监听 `127.0.0.1`，还没有登录。

## 命令

```sh
bun install
bun run dev        # 服务端 :4780 和界面 :5173 一起起，打开 http://localhost:5173
bun run check      # lint、类型检查、测试
```

- `LILA_HOME=/某个文件夹`：用独立的数据库和状态（默认 `~/.lila`）。
- `LILA_PORT=端口`：服务端的端口，界面的代理跟着走。
- 改了 `apps/server/src/db/schema.ts` 之后：`bun run --filter '@lila/server' db:generate`，生成的迁移文件要提交。

## 还没有的

- 会话、引擎、权限：都还没做。
- 桌面壳（Electron）：先在浏览器里用。
- 服务端编译成单个可执行文件：迁移文件要先打进程序里，现在没做。
