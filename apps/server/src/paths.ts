import { homedir } from 'node:os'
import { join } from 'node:path'

/** Where Lila keeps its state. `LILA_HOME` gives an isolated copy for tests and real runs. */
export function lilaHome(env: Record<string, string | undefined> = process.env): string {
  return env.LILA_HOME?.trim() || join(homedir(), '.lila')
}

export function databasePath(home: string): string {
  return join(home, 'lila.db')
}
