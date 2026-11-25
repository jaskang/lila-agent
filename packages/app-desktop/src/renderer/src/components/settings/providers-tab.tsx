import * as React from "react";
import { ChevronDown, ChevronUp, KeyRound, ListChecks, LogIn, LogOut, RotateCw } from "lucide-react";
import { toast } from "sonner";
import type { AuthMethod, ProviderAuthInfo } from "@lila/protocol";
import { IconButton } from "@/components/icon-button";
import { SearchInput } from "@/components/search-input";
import { LoginFlowCard } from "@/components/settings/login-flow-card";
import { ProviderModelsView } from "@/components/settings/provider-models-view";
import { SettingsSection } from "@/components/settings/shared";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Spinner } from "@/components/ui/spinner";
import { actions, useLila } from "@/lib/store";

const FEATURED_PROVIDER_ORDER = ["openai-codex", "anthropic", "deepseek", "kimi-coding", "minimax-cn", "zai-coding-cn"];
const FEATURED_PROVIDER_SET = new Set(FEATURED_PROVIDER_ORDER);

function providerRank(id: string): number {
  const index = FEATURED_PROVIDER_ORDER.indexOf(id);
  return index >= 0 ? index : FEATURED_PROVIDER_ORDER.length + 1;
}

function sourceLabel(provider: ProviderAuthInfo): string | undefined {
  if (!provider.configured) return undefined;
  if (provider.storedType === "oauth") return "账号已登录";
  if (provider.storedType === "api_key") return "API Key";
  if (provider.source === "environment") return provider.sourceLabel ? `环境变量 ${provider.sourceLabel}` : "环境变量";
  if (provider.source === "models_json_key" || provider.source === "models_json_command") return "models.json";
  return provider.sourceLabel || "已配置";
}

export function ProvidersTab({
  providers,
  loading,
  error,
  onReload,
}: {
  providers: ProviderAuthInfo[] | undefined;
  loading: boolean;
  error: string | undefined;
  onReload: () => Promise<void>;
}) {
  const models = useLila((state) => state.models);
  const authLogin = useLila((state) => state.authLogin);
  const [query, setQuery] = React.useState("");
  const [showAllUnconfigured, setShowAllUnconfigured] = React.useState(false);
  const [busyId, setBusyId] = React.useState<string | undefined>();
  const [confirmLogout, setConfirmLogout] = React.useState<ProviderAuthInfo>();
  const [managingProvider, setManagingProvider] = React.useState<{ id: string; name: string }>();

  const modelsByProvider = React.useMemo(() => {
    const map = new Map<string, typeof models>();
    for (const model of models) {
      const list = map.get(model.provider);
      if (list) list.push(model);
      else map.set(model.provider, [model]);
    }
    return map;
  }, [models]);

  const prevStatusRef = React.useRef(authLogin?.status);
  React.useEffect(() => {
    if (prevStatusRef.current !== "done" && authLogin?.status === "done") {
      const finished = { id: authLogin.providerId, name: authLogin.providerName };
      actions.clearAuthLogin();
      void onReload();
      setManagingProvider(finished);
    }
    prevStatusRef.current = authLogin?.status;
  }, [authLogin, onReload]);

  const sorted = React.useMemo(() => {
    const list = providers ?? [];
    const needle = query.trim().toLowerCase();
    const filtered = needle
      ? list.filter((provider) => provider.name.toLowerCase().includes(needle) || provider.id.toLowerCase().includes(needle))
      : list;
    const configured = filtered
      .filter((provider) => provider.configured)
      .sort((a, b) => providerRank(a.id) - providerRank(b.id) || a.name.localeCompare(b.name));
    const unconfiguredAll = filtered
      .filter((provider) => !provider.configured)
      .sort((a, b) => providerRank(a.id) - providerRank(b.id) || a.name.localeCompare(b.name));

    if (needle || showAllUnconfigured) {
      return { configured, unconfiguredVisible: unconfiguredAll, hiddenCount: 0 };
    }
    const featured = unconfiguredAll.filter((provider) => FEATURED_PROVIDER_SET.has(provider.id));
    return { configured, unconfiguredVisible: featured, hiddenCount: unconfiguredAll.length - featured.length };
  }, [providers, query, showAllUnconfigured]);

  const startLogin = async (provider: ProviderAuthInfo, method: AuthMethod) => {
    setBusyId(`${provider.id}:${method}`);
    try {
      await actions.startProviderLogin(provider.id, provider.name, method);
    } finally {
      setBusyId(undefined);
    }
  };

  const logout = async (provider: ProviderAuthInfo) => {
    setBusyId(`${provider.id}:logout`);
    try {
      if (await actions.logoutProvider(provider.id)) {
        toast.success(`已清除 ${provider.name} 的凭据`);
        await onReload();
      }
    } finally {
      setBusyId(undefined);
    }
  };

  if (managingProvider) {
    return (
      <ProviderModelsView
        providerId={managingProvider.id}
        providerName={managingProvider.name}
        models={modelsByProvider.get(managingProvider.id) ?? []}
        onBack={() => setManagingProvider(undefined)}
      />
    );
  }

  const renderRow = (provider: ProviderAuthInfo) => {
    const isLoggingThis = authLogin?.status === "running" && authLogin.providerId === provider.id;
    const canLogout = provider.source === "stored" || Boolean(provider.storedType);
    const providerModels = modelsByProvider.get(provider.id) ?? [];
    const enabledCount = providerModels.filter((item) => item.enabled).length;
    const detail = [
      sourceLabel(provider),
      provider.configured && providerModels.length > 0
        ? enabledCount === providerModels.length
          ? `${providerModels.length} 个模型`
          : `${enabledCount}/${providerModels.length} 个模型`
        : !provider.configured && provider.modelCount > 0
          ? `${provider.modelCount} 个模型`
          : undefined,
      !provider.supportsOAuth && !provider.supportsApiKey && provider.apiKeyLabel ? provider.apiKeyLabel : undefined,
    ]
      .filter(Boolean)
      .join(" · ");

    return (
      <div key={provider.id} className="flex flex-wrap items-center justify-between gap-3 px-1 py-2.5">
        <div className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="truncate text-sm text-foreground">{provider.name}</span>
          {detail ? <span className="truncate text-xs text-muted-foreground tabular-nums">{detail}</span> : null}
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          {provider.configured && providerModels.length > 0 ? (
            <Button
              type="button"
              size="xs"
              variant="outline"
              disabled={Boolean(busyId) || isLoggingThis}
              onClick={() => setManagingProvider({ id: provider.id, name: provider.name })}
            >
              <ListChecks />
              模型
            </Button>
          ) : null}
          {provider.supportsOAuth ? (
            <Button
              type="button"
              size="xs"
              variant={provider.configured ? "outline" : "default"}
              disabled={Boolean(busyId) || isLoggingThis}
              onClick={() => void startLogin(provider, "oauth")}
            >
              {busyId === `${provider.id}:oauth` ? <Spinner /> : <LogIn />}
              {provider.storedType === "oauth" ? "重新登录" : "登录"}
            </Button>
          ) : null}
          {provider.supportsApiKey ? (
            <Button
              type="button"
              size="xs"
              variant={provider.supportsOAuth || provider.configured ? "outline" : "default"}
              disabled={Boolean(busyId) || isLoggingThis}
              onClick={() => void startLogin(provider, "api_key")}
            >
              {busyId === `${provider.id}:api_key` ? <Spinner /> : <KeyRound />}
              {provider.storedType === "api_key" ? "更新 Key" : "API Key"}
            </Button>
          ) : null}
          {canLogout ? (
            <IconButton
              size="icon-xs"
              label="清除凭据"
              disabled={Boolean(busyId) || isLoggingThis}
              className="hover:text-destructive"
              onClick={() => setConfirmLogout(provider)}
            >
              {busyId === `${provider.id}:logout` ? <Spinner /> : <LogOut />}
            </IconButton>
          ) : null}
        </div>
      </div>
    );
  };

  return (
    <div className="flex flex-col gap-5">
      <div className="flex items-center gap-2">
        <SearchInput value={query} onChange={setQuery} placeholder="搜索服务商" ariaLabel="搜索服务商" />
        <IconButton label="刷新" disabled={loading} onClick={() => void onReload()}>
          {loading ? <Spinner /> : <RotateCw />}
        </IconButton>
      </div>

      {authLogin ? <LoginFlowCard flow={authLogin} onDone={() => void onReload()} /> : null}

      {loading && !providers ? (
        <div className="flex items-center justify-center gap-2 py-12 text-xs text-muted-foreground">
          <Spinner />
          正在加载…
        </div>
      ) : error ? (
        <Empty className="py-10">
          <EmptyHeader>
            <EmptyTitle>读取失败</EmptyTitle>
            <EmptyDescription>{error}</EmptyDescription>
          </EmptyHeader>
          <Button type="button" size="sm" variant="outline" onClick={() => void onReload()}>
            <RotateCw />
            重试
          </Button>
        </Empty>
      ) : !sorted.configured.length && !sorted.unconfiguredVisible.length ? (
        <Empty className="py-10">
          <EmptyHeader>
            <EmptyTitle>无匹配服务商</EmptyTitle>
          </EmptyHeader>
        </Empty>
      ) : (
        <div className="flex flex-col gap-6">
          {sorted.configured.length ? <SettingsSection title="已配置">{sorted.configured.map(renderRow)}</SettingsSection> : null}
          {sorted.unconfiguredVisible.length ? (
            <div className="flex flex-col gap-2">
              <SettingsSection title={query.trim() ? "未配置" : "更多服务商"}>
                {sorted.unconfiguredVisible.map(renderRow)}
              </SettingsSection>
              {!query.trim() && (sorted.hiddenCount > 0 || showAllUnconfigured) ? (
                <div className="flex justify-center">
                  <Button
                    type="button"
                    size="xs"
                    variant="ghost"
                    className="text-muted-foreground hover:text-foreground"
                    onClick={() => setShowAllUnconfigured((current) => !current)}
                  >
                    {showAllUnconfigured ? (
                      <>
                        <ChevronUp />
                        收起
                      </>
                    ) : (
                      <>
                        <ChevronDown />
                        展开全部（+{sorted.hiddenCount}）
                      </>
                    )}
                  </Button>
                </div>
              ) : null}
            </div>
          ) : null}
        </div>
      )}

      <AlertDialog open={Boolean(confirmLogout)} onOpenChange={(next) => !next && setConfirmLogout(undefined)}>
        <AlertDialogContent size="sm">
          <AlertDialogHeader>
            <AlertDialogTitle>清除「{confirmLogout?.name}」的凭据？</AlertDialogTitle>
            <AlertDialogDescription>将移除本地登录态或 API Key。</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>取消</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                const target = confirmLogout;
                setConfirmLogout(undefined);
                if (target) void logout(target);
              }}
            >
              清除
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
