import * as React from "react";
import {
  Check,
  Copy,
  ExternalLink,
  GitBranchPlus,
  Inbox,
  MessageSquare,
  Pencil,
  Play,
  Plus,
  Trash2,
} from "lucide-react";
import { toast } from "sonner";
import type { AssistantTask, TaskLink, TaskPriority, TaskSource, TaskStatus } from "@lila/protocol";
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
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "cn";
import { actions, useLila } from "@/lib/store";

const STATUS_META: Record<TaskStatus, { label: string; badgeClass: string }> = {
  inbox: { label: "待处理", badgeClass: "bg-muted text-muted-foreground" },
  clarifying: { label: "待澄清", badgeClass: "bg-warning/15 text-warning-foreground border-warning/30" },
  running: { label: "执行中", badgeClass: "bg-info/15 text-info-foreground border-info/30" },
  review: { label: "待验收", badgeClass: "bg-success/15 text-success-foreground border-success/30" },
  done: { label: "已完成", badgeClass: "bg-muted/70 text-muted-foreground" },
};

const SOURCE_LABELS: Record<TaskSource, string> = {
  manual: "手动",
  wechat: "微信",
  whatsapp: "WhatsApp",
  github: "GitHub",
  asana: "Asana",
  schedule: "定时任务",
};

const PRIORITY_LABELS: Record<TaskPriority, string> = {
  high: "高优",
  normal: "普通",
  low: "低优",
};

const LINK_KIND_LABEL: Record<NonNullable<TaskLink["kind"]>, string> = {
  design: "设计稿",
  issue: "需求",
  pr: "PR",
  doc: "文档",
  other: "链接",
};

const STATUS_OPTIONS: Array<{ value: TaskStatus; label: string }> = [
  { value: "inbox", label: "待处理" },
  { value: "clarifying", label: "待澄清" },
  { value: "running", label: "执行中" },
  { value: "review", label: "待验收" },
  { value: "done", label: "已完成" },
];

const SOURCE_OPTIONS = (Object.entries(SOURCE_LABELS) as Array<[TaskSource, string]>).map(([value, label]) => ({ value, label }));
const PRIORITY_OPTIONS = (Object.entries(PRIORITY_LABELS) as Array<[TaskPriority, string]>).map(([value, label]) => ({ value, label }));

function parseLinkLines(raw: string): TaskLink[] {
  return raw
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const match = line.match(/^(https?:\/\/\S+)(?:\s+(.+))?$/i);
      if (match) return { url: match[1]!, ...(match[2]?.trim() ? { label: match[2].trim() } : {}) };
      return { url: line };
    });
}

function formatLinkLines(links: readonly TaskLink[]): string {
  return links.map((link) => (link.label ? `${link.url} ${link.label}` : link.url)).join("\n");
}

function parseLines(raw: string): string[] {
  return raw
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
}

type FilterValue = "all" | TaskStatus;

interface TaskFormState {
  id?: string;
  title: string;
  projectId: string;
  source: TaskSource;
  priority: TaskPriority;
  status: TaskStatus;
  summary: string;
  linksText: string;
  questionsText: string;
  draftReply: string;
}

function emptyForm(defaultProjectId: string): TaskFormState {
  return {
    title: "",
    projectId: defaultProjectId,
    source: "manual",
    priority: "normal",
    status: "inbox",
    summary: "",
    linksText: "",
    questionsText: "",
    draftReply: "",
  };
}

function taskToForm(task: AssistantTask): TaskFormState {
  return {
    id: task.id,
    title: task.title,
    projectId: task.projectId ?? "",
    source: task.source,
    priority: task.priority,
    status: task.status,
    summary: task.summary,
    linksText: formatLinkLines(task.links),
    questionsText: task.blockingQuestions.join("\n"),
    draftReply: task.draftReply ?? "",
  };
}

export function TasksPanel({
  defaultProjectId,
  createToken,
}: {
  defaultProjectId?: string;
  createToken: number;
}) {
  const tasks = useLila((state) => state.tasks);
  const projects = useLila((state) => state.projects);
  const sessions = useLila((state) => state.sessions);
  const connected = useLila((state) => state.connection === "connected");
  const defaultApprovalMode = useLila((state) => state.defaultApprovalMode);

  const [filter, setFilter] = React.useState<FilterValue>("all");
  const [formOpen, setFormOpen] = React.useState(false);
  const [form, setForm] = React.useState<TaskFormState>(() => emptyForm(defaultProjectId ?? ""));
  const [saving, setSaving] = React.useState(false);
  const [startingId, setStartingId] = React.useState<string>();
  const [deleting, setDeleting] = React.useState<AssistantTask>();
  const [copiedKey, setCopiedKey] = React.useState<string>();
  const lastCreateTokenRef = React.useRef(createToken);

  React.useEffect(() => {
    if (createToken === lastCreateTokenRef.current) return;
    lastCreateTokenRef.current = createToken;
    setForm(emptyForm(defaultProjectId ?? projects[0]?.id ?? ""));
    setFormOpen(true);
  }, [createToken, defaultProjectId, projects]);

  const projectById = React.useMemo(() => new Map(projects.map((project) => [project.id, project])), [projects]);
  const projectItems = React.useMemo(
    () => [{ value: "__none__", label: "暂不绑定项目" }, ...projects.map((p) => ({ value: p.id, label: p.name }))],
    [projects],
  );

  const counts = React.useMemo(() => {
    const map: Record<FilterValue, number> = {
      all: tasks.length,
      inbox: 0,
      clarifying: 0,
      running: 0,
      review: 0,
      done: 0,
    };
    for (const task of tasks) map[task.status] += 1;
    return map;
  }, [tasks]);

  const filteredTasks = React.useMemo(
    () => (filter === "all" ? tasks : tasks.filter((task) => task.status === filter)),
    [tasks, filter],
  );

  const openNewForm = () => {
    setForm(emptyForm(defaultProjectId ?? projects[0]?.id ?? ""));
    setFormOpen(true);
  };

  const openEditForm = (task: AssistantTask) => {
    setForm(taskToForm(task));
    setFormOpen(true);
  };

  const handleSubmit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!form.title.trim()) {
      toast.error("请输入任务标题");
      return;
    }
    setSaving(true);
    try {
      const links = parseLinkLines(form.linksText);
      const blockingQuestions = parseLines(form.questionsText);
      const projectId = form.projectId && form.projectId !== "__none__" ? form.projectId : undefined;
      if (form.id) {
        const updated = await actions.updateTask(form.id, {
          title: form.title.trim(),
          projectId: projectId ?? null,
          source: form.source,
          priority: form.priority,
          status: form.status,
          summary: form.summary,
          links,
          blockingQuestions,
          draftReply: form.draftReply.trim() ? form.draftReply.trim() : null,
        });
        if (updated) {
          toast.success("任务已更新");
          setFormOpen(false);
        }
      } else {
        const created = await actions.createTask({
          title: form.title.trim(),
          ...(projectId ? { projectId } : {}),
          source: form.source,
          priority: form.priority,
          status: form.status,
          summary: form.summary,
          ...(links.length ? { links } : {}),
          ...(blockingQuestions.length ? { blockingQuestions } : {}),
          ...(form.draftReply.trim() ? { draftReply: form.draftReply.trim() } : {}),
        });
        if (created) setFormOpen(false);
      }
    } finally {
      setSaving(false);
    }
  };

  const handleCopy = (key: string, text: string, label: string) => {
    void navigator.clipboard
      .writeText(text)
      .then(() => {
        setCopiedKey(key);
        toast.success(`已复制${label}`);
        setTimeout(() => setCopiedKey((curr) => (curr === key ? undefined : curr)), 1800);
      })
      .catch(() => toast.error("复制失败"));
  };

  const handleStart = async (task: AssistantTask, useWorktree: boolean) => {
    const projectId = task.projectId ?? defaultProjectId ?? projects[0]?.id;
    if (!projectId) {
      toast.error("请先在侧栏添加一个项目，再启动任务");
      return;
    }
    setStartingId(`${task.id}:${useWorktree ? "wt" : "direct"}`);
    try {
      await actions.startTask({
        id: task.id,
        projectId,
        useWorktree,
        approvalMode: defaultApprovalMode,
      });
    } finally {
      setStartingId(undefined);
    }
  };

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-1">
          {(["all", "inbox", "clarifying", "running", "review", "done"] as FilterValue[]).map((value) => {
            const active = filter === value;
            const label = value === "all" ? "全部" : STATUS_META[value].label;
            return (
              <Button
                key={value}
                type="button"
                size="xs"
                variant={active ? "secondary" : "ghost"}
                onClick={() => setFilter(value)}
                className={cn("gap-1 font-normal", active && "font-medium")}
              >
                {label}
                <span className="font-mono text-[10px] text-muted-foreground tabular-nums">{counts[value]}</span>
              </Button>
            );
          })}
        </div>
        <Button type="button" size="sm" disabled={!connected} onClick={openNewForm}>
          <Plus />
          快速收集任务
        </Button>
      </div>

      {formOpen ? (
        <form onSubmit={(e) => void handleSubmit(e)} className="surface-enter flex flex-col gap-3.5 rounded-xl border border-border bg-muted/20 p-4">
          <div className="flex items-center justify-between">
            <h3 className="text-sm font-medium">{form.id ? "编辑任务" : "快速收集任务"}</h3>
            <Button type="button" size="xs" variant="ghost" onClick={() => setFormOpen(false)}>
              取消
            </Button>
          </div>
          <div className="grid gap-3 sm:grid-cols-2">
            <div className="flex flex-col gap-1.5 sm:col-span-2">
              <Label htmlFor="task-title">任务标题</Label>
              <Input
                id="task-title"
                value={form.title}
                onChange={(e) => setForm((curr) => ({ ...curr, title: e.target.value }))}
                placeholder="例如：实现结算页新交互设计稿 / 修复 Asana #42 导出报错"
                autoFocus
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label>来源渠道</Label>
              <Select
                value={form.source}
                onValueChange={(value) => {
                  if (value) setForm((curr) => ({ ...curr, source: value as TaskSource }));
                }}
                items={SOURCE_OPTIONS}
              >
                <SelectTrigger size="sm">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {SOURCE_OPTIONS.map((opt) => (
                    <SelectItem key={opt.value} value={opt.value}>
                      {opt.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="flex flex-col gap-1.5">
              <Label>优先级</Label>
              <Select
                value={form.priority}
                onValueChange={(value) => {
                  if (value) setForm((curr) => ({ ...curr, priority: value as TaskPriority }));
                }}
                items={PRIORITY_OPTIONS}
              >
                <SelectTrigger size="sm">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {PRIORITY_OPTIONS.map((opt) => (
                    <SelectItem key={opt.value} value={opt.value}>
                      {opt.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="flex flex-col gap-1.5">
              <Label>关联项目</Label>
              <Select
                value={form.projectId || "__none__"}
                onValueChange={(value) => {
                  setForm((curr) => ({ ...curr, projectId: value === "__none__" || !value ? "" : value }));
                }}
                items={projectItems}
              >
                <SelectTrigger size="sm">
                  <SelectValue placeholder="选择关联项目" />
                </SelectTrigger>
                <SelectContent>
                  {projectItems.map((item) => (
                    <SelectItem key={item.value} value={item.value}>
                      {item.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="flex flex-col gap-1.5">
              <Label>当前状态</Label>
              <Select
                value={form.status}
                onValueChange={(value) => {
                  if (value) setForm((curr) => ({ ...curr, status: value as TaskStatus }));
                }}
                items={STATUS_OPTIONS}
              >
                <SelectTrigger size="sm">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {STATUS_OPTIONS.map((opt) => (
                    <SelectItem key={opt.value} value={opt.value}>
                      {opt.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="flex flex-col gap-1.5 sm:col-span-2">
              <Label htmlFor="task-summary">需求说明 / 聊天摘录（粘贴内容里的设计稿与 Issue URL 会自动提取）</Label>
              <Textarea
                id="task-summary"
                rows={4}
                value={form.summary}
                onChange={(e) => setForm((curr) => ({ ...curr, summary: e.target.value }))}
                placeholder="直接粘贴微信/WhatsApp 需求对话、Asana 描述或设计稿链接…"
              />
            </div>
            <div className="flex flex-col gap-1.5 sm:col-span-2">
              <Label htmlFor="task-links">设计稿 / Issue / PR 链接（每行一条，支持“URL 备注”）</Label>
              <Textarea
                id="task-links"
                rows={2}
                value={form.linksText}
                onChange={(e) => setForm((curr) => ({ ...curr, linksText: e.target.value }))}
                placeholder="https://example.com/design.html 结算页交互原型"
              />
            </div>
            {form.id ? (
              <>
                <div className="flex flex-col gap-1.5 sm:col-span-2">
                  <Label htmlFor="task-questions">待澄清问题（每行一条）</Label>
                  <Textarea
                    id="task-questions"
                    rows={2}
                    value={form.questionsText}
                    onChange={(e) => setForm((curr) => ({ ...curr, questionsText: e.target.value }))}
                    placeholder="例如：弹窗取消后是否保留已填写内容？"
                  />
                </div>
                <div className="flex flex-col gap-1.5 sm:col-span-2">
                  <Label htmlFor="task-draft-reply">拟对外汇报 / 确认草稿</Label>
                  <Textarea
                    id="task-draft-reply"
                    rows={2}
                    value={form.draftReply}
                    onChange={(e) => setForm((curr) => ({ ...curr, draftReply: e.target.value }))}
                    placeholder="供你审阅后一键复制发给设计师/客户/同事…"
                  />
                </div>
              </>
            ) : null}
          </div>
          <div className="flex justify-end gap-2">
            <Button type="button" size="sm" variant="outline" onClick={() => setFormOpen(false)}>
              取消
            </Button>
            <Button type="submit" size="sm" disabled={saving || !connected}>
              {form.id ? "保存更新" : "存入收件箱"}
            </Button>
          </div>
        </form>
      ) : null}

      {filteredTasks.length === 0 ? (
        <Empty className="border border-dashed py-10">
          <EmptyHeader>
            <EmptyMedia variant="icon">
              <Inbox />
            </EmptyMedia>
            <EmptyTitle>{tasks.length === 0 ? "任务收件箱还是空的" : "该状态下暂无任务"}</EmptyTitle>
            <EmptyDescription>
              把微信、WhatsApp、Asana、GitHub 的需求片段和 HTML 设计稿链接快速收口到这里，确认后一键隔离 Worktree 开工；也可以直接在对话中让 Lila 帮你记录。
            </EmptyDescription>
          </EmptyHeader>
        </Empty>
      ) : (
        <div className="flex flex-col gap-3">
          {filteredTasks.map((task) => {
            const statusMeta = STATUS_META[task.status];
            const project = task.projectId ? projectById.get(task.projectId) : undefined;
            const linkedSession = task.sessionId ? sessions[task.sessionId] : undefined;
            const startingDirect = startingId === `${task.id}:direct`;
            const startingWorktree = startingId === `${task.id}:wt`;
            const isBusy = Boolean(startingId);
            return (
              <div key={task.id} className="flex flex-col gap-2.5 rounded-xl border border-border/80 bg-card/50 p-3.5">
                <div className="flex items-start justify-between gap-2">
                  <div className="flex min-w-0 flex-1 flex-col gap-1">
                    <div className="flex flex-wrap items-center gap-1.5">
                      <Badge variant="outline" className={cn("h-5 px-1.5 text-[11px] font-medium", statusMeta.badgeClass)}>
                        {statusMeta.label}
                      </Badge>
                      <Badge variant="secondary" className="h-5 px-1.5 text-[11px] font-normal">
                        {SOURCE_LABELS[task.source]}
                      </Badge>
                      {task.priority === "high" ? (
                        <Badge variant="destructive" className="h-5 px-1.5 text-[11px]">
                          高优
                        </Badge>
                      ) : null}
                      {project ? <span className="truncate text-xs text-muted-foreground">{project.name}</span> : null}
                    </div>
                    <h4 className="text-sm font-medium leading-snug text-foreground">{task.title}</h4>
                  </div>
                  <div className="flex shrink-0 items-center gap-1">
                    <Select
                      value={task.status}
                      disabled={!connected}
                      onValueChange={(value) => {
                        if (value) void actions.updateTask(task.id, { status: value as TaskStatus });
                      }}
                      items={STATUS_OPTIONS}
                    >
                      <SelectTrigger size="sm" className="h-7 w-24 text-xs">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent align="end">
                        {STATUS_OPTIONS.map((opt) => (
                          <SelectItem key={opt.value} value={opt.value}>
                            {opt.label}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                    <Button type="button" size="icon-xs" variant="ghost" aria-label="编辑任务" onClick={() => openEditForm(task)}>
                      <Pencil className="size-3.5" />
                    </Button>
                    <Button
                      type="button"
                      size="icon-xs"
                      variant="ghost"
                      aria-label="删除任务"
                      className="text-muted-foreground hover:text-destructive-foreground"
                      onClick={() => setDeleting(task)}
                    >
                      <Trash2 className="size-3.5" />
                    </Button>
                  </div>
                </div>

                {task.summary ? (
                  <p className="line-clamp-3 whitespace-pre-wrap text-xs leading-relaxed text-muted-foreground">{task.summary}</p>
                ) : null}

                {task.links.length > 0 ? (
                  <div className="flex flex-wrap gap-1.5">
                    {task.links.map((link) => (
                      <button
                        key={link.url}
                        type="button"
                        onClick={() => void actions.openExternalUrl(link.url)}
                        className="inline-flex max-w-full items-center gap-1 rounded-md border border-border/70 bg-muted/40 px-2 py-0.5 text-[11px] text-foreground transition-colors hover:bg-muted"
                        title={link.url}
                      >
                        <span className="shrink-0 text-muted-foreground">[{LINK_KIND_LABEL[link.kind ?? "other"]}]</span>
                        <span className="max-w-52 truncate">{link.label || link.url}</span>
                        <ExternalLink className="size-3 shrink-0 text-muted-foreground" />
                      </button>
                    ))}
                  </div>
                ) : null}

                {task.blockingQuestions.length > 0 ? (
                  <div className="rounded-lg border border-warning/30 bg-warning/10 p-2.5 text-xs">
                    <div className="mb-1 flex items-center justify-between gap-2">
                      <span className="font-medium text-warning-foreground">待澄清问题（{task.blockingQuestions.length}）</span>
                      <Button
                        type="button"
                        size="xs"
                        variant="ghost"
                        className="h-6 gap-1 px-1.5 text-[11px]"
                        onClick={() => handleCopy(`q:${task.id}`, task.blockingQuestions.map((q, i) => `${i + 1}. ${q}`).join("\n"), "待澄清问题")}
                      >
                        {copiedKey === `q:${task.id}` ? <Check className="size-3" /> : <Copy className="size-3" />}
                        复制提问
                      </Button>
                    </div>
                    <ul className="list-disc space-y-0.5 pl-4 text-muted-foreground">
                      {task.blockingQuestions.map((q, idx) => (
                        <li key={idx}>{q}</li>
                      ))}
                    </ul>
                  </div>
                ) : null}

                {task.draftReply ? (
                  <div className="rounded-lg border border-border bg-muted/35 p-2.5 text-xs">
                    <div className="mb-1 flex items-center justify-between gap-2">
                      <span className="font-medium text-foreground">对外回复草稿（人工审阅后发送）</span>
                      <Button
                        type="button"
                        size="xs"
                        variant="ghost"
                        className="h-6 gap-1 px-1.5 text-[11px]"
                        onClick={() => handleCopy(`r:${task.id}`, task.draftReply!, "回复草稿")}
                      >
                        {copiedKey === `r:${task.id}` ? <Check className="size-3" /> : <Copy className="size-3" />}
                        复制草稿
                      </Button>
                    </div>
                    <p className="whitespace-pre-wrap leading-relaxed text-muted-foreground">{task.draftReply}</p>
                  </div>
                ) : null}

                <div className="flex flex-wrap items-center justify-between gap-2 border-t border-border/50 pt-2">
                  <div className="flex flex-wrap items-center gap-1.5">
                    {linkedSession ? (
                      <Button
                        type="button"
                        size="xs"
                        variant="outline"
                        onClick={() => {
                          actions.setAutomationOpen(false);
                          actions.select(linkedSession.id);
                        }}
                      >
                        <MessageSquare className="size-3.5" />
                        查看执行会话
                      </Button>
                    ) : null}
                    {task.status !== "done" ? (
                      <>
                        <Button
                          type="button"
                          size="xs"
                          variant={linkedSession ? "ghost" : "default"}
                          disabled={!connected || isBusy}
                          onClick={() => void handleStart(task, false)}
                        >
                          <Play className="size-3.5" />
                          {startingDirect ? "启动中…" : linkedSession ? "重新开工" : "开工执行"}
                        </Button>
                        <Button
                          type="button"
                          size="xs"
                          variant="outline"
                          disabled={!connected || isBusy}
                          onClick={() => void handleStart(task, true)}
                        >
                          <GitBranchPlus className="size-3.5" />
                          {startingWorktree ? "创建 Worktree…" : "隔离 Worktree 开工"}
                        </Button>
                      </>
                    ) : null}
                  </div>
                  <span className="font-mono text-[11px] text-muted-foreground">
                    {new Date(task.updatedAt).toLocaleString("zh-CN", { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" })}
                  </span>
                </div>
              </div>
            );
          })}
        </div>
      )}

      <AlertDialog open={Boolean(deleting)} onOpenChange={(open) => { if (!open) setDeleting(undefined); }}>
        <AlertDialogContent size="sm">
          <AlertDialogHeader>
            <AlertDialogTitle>删除该任务？</AlertDialogTitle>
            <AlertDialogDescription>
              将从任务收件箱移除「{deleting?.title}」，不会删除已产生的会话历史或代码文件。
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>取消</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                if (deleting) void actions.deleteTask(deleting.id);
                setDeleting(undefined);
              }}
            >
              删除
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
