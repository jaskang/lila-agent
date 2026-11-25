import * as React from "react";
import {
  ChevronDown,
  ChevronRight,
  CircleAlert,
  FolderOpen,
  FolderPlus,
  MessageCircleQuestion,
  Moon,
  MoreHorizontal,
  Pencil,
  Plus,
  Settings,
  SquarePen,
  Sun,
  Trash2,
} from "lucide-react";
import type { Project, SessionSummary } from "@lila/protocol";
import { SearchInput } from "@/components/search-input";
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
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
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
  SidebarTrigger,
  useSidebar,
} from "@/components/ui/sidebar";
import { Spinner } from "@/components/ui/spinner";
import { actions, useLila } from "@/lib/store";
import { useTheme } from "@/lib/theme";

const RELATIVE = new Intl.RelativeTimeFormat("zh-CN", { numeric: "auto" });
const COLLAPSED_KEY = "lila.collapsedProjects";
const PROJECT_PREVIEW_LIMIT = 5;

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
  const ts = Date.parse(iso);
  if (!Number.isFinite(ts)) return "";
  const minutes = Math.round((ts - now) / 60_000);
  if (Math.abs(minutes) < 1) return "刚刚";
  if (Math.abs(minutes) < 60) return RELATIVE.format(minutes, "minute");
  const hours = Math.round(minutes / 60);
  if (Math.abs(hours) < 24) return RELATIVE.format(hours, "hour");
  return RELATIVE.format(Math.round(hours / 24), "day");
}

function StatusIcon({ status }: { status: SessionSummary["status"] }) {
  if (status === "running") return <Spinner className="text-muted-foreground" />;
  if (status === "waiting") return <MessageCircleQuestion className="text-warning-foreground" />;
  if (status === "error") return <CircleAlert className="text-destructive-foreground" />;
  return null;
}

function SessionRow({
  session,
  active,
  now,
  onSelectSession,
}: {
  session: SessionSummary;
  active: boolean;
  now: number;
  onSelectSession: (sessionId: string) => void;
}) {
  const [editing, setEditing] = React.useState(false);
  const [confirming, setConfirming] = React.useState(false);
  const [title, setTitle] = React.useState(session.title);
  const inputRef = React.useRef<HTMLInputElement>(null);
  const savingRef = React.useRef(false);
  const busy = session.status === "running" || session.status === "waiting";

  const startEditing = () => {
    setTitle(session.title);
    setEditing(true);
  };

  React.useEffect(() => {
    if (!editing) return;
    requestAnimationFrame(() => {
      inputRef.current?.focus();
      inputRef.current?.select();
    });
  }, [editing]);

  async function save(): Promise<void> {
    if (savingRef.current) return;
    savingRef.current = true;
    const currentTitle = session.title;
    const next = title.trim();
    try {
      if (!next || next === currentTitle) {
        setTitle(currentTitle);
        setEditing(false);
        return;
      }
      const ok = await actions.rename(session.id, next);
      if (!ok) setTitle(currentTitle);
      setEditing(false);
    } finally {
      savingRef.current = false;
    }
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
            if (event.key === "Enter") {
              event.preventDefault();
              void save();
            } else if (event.key === "Escape") {
              event.preventDefault();
              setTitle(session.title);
              setEditing(false);
            }
          }}
        />
      </SidebarMenuItem>
    );
  }

  return (
    <SidebarMenuItem>
      <SidebarMenuButton isActive={active} onClick={() => onSelectSession(session.id)} className="h-auto py-1.5">
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="truncate" title={session.title || "新会话"}>
            {session.title || "新会话"}
          </span>
          <span className="text-xs text-muted-foreground">{ago(session.updatedAt, now)}</span>
        </div>
        <StatusIcon status={session.status} />
      </SidebarMenuButton>
      <DropdownMenu>
        <DropdownMenuTrigger
          render={
            <SidebarMenuAction
              showOnHover
              className="peer-data-[size=default]/menu-button:top-1/2 -translate-y-1/2 data-popup-open:opacity-100"
              aria-label="更多操作"
            />
          }
        >
          <MoreHorizontal />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" side="right" className="w-36" finalFocus={false}>
          {!session.readOnly ? (
            <>
              <DropdownMenuItem onClick={startEditing}>
                <Pencil />
                重命名
              </DropdownMenuItem>
              <DropdownMenuSeparator />
            </>
          ) : null}
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
            <AlertDialogDescription>将删除「{session.title || "新会话"}」对应的会话文件。</AlertDialogDescription>
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
    </SidebarMenuItem>
  );
}

function ProjectGroup({
  project,
  list,
  allSessions,
  selectedId,
  isCollapsed,
  now,
  onToggle,
  onSelectSession,
  onStartNew,
}: {
  project: Project;
  list: SessionSummary[];
  allSessions: SessionSummary[];
  selectedId: string | undefined;
  isCollapsed: boolean;
  now: number;
  onToggle: () => void;
  onSelectSession: (sessionId: string) => void;
  onStartNew: (projectId: string) => void;
}) {
  const [removing, setRemoving] = React.useState(false);
  const [showAll, setShowAll] = React.useState(false);
  const status = projectStatus(allSessions);
  const busy = status === "running" || status === "waiting";
  const visibleList = React.useMemo(() => {
    if (showAll || list.length <= PROJECT_PREVIEW_LIMIT) return list;
    const head = list.slice(0, PROJECT_PREVIEW_LIMIT);
    if (selectedId && !head.some((s) => s.id === selectedId)) {
      const activeSession = list.find((s) => s.id === selectedId);
      if (activeSession) return [...head, activeSession];
    }
    return head;
  }, [list, showAll, selectedId]);
  const hiddenCount = Math.max(0, list.length - visibleList.length);

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
        <DropdownMenuTrigger
          render={
            <SidebarGroupAction
              className="right-8 opacity-0 group-hover/project:opacity-100 focus-visible:opacity-100 data-popup-open:opacity-100"
              title="项目操作"
              aria-label={`${project.name} 的操作`}
            />
          }
        >
          <MoreHorizontal />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" side="right" className="w-40" finalFocus={false}>
          <DropdownMenuItem onClick={() => void actions.revealPath(project.path)}>
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
      <SidebarGroupAction title={`在 ${project.name} 中新建会话`} onClick={() => onStartNew(project.id)}>
        <Plus />
        <span className="sr-only">新建会话</span>
      </SidebarGroupAction>
      <AlertDialog open={removing} onOpenChange={setRemoving}>
        <AlertDialogContent size="sm">
          <AlertDialogHeader>
            <AlertDialogTitle>移除项目？</AlertDialogTitle>
            <AlertDialogDescription>仅从列表移除「{project.name}」，保留本地文件。</AlertDialogDescription>
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
      {isCollapsed || !list.length ? null : (
        <SidebarGroupContent>
          <SidebarMenu>
            {visibleList.map((session) => (
              <SessionRow
                key={session.id}
                session={session}
                active={session.id === selectedId}
                now={now}
                onSelectSession={onSelectSession}
              />
            ))}
            {hiddenCount > 0 ? (
              <SidebarMenuItem>
                <Button
                  type="button"
                  size="xs"
                  variant="ghost"
                  className="h-7 w-full justify-start px-2 text-xs font-normal text-muted-foreground hover:text-foreground"
                  onClick={() => setShowAll(true)}
                >
                  展开其余 {hiddenCount} 条
                </Button>
              </SidebarMenuItem>
            ) : showAll && list.length > PROJECT_PREVIEW_LIMIT ? (
              <SidebarMenuItem>
                <Button
                  type="button"
                  size="xs"
                  variant="ghost"
                  className="h-7 w-full justify-start px-2 text-xs font-normal text-muted-foreground hover:text-foreground"
                  onClick={() => setShowAll(false)}
                >
                  收起
                </Button>
              </SidebarMenuItem>
            ) : null}
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
  const { setOpenMobile } = useSidebar();
  const [collapsed, setCollapsed] = React.useState<Record<string, boolean>>(readCollapsed);
  const [query, setQuery] = React.useState("");
  const [now, setNow] = React.useState(() => Date.now());
  const filter = query.trim().toLowerCase();

  React.useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, []);

  const handleSelectSession = React.useCallback(
    (sessionId: string) => {
      actions.select(sessionId);
      setOpenMobile(false);
    },
    [setOpenMobile],
  );

  const handleStartNew = React.useCallback(
    (projectId?: string) => {
      actions.startNew(projectId);
      setOpenMobile(false);
    },
    [setOpenMobile],
  );

  const projectNames = React.useMemo(() => new Map(projects.map((p) => [p.id, p.name.toLowerCase()])), [projects]);

  const { groups, allByProject } = React.useMemo(() => {
    const groups: Record<string, SessionSummary[]> = {};
    const allByProject: Record<string, SessionSummary[]> = {};
    for (const session of Object.values(sessions)) {
      if (session.parentId) continue;
      (allByProject[session.projectId] ??= []).push(session);
      if (filter) {
        const titleMatch = (session.title || "新会话").toLowerCase().includes(filter);
        const projMatch = projectNames.get(session.projectId)?.includes(filter) ?? false;
        if (!titleMatch && !projMatch) continue;
      }
      (groups[session.projectId] ??= []).push(session);
    }
    for (const list of Object.values(groups)) list.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
    return { groups, allByProject };
  }, [sessions, filter, projectNames]);

  const filtering = Boolean(filter);

  const toggle = (projectId: string) =>
    setCollapsed((current) => {
      const next = { ...current, [projectId]: !current[projectId] };
      try {
        localStorage.setItem(COLLAPSED_KEY, JSON.stringify(next));
      } catch {
        // Ignore storage errors.
      }
      return next;
    });

  return (
    <Sidebar variant="inset" className="p-1.5">
      <SidebarHeader className="mt-1.5 h-11 shrink-0 flex-row items-center gap-1 p-0 pr-1 pl-[74px] [-webkit-app-region:drag] md:mt-0">
        <SidebarTrigger className="[-webkit-app-region:no-drag]" title="切换侧边栏（⌘B）" />
      </SidebarHeader>
      <SidebarContent>
        <SidebarGroup className="gap-2">
          <SidebarMenu>
            <SidebarMenuItem>
              <SidebarMenuButton title="新会话（⌘N）" onClick={() => handleStartNew()}>
                <SquarePen />
                新会话
              </SidebarMenuButton>
            </SidebarMenuItem>
          </SidebarMenu>
          <SearchInput variant="sidebar" value={query} onChange={setQuery} placeholder="搜索" ariaLabel="搜索项目或会话" />
        </SidebarGroup>
        {projects.map((project) => {
          const list = groups[project.id] ?? [];
          const projectMatched = filtering && project.name.toLowerCase().includes(filter);
          if (filtering && !list.length && !projectMatched) return null;
          const isCollapsed = Boolean(collapsed[project.id]) && !filtering;
          return (
            <ProjectGroup
              key={project.id}
              project={project}
              list={list}
              allSessions={allByProject[project.id] ?? []}
              selectedId={selectedId}
              isCollapsed={isCollapsed}
              now={now}
              onToggle={() => toggle(project.id)}
              onSelectSession={handleSelectSession}
              onStartNew={handleStartNew}
            />
          );
        })}
      </SidebarContent>
      <SidebarFooter>
        <div className="flex items-center gap-1">
          <SidebarMenu className="min-w-0 flex-1">
            <SidebarMenuItem>
              <SidebarMenuButton onClick={() => void actions.addProject()}>
                <FolderPlus />
                添加项目
              </SidebarMenuButton>
            </SidebarMenuItem>
          </SidebarMenu>
          <ThemeSwitcher />
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label="设置"
            title="设置（⌘,）"
            onClick={() => actions.openSettings()}
            className="shrink-0 text-sidebar-foreground hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
          >
            <Settings />
          </Button>
        </div>
      </SidebarFooter>
    </Sidebar>
  );
}

function ThemeSwitcher() {
  const { resolvedTheme, setTheme } = useTheme();
  const isDark = resolvedTheme === "dark";
  const label = isDark ? "切换到浅色模式" : "切换到暗黑模式";

  return (
    <Button
      type="button"
      variant="ghost"
      size="icon-sm"
      aria-label={label}
      title={label}
      onClick={() => setTheme(isDark ? "light" : "dark")}
      className="shrink-0 text-sidebar-foreground hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
    >
      {isDark ? <Sun /> : <Moon />}
    </Button>
  );
}
