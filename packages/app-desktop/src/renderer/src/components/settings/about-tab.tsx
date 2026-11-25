import * as React from "react";
import { Check, Copy, Power, RotateCw } from "lucide-react";
import { toast } from "sonner";
import type { CoreSettings } from "@lila/protocol";
import { PathRow, SettingsRow, SettingsSection } from "@/components/settings/shared";
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
import { Spinner } from "@/components/ui/spinner";
import { actions, useLila } from "@/lib/store";

export function AboutTab({
  settings,
  loading,
  onReload,
}: {
  settings: CoreSettings | undefined;
  loading: boolean;
  onReload: () => Promise<void>;
}) {
  const connection = useLila((state) => state.connection);
  const modelsCount = useLila((state) => state.models.length);
  const [appInfo, setAppInfo] = React.useState<{ appVersion: string; electronVersion: string; platform: string; arch: string }>();
  const [restarting, setRestarting] = React.useState(false);
  const [confirmRestart, setConfirmRestart] = React.useState(false);
  const [copiedDiag, setCopiedDiag] = React.useState(false);

  React.useEffect(() => {
    void window.lila.appInfo().then(setAppInfo).catch(() => undefined);
  }, []);

  const statusLabel =
    connection === "connected"
      ? "已连接"
      : connection === "connecting"
        ? "连接中…"
        : connection === "incompatible"
          ? "协议不匹配"
          : "已断开";

  const copyDiagnostics = () => {
    const lines = [
      `Lila Desktop: ${appInfo ? `v${appInfo.appVersion} (Electron ${appInfo.electronVersion}, ${appInfo.platform}/${appInfo.arch})` : "unknown"}`,
      `Core: ${settings ? `v${settings.coreVersion} (protocol v${settings.protocolVersion}, PID ${settings.pid})` : statusLabel}`,
      `Models loaded: ${modelsCount}`,
      ...(settings
        ? [
            `Agent dir: ${settings.paths.agentDir}`,
            `settings.json: ${settings.paths.settingsJson}`,
            `models.json: ${settings.paths.modelsJson}`,
            `mcp.json: ${settings.paths.mcpJson}`,
            `core.log: ${settings.paths.coreLog}`,
          ]
        : []),
    ];
    void navigator.clipboard
      .writeText(lines.join("\n"))
      .then(() => {
        setCopiedDiag(true);
        toast.success("已复制诊断信息");
        window.setTimeout(() => setCopiedDiag(false), 1_500);
      })
      .catch(() => toast.error("复制失败"));
  };

  const handleRestartCore = async () => {
    setRestarting(true);
    try {
      await actions.restartCore();
    } finally {
      setRestarting(false);
    }
  };

  return (
    <div className="flex flex-col gap-6">
      <SettingsSection title="状态">
        {appInfo ? (
          <SettingsRow title="桌面端">
            <span className="font-mono text-xs text-muted-foreground">
              v{appInfo.appVersion} · Electron {appInfo.electronVersion} · {appInfo.platform}/{appInfo.arch}
            </span>
          </SettingsRow>
        ) : null}
        <SettingsRow title="Core">
          <span className="font-mono text-xs text-muted-foreground">
            {statusLabel}
            {settings ? ` · v${settings.coreVersion} · 协议 v${settings.protocolVersion} · PID ${settings.pid}` : ""}
          </span>
        </SettingsRow>
        <SettingsRow title="操作" detail={`${modelsCount} 个模型`}>
          <div className="flex flex-wrap items-center gap-1.5">
            <Button type="button" size="xs" variant="outline" onClick={copyDiagnostics}>
              {copiedDiag ? <Check /> : <Copy />}
              {copiedDiag ? "已复制" : "复制诊断"}
            </Button>
            <Button
              type="button"
              size="xs"
              variant="outline"
              disabled={loading || connection !== "connected"}
              onClick={() => {
                void actions.retryModels();
                void onReload();
              }}
            >
              {loading ? <Spinner /> : <RotateCw />}
              刷新
            </Button>
            <Button
              type="button"
              size="xs"
              variant="ghost"
              className="text-muted-foreground hover:text-destructive"
              disabled={restarting}
              onClick={() => setConfirmRestart(true)}
            >
              {restarting ? <Spinner /> : <Power />}
              重启 Core
            </Button>
          </div>
        </SettingsRow>
      </SettingsSection>

      {settings ? (
        <SettingsSection title="文件与日志">
          <PathRow title="Pi 目录" path={settings.paths.agentDir} />
          <PathRow title="settings.json" path={settings.paths.settingsJson} />
          <PathRow title="models.json" path={settings.paths.modelsJson} />
          <PathRow title="mcp.json" path={settings.paths.mcpJson} />
          <PathRow title="core.log" path={settings.paths.coreLog} />
        </SettingsSection>
      ) : null}

      <AlertDialog open={confirmRestart} onOpenChange={setConfirmRestart}>
        <AlertDialogContent size="sm">
          <AlertDialogHeader>
            <AlertDialogTitle>重启 Core？</AlertDialogTitle>
            <AlertDialogDescription>正在运行的任务会中断，已完成的消息已保存。</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>取消</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                setConfirmRestart(false);
                void handleRestartCore();
              }}
            >
              重启
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
