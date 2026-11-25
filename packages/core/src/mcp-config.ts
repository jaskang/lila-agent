import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import type { McpConfigSnapshot, McpExposure, McpScope, McpServerInput, McpServerItem } from "@lila/protocol";
import { UserError } from "./errors.ts";

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function stripBom(text: string): string {
  return text.charCodeAt(0) === 0xfeff ? text.slice(1) : text;
}

function normalizeExposure(raw: unknown): McpExposure {
  if (raw === "deferred" || raw === "direct" || raw === "hidden" || raw === "codemode") return raw;
  return "codemode";
}

function readStringRecord(raw: unknown): Record<string, string> | undefined {
  if (!isRecord(raw)) return undefined;
  const result: Record<string, string> = {};
  for (const [key, value] of Object.entries(raw)) {
    if (typeof value === "string") result[key] = value;
  }
  return Object.keys(result).length ? result : undefined;
}

function readStringArray(raw: unknown): string[] | undefined {
  if (!Array.isArray(raw)) return undefined;
  const list = raw.filter((item): item is string => typeof item === "string");
  return list.length ? list : undefined;
}

function parseServerEntry(name: string, raw: Record<string, unknown>, scope: McpScope, source: string): McpServerItem | undefined {
  const command = typeof raw.command === "string" && raw.command.trim() ? raw.command.trim() : undefined;
  const url = typeof raw.url === "string" && raw.url.trim() ? raw.url.trim() : undefined;
  if (!command && !url) return undefined;
  const enabled = raw.enabled === undefined ? true : Boolean(raw.enabled);
  const exposure = normalizeExposure(raw.exposure);
  const description = typeof raw.description === "string" && raw.description.trim() ? raw.description.trim() : undefined;

  if (url) {
    const headers = readStringRecord(raw.headers);
    return {
      name,
      scope,
      source,
      transport: "http",
      enabled,
      exposure,
      url,
      ...(headers ? { headers } : {}),
      ...(description ? { description } : {}),
    };
  }
  const args = readStringArray(raw.args);
  const env = readStringRecord(raw.env);
  const cwd = typeof raw.cwd === "string" && raw.cwd.trim() ? raw.cwd.trim() : undefined;
  return {
    name,
    scope,
    source,
    transport: "stdio",
    enabled,
    exposure,
    command: command!,
    ...(args ? { args } : {}),
    ...(env ? { env } : {}),
    ...(cwd ? { cwd } : {}),
    ...(description ? { description } : {}),
  };
}

function readConfigFile(path: string, scope: McpScope, servers: McpServerItem[], errors: string[]): void {
  if (!existsSync(path)) return;
  let parsed: unknown;
  try {
    parsed = JSON.parse(stripBom(readFileSync(path, "utf8")));
  } catch (error) {
    errors.push(`${path}: ${error instanceof Error ? error.message : String(error)}`);
    return;
  }
  if (!isRecord(parsed) || (parsed.mcpServers !== undefined && !isRecord(parsed.mcpServers))) {
    errors.push(`${path}: 需要包含 "mcpServers" 对象`);
    return;
  }
  const map = parsed.mcpServers ?? {};
  for (const [name, raw] of Object.entries(map)) {
    if (!isRecord(raw)) {
      errors.push(`${path}: 服务器 "${name}" 配置格式无效`);
      continue;
    }
    const item = parseServerEntry(name, raw, scope, path);
    if (!item) continue;
    servers.push(item);
  }
}

export function loadMcpSnapshot(agentDir: string, projectDir?: string): McpConfigSnapshot {
  const globalPath = join(agentDir, "mcp.json");
  const projectPath = projectDir ? join(projectDir, ".pi", "mcp.json") : undefined;
  const servers: McpServerItem[] = [];
  const errors: string[] = [];
  readConfigFile(globalPath, "global", servers, errors);
  if (projectPath) readConfigFile(projectPath, "project", servers, errors);
  return {
    globalPath,
    ...(projectPath ? { projectPath } : {}),
    servers,
    errors,
  };
}

function editMcpFile(path: string, edit: (servers: Record<string, unknown>, root: Record<string, unknown>) => boolean): void {
  const text = existsSync(path) ? stripBom(readFileSync(path, "utf8")) : undefined;
  let parsed: unknown;
  try {
    parsed = text === undefined ? {} : JSON.parse(text);
  } catch (error) {
    throw new UserError(`无法解析 ${path}：${error instanceof Error ? error.message : String(error)}`);
  }
  if (!isRecord(parsed) || (parsed.mcpServers !== undefined && !isRecord(parsed.mcpServers))) {
    throw new UserError(`${path} 格式不正确，顶层需要包含 "mcpServers" 对象`);
  }
  const servers: Record<string, unknown> = isRecord(parsed.mcpServers) ? { ...parsed.mcpServers } : {};
  if (!edit(servers, parsed)) return;
  parsed.mcpServers = servers;
  const indent = (text && /^([ \t]+)\S/m.exec(text)?.[1]) || "  ";
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, `${JSON.stringify(parsed, null, indent)}\n`);
}

function toJsonServerEntry(input: McpServerInput, existing?: Record<string, unknown>): Record<string, unknown> {
  const base: Record<string, unknown> = existing ? { ...existing } : {};
  // Clear mutually exclusive transport fields when switching transport or editing.
  delete base.command;
  delete base.args;
  delete base.env;
  delete base.cwd;
  delete base.url;
  delete base.headers;

  if (input.transport === "http") {
    let parsedUrl: URL;
    try {
      parsedUrl = new URL(input.url);
    } catch {
      throw new UserError(`无效的服务器地址：${input.url}`);
    }
    if (parsedUrl.protocol !== "http:" && parsedUrl.protocol !== "https:") {
      throw new UserError("远程 MCP 服务器地址必须以 http:// 或 https:// 开头");
    }
    base.url = input.url;
    if (input.headers && Object.keys(input.headers).length) base.headers = input.headers;
  } else {
    base.command = input.command;
    if (input.args?.length) base.args = input.args;
    if (input.env && Object.keys(input.env).length) base.env = input.env;
    if (input.cwd) base.cwd = input.cwd;
  }

  if (input.description) base.description = input.description;
  else delete base.description;

  if (input.exposure && input.exposure !== "codemode") base.exposure = input.exposure;
  else delete base.exposure;

  if (input.enabled === false) base.enabled = false;
  else delete base.enabled;

  return base;
}

export function upsertMcpServer(path: string, name: string, input: McpServerInput): void {
  const trimmed = name.trim();
  if (!trimmed) throw new UserError("服务器名称不能为空");
  editMcpFile(path, (servers) => {
    const existing = isRecord(servers[trimmed]) ? servers[trimmed] : undefined;
    servers[trimmed] = toJsonServerEntry(input, existing);
    return true;
  });
}

export function patchMcpServer(path: string, name: string, patch: { enabled?: boolean | undefined; exposure?: McpExposure | undefined }): void {
  editMcpFile(path, (servers) => {
    const current = servers[name];
    if (!isRecord(current)) throw new UserError(`在 ${path} 中找不到 MCP 服务器 "${name}"`);
    const next = { ...current };
    if (patch.enabled !== undefined) {
      if (patch.enabled) delete next.enabled;
      else next.enabled = false;
    }
    if (patch.exposure !== undefined) {
      if (patch.exposure === "codemode") delete next.exposure;
      else next.exposure = patch.exposure;
    }
    servers[name] = next;
    return true;
  });
}

export function removeMcpServer(path: string, name: string): boolean {
  if (!existsSync(path)) return false;
  let removed = false;
  editMcpFile(path, (servers) => {
    if (!(name in servers)) return false;
    delete servers[name];
    removed = true;
    return true;
  });
  return removed;
}
