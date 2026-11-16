import { homedir } from "node:os";
import { join } from "node:path";
import { DEFAULT_CORE_PORT } from "@lila/protocol";
import { startCore } from "./server.ts";

const port = Number(process.env.LILA_CORE_PORT ?? DEFAULT_CORE_PORT);
const dataDir = process.env.LILA_DATA_DIR ?? join(homedir(), "Library", "Application Support", "Lila");
const databasePath = process.env.LILA_DATABASE_PATH ?? join(dataDir, "lila.sqlite");
const projectPath = process.env.LILA_PROJECT_PATH ?? process.cwd();
const server = startCore({ port, databasePath, projectPath });
console.log(`[core] listening on ${server.url}`);
