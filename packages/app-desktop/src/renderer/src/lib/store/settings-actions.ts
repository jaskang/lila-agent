import { toast } from "sonner";
import type {
  AuthMethod,
  CapabilitiesSnapshot,
  CoreSettings,
  McpConfigSnapshot,
  McpExposure,
  McpScope,
  McpServerInput,
  MemorySnapshot,
  MemoryTarget,
  PackageScope,
  ProviderAuthInfo,
  ProviderQuotaItem,
  ThinkingLevel,
  UsageRange,
  UsageSummary,
} from "@lila/protocol";
import type { CoreClient } from "../client.ts";
import { attempt, message } from "./storage.ts";
import { NOTIFICATIONS_KEY, type NewSessionOptions, type SettingsTab, type State } from "./types.ts";

type StoreContext = {
  client: CoreClient;
  get: () => State;
  set: (partial: Partial<State> | ((state: State) => Partial<State>)) => void;
  loadModels: (refresh?: boolean) => Promise<void>;
  setNewSession: (patch: Partial<NewSessionOptions>) => void;
};

export function createSettingsActions({ client, get, set, loadModels, setNewSession }: StoreContext) {
  const cancelProviderLogin = async (): Promise<void> => {
    const current = get().authLogin;
    set({ authLogin: undefined });
    if (current && current.status === "running" && get().connection === "connected") {
      void client.call({ type: "auth.login.cancel", loginId: current.loginId }).catch(() => {});
    }
  };

  return {
    openSettings(tab?: SettingsTab): void {
      set((state) => ({ settingsOpen: true, paletteOpen: false, settingsTab: tab ?? state.settingsTab }));
    },

    setSettingsOpen(open: boolean): void {
      set((state) => ({
        settingsOpen: open,
        ...(open ? { paletteOpen: false } : {}),
        ...(!open && state.authLogin?.status !== "running" ? { authLogin: undefined } : {}),
      }));
    },

    setSettingsTab(tab: SettingsTab): void {
      set({ settingsTab: tab });
    },

    setSettingsDirty(part: "mcp" | "memory" | "capabilities", dirty: boolean): void {
      set((state) => ({ settingsDirty: { ...state.settingsDirty, [part]: dirty } }));
    },

    async restartCore(): Promise<boolean> {
      try {
        await window.lila.restartCore();
        client.reconnectNow();
        toast.success("Core 服务已重启");
        return true;
      } catch (error) {
        toast.error(message(error));
        return false;
      }
    },

    async openExternalUrl(url: string): Promise<void> {
      try {
        await window.lila.openExternal(url);
      } catch (error) {
        toast.error(message(error));
      }
    },

    async revealPath(path: string): Promise<void> {
      try {
        const res = await window.lila.revealPath(path);
        if (res && !res.ok && res.error) toast.info(res.error);
      } catch (error) {
        toast.error(message(error));
      }
    },

    setNotificationsEnabled(enabled: boolean): void {
      set({ notificationsEnabled: enabled });
      try {
        localStorage.setItem(NOTIFICATIONS_KEY, String(enabled));
      } catch {
        // Ignore storage write errors.
      }
      if (enabled && typeof Notification !== "undefined" && Notification.permission === "default") {
        void Notification.requestPermission();
      }
    },

    async getSettings(): Promise<CoreSettings> {
      return (await client.call({ type: "settings.get" })).settings;
    },

    async updateSettings(patch: {
      defaultModelId?: string | undefined;
      defaultThinkingLevel?: ThinkingLevel | undefined;
      autoCompaction?: boolean | undefined;
      autoRetry?: boolean | undefined;
    }): Promise<CoreSettings | undefined> {
      const result = await attempt(() => client.call({ type: "settings.update", ...patch }));
      if (!result) return undefined;
      if (patch.defaultModelId !== undefined || patch.defaultThinkingLevel !== undefined) {
        setNewSession({
          ...(patch.defaultModelId !== undefined ? { modelId: patch.defaultModelId } : {}),
          ...(patch.defaultThinkingLevel !== undefined ? { thinkingLevel: patch.defaultThinkingLevel } : {}),
        });
        void loadModels();
      }
      return result.settings;
    },

    async listProviders(): Promise<ProviderAuthInfo[]> {
      return (await client.call({ type: "providers.list" })).providers;
    },

    async startProviderLogin(providerId: string, providerName: string, method: AuthMethod): Promise<boolean> {
      if (get().authLogin?.status === "running") await cancelProviderLogin();
      const result = await attempt(() => client.call({ type: "auth.login.start", providerId, method }));
      if (!result) return false;
      set({ authLogin: { loginId: result.loginId, providerId, providerName, method, status: "running" } });
      return true;
    },

    async respondProviderLogin(promptId: string, value: string): Promise<boolean> {
      const current = get().authLogin;
      if (!current || current.status !== "running") return false;
      const result = await attempt(() => client.call({ type: "auth.login.respond", loginId: current.loginId, promptId, value }));
      if (!result) return false;
      set((state) =>
        state.authLogin?.loginId === current.loginId
          ? { authLogin: { ...state.authLogin, prompt: undefined, progress: "正在验证…" } }
          : state,
      );
      return true;
    },

    cancelProviderLogin,

    clearAuthLogin(): void {
      set({ authLogin: undefined });
    },

    async logoutProvider(providerId: string): Promise<boolean> {
      const result = await attempt(() => client.call({ type: "auth.logout", providerId }));
      if (!result) return false;
      void loadModels();
      return true;
    },

    async setProviderModels(providerId: string, modelIds: string[] | null): Promise<boolean> {
      const result = await attempt(() => client.call({ type: "provider.models.set", providerId, modelIds }));
      if (!result) return false;
      set({ models: result.models, defaultModelId: result.defaultModelId, modelsError: undefined });
      return true;
    },

    async listMcp(projectId?: string): Promise<McpConfigSnapshot> {
      return (await client.call({ type: "mcp.list", ...(projectId ? { projectId } : {}) })).config;
    },

    async upsertMcp(scope: McpScope, name: string, config: McpServerInput, projectId?: string): Promise<McpConfigSnapshot | undefined> {
      const result = await attempt(() =>
        client.call({ type: "mcp.upsert", scope, name, config, ...(projectId ? { projectId } : {}) }),
      );
      return result?.config;
    },

    async toggleMcp(
      scope: McpScope,
      name: string,
      patch: { enabled?: boolean | undefined; exposure?: McpExposure | undefined },
      projectId?: string,
    ): Promise<McpConfigSnapshot | undefined> {
      const result = await attempt(() =>
        client.call({
          type: "mcp.toggle",
          scope,
          name,
          ...(patch.enabled !== undefined ? { enabled: patch.enabled } : {}),
          ...(patch.exposure !== undefined ? { exposure: patch.exposure } : {}),
          ...(projectId ? { projectId } : {}),
        }),
      );
      return result?.config;
    },

    async removeMcp(scope: McpScope, name: string, projectId?: string): Promise<McpConfigSnapshot | undefined> {
      const result = await attempt(() =>
        client.call({ type: "mcp.remove", scope, name, ...(projectId ? { projectId } : {}) }),
      );
      return result?.config;
    },

    async getCapabilities(projectId?: string, sessionId?: string): Promise<CapabilitiesSnapshot> {
      return (
        await client.call({
          type: "capabilities.get",
          ...(projectId ? { projectId } : {}),
          ...(sessionId ? { sessionId } : {}),
        })
      ).capabilities;
    },

    async getMemory(projectId?: string, sessionId?: string): Promise<MemorySnapshot> {
      return (
        await client.call({
          type: "memory.get",
          ...(projectId ? { projectId } : {}),
          ...(sessionId ? { sessionId } : {}),
        })
      ).memory;
    },

    async addMemory(target: MemoryTarget, content: string): Promise<MemorySnapshot | undefined> {
      return (await attempt(() => client.call({ type: "memory.add", target, content })))?.memory;
    },

    async replaceMemory(target: MemoryTarget, oldText: string, newContent: string): Promise<MemorySnapshot | undefined> {
      return (await attempt(() => client.call({ type: "memory.replace", target, oldText, newContent })))?.memory;
    },

    async removeMemory(target: MemoryTarget, oldText: string): Promise<MemorySnapshot | undefined> {
      return (await attempt(() => client.call({ type: "memory.remove", target, oldText })))?.memory;
    },

    async updateMemoryConfig(patch: {
      enabled?: boolean | undefined;
      memoryCharLimit?: number | undefined;
      userCharLimit?: number | undefined;
      dreamingEnabled?: boolean | undefined;
      dreamingMinHours?: number | undefined;
      dreamingMinTurns?: number | undefined;
      dreamingModelId?: string | null | undefined;
    }): Promise<MemorySnapshot | undefined> {
      const result = await attempt(() =>
        client.call({
          type: "memory.config.update",
          ...(patch.enabled !== undefined ? { enabled: patch.enabled } : {}),
          ...(patch.memoryCharLimit !== undefined ? { memoryCharLimit: patch.memoryCharLimit } : {}),
          ...(patch.userCharLimit !== undefined ? { userCharLimit: patch.userCharLimit } : {}),
          ...(patch.dreamingEnabled !== undefined ? { dreamingEnabled: patch.dreamingEnabled } : {}),
          ...(patch.dreamingMinHours !== undefined ? { dreamingMinHours: patch.dreamingMinHours } : {}),
          ...(patch.dreamingMinTurns !== undefined ? { dreamingMinTurns: patch.dreamingMinTurns } : {}),
          ...(patch.dreamingModelId !== undefined ? { dreamingModelId: patch.dreamingModelId } : {}),
        }),
      );
      return result?.memory;
    },

    async installPackage(source: string, scope: PackageScope, projectId?: string): Promise<CapabilitiesSnapshot | undefined> {
      return (await attempt(() => client.call({ type: "packages.install", source, scope, ...(projectId ? { projectId } : {}) })))?.capabilities;
    },

    async removePackage(source: string, scope: PackageScope, projectId?: string): Promise<CapabilitiesSnapshot | undefined> {
      return (await attempt(() => client.call({ type: "packages.remove", source, scope, ...(projectId ? { projectId } : {}) })))?.capabilities;
    },

    async updatePackages(source?: string, projectId?: string): Promise<CapabilitiesSnapshot | undefined> {
      return (
        await attempt(() =>
          client.call({
            type: "packages.update",
            ...(source ? { source } : {}),
            ...(projectId ? { projectId } : {}),
          }),
        )
      )?.capabilities;
    },

    async reloadWorkspace(projectId?: string, sessionId?: string): Promise<boolean> {
      const done =
        (await attempt(() =>
          client.call({
            type: "workspace.reload",
            ...(projectId ? { projectId } : {}),
            ...(sessionId ? { sessionId } : {}),
          }),
        )) !== undefined;
      if (done) toast.success("已重新加载技能、扩展和 MCP");
      return done;
    },

    async getUsageSummary(range: UsageRange, onlyConfigured = true): Promise<UsageSummary> {
      return (await client.call({ type: "usage.summary", range, onlyConfigured })).summary;
    },

    async listQuotas(refresh = false): Promise<ProviderQuotaItem[]> {
      return (await client.call({ type: "quotas.list", refresh })).quotas;
    },
  };
}
