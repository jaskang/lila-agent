import { basename, isAbsolute } from 'node:path'
import { zValidator } from '@hono/zod-validator'
import { type Health, newProjectSchema, PROTOCOL, type Project, type ServerEvent } from '@lila/shared'
import { desc, eq } from 'drizzle-orm'
import { Hono } from 'hono'
import type { UpgradeWebSocket } from 'hono/ws'
import type { Db } from './db/index.ts'
import { projects } from './db/schema.ts'
import { Hub } from './hub.ts'

export interface AppDeps {
  db: Db
  version: string
  /** Comes from the runtime's adapter (`@hono/bun`), which keeps this file runtime-neutral. */
  upgradeWebSocket: UpgradeWebSocket
  hub?: Hub
}

export function createApp({ db, version, upgradeWebSocket, hub = new Hub() }: AppDeps) {
  return new Hono()
    .basePath('/api')
    .get('/health', (c) => {
      const health: Health = { ok: true, version, protocol: PROTOCOL, pid: process.pid }
      return c.json(health)
    })
    .get('/projects', (c) => {
      const rows: Project[] = db.select().from(projects).orderBy(desc(projects.createdAt)).all()
      return c.json(rows)
    })
    .post('/projects', zValidator('json', newProjectSchema), (c) => {
      const input = c.req.valid('json')
      if (!isAbsolute(input.path)) return c.json({ error: 'path must be absolute' }, 400)
      const existing = db.select().from(projects).where(eq(projects.path, input.path)).get()
      if (existing) return c.json(existing satisfies Project, 200)
      const project: Project = {
        id: `p${crypto.randomUUID().replaceAll('-', '').slice(0, 8)}`,
        name: input.name ?? basename(input.path),
        path: input.path,
        createdAt: Date.now(),
      }
      db.insert(projects).values(project).run()
      hub.changed('projects')
      return c.json(project, 201)
    })
    .delete('/projects/:id', (c) => {
      const removed = db.delete(projects).where(eq(projects.id, c.req.param('id'))).returning().all()
      if (removed.length === 0) return c.json({ error: 'no such project' }, 404)
      hub.changed('projects')
      return c.json({ removed: true })
    })
    .get(
      '/events',
      upgradeWebSocket(() => {
        let unsubscribe = () => {}
        return {
          onOpen(_event, ws) {
            const send = (event: ServerEvent) => ws.send(JSON.stringify(event))
            send({ event: 'hello', protocol: PROTOCOL, seq: hub.seq })
            unsubscribe = hub.subscribe(send)
          },
          onClose() {
            unsubscribe()
          },
        }
      }),
    )
}

export type AppType = ReturnType<typeof createApp>
