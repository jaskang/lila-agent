import { describe, expect, test } from 'bun:test'
import { upgradeWebSocket } from '@hono/bun'
import { healthSchema, PROTOCOL, projectSchema } from '@lila/shared'
import { createApp } from '../src/app.ts'
import { openDb } from '../src/db/index.ts'
import { Hub } from '../src/hub.ts'

function setup() {
  const hub = new Hub()
  const app = createApp({ db: openDb(':memory:'), version: '0.0.0-test', upgradeWebSocket, hub })
  const post = (body: unknown) =>
    app.request('/api/projects', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    })
  return { app, hub, post }
}

describe('health', () => {
  test('reports version and protocol', async () => {
    const { app } = setup()
    const health = healthSchema.parse(await (await app.request('/api/health')).json())
    expect(health.protocol).toBe(PROTOCOL)
    expect(health.version).toBe('0.0.0-test')
  })
})

describe('projects', () => {
  test('adds, lists and removes a project, announcing each change', async () => {
    const { app, hub, post } = setup()
    const events: unknown[] = []
    hub.subscribe((event) => events.push(event))

    const created = await post({ path: '/tmp/shop' })
    expect(created.status).toBe(201)
    const project = projectSchema.parse(await created.json())
    expect(project.name).toBe('shop')

    const listed = projectSchema.array().parse(await (await app.request('/api/projects')).json())
    expect(listed).toEqual([project])

    const removed = await app.request(`/api/projects/${project.id}`, { method: 'DELETE' })
    expect(removed.status).toBe(200)
    expect(await (await app.request('/api/projects')).json()).toEqual([])

    expect(events).toEqual([
      { event: 'changed', topic: 'projects', seq: 1 },
      { event: 'changed', topic: 'projects', seq: 2 },
    ])
  })

  test('adding the same folder twice returns the existing project', async () => {
    const { hub, post } = setup()
    const first = projectSchema.parse(await (await post({ path: '/tmp/shop' })).json())
    const again = await post({ path: '/tmp/shop', name: 'other' })
    expect(again.status).toBe(200)
    expect(projectSchema.parse(await again.json())).toEqual(first)
    expect(hub.seq).toBe(1)
  })

  test('rejects a relative path and a missing one', async () => {
    const { post } = setup()
    expect((await post({ path: 'shop' })).status).toBe(400)
    expect((await post({})).status).toBe(400)
  })

  test('removing an unknown project is a 404', async () => {
    const { app } = setup()
    expect((await app.request('/api/projects/nope', { method: 'DELETE' })).status).toBe(404)
  })
})
