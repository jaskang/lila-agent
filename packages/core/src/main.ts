import { randomBytes } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, unlinkSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { DEFAULT_CORE_PORT } from "@lila/protocol";
import { startCore } from "./server.ts";

function defaultDataDir(): string {
  if (process.platform === "darwin") return join(homedir(), "Library", "Application Support", "Lila");
  if (process.platform === "win32") return join(process.env.APPDATA || join(homedir(), "AppData", "Roaming"), "Lila");
  return join(process.env.XDG_DATA_HOME || join(homedir(), ".local", "share"), "lila");
}

const port = Number(process.env.LILA_CORE_PORT ?? DEFAULT_CORE_PORT);
const hostname = process.env.LILA_CORE_HOST || "127.0.0.1";
const dataDir = process.env.LILA_DATA_DIR || defaultDataDir();
const tokenPath = join(dataDir, "core-token");
const pidPath = join(dataDir, "core.pid");
const replaceExisting = process.env.LILA_CORE_REPLACE === "1";

async function readHealth(url: string): Promise<{ app?: string; pid?: number } | undefined> {
  try {
    const response = await fetch(url, { signal: AbortSignal.timeout(500) });
    return response.ok ? ((await response.json()) as { app?: string; pid?: number }) : undefined;
  } catch {
    return undefined;
  }
}

async function waitUntilGone(url: string, timeoutMs: number): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (!(await readHealth(url))) return true;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  return false;
}

/** In dev mode, replace any stale detached core holding the port so `bun run dev` always runs latest code. */
async function stopExistingCoreIfNeeded(): Promise<void> {
  if (!replaceExisting) return;
  const urlHost = hostname.includes(":") && !hostname.startsWith("[") ? `[${hostname}]` : hostname;
  const healthUrl = `http://${urlHost}:${port}/health`;
  const current = await readHealth(healthUrl);
  if (!current || current.app !== "lila-core" || !current.pid || current.pid === process.pid) return;
  console.log(`[core] replacing existing core (pid ${current.pid}) on port ${port}`);
  try {
    process.kill(current.pid, "SIGTERM");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ESRCH") return;
    throw error;
  }
  if (await waitUntilGone(healthUrl, 3_000)) return;
  try {
    process.kill(current.pid, "SIGKILL");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ESRCH") return;
    throw error;
  }
  await waitUntilGone(healthUrl, 1_500);
}

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

await stopExistingCoreIfNeeded();
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
