import * as React from "react";
import {
  BarChart3,
  Blocks,
  Brain,
  Check,
  CircleAlert,
  Clock,
  Copy,
  FolderOpen,
  FolderPlus,
  FolderTree,
  GitBranch,
  GitBranchPlus,
  Inbox,
  KeyRound,
  MessageCircleQuestion,
  MessageSquare,
  Minimize2,
  Monitor,
  Moon,
  PanelLeft,
  PanelRightClose,
  Plug,
  RotateCw,
  Search,
  Settings,
  ShieldCheck,
  Square,
  SquarePen,
  Sun,
} from "lucide-react";
import { toast } from "sonner";
import type { ApprovalMode } from "@lila/protocol";
import { CommandDialog, Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList, CommandShortcut } from "@/components/ui/command";
import { useSidebar } from "@/components/ui/sidebar";
import { Spinner } from "@/components/ui/spinner";
import { actions, APPROVAL_LABEL, useLila } from "@/lib/store";
import { useTheme } from "@/lib/theme";

const APPROVAL_MODES = Object.keys(APPROVAL_LABEL) as ApprovalMode[];

export function CommandPalette() {
  const open = useLila((state) => state.paletteOpen);
  const sessions = useLila((state) => state.sessions);
  const projects = useLila((state) => state.projects);
  const selectedId = useLila((state) => state.selectedId);
  const unreadCompletedIds = useLila((state) => state.unreadCompletedIds);
  const rightPanel = useLila((state) => state.rightPanel);
  const activeView = useLila((state) => (state.selectedId ? state.views[state.selectedId] : undefined));
  const activeState = activeView?.state;
  const activeSession = selectedId ? sessions[selectedId] : undefined;
  const activeProject = activeSession ? projects.find((p) => p.id === activeSession.projectId) : undefined;
  const { toggleSidebar } = useSidebar();
  const { theme, setTheme } = useTheme();
  const projectName = React.useMemo(() => new Map(projects.map((project) => [project.id, project.name])), [projects]);
  // Exclude read-only sub-agent sessions from main command palette list.
  const sorted = React.useMemo(
    () =>
      Object.values(sessions)
        .filter((session) => !session.parentId && !session.readOnly)
        .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt)),
    [sessions],
  );
  const nextAttention = React.useMemo(() => {
    const candidates = sorted.filter(
      (session) =>
        session.id !== selectedId &&
        (session.status === "waiting" || session.status === "error" || Boolean(unreadCompletedIds[session.id])),
    );
    return candidates.find((s) => s.status === "waiting") ?? candidates.find((s) => s.status === "error") ?? candidates[0];
  }, [sorted, selectedId, unreadCompletedIds]);
  const ready = activeState?.access === "ready";
  const failed = activeState?.access === "failed";
  const canStop = ready && (activeState.runActive || activeState.compacting);

  const lastAssistantText = React.useMemo(() => {
    const items = activeView?.items;
    if (!items?.length) return undefined;
    for (let i = items.length - 1; i >= 0; i--) {
      const item = items[i];
      if (item?.kind === "assistant" && item.text.trim()) return item.text;
    }
    return undefined;
  }, [activeView?.items]);

  const run = (action: () => void) => {
    actions.setPaletteOpen(false);
    action();
  };

  return (
    <CommandDialog open={open} onOpenChange={actions.setPaletteOpen} title="命令面板" description="切换会话或执行命令">
      <Command label="搜索会话、项目或命令">
        <CommandInput placeholder="搜索会话、项目或命令（支持中英文/拼音）…" />
        <CommandList>
          <CommandEmpty>没有匹配的结果</CommandEmpty>
          {selectedId ? (
            <CommandGroup heading="当前会话">
              <CommandItem
                value="session-search 查找 搜索 会话内容 chazhao sousuo find"
                onSelect={() => run(() => window.dispatchEvent(new CustomEvent("lila:open-search")))}
              >
                <Search />
                查找会话内容
                <CommandShortcut>⌘F</CommandShortcut>
              </CommandItem>
              <CommandItem value="session-files 文件 目录树 浏览 wenjian files" onSelect={() => run(() => actions.openRightPanel("files"))}>
                <FolderTree />
                浏览项目文件
                <CommandShortcut>⌘E</CommandShortcut>
              </CommandItem>
              <CommandItem value="session-git 改动 diff git 工作区 gaidong" onSelect={() => run(() => actions.openRightPanel("changes"))}>
                <GitBranch />
                查看工作区 Git 改动
                <CommandShortcut>⇧⌘G</CommandShortcut>
              </CommandItem>
              {rightPanel ? (
                <CommandItem value="close-right-panel 关闭右侧面板 guanbi" onSelect={() => run(() => actions.closeRightPanel())}>
                  <PanelRightClose />
                  关闭右侧面板
                </CommandItem>
              ) : null}
              {activeProject ? (
                <>
                  <CommandItem
                    value={`new-worktree 新建隔离 worktree 分支 并行 fenzhi bingxing ${activeProject.name}`}
                    onSelect={() =>
                      run(() => {
                        void actions.createWorktreeProject(activeProject.id).then((created) => {
                          if (created) actions.startNew(created.project.id);
                        });
                      })
                    }
                  >
                    <GitBranchPlus />
                    从当前项目新建隔离 Worktree
                  </CommandItem>
                  <CommandItem value={`reveal-project 打开项目目录 访达 finder dakai ${activeProject.name}`} onSelect={() => run(() => void actions.revealPath(activeProject.path))}>
                    <FolderOpen />
                    在文件夹中显示当前项目
                  </CommandItem>
                </>
              ) : null}
              {lastAssistantText ? (
                <CommandItem
                  value="copy-last-reply 复制最后一条回复 fuzhi copy"
                  onSelect={() =>
                    run(() => {
                      void navigator.clipboard
                        .writeText(lastAssistantText)
                        .then(() => toast.success("已复制最后一条回复"))
                        .catch(() => toast.error("复制失败"));
                    })
                  }
                >
                  <Copy />
                  复制最后一条助手回复
                </CommandItem>
              ) : null}
              {failed ? (
                <CommandItem value="session-retry 重试初始化 chongshi retry" onSelect={() => run(() => void actions.retrySession(selectedId))}>
                  <RotateCw />
                  重试初始化会话
                </CommandItem>
              ) : null}
              {canStop ? (
                <CommandItem value="session-abort 停止 中断 tingzhi stop" onSelect={() => run(() => void actions.abort(selectedId))}>
                  <Square />
                  停止当前任务
                </CommandItem>
              ) : ready ? (
                <>
                  <CommandItem value="session-compact 压缩上下文 yasuo compact" onSelect={() => run(() => void actions.compact(selectedId))}>
                    <Minimize2 />
                    压缩上下文
                  </CommandItem>
                  <CommandItem value="session-reload 重新加载 技能 MCP 扩展 chongxinjiazai reload" onSelect={() => run(() => void actions.reload(selectedId))}>
                    <RotateCw />
                    重新加载技能、扩展和 MCP
                  </CommandItem>
                  {APPROVAL_MODES.map((mode) => (
                    <CommandItem
                      key={mode}
                      value={`approval ${mode} 审批模式 shenpi ${APPROVAL_LABEL[mode]}`}
                      onSelect={() => run(() => void actions.configure(selectedId, { approvalMode: mode }))}
                    >
                      <ShieldCheck />
                      审批方式：{APPROVAL_LABEL[mode]}
                      {activeState.approvalMode === mode ? <Check className="ms-auto size-4 text-muted-foreground" /> : null}
                    </CommandItem>
                  ))}
                </>
              ) : null}
            </CommandGroup>
          ) : null}
          <CommandGroup heading="命令">
            {nextAttention ? (
              <CommandItem
                value={`next-attention 待处理 需要关注 审批 跳转 shenpi guanzhu ${nextAttention.title}`}
                onSelect={() => run(() => actions.select(nextAttention.id))}
              >
                <MessageCircleQuestion className="text-warning-foreground" />
                跳到待关注会话：{nextAttention.title || "新会话"}
              </CommandItem>
            ) : null}
            <CommandItem value="new-session 新会话 xinhuihua xhh" onSelect={() => run(() => actions.startNew())}>
              <SquarePen />
              新会话
              <CommandShortcut>⌘N</CommandShortcut>
            </CommandItem>
            <CommandItem
              value="automation-tasks 任务收件箱 待办 需求 renwu inbox tasks"
              onSelect={() => run(() => actions.openAutomation("tasks"))}
            >
              <Inbox />
              打开任务收件箱
              <CommandShortcut>⇧⌘J</CommandShortcut>
            </CommandItem>
            <CommandItem
              value="automation-task-new 快速收集 新建任务 微信 whatsapp 设计稿 xinjianrenwu capture"
              onSelect={() => run(() => actions.openAutomation("tasks", true))}
            >
              <Inbox />
              快速收集新任务
            </CommandItem>
            <CommandItem
              value="automation-schedules 定时任务 调度 cron 心跳 dingshi schedule"
              onSelect={() => run(() => actions.openAutomation("schedules"))}
            >
              <Clock />
              管理定时任务
            </CommandItem>
            <CommandItem
              value="automation-schedule-new 新建定时任务 cron 周期 xinjiandingshi"
              onSelect={() => run(() => actions.openAutomation("schedules", true))}
            >
              <Clock />
              新建定时任务
            </CommandItem>
            <CommandItem value="toggle-sidebar 切换侧边栏 折叠 展开 cebianlan" onSelect={() => run(toggleSidebar)}>
              <PanelLeft />
              切换侧边栏
              <CommandShortcut>⌘B</CommandShortcut>
            </CommandItem>
            {projects.map((project) => (
              <CommandItem key={project.id} value={`new-in 在 ${project.name} 中新建会话 xinhuihua`} onSelect={() => run(() => actions.startNew(project.id))}>
                <SquarePen />
                在 {project.name} 中新建会话
              </CommandItem>
            ))}
            <CommandItem value="open-settings 设置 偏好 配置 shezhi sz settings preferences" onSelect={() => run(() => actions.openSettings())}>
              <Settings />
              打开设置
              <CommandShortcut>⌘,</CommandShortcut>
            </CommandItem>
            <CommandItem value="provider-settings 模型 服务商 登录 moxing fuwushang api key oauth login" onSelect={() => run(() => actions.openSettings("providers"))}>
              <KeyRound />
              配置模型与服务商
            </CommandItem>
            <CommandItem value="usage-settings 用量 额度 余额 token 花费 配额 yongliang edu usage quota balance" onSelect={() => run(() => actions.openSettings("usage"))}>
              <BarChart3 />
              查看模型与服务商用量
            </CommandItem>
            <CommandItem value="mcp-settings MCP 服务器 工具 插件" onSelect={() => run(() => actions.openSettings("mcp"))}>
              <Plug />
              管理 MCP 服务器
            </CommandItem>
            <CommandItem value="memory-settings 长期记忆 记忆 偏好 画像 jiyi memory user" onSelect={() => run(() => actions.openSettings("memory"))}>
              <Brain />
              管理长期记忆
            </CommandItem>
            <CommandItem value="capabilities-settings 能力 技能 扩展 提示模板 包 jineng kuozhan skills extensions" onSelect={() => run(() => actions.openSettings("capabilities"))}>
              <Blocks />
              管理技能与扩展
            </CommandItem>
            <CommandItem value="add-project 添加项目 tianjiaxiangmu" onSelect={() => run(() => void actions.addProject())}>
              <FolderPlus />
              添加项目…
            </CommandItem>
            <CommandItem value="theme-light 浅色模式 外观 主题 qianse light" onSelect={() => run(() => setTheme("light"))}>
              <Sun />
              切换到浅色模式
              {theme === "light" ? <Check className="ms-auto size-4 text-muted-foreground" /> : null}
            </CommandItem>
            <CommandItem value="theme-dark 暗黑模式 深色模式 外观 主题 anhei shense dark" onSelect={() => run(() => setTheme("dark"))}>
              <Moon />
              切换到暗黑模式
              {theme === "dark" ? <Check className="ms-auto size-4 text-muted-foreground" /> : null}
            </CommandItem>
            <CommandItem value="theme-system 跟随系统 外观 主题 gensui system" onSelect={() => run(() => setTheme("system"))}>
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
                  value={`${session.title || "新会话"} ${projectName.get(session.projectId) ?? ""}`}
                  onSelect={() => run(() => actions.select(session.id))}
                >
                  {session.status === "running" ? (
                    <Spinner />
                  ) : session.status === "waiting" ? (
                    <MessageCircleQuestion className="text-warning-foreground" />
                  ) : session.status === "error" ? (
                    <CircleAlert className="text-destructive-foreground" />
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
