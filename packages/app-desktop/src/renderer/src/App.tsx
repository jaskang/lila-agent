import * as React from "react";
import { RotateCw, WifiOff } from "lucide-react";
import { AppSidebar } from "@/components/app-sidebar";
import { ChangesPanel } from "@/components/changes-panel";
import { CommandPalette } from "@/components/command-palette";
import { Conversation } from "@/components/conversation";
import { FilesPanel } from "@/components/files";
import { NewSession } from "@/components/new-session";
import { ReplyChangesPanel } from "@/components/reply-changes";
import { RightPanelShell } from "@/components/right-panel";
import { SettingsDialog } from "@/components/settings";
import { SubsessionPreview } from "@/components/subsession-preview";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { SidebarInset, SidebarProvider } from "@/components/ui/sidebar";
import { Toaster } from "@/components/ui/sonner";
import { Spinner } from "@/components/ui/spinner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { actions, client, useLila } from "@/lib/store";

const SIDEBAR_OPEN_KEY = "lila.sidebarOpen";

function readSidebarOpen(): boolean {
  try {
    const saved = localStorage.getItem(SIDEBAR_OPEN_KEY);
    if (saved === "false") return false;
    if (saved === "true") return true;
  } catch {
    // Ignore storage read errors.
  }
  return true;
}

function ConnectionBanner() {
  const connection = useLila((state) => state.connection);
  const detail = useLila((state) => state.connectionDetail);
  const disconnected = connection !== "connected";
  // A short reconnect should not flash a banner, and reconnect attempts must not reset this timer.
  const [visible, setVisible] = React.useState(false);
  React.useEffect(() => {
    if (!disconnected) {
      setVisible(false);
      return;
    }
    const timer = setTimeout(() => setVisible(true), 800);
    return () => clearTimeout(timer);
  }, [disconnected]);
  if (!visible || !disconnected) return null;
  return (
    <div className="absolute inset-x-0 top-12 z-20 mx-auto w-fit max-w-lg px-4">
      <Alert variant={connection === "incompatible" ? "destructive" : "default"} className="shadow-md">
        {connection === "connecting" ? <Spinner /> : <WifiOff />}
        <AlertTitle>{connection === "incompatible" ? "core 版本不匹配" : connection === "connecting" ? "正在连接 core…" : "与 core 的连接断开了"}</AlertTitle>
        <AlertDescription>
          <span>{detail ?? "正在自动重连，任务不会因为窗口断线而中断。"}</span>
          <div className="mt-2 flex flex-wrap gap-2">
            {connection === "incompatible" ? (
              <Button size="sm" variant="outline" onClick={() => window.location.reload()}>
                <RotateCw />
                重新加载界面
              </Button>
            ) : (
              <Button size="sm" variant="outline" onClick={() => actions.reconnectNow()}>
                <RotateCw />
                立即重连
              </Button>
            )}
          </div>
        </AlertDescription>
      </Alert>
    </div>
  );
}

export function App() {
  const selectedId = useLila((state) => state.selectedId);
  const selectedProjectId = useLila((state) => (state.selectedId ? state.sessions[state.selectedId]?.projectId : undefined));
  const previewId = useLila((state) => state.previewId);
  const rightPanel = useLila((state) => state.rightPanel);
  const replyReview = useLila((state) => state.replyReview);
  const [sidebarOpen, setSidebarOpen] = React.useState(readSidebarOpen);
  const workspaceRef = React.useRef<HTMLDivElement>(null);

  const handleSidebarOpenChange = React.useCallback((open: boolean) => {
    setSidebarOpen(open);
    try {
      localStorage.setItem(SIDEBAR_OPEN_KEY, String(open));
    } catch {
      // Ignore storage write errors.
    }
  }, []);

  React.useEffect(() => {
    client.start();
    if (typeof Notification !== "undefined" && Notification.permission === "default") void Notification.requestPermission();
    const onKey = (event: KeyboardEvent) => {
      const mod = (event.metaKey || event.ctrlKey) && !event.altKey;
      if (!mod) return;
      const key = event.key.toLowerCase();
      const state = useLila.getState();
      if (!event.shiftKey) {
        if (key === "k") {
          event.preventDefault();
          actions.setPaletteOpen(!state.paletteOpen);
          return;
        }
        if (key === ",") {
          event.preventDefault();
          actions.setSettingsOpen(!state.settingsOpen);
          return;
        }
        if (state.settingsOpen || state.paletteOpen) return;
        if (key === "n") {
          event.preventDefault();
          actions.startNew();
        } else if (key === "e" && state.selectedId) {
          event.preventDefault();
          actions.toggleRightPanel("files");
        }
      } else if (!state.settingsOpen && !state.paletteOpen && key === "g" && state.selectedId) {
        event.preventDefault();
        actions.toggleRightPanel("changes");
      }
    };
    const offMenu = window.lila.onMenuAction((action) => {
      if (action === "settings") actions.openSettings();
      else if (action === "new-session") {
        actions.closeSettingsThen(() => {
          actions.setPaletteOpen(false);
          actions.startNew();
        });
      }
    });
    window.addEventListener("keydown", onKey);
    return () => {
      offMenu();
      window.removeEventListener("keydown", onKey);
      client.dispose();
    };
  }, []);

  const rightPanelContent =
    selectedId && rightPanel === "subsession" && previewId
      ? { label: "子会话预览", node: <SubsessionPreview key={previewId} sessionId={previewId} /> }
      : selectedId && rightPanel === "review" && replyReview
        ? { label: "本轮文件改动", node: <ReplyChangesPanel review={replyReview} /> }
        : selectedId && rightPanel === "files" && selectedProjectId
          ? { label: "项目文件与预览", node: <FilesPanel projectId={selectedProjectId} /> }
          : selectedId && rightPanel === "changes" && selectedProjectId
            ? { label: "工作区改动", node: <ChangesPanel projectId={selectedProjectId} /> }
            : undefined;

  return (
    <TooltipProvider>
      <SidebarProvider open={sidebarOpen} onOpenChange={handleSidebarOpenChange} className="h-svh min-h-0">
        <AppSidebar />
        <SidebarInset className="relative m-1.5 min-h-0 flex-row overflow-visible bg-transparent shadow-none md:peer-data-[variant=inset]:m-1.5 md:peer-data-[variant=inset]:ml-0 md:peer-data-[variant=inset]:rounded-none md:peer-data-[variant=inset]:shadow-none md:peer-data-[variant=inset]:peer-data-[state=collapsed]:ml-1.5">
          <div ref={workspaceRef} className="relative flex min-h-0 min-w-0 flex-1">
            <div data-surface="panel" className="@container relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden rounded-xl bg-workspace-surface shadow-[var(--panel-shadow)] ring-1 ring-[var(--panel-border)]">
              <ConnectionBanner />
              {selectedId ? <Conversation key={selectedId} sessionId={selectedId} /> : <NewSession />}
            </div>
            <RightPanelShell
              open={Boolean(rightPanelContent)}
              label={rightPanelContent?.label}
              containerRef={workspaceRef}
              onClose={() => actions.closeRightPanel()}
            >
              {rightPanelContent?.node}
            </RightPanelShell>
          </div>
        </SidebarInset>
        <CommandPalette />
        <SettingsDialog />
      </SidebarProvider>
      <Toaster position="bottom-right" richColors closeButton />
    </TooltipProvider>
  );
}
