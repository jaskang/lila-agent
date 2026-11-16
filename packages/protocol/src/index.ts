import { z } from "zod";

export const PROTOCOL_VERSION = 1;

export const DEFAULT_CORE_PORT = 7457;

export const ServerHello = z.object({
  type: z.literal("hello"),
  protocolVersion: z.literal(PROTOCOL_VERSION),
  coreVersion: z.string(),
});
export type ServerHello = z.infer<typeof ServerHello>;
