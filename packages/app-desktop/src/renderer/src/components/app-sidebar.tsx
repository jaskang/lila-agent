import * as React from "react";
import { Check, ChevronDown, ChevronRight, CircleAlert, FolderPlus, MessageCircleQuestion, Pencil, Plus, Trash2, X } from "lucide-react";
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
  SidebarMenuAction,
  SidebarMenuButton,
  SidebarMenuItem,
} from "@/components/ui/sidebar";
import { Input } from "@/components/ui/input";
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

function SessionRow({ session }: { session: SessionSummary }) {
  const selectedId = useLila((state) => state.selectedId);
  const [editing, setEditing] = React.useState(false);
  const [title, setTitle] = React.useState(session.title || "");
  const inputRef = React.useRef<HTMLInputElement>(null);
  const busy = session.status === "running" || session.status === "waiting";

  React.useEffect(() => {
    setTitle(session.title || "");
  }, [session.title]);

  React.useEffect(() => {
    if (editing) {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [editing]);

  async function save(): Promise<void> {
    const next = title.trim();
    if (!next || next === session.title) {
      setEditing(false);
      return;
    }
    if (await actions.renameSession(session.id, next)) setEditing(false);
  }

  async function remove(): Promise<void> {
    if (busy) return;
    if (!window.confirm(`删除会话“${session.title || "新会话"}”？消息记录也会被删除。`)) return;
    await actions.deleteSession(session.id);
  }

  return (
    <SidebarMenuItem>
      {editing ? (
        <div className="flex items-center gap-1 px-1 py-1">
          <Input
            ref={inputRef}
            value={title}
            maxLength={120}
            aria-label="会话名称"
            className="h-7 min-w-0 flex-1"
            onChange={(event) => setTitle(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") void save();
              if (event.key === "Escape") setEditing(false);
            }}
          />
          <button type="button" className="rounded p-1 hover:bg-sidebar-accent" aria-label="保存名称" onClick={() => void save()}>
            <Check />
          </button>
          <button type="button" className="rounded p-1 hover:bg-sidebar-accent" aria-label="取消重命名" onClick={() => setEditing(false)}>
            <X />
          </button>
        </div>
      ) : (
        <>
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
          <SidebarMenuAction showOnHover aria-label="重命名会话" title="重命名会话" onClick={() => setEditing(true)}>
            <Pencil />
          </SidebarMenuAction>
          <SidebarMenuAction showOnHover className="right-7" aria-label="删除会话" title="删除会话" disabled={busy} onClick={() => void remove()}>
            <Trash2 />
          </SidebarMenuAction>
        </>
      )}
    </SidebarMenuItem>
  );
}

export function AppSidebar({ onNewSession }: { onNewSession: (projectId?: string) => void }) {
  const projects = useLila((state) => state.projects);
  const sessions = useLila((state) => state.sessions);
  const [collapsed, setCollapsed] = React.useState<Record<string, boolean>>({});
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
            <SidebarGroupLabel
              title={project.path}
              role="button"
              tabIndex={0}
              aria-expanded={!collapsed[project.id]}
              className="cursor-pointer hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
              onClick={() => setCollapsed((current) => ({ ...current, [project.id]: !current[project.id] }))}
              onKeyDown={(event) => {
                if (event.key === "Enter" || event.key === " ") {
                  event.preventDefault();
                  setCollapsed((current) => ({ ...current, [project.id]: !current[project.id] }));
                }
              }}
            >
              {collapsed[project.id] ? <ChevronRight /> : <ChevronDown />}
              {project.name}
            </SidebarGroupLabel>
            <SidebarGroupAction title={`在 ${project.name} 中新建会话`} onClick={() => onNewSession(project.id)}>
              <Plus />
              <span className="sr-only">新建会话</span>
            </SidebarGroupAction>
            {!collapsed[project.id] ? <SidebarGroupContent>
              <SidebarMenu>
                {(byProject[project.id] ?? []).map((session) => (
                  <SessionRow key={session.id} session={session} />
                ))}
                {!byProject[project.id]?.length ? (
                  <p className="px-2 py-1 text-xs text-muted-foreground">还没有会话</p>
                ) : null}
              </SidebarMenu>
            </SidebarGroupContent> : null}
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
