import type { McpExposure, McpScope, McpServerInput, McpServerItem } from "@lila/protocol";

export const EXPOSURE_OPTIONS: Array<{ value: McpExposure; label: string }> = [
  { value: "codemode", label: "脚本模式" },
  { value: "deferred", label: "按需搜索" },
  { value: "direct", label: "直接暴露" },
  { value: "hidden", label: "隐藏" },
];

export type McpFormState = {
  editingKey?: string;
  scope: McpScope;
  name: string;
  transport: "stdio" | "http";
  command: string;
  argsText: string;
  cwd: string;
  envText: string;
  url: string;
  headersText: string;
  description: string;
  exposure: McpExposure;
  enabled: boolean;
};

export function emptyMcpForm(defaultScope: McpScope): McpFormState {
  return {
    scope: defaultScope,
    name: "",
    transport: "stdio",
    command: "",
    argsText: "",
    cwd: "",
    envText: "",
    url: "",
    headersText: "",
    description: "",
    exposure: "codemode",
    enabled: true,
  };
}

export function mcpFormFromItem(item: McpServerItem): McpFormState {
  return {
    editingKey: `${item.scope}:${item.name}`,
    scope: item.scope,
    name: item.name,
    transport: item.transport,
    command: item.command ?? "",
    argsText: (item.args ?? []).join("\n"),
    cwd: item.cwd ?? "",
    envText: item.env
      ? Object.entries(item.env)
          .map(([k, v]) => `${k}=${v}`)
          .join("\n")
      : "",
    url: item.url ?? "",
    headersText: item.headers
      ? Object.entries(item.headers)
          .map(([k, v]) => `${k}: ${v}`)
          .join("\n")
      : "",
    description: item.description ?? "",
    exposure: item.exposure,
    enabled: item.enabled,
  };
}

export function parseArgs(raw: string): string[] {
  const trimmed = raw.trim();
  if (!trimmed) return [];
  if (trimmed.includes("\n")) {
    return trimmed
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean);
  }
  const tokens: string[] = [];
  const regex = /"([^"\\]*(?:\\.[^"\\]*)*)"|'([^']*)'|(\S+)/g;
  for (const match of trimmed.matchAll(regex)) {
    const token = match[1]?.replaceAll('\\"', '"') ?? match[2] ?? match[3];
    if (token !== undefined) tokens.push(token);
  }
  return tokens;
}

export function parseKeyValues(raw: string, separator: "=" | ":"): { data?: Record<string, string>; invalidLines: number[] } {
  const result: Record<string, string> = {};
  const invalidLines: number[] = [];
  const lines = raw.split(/\r?\n/);
  for (let i = 0; i < lines.length; i++) {
    const trimmed = lines[i]!.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const index = trimmed.indexOf(separator);
    if (index <= 0) {
      invalidLines.push(i + 1);
      continue;
    }
    const key = trimmed.slice(0, index).trim();
    const value = trimmed.slice(index + 1).trim();
    if (!key) {
      invalidLines.push(i + 1);
      continue;
    }
    result[key] = value;
  }
  return { data: Object.keys(result).length ? result : undefined, invalidLines };
}

export function parseMcpJsonImport(raw: string): Array<{ name: string; input: McpServerInput }> {
  const parsed = JSON.parse(raw) as unknown;
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error("请输入合法的 JSON 对象");
  }
  const root = parsed as Record<string, unknown>;
  const serversObj =
    root.mcpServers && typeof root.mcpServers === "object" && !Array.isArray(root.mcpServers)
      ? (root.mcpServers as Record<string, unknown>)
      : root;

  const items: Array<{ name: string; input: McpServerInput }> = [];
  for (const [name, entry] of Object.entries(serversObj)) {
    if (!name.trim() || !entry || typeof entry !== "object" || Array.isArray(entry)) continue;
    const rec = entry as Record<string, unknown>;
    const url =
      typeof rec.url === "string" && rec.url.trim()
        ? rec.url.trim()
        : typeof rec.serverUrl === "string" && rec.serverUrl.trim()
          ? rec.serverUrl.trim()
          : undefined;
    const command = typeof rec.command === "string" && rec.command.trim() ? rec.command.trim() : undefined;
    const enabled = typeof rec.disabled === "boolean" ? !rec.disabled : typeof rec.enabled === "boolean" ? rec.enabled : true;
    const description = typeof rec.description === "string" && rec.description.trim() ? rec.description.trim() : undefined;
    if (url) {
      const headers =
        rec.headers && typeof rec.headers === "object" && !Array.isArray(rec.headers)
          ? Object.fromEntries(Object.entries(rec.headers as Record<string, unknown>).map(([k, v]) => [k, String(v)]))
          : undefined;
      items.push({
        name: name.trim(),
        input: {
          transport: "http",
          url,
          ...(headers && Object.keys(headers).length ? { headers } : {}),
          ...(description ? { description } : {}),
          enabled,
          exposure: "codemode",
        },
      });
    } else if (command) {
      const args = Array.isArray(rec.args) ? rec.args.map((v) => String(v)) : undefined;
      const env =
        rec.env && typeof rec.env === "object" && !Array.isArray(rec.env)
          ? Object.fromEntries(Object.entries(rec.env as Record<string, unknown>).map(([k, v]) => [k, String(v)]))
          : undefined;
      const cwd = typeof rec.cwd === "string" && rec.cwd.trim() ? rec.cwd.trim() : undefined;
      items.push({
        name: name.trim(),
        input: {
          transport: "stdio",
          command,
          ...(args?.length ? { args } : {}),
          ...(env && Object.keys(env).length ? { env } : {}),
          ...(cwd ? { cwd } : {}),
          ...(description ? { description } : {}),
          enabled,
          exposure: "codemode",
        },
      });
    }
  }
  if (!items.length) throw new Error("未识别到包含 command 或 url 的 MCP 服务器配置");
  return items;
}
