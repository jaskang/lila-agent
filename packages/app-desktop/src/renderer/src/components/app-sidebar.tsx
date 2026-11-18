import * as React from "react";
import { Bot, ChevronDown, ChevronRight, CircleAlert, FolderPlus, MessageCircleQuestion, MoreHorizontal, Pencil, Plus, Search, SquarePen, Trash2 } from "lucide-react";
import type { SessionSummary } from "@lila/protocol";
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
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Kbd } from "@/components/ui/kbd";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupAction,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarInput,
  SidebarMenu,
  SidebarMenuAction,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarMenuSub,
  SidebarMenuSubButton,
  SidebarMenuSubItem,
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

function StatusIcon({ status }: { status: SessionSummary["status"] }) {
  if (status === "running") return <Spinner className="text-muted-foreground" />;
  if (status === "waiting") return <MessageCircleQuestion className="text-amber-600 dark:text-amber-400" />;
  if (status === "error") return <CircleAlert className="text-destructive" />;
  return null;
}

/** Sessions a sub-agent ran, folded under the session that spawned them, as Pi's `/resume` shows them. */
function SubSessions({ sessions, selectedId }: { sessions: SessionSummary[]; selectedId: string | undefined }) {
  const [open, setOpen] = React.useState(false);
  const expanded = open || sessions.some((session) => session.id === selectedId || session.status !== "idle");
  return (
    <SidebarMenuSub>
      <SidebarMenuSubItem>
        <SidebarMenuSubButton size="sm" render={<button type="button" aria-expanded={expanded} onClick={() => setOpen(!expanded)} />}>
          {expanded ? <ChevronDown /> : <ChevronRight />}
          <span className="text-muted-foreground">{sessions.length} 个子 Agent</span>
        </SidebarMenuSubButton>
      </SidebarMenuSubItem>
      {expanded
        ? sessions.map((session) => (
            <SidebarMenuSubItem key={session.id}>
              <SidebarMenuSubButton
                size="sm"
                isActive={session.id === selectedId}
                render={<button type="button" onClick={() => actions.select(session.id)} />}
              >
                <Bot />
                <span className="truncate">{session.title}</span>
                <StatusIcon status={session.status} />
              </SidebarMenuSubButton>
            </SidebarMenuSubItem>
          ))
        : null}
    </SidebarMenuSub>
  );
}

function SessionRow({ session, active, subSessions, selectedId }: { session: SessionSummary; active: boolean; subSessions?: SessionSummary[] | undefined; selectedId: string | undefined }) {
  const [editing, setEditing] = React.useState(false);
  const [confirming, setConfirming] = React.useState(false);
  const [title, setTitle] = React.useState(session.title);
  const inputRef = React.useRef<HTMLInputElement>(null);
  const busy = session.status === "running" || session.status === "waiting";

  React.useEffect(() => {
    if (!editing) return;
    setTitle(session.title);
    requestAnimationFrame(() => {
      inputRef.current?.focus();
      inputRef.current?.select();
    });
  }, [editing, session.title]);

  async function save(): Promise<void> {
    const next = title.trim();
    if (!next || next === session.title || (await actions.rename(session.id, next))) setEditing(false);
  }

  if (editing) {
    return (
      <SidebarMenuItem>
        <Input
          ref={inputRef}
          value={title}
          maxLength={120}
          aria-label="会话名称"
          className="h-8 bg-background"
          onChange={(event) => setTitle(event.target.value)}
          onBlur={() => void save()}
          onKeyDown={(event) => {
            if (event.nativeEvent.isComposing) return;
            if (event.key === "Enter") void save();
            if (event.key === "Escape") setEditing(false);
          }}
        />
      </SidebarMenuItem>
    );
  }

  return (
    <SidebarMenuItem>
      <SidebarMenuButton isActive={active} onClick={() => actions.select(session.id)} className="h-auto py-1.5 pr-8">
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="truncate">{session.title || "新会话"}</span>
          <span className="text-xs text-muted-foreground">{ago(session.updatedAt)}</span>
        </div>
        <StatusIcon status={session.status} />
      </SidebarMenuButton>
      <DropdownMenu>
        <DropdownMenuTrigger render={<SidebarMenuAction showOnHover className="top-2.5" aria-label="更多操作" />}>
          <MoreHorizontal />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" side="right" className="w-36" finalFocus={false}>
          <DropdownMenuItem onClick={() => setEditing(true)}>
            <Pencil />
            重命名
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem variant="destructive" disabled={busy} onClick={() => setConfirming(true)}>
            <Trash2 />
            删除
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <AlertDialog open={confirming} onOpenChange={setConfirming}>
        <AlertDialogContent size="sm">
          <AlertDialogHeader>
            <AlertDialogTitle>删除会话？</AlertDialogTitle>
            <AlertDialogDescription>“{session.title || "新会话"}”的会话文件会被删除，pi 命令行里也会看不到它。</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>取消</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                setConfirming(false);
                void actions.remove(session.id);
              }}
            >
              删除
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      {subSessions?.length ? <SubSessions sessions={subSessions} selectedId={selectedId} /> : null}
    </SidebarMenuItem>
  );
}

export function AppSidebar() {
  const projects = useLila((state) => state.projects);
  const sessions = useLila((state) => state.sessions);
  const selectedId = useLila((state) => state.selectedId);
  const [collapsed, setCollapsed] = React.useState<Record<string, boolean>>({});
  const [query, setQuery] = React.useState("");
  const filter = query.trim().toLowerCase();

  const byProject = React.useMemo(() => {
    const groups: Record<string, SessionSummary[]> = {};
    const children: Record<string, SessionSummary[]> = {};
    for (const session of Object.values(sessions)) {
      if (filter && !(session.title || "新会话").toLowerCase().includes(filter)) continue;
      // While filtering, matching sub-agent sessions show on their own.
      if (!filter && session.parentId && sessions[session.parentId]) (children[session.parentId] ??= []).push(session);
      else (groups[session.projectId] ??= []).push(session);
    }
    for (const list of [...Object.values(groups), ...Object.values(children)]) list.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
    return { groups, children };
  }, [sessions, filter]);

  const toggle = (projectId: string) => setCollapsed((current) => ({ ...current, [projectId]: !current[projectId] }));

  return (
    <Sidebar variant="inset">
      {/* Leaves room for the macOS traffic lights; the strip also drags the window. */}
      <SidebarHeader className="h-12 [-webkit-app-region:drag]" />
      <SidebarContent>
        <SidebarGroup className="gap-2">
          <SidebarMenu>
            <SidebarMenuItem>
              <SidebarMenuButton onClick={() => actions.startNew()}>
                <SquarePen />
                新会话
                <Kbd className="ms-auto">⌘N</Kbd>
              </SidebarMenuButton>
            </SidebarMenuItem>
            <SidebarMenuItem>
              <SidebarMenuButton onClick={() => actions.setPaletteOpen(true)}>
                <Search />
                搜索
                <Kbd className="ms-auto">⌘K</Kbd>
              </SidebarMenuButton>
            </SidebarMenuItem>
          </SidebarMenu>
          <SidebarInput value={query} placeholder="筛选会话…" aria-label="筛选会话" onChange={(event) => setQuery(event.target.value)} />
        </SidebarGroup>
        {projects.map((project) => {
          const list = byProject.groups[project.id] ?? [];
          if (filter && !list.length) return null;
          const isCollapsed = Boolean(collapsed[project.id]) && !filter;
          return (
            <SidebarGroup key={project.id}>
              <SidebarGroupLabel
                title={project.path}
                render={<button type="button" aria-expanded={!isCollapsed} onClick={() => toggle(project.id)} />}
                className="cursor-pointer gap-1 hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
              >
                {isCollapsed ? <ChevronRight /> : <ChevronDown />}
                <span className="truncate">{project.name}</span>
              </SidebarGroupLabel>
              <SidebarGroupAction title={`在 ${project.name} 中新建会话`} onClick={() => actions.startNew(project.id)}>
                <Plus />
                <span className="sr-only">新建会话</span>
              </SidebarGroupAction>
              {isCollapsed ? null : (
                <SidebarGroupContent>
                  <SidebarMenu>
                    {list.map((session) => (
                      <SessionRow
                        key={session.id}
                        session={session}
                        active={session.id === selectedId}
                        subSessions={byProject.children[session.id]}
                        selectedId={selectedId}
                      />
                    ))}
                    {!list.length ? <p className="px-2 py-1 text-xs text-muted-foreground">还没有会话</p> : null}
                  </SidebarMenu>
                </SidebarGroupContent>
              )}
            </SidebarGroup>
          );
        })}
        {filter && !Object.keys(byProject.groups).length ? <p className="px-4 py-2 text-xs text-muted-foreground">没有匹配的会话</p> : null}
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
