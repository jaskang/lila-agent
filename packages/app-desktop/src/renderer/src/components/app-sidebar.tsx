import * as React from "react";
import { CircleAlert, FolderPlus, MessageCircleQuestion, Plus } from "lucide-react";
import type { SessionSummary } from "@lila/protocol";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupAction,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from "@/components/ui/sidebar";
import { Spinner } from "@/components/ui/spinner";
import { actions, useLila } from "@/lib/store";

const RELATIVE = new Intl.RelativeTimeFormat("zh-CN", { numeric: "auto" });

function ago(iso: string): string {
  const minutes = Math.round((Date.parse(iso) - Date.now()) / 60_000);
  if (Math.abs(minutes) < 1) return "刚刚";
  if (Math.abs(minutes) < 60) return RELATIVE.format(minutes, "minute");
  const hours = Math.round(minutes / 60);
  if (Math.abs(hours) < 24) return RELATIVE.format(hours, "hour");
  return RELATIVE.format(Math.round(hours / 24), "day");
}

function StatusIcon({ session }: { session: SessionSummary }) {
  if (session.status === "waiting") return <MessageCircleQuestion className="text-amber-600 dark:text-amber-400" />;
  if (session.status === "running") return <Spinner />;
  if (session.status === "failed" || session.status === "interrupted") return <CircleAlert className="text-destructive" />;
  return null;
}

export function AppSidebar({ onNewSession }: { onNewSession: (projectId?: string) => void }) {
  const projects = useLila((state) => state.projects);
  const sessions = useLila((state) => state.sessions);
  const selectedId = useLila((state) => state.selectedId);
  const byProject = React.useMemo(() => {
    const groups: Record<string, SessionSummary[]> = {};
    for (const session of Object.values(sessions)) (groups[session.projectId] ??= []).push(session);
    for (const list of Object.values(groups)) list.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
    return groups;
  }, [sessions]);

  return (
    <Sidebar variant="inset">
      {/* Leaves room for the macOS traffic lights; the strip also drags the window. */}
      <SidebarHeader className="h-12 [-webkit-app-region:drag]" />
      <SidebarContent>
        <SidebarGroup>
          <SidebarMenu>
            <SidebarMenuItem>
              <SidebarMenuButton onClick={() => onNewSession()}>
                <Plus />
                新会话
              </SidebarMenuButton>
            </SidebarMenuItem>
          </SidebarMenu>
        </SidebarGroup>
        {projects.map((project) => (
          <SidebarGroup key={project.id}>
            <SidebarGroupLabel title={project.path}>{project.name}</SidebarGroupLabel>
            <SidebarGroupAction title={`在 ${project.name} 中新建会话`} onClick={() => onNewSession(project.id)}>
              <Plus />
              <span className="sr-only">新建会话</span>
            </SidebarGroupAction>
            <SidebarGroupContent>
              <SidebarMenu>
                {(byProject[project.id] ?? []).map((session) => (
                  <SidebarMenuItem key={session.id}>
                    <SidebarMenuButton
                      isActive={session.id === selectedId}
                      onClick={() => actions.select(session.id)}
                      className="h-auto py-1.5"
                    >
                      <div className="flex min-w-0 flex-1 flex-col">
                        <span className="truncate">{session.title || "新会话"}</span>
                        <span className="text-xs text-muted-foreground">{ago(session.updatedAt)}</span>
                      </div>
                      <StatusIcon session={session} />
                    </SidebarMenuButton>
                  </SidebarMenuItem>
                ))}
                {!byProject[project.id]?.length ? (
                  <p className="px-2 py-1 text-xs text-muted-foreground">还没有会话</p>
                ) : null}
              </SidebarMenu>
            </SidebarGroupContent>
          </SidebarGroup>
        ))}
      </SidebarContent>
      <SidebarFooter>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton onClick={() => void actions.addProject()}>
              <FolderPlus />
              添加项目
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarFooter>
    </Sidebar>
  );
}
