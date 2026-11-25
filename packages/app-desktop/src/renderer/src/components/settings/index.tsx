import * as React from "react";
import { BarChart3, Blocks, Brain, Info, KeyRound, Plug, SlidersHorizontal } from "lucide-react";
import { toast } from "sonner";
import type { CoreSettings, ProviderAuthInfo, ThinkingLevel } from "@lila/protocol";
import { AboutTab } from "@/components/settings/about-tab";
import { CapabilitiesSettingsTab } from "@/components/settings/capabilities-tab";
import { GeneralTab } from "@/components/settings/general-tab";
import { McpSettingsTab } from "@/components/settings/mcp-tab";
import { MemorySettingsTab } from "@/components/settings/memory-tab";
import { ProvidersTab } from "@/components/settings/providers-tab";
import { UsageSettingsTab } from "@/components/settings/usage-tab";
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
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { cn } from "cn";
import { useNewSessionProjectId } from "@/hooks/use-current-project";
import { actions, type SettingsTab, useLila } from "@/lib/store";

const NAV_ITEMS: Array<{ id: SettingsTab; label: string; icon: React.ComponentType<{ className?: string }> }> = [
  { id: "general", label: "通用", icon: SlidersHorizontal },
  { id: "providers", label: "模型与服务商", icon: KeyRound },
  { id: "usage", label: "用量", icon: BarChart3 },
  { id: "mcp", label: "MCP", icon: Plug },
  { id: "memory", label: "长期记忆", icon: Brain },
  { id: "capabilities", label: "技能与扩展", icon: Blocks },
  { id: "about", label: "关于", icon: Info },
];

export function SettingsDialog() {
  const open = useLila((state) => state.settingsOpen);
  const tab = useLila((state) => state.settingsTab);
  const connected = useLila((state) => state.connection === "connected");
  const authRunning = useLila((state) => state.authLogin?.status === "running");
  const settingsDirty = useLila((state) => state.settingsDirty);
  const confirmDiscard = useLila((state) => state.confirmSettingsDiscard);
  const selectedId = useLila((state) => state.selectedId);
  const newSessionProjectId = useNewSessionProjectId();
  const sessionProjectId = useLila((state) => (state.selectedId ? state.sessions[state.selectedId]?.projectId : undefined));
  const activeProjectId = selectedId ? sessionProjectId : newSessionProjectId;
  const activeProjectName = useLila((state) =>
    activeProjectId ? state.projects.find((item) => item.id === activeProjectId)?.name : undefined,
  );

  const [settings, setSettings] = React.useState<CoreSettings>();
  const [settingsLoading, setSettingsLoading] = React.useState(false);
  const [savingKey, setSavingKey] = React.useState<string>();

  const [providers, setProviders] = React.useState<ProviderAuthInfo[]>();
  const [providersLoading, setProvidersLoading] = React.useState(false);
  const [providersError, setProvidersError] = React.useState<string>();

  const loadSettings = React.useCallback(async () => {
    if (!connected) return;
    setSettingsLoading(true);
    try {
      setSettings(await actions.getSettings());
    } catch (error) {
      toast.error(error instanceof Error ? error.message : String(error));
    } finally {
      setSettingsLoading(false);
    }
  }, [connected]);

  const loadProviders = React.useCallback(async () => {
    if (!connected) return;
    setProvidersLoading(true);
    setProvidersError(undefined);
    try {
      setProviders(await actions.listProviders());
    } catch (error) {
      setProvidersError(error instanceof Error ? error.message : String(error));
    } finally {
      setProvidersLoading(false);
    }
  }, [connected]);

  React.useEffect(() => {
    if (!open || !connected) return;
    void loadSettings();
  }, [open, connected, loadSettings]);

  React.useEffect(() => {
    if (!open || !connected) return;
    if (tab === "providers") void loadProviders();
  }, [open, connected, tab, loadProviders]);

  const handleUpdateSettings = React.useCallback(
    async (
      key: string,
      patch: {
        defaultModelId?: string | undefined;
        defaultThinkingLevel?: ThinkingLevel | undefined;
        autoCompaction?: boolean | undefined;
        autoRetry?: boolean | undefined;
      },
    ) => {
      setSavingKey(key);
      try {
        const updated = await actions.updateSettings(patch);
        if (updated) setSettings(updated);
      } finally {
        setSavingKey(undefined);
      }
    },
    [],
  );

  const activeNav = NAV_ITEMS.find((item) => item.id === tab) ?? NAV_ITEMS[0]!;

  return (
    <Dialog open={open} onOpenChange={actions.setSettingsOpen}>
      <DialogContent className="flex h-[min(44rem,calc(100vh-3.5rem))] w-[min(56rem,calc(100vw-3rem))] flex-col gap-0 overflow-hidden p-0 sm:max-w-[56rem] sm:flex-row">
        <aside className="flex shrink-0 flex-col border-b border-border/60 bg-muted/25 p-3 sm:w-48 sm:border-e sm:border-b-0">
          <nav aria-label="设置分组" className="flex flex-row gap-1 overflow-x-auto sm:flex-col">
            {NAV_ITEMS.map(({ id, label, icon: Icon }) => {
              const active = id === tab;
              const hasDot =
                (id === "providers" && authRunning) ||
                (id === "mcp" && settingsDirty.mcp) ||
                (id === "memory" && settingsDirty.memory) ||
                (id === "capabilities" && settingsDirty.capabilities);
              return (
                <Button
                  key={id}
                  type="button"
                  size="sm"
                  variant="ghost"
                  aria-current={active ? "page" : undefined}
                  onClick={() => actions.setSettingsTab(id)}
                  className={cn(
                    "justify-start gap-2.5 px-2.5 font-normal text-muted-foreground hover:text-foreground",
                    active && "bg-accent font-medium text-accent-foreground",
                  )}
                >
                  <Icon className="size-4 shrink-0" />
                  <span className="truncate">{label}</span>
                  {hasDot ? <span aria-hidden className="ms-auto size-1.5 shrink-0 rounded-full bg-info" /> : null}
                </Button>
              );
            })}
          </nav>
        </aside>

        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
          <header className="flex h-12 shrink-0 items-center px-6 pr-12">
            <DialogTitle className="text-sm font-medium">{activeNav.label}</DialogTitle>
          </header>
          <Separator />
          <ScrollArea className="min-h-0 flex-1">
            <div className="px-6 py-5">
              {tab === "general" ? (
                <GeneralTab settings={settings} savingKey={savingKey} onUpdateSettings={handleUpdateSettings} />
              ) : tab === "providers" ? (
                <ProvidersTab providers={providers} loading={providersLoading} error={providersError} onReload={loadProviders} />
              ) : tab === "usage" ? (
                <UsageSettingsTab />
              ) : tab === "mcp" ? (
                <McpSettingsTab projectId={activeProjectId} projectName={activeProjectName} sessionId={selectedId} />
              ) : tab === "memory" ? (
                <MemorySettingsTab projectId={activeProjectId} sessionId={selectedId} />
              ) : tab === "capabilities" ? (
                <CapabilitiesSettingsTab projectId={activeProjectId} projectName={activeProjectName} sessionId={selectedId} />
              ) : (
                <AboutTab settings={settings} loading={settingsLoading} onReload={loadSettings} />
              )}
            </div>
          </ScrollArea>
        </div>
        <AlertDialog open={confirmDiscard} onOpenChange={(open) => { if (!open) actions.resolveSettingsDiscard(false); }}>
          <AlertDialogContent size="sm">
            <AlertDialogHeader>
              <AlertDialogTitle>放弃未保存的 MCP 更改？</AlertDialogTitle>
              <AlertDialogDescription>离开后会丢失当前表单内容。</AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>继续编辑</AlertDialogCancel>
              <AlertDialogAction variant="destructive" onClick={() => actions.resolveSettingsDiscard(true)}>
                放弃更改
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </DialogContent>
    </Dialog>
  );
}
