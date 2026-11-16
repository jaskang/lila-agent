import { useEffect, useState } from "react";
import { Check, CircleAlert, LoaderCircle } from "lucide-react";
import { ServerHello, DEFAULT_CORE_PORT } from "@lila/protocol";
import { Button } from "@/components/ui/button";

type ConnectionState = "connecting" | "connected" | "disconnected";

export function App() {
  const [connection, setConnection] = useState<ConnectionState>("connecting");

  useEffect(() => {
    const socket = new WebSocket(`ws://127.0.0.1:${DEFAULT_CORE_PORT}/ws`);
    socket.onopen = () => setConnection("connecting");
    socket.onmessage = (event) => {
      try {
        if (ServerHello.safeParse(JSON.parse(String(event.data))).success) {
          setConnection("connected");
        }
      } catch {
        setConnection("disconnected");
      }
    };
    socket.onerror = () => setConnection("disconnected");
    socket.onclose = () => setConnection("disconnected");
    return () => socket.close();
  }, []);

  const status = {
    connecting: { label: "正在连接 core…", icon: LoaderCircle, className: "text-muted-foreground" },
    connected: { label: "core 已连接", icon: Check, className: "text-emerald-600" },
    disconnected: { label: "core 未连接", icon: CircleAlert, className: "text-destructive" },
  }[connection];
  const Icon = status.icon;

  return (
    <main className="flex min-h-screen items-center justify-center bg-background p-8 text-foreground">
      <section className="w-full max-w-xl rounded-xl border bg-card p-8 shadow-sm">
        <p className="text-sm font-medium text-muted-foreground">Lila</p>
        <h1 className="mt-2 text-3xl font-semibold tracking-tight">Agent 工作台</h1>
        <p className="mt-3 text-muted-foreground">仓库骨架已启动，下一步可以接入会话和事件流。</p>
        <div className={`mt-8 flex items-center gap-2 text-sm ${status.className}`}>
          <Icon className={connection === "connecting" ? "size-4 animate-spin" : "size-4"} />
          {status.label}
        </div>
        <Button className="mt-8" disabled={connection !== "connected"}>
          新建会话
        </Button>
      </section>
    </main>
  );
}
