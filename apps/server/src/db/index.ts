// The only file that touches Bun's SQLite. Everything else goes through `Db`,
// so moving the server to Node means swapping the driver here.
import { Database } from 'bun:sqlite'
import { mkdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { drizzle } from 'drizzle-orm/bun-sqlite'
import { migrate } from 'drizzle-orm/bun-sqlite/migrator'
import * as schema from './schema.ts'

export type Db = ReturnType<typeof drizzle<typeof schema>>

const MIGRATIONS = join(import.meta.dir, '../../drizzle')

/** Opens the database at `path` (`:memory:` for tests) and brings it up to date. */
export function openDb(path: string): Db {
  if (path !== ':memory:') mkdirSync(dirname(path), { recursive: true })
  const sqlite = new Database(path)
  sqlite.exec('PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;')
  const db = drizzle(sqlite, { schema })
  migrate(db, { migrationsFolder: MIGRATIONS })
  return db
}

export { schema }
