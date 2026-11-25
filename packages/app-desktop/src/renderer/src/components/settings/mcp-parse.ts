import {
  McpImportEntry,
  McpServerInput,
  McpServerName,
  type McpExposure,
  type McpScope,
  type McpServerItem,
} from "@lila/protocol";

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

export type McpFormErrors = Partial<Record<"name" | "command" | "url" | "envText" | "headersText", string>>;

export function parseMcpForm(form: McpFormState): { entry: McpImportEntry } | { errors: McpFormErrors } {
  const errors: McpFormErrors = {};
  const name = McpServerName.safeParse(form.name);
  if (!name.success) errors.name = name.error.issues[0]?.message ?? "服务器名称无效";

  const description = form.description.trim() || undefined;
  const shared = { description, exposure: form.exposure, enabled: form.enabled };
  let input: McpServerInput;
  if (form.transport === "http") {
    const headers = parseKeyValues(form.headersText, ":");
    if (headers.invalidLines.length) errors.headersText = `第 ${headers.invalidLines.join("、")} 行需要使用「名称: 值」格式`;
    input = { ...shared, transport: "http", url: form.url, headers: headers.data };
  } else {
    const env = parseKeyValues(form.envText, "=");
    if (env.invalidLines.length) errors.envText = `第 ${env.invalidLines.join("、")} 行需要使用「名称=值」格式`;
    input = { ...shared, transport: "stdio", command: form.command, args: parseArgs(form.argsText), cwd: form.cwd.trim() || undefined, env: env.data };
  }

  const config = McpServerInput.safeParse(input);
  if (!config.success) errors[input.transport === "http" ? "url" : "command"] = config.error.issues[0]?.message ?? "连接配置无效";
  if (!name.success || !config.success || Object.keys(errors).length) return { errors };
  return { entry: { name: name.data, config: config.data } };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function parseMcpJsonImport(raw: string): McpImportEntry[] {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error("JSON 格式有误，请检查引号、逗号和括号");
  }
  if (!isRecord(parsed)) throw new Error("请输入包含服务器配置的 JSON 对象");
  const servers = Object.hasOwn(parsed, "mcpServers") ? parsed.mcpServers : parsed;
  if (!isRecord(servers)) throw new Error("mcpServers 必须是一个对象");

  const imports: McpImportEntry[] = [];
  const issues: string[] = [];
  const names = new Set<string>();
  for (const [rawName, entry] of Object.entries(servers)) {
    const name = rawName.trim();
    const label = name.length > 48 ? `${name.slice(0, 48)}…` : name || "未命名服务器";
    if (!isRecord(entry)) {
      issues.push(`「${label}」的配置必须是一个对象`);
      continue;
    }

    const url = [entry.url, entry.serverUrl].find((value): value is string => typeof value === "string" && Boolean(value.trim()));
    const command = typeof entry.command === "string" ? entry.command.trim() : "";
    const enabled = typeof entry.disabled === "boolean" ? !entry.disabled : typeof entry.enabled === "boolean" ? entry.enabled : true;
    const description = typeof entry.description === "string" ? entry.description : undefined;
    const exposure = entry.exposure === "direct" || entry.exposure === "deferred" || entry.exposure === "hidden" ? entry.exposure : "codemode";
    const shared = { enabled, description, exposure };
    const config = url ? {
      ...shared,
      transport: "http",
      url,
      headers: isRecord(entry.headers) ? Object.fromEntries(Object.entries(entry.headers).map(([key, value]) => [key, String(value)])) : undefined,
    } : {
      ...shared,
      transport: "stdio",
      command,
      args: Array.isArray(entry.args) ? entry.args.map(String) : undefined,
      env: isRecord(entry.env) ? Object.fromEntries(Object.entries(entry.env).map(([key, value]) => [key, String(value)])) : undefined,
      cwd: typeof entry.cwd === "string" ? entry.cwd.trim() || undefined : undefined,
    };
    const result = McpImportEntry.safeParse({ name, config });
    if (!result.success) {
      for (const issue of result.error.issues) issues.push(`「${label}」${issue.message}`);
      continue;
    }
    if (names.has(name)) {
      issues.push(`导入内容包含同名服务器「${label}」`);
      continue;
    }
    names.add(name);
    imports.push(result.data);
  }

  if (issues.length) throw new Error(issues.join("\n"));
  if (!imports.length) throw new Error("未找到可导入的 MCP 服务器");
  return imports;
}
