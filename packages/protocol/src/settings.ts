import { z } from "zod";

export const ThinkingLevel = z.enum(["off", "minimal", "low", "medium", "high", "xhigh", "max"]);
export type ThinkingLevel = z.infer<typeof ThinkingLevel>;

export type ModelInfo = {
  /** `provider/modelId`, as Pi writes it. */
  id: string;
  provider: string;
  providerName: string;
  label: string;
  enabled: boolean;
  reasoning: boolean;
  thinkingLevels: ThinkingLevel[];
  defaultThinkingLevel: ThinkingLevel;
  images: boolean;
  contextWindow?: number;
};

export const AuthMethod = z.enum(["oauth", "api_key"]);
export type AuthMethod = z.infer<typeof AuthMethod>;

export type ProviderAuthInfo = {
  id: string;
  name: string;
  configured: boolean;
  source?: "stored" | "runtime" | "environment" | "fallback" | "models_json_key" | "models_json_command";
  sourceLabel?: string;
  storedType?: AuthMethod;
  supportsOAuth: boolean;
  oauthLabel?: string;
  supportsApiKey: boolean;
  apiKeyLabel?: string;
  modelCount: number;
  availableModelCount: number;
};

export type AuthLoginPrompt =
  | { promptId: string; type: "text" | "secret" | "manual_code"; message: string; placeholder?: string }
  | { promptId: string; type: "select"; message: string; options: Array<{ id: string; label: string; description?: string }> };

export type AuthLoginEvent =
  | { type: "auth_url"; url: string; instructions?: string }
  | { type: "device_code"; userCode: string; verificationUri: string }
  | { type: "info"; message: string; links?: Array<{ url: string; label?: string }> }
  | { type: "progress"; message: string }
  | { type: "prompt"; prompt: AuthLoginPrompt }
  | { type: "prompt_cleared"; promptId: string }
  | { type: "done" }
  | { type: "error"; message: string };

export type CoreSettings = {
  defaultModelId?: string;
  defaultThinkingLevel: ThinkingLevel;
  autoCompaction: boolean;
  autoRetry: boolean;
  paths: {
    agentDir: string;
    settingsJson: string;
    modelsJson: string;
    mcpJson: string;
    dataDir: string;
    coreLog: string;
  };
  coreVersion: string;
  protocolVersion: number;
  pid: number;
};

export const McpScope = z.enum(["global", "project"]);
export type McpScope = z.infer<typeof McpScope>;

export const McpExposure = z.enum(["codemode", "deferred", "direct", "hidden"]);
export type McpExposure = z.infer<typeof McpExposure>;

export const McpServerName = z.string().trim()
  .min(1, { error: "请输入服务器名称" })
  .max(120, { error: "服务器名称最多 120 个字符" });

export const McpServerInput = z.discriminatedUnion("transport", [
  z.object({
    transport: z.literal("stdio"),
    command: z.string().trim().min(1, { error: "请输入命令" }),
    args: z.array(z.string()).optional(),
    env: z.record(z.string(), z.string()).optional(),
    cwd: z.string().trim().min(1).optional(),
    description: z.string().trim().optional(),
    exposure: McpExposure.optional(),
    enabled: z.boolean().optional(),
  }),
  z.object({
    transport: z.literal("http"),
    url: z.string().trim().pipe(z.url({ protocol: /^https?$/, error: "请输入有效的 HTTP 或 HTTPS 地址" })),
    headers: z.record(z.string(), z.string()).optional(),
    description: z.string().trim().optional(),
    exposure: McpExposure.optional(),
    enabled: z.boolean().optional(),
  }),
]);
export type McpServerInput = z.infer<typeof McpServerInput>;

export const McpImportEntry = z.object({ name: McpServerName, config: McpServerInput });
export type McpImportEntry = z.infer<typeof McpImportEntry>;

export type McpServerItem = {
  name: string;
  scope: McpScope;
  source: string;
  transport: "stdio" | "http";
  enabled: boolean;
  exposure: McpExposure;
  description?: string;
  command?: string;
  args?: string[];
  env?: Record<string, string>;
  cwd?: string;
  url?: string;
  headers?: Record<string, string>;
};

export type McpConfigSnapshot = {
  globalPath: string;
  projectPath?: string;
  servers: McpServerItem[];
  errors: string[];
};

export const PackageScope = z.enum(["user", "project"]);
export type PackageScope = z.infer<typeof PackageScope>;

export type ConfiguredPackageInfo = {
  source: string;
  scope: PackageScope;
  installedPath?: string;
};

export type CapabilitiesSnapshot = {
  agentDir: string;
  cwd?: string;
  packages: ConfiguredPackageInfo[];
  skills: Array<{ name: string; description: string; path: string; scope: string }>;
  prompts: Array<{ name: string; description: string; path: string }>;
  extensions: Array<{ path: string; tools: string[]; commands: string[] }>;
  contextFiles: string[];
  diagnostics: string[];
};

export const MemoryTarget = z.enum(["memory", "user"]);
export type MemoryTarget = z.infer<typeof MemoryTarget>;

export type MemoryStoreSnapshot = {
  target: MemoryTarget;
  filePath: string;
  entries: string[];
  charCount: number;
  charLimit: number;
  percent: number;
};

export type MemoryDreamingStatus = {
  enabled: boolean;
  minHoursSinceLastRun: number;
  minTurnsSinceLastRun: number;
  /** `provider/modelId` when explicitly configured; omitted means follow active session model (or default). */
  modelId?: string;
  lastConsolidatedAt?: string;
  lastSessionCount: number;
};

export type MemorySnapshot = {
  enabled: boolean;
  /** True when the user installed a third-party memory extension via Pi packages (Lila's built-in defers to it). */
  externalExtension?: string;
  dir: string;
  memory: MemoryStoreSnapshot;
  user: MemoryStoreSnapshot;
  dreaming: MemoryDreamingStatus;
};

export const UsageRange = z.enum(["today", "7d", "30d", "all"]);
export type UsageRange = z.infer<typeof UsageRange>;

export type UsageTotals = {
  requests: number;
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
  totalTokens: number;
  cost: number;
};

export type UsageBucket = UsageTotals & {
  key: string;
  label: string;
  provider?: string;
};

export type DailyUsagePoint = UsageTotals & {
  date: string;
};

export type UsageSummary = {
  range: UsageRange;
  configuredProviders: string[];
  totals: UsageTotals;
  byProvider: UsageBucket[];
  byModel: UsageBucket[];
  byDay: DailyUsagePoint[];
  scannedFiles: number;
};

export type QuotaWindow = {
  label: string;
  usedPercent?: number;
  remainingText?: string;
  resetsAt?: string;
  status?: "ok" | "warning" | "limited";
};

export type QuotaBalance = {
  currency: string;
  total: string;
  details?: string;
};

export type ProviderQuotaItem = {
  providerId: string;
  providerName: string;
  source: "api" | "headers" | "unsupported";
  status: "ok" | "waiting" | "error" | "unsupported";
  plan?: string;
  balances?: QuotaBalance[];
  windows?: QuotaWindow[];
  updatedAt?: string;
  error?: string;
  billingUrl?: string;
};
