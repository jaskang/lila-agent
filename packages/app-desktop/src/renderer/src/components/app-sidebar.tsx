import * as React from "react";
import {
  Bot,
  ChevronDown,
  ChevronRight,
  CircleAlert,
  FolderOpen,
  FolderPlus,
  MessageCircleQuestion,
  MoreHorizontal,
  Pencil,
  Plus,
  Search,
  SquarePen,
  Trash2,
  X,
} from "lucide-react";
import type { Project, SessionSummary } from "@lila/protocol";
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
  SidebarTrigger,
} from "@/components/ui/sidebar";
import { Spinner } from "@/components/ui/spinner";
import { actions, useLila } from "@/lib/store";

const RELATIVE = new Intl.RelativeTimeFormat("zh-CN", { numeric: "auto" });
const COLLAPSED_KEY = "lila.collapsedProjects";

function readCollapsed(): Record<string, boolean> {
  try {
    const parsed = JSON.parse(localStorage.getItem(COLLAPSED_KEY) ?? "null") as unknown;
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) return parsed as Record<string, boolean>;
  } catch {
    // Ignore invalid stored state.
  }
  return {};
}

function projectStatus(sessions: SessionSummary[]): SessionSummary["status"] {
  if (sessions.some((session) => session.status === "waiting")) return "waiting";
  if (sessions.some((session) => session.status === "running")) return "running";
  if (sessions.some((session) => session.status === "error")) return "error";
  return "idle";
}

function ago(iso: string, now: number): string {
  const minutes = Math.round((Date.parse(iso) - now) / 60_000);
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

/** Active (running/waiting/error) or currently-viewed sub-agent sessions, shown directly under their parent. */
function SubSessions({ sessions, selectedId }: { sessions: SessionSummary[]; selectedId: string | undefined }) {
  const visible = sessions.filter((session) => session.status !== "idle" || session.id === selectedId);
  if (!visible.length) return null;
  return (
    <SidebarMenuSub>
      {visible.map((session) => (
        <SidebarMenuSubItem key={session.id}>
          <SidebarMenuSubButton
            size="sm"
            isActive={session.id === selectedId}
            render={<button type="button" title={session.title} onClick={() => actions.select(session.id)} />}
          >
            <Bot />
            <span className="truncate">{session.title}</span>
            <StatusIcon status={session.status} />
          </SidebarMenuSubButton>
        </SidebarMenuSubItem>
      ))}
    </SidebarMenuSub>
  );
}

function SessionRow({
  session,
  active,
  subSessions,
  selectedId,
  now,
}: {
  session: SessionSummary;
  active: boolean;
  subSessions?: SessionSummary[] | undefined;
  selectedId: string | undefined;
  now: number;
}) {
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
          <span className="text-xs text-muted-foreground">{ago(session.updatedAt, now)}</span>
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

function ProjectGroup({
  project,
  list,
  allSessions,
  childrenByParent,
  selectedId,
  isCollapsed,
  now,
  onToggle,
}: {
  project: Project;
  list: SessionSummary[];
  allSessions: SessionSummary[];
  childrenByParent: Record<string, SessionSummary[]>;
  selectedId: string | undefined;
  isCollapsed: boolean;
  now: number;
  onToggle: () => void;
}) {
  const [removing, setRemoving] = React.useState(false);
  const status = projectStatus(allSessions);
  const busy = status === "running" || status === "waiting";
  return (
    <SidebarGroup className="group/project">
      <SidebarGroupLabel
        title={project.path}
        render={<button type="button" aria-expanded={!isCollapsed} onClick={onToggle} />}
        className="cursor-pointer gap-1 pr-12 hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
      >
        {isCollapsed ? <ChevronRight /> : <ChevronDown />}
        <span className="truncate">{project.name}</span>
        {isCollapsed && status !== "idle" ? <StatusIcon status={status} /> : null}
      </SidebarGroupLabel>
      <DropdownMenu>
        <DropdownMenuTrigger render={<SidebarGroupAction className="right-8 opacity-0 group-hover/project:opacity-100 focus-visible:opacity-100 data-popup-open:opacity-100" title="项目操作" aria-label={`${project.name} 的操作`} />}>
          <MoreHorizontal />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" side="right" className="w-40" finalFocus={false}>
          <DropdownMenuItem onClick={() => window.lila.revealPath(project.path)}>
            <FolderOpen />
            在访达中显示
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem variant="destructive" disabled={busy} onClick={() => setRemoving(true)}>
            <Trash2 />
            移除项目
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <SidebarGroupAction title={`在 ${project.name} 中新建会话`} onClick={() => actions.startNew(project.id)}>
        <Plus />
        <span className="sr-only">新建会话</span>
      </SidebarGroupAction>
      <AlertDialog open={removing} onOpenChange={setRemoving}>
        <AlertDialogContent size="sm">
          <AlertDialogHeader>
            <AlertDialogTitle>移除项目？</AlertDialogTitle>
            <AlertDialogDescription>只会将“{project.name}”从 Lila 列表移除，不会删除本地目录或历史会话文件。</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>取消</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                setRemoving(false);
                void actions.removeProject(project.id);
              }}
            >
              移除
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      {isCollapsed ? null : (
        <SidebarGroupContent>
          <SidebarMenu>
            {list.map((session) => (
              <SessionRow
                key={session.id}
                session={session}
                active={session.id === selectedId}
                subSessions={childrenByParent[session.id]}
                selectedId={selectedId}
                now={now}
              />
            ))}
            {!list.length ? <p className="px-2 py-1 text-xs text-muted-foreground">还没有会话</p> : null}
          </SidebarMenu>
        </SidebarGroupContent>
      )}
    </SidebarGroup>
  );
}

export function AppSidebar() {
  const projects = useLila((state) => state.projects);
  const sessions = useLila((state) => state.sessions);
  const selectedId = useLila((state) => state.selectedId);
  const [collapsed, setCollapsed] = React.useState<Record<string, boolean>>(readCollapsed);
  const [query, setQuery] = React.useState("");
  const [now, setNow] = React.useState(() => Date.now());
  const filter = query.trim().toLowerCase();

  React.useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, []);

  const byProject = React.useMemo(() => {
    const groups: Record<string, SessionSummary[]> = {};
    const allByProject: Record<string, SessionSummary[]> = {};
    const children: Record<string, SessionSummary[]> = {};
    for (const session of Object.values(sessions)) {
      (allByProject[session.projectId] ??= []).push(session);
      if (filter && !(session.title || "新会话").toLowerCase().includes(filter)) continue;
      // While filtering, matching sub-agent sessions show on their own.
      if (!filter && session.parentId && sessions[session.parentId]) (children[session.parentId] ??= []).push(session);
      else (groups[session.projectId] ??= []).push(session);
    }
    for (const list of [...Object.values(groups), ...Object.values(children)]) list.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
    return { groups, allByProject, children };
  }, [sessions, filter]);

  const toggle = (projectId: string) =>
    setCollapsed((current) => {
      const next = { ...current, [projectId]: !current[projectId] };
      localStorage.setItem(COLLAPSED_KEY, JSON.stringify(next));
      return next;
    });

  return (
    <Sidebar variant="inset" className="p-1.5">
      {/* 6px outer inset + 40px header centers at y = 26px, matching the macOS traffic lights and main card toolbar. */}
      <SidebarHeader className="mt-1.5 h-10 shrink-0 flex-row items-center gap-1 p-0 pr-1 pl-[74px] [-webkit-app-region:drag] md:mt-0">
        <SidebarTrigger className="[-webkit-app-region:no-drag]" title="切换侧边栏（⌘B）" />
      </SidebarHeader>
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
          <div className="relative">
            <SidebarInput
              value={query}
              placeholder="筛选会话…"
              aria-label="筛选会话"
              className={query ? "pr-7" : undefined}
              onChange={(event) => setQuery(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Escape" && query) {
                  event.preventDefault();
                  setQuery("");
                }
              }}
            />
            {query ? (
              <button
                type="button"
                aria-label="清空筛选"
                onClick={() => setQuery("")}
                className="absolute top-1/2 right-1.5 -translate-y-1/2 rounded-sm p-0.5 text-muted-foreground hover:bg-sidebar-accent hover:text-foreground"
              >
                <X className="size-3.5" />
              </button>
            ) : null}
          </div>
        </SidebarGroup>
        {projects.map((project) => {
          const list = byProject.groups[project.id] ?? [];
          if (filter && !list.length) return null;
          const isCollapsed = Boolean(collapsed[project.id]) && !filter;
          return (
            <ProjectGroup
              key={project.id}
              project={project}
              list={list}
              allSessions={byProject.allByProject[project.id] ?? []}
              childrenByParent={byProject.children}
              selectedId={selectedId}
              isCollapsed={isCollapsed}
              now={now}
              onToggle={() => toggle(project.id)}
            />
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
