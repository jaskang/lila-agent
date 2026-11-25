import * as React from "react";
import { KeyRound, Monitor, Moon, Sun } from "lucide-react";
import type { ApprovalMode, CoreSettings, ThinkingLevel } from "@lila/protocol";
import { SettingsRow, SettingsSection, useGroupedModels } from "@/components/settings/shared";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { cn } from "cn";
import { actions, APPROVAL_LABEL, useLila } from "@/lib/store";
import { type ThemeMode, useTheme } from "@/lib/theme";

const THINKING_LEVEL_OPTIONS: Array<{ value: ThinkingLevel; label: string }> = [
  { value: "off", label: "关闭" },
  { value: "minimal", label: "极简" },
  { value: "low", label: "低" },
  { value: "medium", label: "中" },
  { value: "high", label: "高" },
  { value: "xhigh", label: "很高" },
  { value: "max", label: "最高" },
];

const THEME_OPTIONS: Array<{ value: ThemeMode; label: string; icon: React.ComponentType<{ className?: string }> }> = [
  { value: "system", label: "系统", icon: Monitor },
  { value: "light", label: "浅色", icon: Sun },
  { value: "dark", label: "深色", icon: Moon },
];

const APPROVAL_MODES = Object.keys(APPROVAL_LABEL) as ApprovalMode[];

export function GeneralTab({
  settings,
  savingKey,
  onUpdateSettings,
}: {
  settings: CoreSettings | undefined;
  savingKey: string | undefined;
  onUpdateSettings: (
    key: string,
    patch: {
      defaultModelId?: string | undefined;
      defaultThinkingLevel?: ThinkingLevel | undefined;
      autoCompaction?: boolean | undefined;
      autoRetry?: boolean | undefined;
    },
  ) => Promise<void>;
}) {
  const { theme, setTheme } = useTheme();
  const notificationsEnabled = useLila((state) => state.notificationsEnabled);
  const newSession = useLila((state) => state.newSession);
  const models = useLila((state) => state.models);
  const connected = useLila((state) => state.connection === "connected");

  const notificationDenied = typeof Notification !== "undefined" && Notification.permission === "denied";
  const selectedModelId = settings?.defaultModelId ?? newSession.modelId ?? "";
  const selectedThinking = settings?.defaultThinkingLevel ?? newSession.thinkingLevel ?? "medium";
  const { enabledModels, groupedModels } = useGroupedModels(models, selectedModelId);

  const activeDefaultModel = React.useMemo(
    () => models.find((item) => item.id === selectedModelId),
    [models, selectedModelId],
  );

  const thinkingOptions = React.useMemo(() => {
    if (!activeDefaultModel?.thinkingLevels.length) return THINKING_LEVEL_OPTIONS;
    const allowed = new Set(activeDefaultModel.thinkingLevels);
    const filtered = THINKING_LEVEL_OPTIONS.filter((opt) => allowed.has(opt.value));
    if (!filtered.some((opt) => opt.value === selectedThinking)) {
      const currentOpt = THINKING_LEVEL_OPTIONS.find((opt) => opt.value === selectedThinking);
      if (currentOpt) return [currentOpt, ...filtered];
    }
    return filtered.length ? filtered : THINKING_LEVEL_OPTIONS;
  }, [activeDefaultModel, selectedThinking]);

  const modelItems = React.useMemo(
    () => enabledModels.map((model) => ({ value: model.id, label: `${model.label} (${model.providerName})` })),
    [enabledModels],
  );

  return (
    <div className="flex flex-col gap-6">
      <SettingsSection title="界面">
        <SettingsRow title="外观">
          <div className="inline-flex rounded-lg bg-muted/60 p-0.5">
            {THEME_OPTIONS.map(({ value, label, icon: Icon }) => {
              const active = theme === value;
              return (
                <Button
                  key={value}
                  type="button"
                  size="xs"
                  variant="ghost"
                  aria-pressed={active}
                  className={cn(
                    "gap-1.5 px-2.5 font-normal text-muted-foreground hover:text-foreground",
                    active && "bg-background font-medium text-foreground shadow-2xs",
                  )}
                  onClick={() => setTheme(value)}
                >
                  <Icon className="size-3.5" />
                  {label}
                </Button>
              );
            })}
          </div>
        </SettingsRow>
        <SettingsRow title="桌面通知" detail={notificationDenied ? "系统已关闭通知权限" : undefined}>
          <Switch
            checked={notificationsEnabled}
            onCheckedChange={(checked) => actions.setNotificationsEnabled(checked)}
            aria-label="桌面通知"
          />
        </SettingsRow>
      </SettingsSection>

      <SettingsSection title="新会话默认">
        <SettingsRow title="默认审批">
          <Select
            value={newSession.approvalMode}
            onValueChange={(value) => {
              if (value) actions.setNewSession({ approvalMode: value as ApprovalMode });
            }}
            items={APPROVAL_MODES.map((mode) => ({ value: mode, label: APPROVAL_LABEL[mode] }))}
          >
            <SelectTrigger size="sm" className="w-44">
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="end">
              {APPROVAL_MODES.map((mode) => (
                <SelectItem key={mode} value={mode}>
                  {APPROVAL_LABEL[mode]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </SettingsRow>

        <SettingsRow title="默认模型">
          {enabledModels.length ? (
            <Select
              value={selectedModelId}
              disabled={!connected || savingKey === "defaultModelId"}
              onValueChange={(value) => {
                if (value) void onUpdateSettings("defaultModelId", { defaultModelId: value });
              }}
              items={modelItems}
            >
              <SelectTrigger size="sm" className="w-60">
                <SelectValue placeholder="选择模型" />
              </SelectTrigger>
              <SelectContent align="end" className="max-h-72">
                {groupedModels.map(([providerId, group]) => (
                  <SelectGroup key={providerId}>
                    <SelectLabel>{group.providerName}</SelectLabel>
                    {group.items.map((model) => (
                      <SelectItem key={model.id} value={model.id}>
                        <span className="truncate">{model.label}</span>
                      </SelectItem>
                    ))}
                  </SelectGroup>
                ))}
              </SelectContent>
            </Select>
          ) : (
            <Button type="button" size="sm" variant="outline" onClick={() => actions.setSettingsTab("providers")}>
              <KeyRound />
              配置模型
            </Button>
          )}
        </SettingsRow>

        <SettingsRow title="默认推理">
          <Select
            value={selectedThinking}
            disabled={!connected || !settings || savingKey === "defaultThinkingLevel"}
            onValueChange={(value) => {
              if (value) void onUpdateSettings("defaultThinkingLevel", { defaultThinkingLevel: value as ThinkingLevel });
            }}
            items={thinkingOptions}
          >
            <SelectTrigger size="sm" className="w-44">
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="end">
              {thinkingOptions.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </SettingsRow>
      </SettingsSection>

      <SettingsSection title="运行">
        <SettingsRow title="自动压缩上下文">
          <Switch
            checked={settings?.autoCompaction ?? true}
            disabled={!connected || !settings || savingKey === "autoCompaction"}
            onCheckedChange={(checked) => void onUpdateSettings("autoCompaction", { autoCompaction: checked })}
            aria-label="自动压缩上下文"
          />
        </SettingsRow>
        <SettingsRow title="失败自动重试">
          <Switch
            checked={settings?.autoRetry ?? true}
            disabled={!connected || !settings || savingKey === "autoRetry"}
            onCheckedChange={(checked) => void onUpdateSettings("autoRetry", { autoRetry: checked })}
            aria-label="失败自动重试"
          />
        </SettingsRow>
      </SettingsSection>
    </div>
  );
}
