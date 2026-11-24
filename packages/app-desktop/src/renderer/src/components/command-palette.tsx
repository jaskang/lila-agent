import * as React from "react";
import { Blocks, Check, CircleAlert, FolderPlus, FolderTree, GitBranch, MessageCircleQuestion, MessageSquare, Minimize2, Monitor, Moon, PanelLeft, RotateCw, Square, SquarePen, Sun } from "lucide-react";
import { CommandDialog, Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList, CommandShortcut } from "@/components/ui/command";
import { useSidebar } from "@/components/ui/sidebar";
import { Spinner } from "@/components/ui/spinner";
import { actions, useLila } from "@/lib/store";
import { useTheme } from "@/lib/theme";

export function CommandPalette() {
  const open = useLila((state) => state.paletteOpen);
  const sessions = useLila((state) => state.sessions);
  const projects = useLila((state) => state.projects);
  const selectedId = useLila((state) => state.selectedId);
  const activeState = useLila((state) => (state.selectedId ? state.views[state.selectedId]?.state : undefined));
  const { toggleSidebar } = useSidebar();
  const { theme, setTheme } = useTheme();
  const projectName = React.useMemo(() => new Map(projects.map((project) => [project.id, project.name])), [projects]);
  const sorted = React.useMemo(() => Object.values(sessions).sort((a, b) => b.updatedAt.localeCompare(a.updatedAt)), [sessions]);
  const ready = activeState?.access === "ready";
  const canStop = ready && (activeState.runActive || activeState.compacting);

  const run = (action: () => void) => {
    actions.setPaletteOpen(false);
    action();
  };

  return (
    <CommandDialog open={open} onOpenChange={actions.setPaletteOpen} title="命令面板" description="切换会话或执行命令">
      <Command>
        <CommandInput placeholder="搜索会话、项目或命令…" />
        <CommandList>
          <CommandEmpty>没有匹配的结果</CommandEmpty>
          {selectedId ? (
            <CommandGroup heading="当前会话">
              <CommandItem value="session-files 文件 目录树 浏览 files" onSelect={() => run(() => actions.openRightPanel("files"))}>
                <FolderTree />
                浏览项目文件
                <CommandShortcut>⌘E</CommandShortcut>
              </CommandItem>
              <CommandItem value="session-git 改动 diff git 工作区" onSelect={() => run(() => actions.openRightPanel("changes"))}>
                <GitBranch />
                查看工作区 Git 改动
                <CommandShortcut>⇧⌘G</CommandShortcut>
              </CommandItem>
              {ready ? (
                <CommandItem value="session-resources 能力 技能 MCP 扩展" onSelect={() => run(() => actions.setResourcesOpen(true))}>
                  <Blocks />
                  打开能力面板（技能 / MCP / 扩展）
                </CommandItem>
              ) : null}
              {canStop ? (
                <CommandItem value="session-abort 停止 中断" onSelect={() => run(() => void actions.abort(selectedId))}>
                  <Square />
                  停止当前任务
                </CommandItem>
              ) : ready ? (
                <>
                  <CommandItem value="session-compact 压缩上下文" onSelect={() => run(() => void actions.compact(selectedId))}>
                    <Minimize2 />
                    压缩上下文
                  </CommandItem>
                  <CommandItem value="session-reload 重新加载 技能 MCP 扩展" onSelect={() => run(() => void actions.reload(selectedId))}>
                    <RotateCw />
                    重新加载技能、扩展和 MCP
                  </CommandItem>
                </>
              ) : null}
            </CommandGroup>
          ) : null}
          <CommandGroup heading="命令">
            <CommandItem value="new-session 新会话" onSelect={() => run(() => actions.startNew())}>
              <SquarePen />
              新会话
              <CommandShortcut>⌘N</CommandShortcut>
            </CommandItem>
            <CommandItem value="toggle-sidebar 切换侧边栏 折叠 展开" onSelect={() => run(toggleSidebar)}>
              <PanelLeft />
              切换侧边栏
              <CommandShortcut>⌘B</CommandShortcut>
            </CommandItem>
            {projects.map((project) => (
              <CommandItem key={project.id} value={`new-in ${project.id} 在 ${project.name} 中新建会话`} onSelect={() => run(() => actions.startNew(project.id))}>
                <SquarePen />
                在 {project.name} 中新建会话
              </CommandItem>
            ))}
            <CommandItem value="add-project 添加项目" onSelect={() => run(() => void actions.addProject())}>
              <FolderPlus />
              添加项目…
            </CommandItem>
            <CommandItem value="theme-light 浅色模式 外观 主题 light" onSelect={() => run(() => setTheme("light"))}>
              <Sun />
              切换到浅色模式
              {theme === "light" ? <Check className="ms-auto size-4 text-muted-foreground" /> : null}
            </CommandItem>
            <CommandItem value="theme-dark 暗黑模式 深色模式 外观 主题 dark" onSelect={() => run(() => setTheme("dark"))}>
              <Moon />
              切换到暗黑模式
              {theme === "dark" ? <Check className="ms-auto size-4 text-muted-foreground" /> : null}
            </CommandItem>
            <CommandItem value="theme-system 跟随系统 外观 主题 system" onSelect={() => run(() => setTheme("system"))}>
              <Monitor />
              外观跟随系统
              {theme === "system" ? <Check className="ms-auto size-4 text-muted-foreground" /> : null}
            </CommandItem>
          </CommandGroup>
          {sorted.length ? (
            <CommandGroup heading="会话">
              {sorted.map((session) => (
                <CommandItem
                  key={session.id}
                  value={`session ${session.id} ${session.title || "新会话"} ${projectName.get(session.projectId) ?? ""}`}
                  onSelect={() => run(() => actions.select(session.id))}
                >
                  {session.status === "running" ? (
                    <Spinner />
                  ) : session.status === "waiting" ? (
                    <MessageCircleQuestion className="text-warning-foreground" />
                  ) : session.status === "error" ? (
                    <CircleAlert className="text-destructive" />
                  ) : (
                    <MessageSquare />
                  )}
                  <span className="min-w-0 truncate">{session.title || "新会话"}</span>
                  <CommandShortcut className="tracking-normal">{projectName.get(session.projectId)}</CommandShortcut>
                </CommandItem>
              ))}
            </CommandGroup>
          ) : null}
        </CommandList>
      </Command>
    </CommandDialog>
  );
}
