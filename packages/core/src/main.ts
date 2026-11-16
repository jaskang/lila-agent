import { DEFAULT_CORE_PORT } from "@lila/protocol";
import { startCore } from "./server.ts";

const port = Number(process.env.LILA_CORE_PORT ?? DEFAULT_CORE_PORT);
const server = startCore({ port });
console.log(`[core] listening on ${server.url}`);
