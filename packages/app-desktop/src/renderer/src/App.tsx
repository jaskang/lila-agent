import * as React from "react";
import { RotateCw, WifiOff } from "lucide-react";
import { AppSidebar } from "@/components/app-sidebar";
import { CommandPalette } from "@/components/command-palette";
import { Conversation } from "@/components/conversation";
import { NewSession } from "@/components/new-session";
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
  // A short reconnect should not flash a banner.
  const [visible, setVisible] = React.useState(false);
  React.useEffect(() => {
    if (connection === "connected") {
      setVisible(false);
      return;
    }
    const timer = setTimeout(() => setVisible(true), 800);
    return () => clearTimeout(timer);
  }, [connection]);
  if (!visible || connection === "connected") return null;
  return (
    <div className="absolute inset-x-0 top-12 z-20 mx-auto w-fit max-w-lg px-4">
      <Alert variant={connection === "incompatible" ? "destructive" : "default"} className="shadow-md">
        {connection === "connecting" ? <Spinner /> : <WifiOff />}
        <AlertTitle>{connection === "incompatible" ? "core 版本不匹配" : connection === "connecting" ? "正在连接 core…" : "与 core 的连接断开了"}</AlertTitle>
        <AlertDescription>
          {detail ?? "正在自动重连，任务不会因为窗口断线而中断。"}
          {connection === "incompatible" ? (
            <Button size="sm" variant="outline" className="mt-2" onClick={() => window.location.reload()}>
              <RotateCw />
              重新连接
            </Button>
          ) : null}
        </AlertDescription>
      </Alert>
    </div>
  );
}

export function App() {
  const selectedId = useLila((state) => state.selectedId);
  const previewId = useLila((state) => state.previewId);
  const [sidebarOpen, setSidebarOpen] = React.useState(readSidebarOpen);

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
      if (!event.metaKey || event.shiftKey || event.altKey) return;
      const key = event.key.toLowerCase();
      if (key === "n") {
        event.preventDefault();
        actions.startNew();
      } else if (key === "k") {
        event.preventDefault();
        actions.setPaletteOpen(!useLila.getState().paletteOpen);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      client.dispose();
    };
  }, []);

  return (
    <TooltipProvider>
      <SidebarProvider open={sidebarOpen} onOpenChange={handleSidebarOpenChange} className="h-svh min-h-0">
        <AppSidebar />
        <SidebarInset className="relative m-1.5 min-h-0 overflow-hidden rounded-xl bg-background shadow-sm ring-1 ring-border/60 md:peer-data-[variant=inset]:m-1.5 md:peer-data-[variant=inset]:ml-0 md:peer-data-[variant=inset]:peer-data-[state=collapsed]:ml-1.5">
          <ConnectionBanner />
          {selectedId ? (
            <div className="relative flex min-h-0 flex-1">
              <Conversation key={selectedId} sessionId={selectedId} />
              {previewId ? <SubsessionPreview key={previewId} sessionId={previewId} /> : null}
            </div>
          ) : <NewSession />}
        </SidebarInset>
        <CommandPalette />
      </SidebarProvider>
      <Toaster position="bottom-right" richColors closeButton />
    </TooltipProvider>
  );
}
