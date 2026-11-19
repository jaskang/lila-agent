import { randomBytes } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, unlinkSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { DEFAULT_CORE_PORT } from "@lila/protocol";
import { startCore } from "./server.ts";

const port = Number(process.env.LILA_CORE_PORT ?? DEFAULT_CORE_PORT);
const hostname = process.env.LILA_CORE_HOST || "127.0.0.1";
const dataDir = process.env.LILA_DATA_DIR ?? join(homedir(), "Library", "Application Support", "Lila");
const tokenPath = join(dataDir, "core-token");
const pidPath = join(dataDir, "core.pid");

/** Shared secret for local clients; readable only by the current user. */
function loadToken(): string {
  mkdirSync(dataDir, { recursive: true, mode: 0o700 });
  if (existsSync(tokenPath)) {
    const token = readFileSync(tokenPath, "utf8").trim();
    if (token) return token;
  }
  const token = randomBytes(32).toString("base64url");
  writeFileSync(tokenPath, token, { mode: 0o600 });
  chmodSync(tokenPath, 0o600);
  return token;
}

function writePid(): void {
  writeFileSync(pidPath, `${process.pid}\n`, { mode: 0o600 });
}

function clearPid(): void {
  try {
    if (existsSync(pidPath) && readFileSync(pidPath, "utf8").trim() === String(process.pid)) unlinkSync(pidPath);
  } catch {
    // Best-effort cleanup on exit.
  }
}

const core = await startCore({ port, hostname, dataDir, token: loadToken() });
writePid();
console.log(`[core] listening on ${core.url} (data ${dataDir}, pid ${process.pid})`);

// Watch resident memory so a slow leak in this long-lived process shows up in the log.
setInterval(() => {
  const { rss, heapUsed } = process.memoryUsage();
  console.log(`[core] memory rss=${Math.round(rss / 1048576)}MB heap=${Math.round(heapUsed / 1048576)}MB`);
}, 10 * 60_000).unref();

let stopping = false;
for (const signal of ["SIGINT", "SIGTERM"] as const) {
  process.on(signal, () => {
    if (stopping) return;
    stopping = true;
    core
      .stop()
      .catch((error: unknown) => console.error("[core] shutdown failed", error))
      .finally(() => {
        clearPid();
        process.exit(0);
      });
  });
}
