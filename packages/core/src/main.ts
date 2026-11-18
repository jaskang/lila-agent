import { chmodSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { DEFAULT_CORE_PORT } from "@lila/protocol";
import { startCore } from "./server.ts";

const port = Number(process.env.LILA_CORE_PORT ?? DEFAULT_CORE_PORT);
const dataDir = process.env.LILA_DATA_DIR ?? join(homedir(), "Library", "Application Support", "Lila");
const databasePath = join(dataDir, "lila.sqlite");
const tokenPath = join(dataDir, "core-token");

/** Shared secret for local clients; readable only by the current user. */
function loadToken(): string {
  mkdirSync(dataDir, { recursive: true, mode: 0o700 });
  if (existsSync(tokenPath)) {
    const token = readFileSync(tokenPath, "utf8").trim();
    if (token) return token;
  }
  const token = Buffer.from(crypto.getRandomValues(new Uint8Array(32))).toString("base64url");
  writeFileSync(tokenPath, token, { mode: 0o600 });
  chmodSync(tokenPath, 0o600);
  return token;
}

const server = await startCore({ port, databasePath, token: loadToken() });
console.log(`[core] listening on ${server.url} (data ${dataDir})`);

// Watch resident memory so a slow leak in a long-lived Bun process shows up in the log.
setInterval(() => {
  const { rss, heapUsed } = process.memoryUsage();
  console.log(`[core] memory rss=${Math.round(rss / 1048576)}MB heap=${Math.round(heapUsed / 1048576)}MB`);
}, 10 * 60_000).unref();

for (const signal of ["SIGINT", "SIGTERM"] as const) {
  process.on(signal, () => {
    void server.stop(true).finally(() => process.exit(0));
  });
}
