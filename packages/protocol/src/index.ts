/**
 * Lila's wire protocol between core and its clients (desktop now, mobile later).
 *
 * Commands from clients are validated with Zod at the core boundary. Messages from core are plain
 * TypeScript types: core is trusted once the socket is authenticated, and streaming frames are too
 * frequent to validate one by one.
 */

export const PROTOCOL_VERSION = 19;
export const DEFAULT_CORE_PORT = 7457;
export const MAX_CORE_MESSAGE_BYTES = 64 * 1024 * 1024;

export * from "./settings.js";
export * from "./session.js";
export * from "./workspace.js";
export * from "./automation.js";
export * from "./commands.js";
