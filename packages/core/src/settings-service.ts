import { clampThinkingLevel, getSupportedThinkingLevels } from "@earendil-works/pi-ai/compat";
import { homedir } from "node:os";
import { join } from "node:path";
import { type AgentSessionServices, createAgentSessionServices, getAgentDir } from "@earendil-works/pi-coding-agent";
import {
  type AuthLoginEvent,
  type AuthMethod,
  type CoreSettings,
  type ModelInfo,
  PROTOCOL_VERSION,
  type ProviderAuthInfo,
  type ProviderQuotaItem,
  ThinkingLevel,
  type UsageRange,
  type UsageSummary,
} from "@lila/protocol";
import pkg from "../package.json" with { type: "json" };
import { AuthLoginManager } from "./auth-flows.ts";
import { UserError } from "./errors.ts";
import { QuotaTracker } from "./quotas.ts";
import { scanLocalUsage } from "./usage.ts";

export async function settleServices(promise: Promise<AgentSessionServices>): Promise<void> {
  try {
    const services = await promise;
    await services.modelRuntime.settleCredentialWrites();
  } catch {
    // Creation already failed or credentials could not be flushed during teardown.
  }
}

function escapeRegex(value: string): string {
  return value.replace(/[.+?^${}()|[\]\\]/g, "\\$&");
}

function matchesEnabledPattern(fullId: string, pattern: string): boolean {
  const normalized = pattern.trim().toLowerCase();
  if (!normalized) return false;
  const target = fullId.toLowerCase();
  if (!normalized.includes("*")) return target === normalized;
  const regex = new RegExp(`^${normalized.split("*").map(escapeRegex).join(".*")}$`);
  return regex.test(target);
}

function isModelEnabled(fullId: string, enabledModels: readonly string[] | undefined): boolean {
  if (!enabledModels || enabledModels.length === 0) return true;
  return enabledModels.some((pattern) => matchesEnabledPattern(fullId, pattern));
}

function belongsToProvider(pattern: string, providerId: string): boolean {
  return pattern.trim().toLowerCase().startsWith(`${providerId.toLowerCase()}/`);
}

export class SettingsService {
  readonly #dataDir: string;
  readonly #syncLiveRuntime: () => Promise<void>;
  readonly #authLogins = new AuthLoginManager();
  readonly #quotas: QuotaTracker;
  #modelServices: Promise<AgentSessionServices> | undefined;
  #lastCatalogRefreshAt = 0;

  constructor(dataDir: string, syncLiveRuntime: () => Promise<void>) {
    this.#dataDir = dataDir;
    this.#syncLiveRuntime = syncLiveRuntime;
    this.#quotas = new QuotaTracker(dataDir);
  }

  /** One services instance for the user's agentDir, so models registered by user extensions are listed too. */
  getModelServices(): { promise: Promise<AgentSessionServices>; cached: boolean } {
    const cached = this.#modelServices !== undefined;
    this.#modelServices ??= createAgentSessionServices({ cwd: homedir(), agentDir: getAgentDir() }).catch((error: unknown) => {
      this.#modelServices = undefined;
      throw error;
    });
    return { promise: this.#modelServices, cached };
  }

  invalidate(): void {
    const models = this.#modelServices;
    this.#modelServices = undefined;
    if (models) void settleServices(models);
  }

  recordResponseHeaders(providerId: string | undefined, headers: Record<string, string>): void {
    this.#quotas.recordResponseHeaders(providerId, headers);
  }

  async #refreshModels(services: AgentSessionServices, options?: { providers?: string[]; forceNetwork?: boolean }): Promise<void> {
    const now = Date.now();
    const allowNetwork =
      process.env.PI_OFFLINE === undefined && (options?.forceNetwork === true || now - this.#lastCatalogRefreshAt >= 5 * 60_000);
    if (!allowNetwork) {
      await services.modelRuntime.refresh({ allowNetwork: false, ...(options?.providers ? { providers: options.providers } : {}) });
      return;
    }
    this.#lastCatalogRefreshAt = now;
    try {
      await services.modelRuntime.refresh({
        allowNetwork: true,
        ...(options?.forceNetwork ? { force: true } : {}),
        ...(options?.providers ? { providers: options.providers } : {}),
        signal: AbortSignal.timeout(6_000),
      });
    } catch {
      await services.modelRuntime
        .refresh({ allowNetwork: false, ...(options?.providers ? { providers: options.providers } : {}) })
        .catch(() => undefined);
    }
  }

  #buildModelsSnapshot(services: AgentSessionServices): { models: ModelInfo[]; defaultModelId?: string } {
    const providerNames = new Map(services.modelRuntime.getProviders().map((provider) => [provider.id, provider.name || provider.id]));
    const enabledPatterns = services.settingsManager.getEnabledModels();
    const models = services.modelRuntime.getAvailableSnapshot().map((model): ModelInfo => {
      const fullId = `${model.provider}/${model.id}`;
      return {
        id: fullId,
        provider: model.provider,
        providerName: providerNames.get(model.provider) ?? model.provider,
        label: model.name,
        enabled: isModelEnabled(fullId, enabledPatterns),
        reasoning: model.reasoning,
        thinkingLevels: getSupportedThinkingLevels(model),
        defaultThinkingLevel: clampThinkingLevel(
          model,
          services.settingsManager.getModelThinkingLevel(model.provider, model.id) ?? services.settingsManager.getDefaultThinkingLevel() ?? "medium",
        ),
        images: model.input.includes("image"),
        contextWindow: model.contextWindow,
      };
    });
    const provider = services.settingsManager.getDefaultProvider();
    const modelId = services.settingsManager.getDefaultModel();
    return { models, ...(provider && modelId ? { defaultModelId: `${provider}/${modelId}` } : {}) };
  }

  #buildSettings(services: AgentSessionServices): CoreSettings {
    const provider = services.settingsManager.getDefaultProvider();
    const modelId = services.settingsManager.getDefaultModel();
    const rawThinking = services.settingsManager.getDefaultThinkingLevel() ?? "medium";
    const parsedThinking = ThinkingLevel.safeParse(rawThinking);
    const agentDir = getAgentDir();
    return {
      ...(provider && modelId ? { defaultModelId: `${provider}/${modelId}` } : {}),
      defaultThinkingLevel: parsedThinking.success ? parsedThinking.data : "medium",
      autoCompaction: services.settingsManager.getCompactionEnabled(),
      autoRetry: services.settingsManager.getRetryEnabled(),
      paths: {
        agentDir,
        settingsJson: join(agentDir, "settings.json"),
        modelsJson: join(agentDir, "models.json"),
        mcpJson: join(agentDir, "mcp.json"),
        dataDir: this.#dataDir,
        coreLog: join(this.#dataDir, "core.log"),
      },
      coreVersion: pkg.version,
      protocolVersion: PROTOCOL_VERSION,
      pid: process.pid,
    };
  }

  async listModels(forceRefresh = false): Promise<{ models: ModelInfo[]; defaultModelId?: string }> {
    const { promise, cached } = this.getModelServices();
    const services = await promise;
    if (cached) await services.settingsManager.reload();
    await this.#refreshModels(services, { forceNetwork: forceRefresh });
    await this.#syncLiveRuntime();
    return this.#buildModelsSnapshot(services);
  }

  async setProviderModels(providerId: string, modelIds: string[] | null): Promise<{ models: ModelInfo[]; defaultModelId?: string }> {
    const { promise } = this.getModelServices();
    const services = await promise;
    await services.settingsManager.reload();
    await services.modelRuntime.refresh({ allowNetwork: false });

    const configuredIds = services.modelRuntime
      .getProviders()
      .filter((provider) => services.modelRuntime.getProviderAuthStatus(provider.id).configured)
      .map((provider) => provider.id);

    const existing = services.settingsManager.getEnabledModels();
    let nextPatterns: string[] | undefined;

    if (!existing || existing.length === 0) {
      if (modelIds === null) {
        nextPatterns = undefined;
      } else {
        const others = configuredIds.filter((id) => id !== providerId).map((id) => `${id}/*`);
        const selected = [...new Set(modelIds.map((id) => (id.includes("/") ? id : `${providerId}/${id}`)))];
        nextPatterns = [...others, ...selected];
      }
    } else {
      const kept = existing.filter((pattern) => !belongsToProvider(pattern, providerId));
      const added =
        modelIds === null
          ? [`${providerId}/*`]
          : [...new Set(modelIds.map((id) => (id.includes("/") ? id : `${providerId}/${id}`)))];
      const combined = [...kept, ...added];
      const allWildcard =
        combined.length > 0 &&
        combined.every((pattern) => pattern.endsWith("/*")) &&
        configuredIds.every((id) => combined.some((pattern) => pattern.toLowerCase() === `${id.toLowerCase()}/*`));
      nextPatterns = allWildcard ? undefined : combined;
    }

    services.settingsManager.setEnabledModels(nextPatterns);
    await services.settingsManager.flush();
    await this.#syncLiveRuntime();
    return this.#buildModelsSnapshot(services);
  }

  async getSettings(): Promise<CoreSettings> {
    const { promise, cached } = this.getModelServices();
    const services = await promise;
    if (cached) await services.settingsManager.reload();
    return this.#buildSettings(services);
  }

  async updateSettings(patch: {
    defaultModelId?: string | undefined;
    defaultThinkingLevel?: ThinkingLevel | undefined;
    autoCompaction?: boolean | undefined;
    autoRetry?: boolean | undefined;
  }): Promise<CoreSettings> {
    const { promise } = this.getModelServices();
    const services = await promise;
    await services.settingsManager.reload();
    if (patch.defaultModelId !== undefined) {
      const slash = patch.defaultModelId.indexOf("/");
      if (slash <= 0 || slash >= patch.defaultModelId.length - 1) {
        throw new UserError(`无效的模型标识：${patch.defaultModelId}`);
      }
      services.settingsManager.setDefaultModelAndProvider(patch.defaultModelId.slice(0, slash), patch.defaultModelId.slice(slash + 1));
    }
    if (patch.defaultThinkingLevel !== undefined) services.settingsManager.setDefaultThinkingLevel(patch.defaultThinkingLevel);
    if (patch.autoCompaction !== undefined) services.settingsManager.setCompactionEnabled(patch.autoCompaction);
    if (patch.autoRetry !== undefined) services.settingsManager.setRetryEnabled(patch.autoRetry);
    await services.settingsManager.flush();
    await this.#syncLiveRuntime();
    return this.#buildSettings(services);
  }

  async listProviders(): Promise<ProviderAuthInfo[]> {
    const { promise } = this.getModelServices();
    const services = await promise;
    await this.#refreshModels(services);
    const stored = new Map((await services.modelRuntime.listCredentials()).map((entry) => [entry.providerId, entry.type]));
    const availableByProvider = new Map<string, number>();
    for (const model of services.modelRuntime.getAvailableSnapshot()) {
      availableByProvider.set(model.provider, (availableByProvider.get(model.provider) ?? 0) + 1);
    }
    return services.modelRuntime.getProviders().map((provider): ProviderAuthInfo => {
      const status = services.modelRuntime.getProviderAuthStatus(provider.id);
      const storedType = stored.get(provider.id);
      const oauthLabel = provider.auth.oauth?.name;
      const apiKeyLabel = provider.auth.apiKey?.name;
      return {
        id: provider.id,
        name: provider.name || provider.id,
        configured: status.configured,
        ...(status.source ? { source: status.source } : {}),
        ...(status.label ? { sourceLabel: status.label } : {}),
        ...(storedType ? { storedType } : {}),
        supportsOAuth: Boolean(provider.auth.oauth),
        ...(oauthLabel ? { oauthLabel } : {}),
        supportsApiKey: Boolean(provider.auth.apiKey?.login),
        ...(apiKeyLabel ? { apiKeyLabel } : {}),
        modelCount: services.modelRuntime.getModels(provider.id).length,
        availableModelCount: availableByProvider.get(provider.id) ?? 0,
      };
    });
  }

  async startProviderLogin(
    owner: unknown,
    providerId: string,
    method: AuthMethod,
    emit: (event: AuthLoginEvent) => void,
  ): Promise<string> {
    const { promise } = this.getModelServices();
    const services = await promise;
    const provider = services.modelRuntime.getProvider(providerId);
    if (!provider) throw new UserError(`未知服务商：${providerId}`);
    if (method === "oauth" && !provider.auth.oauth) throw new UserError(`${provider.name || provider.id} 不支持账号登录`);
    if (method === "api_key" && !provider.auth.apiKey?.login) throw new UserError(`${provider.name || provider.id} 不支持 API Key 登录`);

    return this.#authLogins.start(
      owner,
      providerId,
      async (interaction) => {
        await services.modelRuntime.login(providerId, method, interaction, {
          getDeviceId: () => services.settingsManager.getOrCreateDeviceId(),
        });
      },
      emit,
      async () => {
        await this.#refreshModels(services, { providers: [providerId], forceNetwork: true }).catch(() => undefined);
        await services.settingsManager.reload().catch(() => undefined);
        const currentEnabled = services.settingsManager.getEnabledModels();
        if (currentEnabled && currentEnabled.length > 0 && !currentEnabled.some((pattern) => belongsToProvider(pattern, providerId))) {
          services.settingsManager.setEnabledModels([...currentEnabled, `${providerId}/*`]);
          await services.settingsManager.flush().catch(() => undefined);
        }
        await this.#syncLiveRuntime();
      },
    );
  }

  respondProviderLogin(loginId: string, promptId: string, value: string): void {
    this.#authLogins.respond(loginId, promptId, value);
  }

  cancelProviderLogin(loginId: string): void {
    this.#authLogins.cancel(loginId);
  }

  cancelLoginsForOwner(owner: unknown): void {
    this.#authLogins.cancelByOwner(owner);
  }

  async logoutProvider(providerId: string): Promise<void> {
    const { promise } = this.getModelServices();
    const services = await promise;
    await services.modelRuntime.logout(providerId);
    this.#quotas.clearProvider(providerId);
    await services.modelRuntime.refresh({ allowNetwork: false }).catch(() => undefined);
    await this.#syncLiveRuntime();
  }

  async getUsageSummary(range: UsageRange, onlyConfigured = true): Promise<UsageSummary> {
    const { promise } = this.getModelServices();
    const services = await promise;
    await services.modelRuntime.refresh({ allowNetwork: false });
    const configuredProviders = new Map<string, string>();
    for (const provider of services.modelRuntime.getProviders()) {
      if (services.modelRuntime.getProviderAuthStatus(provider.id).configured) {
        configuredProviders.set(provider.id, provider.name || provider.id);
      }
    }
    const modelLabels = new Map<string, string>();
    for (const model of services.modelRuntime.getAvailableSnapshot()) {
      modelLabels.set(`${model.provider}/${model.id}`, model.name || model.id);
    }
    return scanLocalUsage({
      agentDir: getAgentDir(),
      range,
      configuredProviders,
      modelLabels,
      onlyConfigured,
    });
  }

  async listQuotas(refresh = false): Promise<ProviderQuotaItem[]> {
    const { promise } = this.getModelServices();
    const services = await promise;
    return this.#quotas.listForConfiguredProviders(services, refresh);
  }

  async dispose(): Promise<void> {
    this.#authLogins.dispose();
    const modelServices = this.#modelServices;
    this.#modelServices = undefined;
    if (modelServices) await settleServices(modelServices);
  }
}
