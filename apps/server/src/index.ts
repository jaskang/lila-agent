import { upgradeWebSocket, websocket } from '@hono/bun'
import { DEFAULT_PORT } from '@lila/shared'
import pkg from '../package.json' with { type: 'json' }
import { createApp } from './app.ts'
import { openDb } from './db/index.ts'
import { databasePath, lilaHome } from './paths.ts'

const home = lilaHome()
const port = Number(process.env.LILA_PORT) || DEFAULT_PORT
const app = createApp({ db: openDb(databasePath(home)), version: pkg.version, upgradeWebSocket })

// Loopback only: there is no authentication yet.
const server = Bun.serve({ hostname: '127.0.0.1', port, fetch: app.fetch, websocket })

console.log(`lilad ${pkg.version} listening on ${server.url} (home ${home})`)
