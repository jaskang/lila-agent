import * as React from "react";
import { RotateCw, WifiOff } from "lucide-react";
import { AppSidebar } from "@/components/app-sidebar";
import { Conversation } from "@/components/conversation";
import { NewSession } from "@/components/new-session";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { SidebarInset, SidebarProvider } from "@/components/ui/sidebar";
import { Toaster } from "@/components/ui/sonner";
import { Spinner } from "@/components/ui/spinner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { actions, client, useLila } from "@/lib/store";

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
    <div className="absolute inset-x-0 top-14 z-20 mx-auto w-fit max-w-lg px-4">
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
  const [newSessionProject, setNewSessionProject] = React.useState<string>();

  React.useEffect(() => {
    client.start();
    if (Notification.permission === "default") void Notification.requestPermission();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "n" && event.metaKey) {
        event.preventDefault();
        setNewSessionProject(undefined);
        actions.select(undefined);
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
      <SidebarProvider className="h-svh min-h-0">
        <AppSidebar
          onNewSession={(projectId) => {
            setNewSessionProject(projectId);
            actions.select(undefined);
          }}
        />
        <SidebarInset className="relative min-h-0 overflow-hidden">
          <ConnectionBanner />
          {selectedId ? <Conversation key={selectedId} sessionId={selectedId} /> : <NewSession {...(newSessionProject ? { projectId: newSessionProject } : {})} />}
        </SidebarInset>
      </SidebarProvider>
      <Toaster position="bottom-right" richColors closeButton />
    </TooltipProvider>
  );
}
