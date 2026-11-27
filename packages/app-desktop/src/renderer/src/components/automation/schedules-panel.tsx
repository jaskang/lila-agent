import * as React from "react";
import { Clock, MessageSquare, Pencil, Play, Plus, Trash2 } from "lucide-react";
import { toast } from "sonner";
import type { ApprovalMode, ScheduledJob, ScheduleSessionMode } from "@lila/protocol";
import { ApprovalSelect } from "@/components/session-controls";
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
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "cn";
import { actions, useLila } from "@/lib/store";

const PRESETS: Array<{ label: string; value: string }> = [
  { label: "工作日 9:00", value: "0 9 * * 1-5" },
  { label: "每天 9:00", value: "0 9 * * *" },
  { label: "每小时", value: "1h" },
  { label: "每 15 分钟", value: "15m" },
  { label: "30 分钟后一次", value: "+30m" },
];

const SESSION_MODE_OPTIONS: Array<{ value: ScheduleSessionMode; label: string }> = [
  { value: "new", label: "每次新建独立会话（推荐）" },
  { value: "reuse", label: "复用同一个固定会话" },
];

const STATUS_BADGE: Record<ScheduledJob["lastStatus"], { label: string; className: string }> = {
  idle: { label: "待触发", className: "bg-muted text-muted-foreground" },
  running: { label: "运行中", className: "bg-info/15 text-info-foreground border-info/30" },
  success: { label: "已触发", className: "bg-success/15 text-success-foreground border-success/30" },
  skipped: { label: "已跳过", className: "bg-warning/15 text-warning-foreground border-warning/30" },
  error: { label: "出错", className: "bg-destructive/15 text-destructive-foreground border-destructive/30" },
};

function formatDateTime(iso?: string): string {
  if (!iso) return "—";
  const ts = Date.parse(iso);
  if (!Number.isFinite(ts)) return iso;
  return new Date(ts).toLocaleString("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

interface ScheduleFormState {
  id?: string;
  name: string;
  projectId: string;
  schedule: string;
  prompt: string;
  sessionMode: ScheduleSessionMode;
  approvalMode: ApprovalMode;
  modelId: string;
  enabled: boolean;
}

function emptyScheduleForm(projectId: string, approvalMode: ApprovalMode): ScheduleFormState {
  return {
    name: "",
    projectId,
    schedule: "0 9 * * 1-5",
    prompt: "",
    sessionMode: "new",
    approvalMode,
    modelId: "__default__",
    enabled: true,
  };
}

function scheduleToForm(job: ScheduledJob): ScheduleFormState {
  return {
    id: job.id,
    name: job.name,
    projectId: job.projectId,
    schedule: job.schedule,
    prompt: job.prompt,
    sessionMode: job.sessionMode,
    approvalMode: job.approvalMode,
    modelId: job.modelId ?? "__default__",
    enabled: job.enabled,
  };
}

export function SchedulesPanel({
  defaultProjectId,
  createToken,
}: {
  defaultProjectId?: string;
  createToken: number;
}) {
  const schedules = useLila((state) => state.schedules);
  const projects = useLila((state) => state.projects);
  const models = useLila((state) => state.models);
  const sessions = useLila((state) => state.sessions);
  const connected = useLila((state) => state.connection === "connected");
  const defaultApprovalMode = useLila((state) => state.defaultApprovalMode);

  const [formOpen, setFormOpen] = React.useState(false);
  const [form, setForm] = React.useState<ScheduleFormState>(() =>
    emptyScheduleForm(defaultProjectId ?? projects[0]?.id ?? "", defaultApprovalMode),
  );
  const [saving, setSaving] = React.useState(false);
  const [runningId, setRunningId] = React.useState<string>();
  const [deleting, setDeleting] = React.useState<ScheduledJob>();
  const lastCreateTokenRef = React.useRef(createToken);

  React.useEffect(() => {
    if (createToken === lastCreateTokenRef.current) return;
    lastCreateTokenRef.current = createToken;
    setForm(emptyScheduleForm(defaultProjectId ?? projects[0]?.id ?? "", defaultApprovalMode));
    setFormOpen(true);
  }, [createToken, defaultProjectId, projects, defaultApprovalMode]);

  const projectById = React.useMemo(() => new Map(projects.map((project) => [project.id, project])), [projects]);
  const projectItems = React.useMemo(() => projects.map((p) => ({ value: p.id, label: p.name })), [projects]);
  const modelItems = React.useMemo(
    () => [
      { value: "__default__", label: "跟随全局默认模型" },
      ...models.filter((m) => m.enabled).map((m) => ({ value: m.id, label: `${m.label} (${m.providerName})` })),
    ],
    [models],
  );

  const enabledCount = React.useMemo(() => schedules.filter((job) => job.enabled).length, [schedules]);

  const openNewForm = () => {
    if (!projects.length) {
      toast.error("请先在侧栏添加至少一个项目");
      return;
    }
    setForm(emptyScheduleForm(defaultProjectId ?? projects[0]?.id ?? "", defaultApprovalMode));
    setFormOpen(true);
  };

  const openEditForm = (job: ScheduledJob) => {
    setForm(scheduleToForm(job));
    setFormOpen(true);
  };

  const handleSubmit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!form.name.trim()) {
      toast.error("请输入定时任务名称");
      return;
    }
    if (!form.projectId) {
      toast.error("请选择运行项目");
      return;
    }
    if (!form.schedule.trim()) {
      toast.error("请输入调度表达式");
      return;
    }
    if (!form.prompt.trim()) {
      toast.error("请输入触发时发送的指令内容");
      return;
    }
    setSaving(true);
    try {
      const modelId = form.modelId && form.modelId !== "__default__" ? form.modelId : undefined;
      if (form.id) {
        const updated = await actions.updateSchedule(form.id, {
          name: form.name.trim(),
          projectId: form.projectId,
          schedule: form.schedule.trim(),
          prompt: form.prompt.trim(),
          sessionMode: form.sessionMode,
          approvalMode: form.approvalMode,
          modelId: modelId ?? null,
          enabled: form.enabled,
        });
        if (updated) {
          toast.success(`已更新定时任务「${updated.name}」`);
          setFormOpen(false);
        }
      } else {
        const created = await actions.createSchedule({
          name: form.name.trim(),
          projectId: form.projectId,
          schedule: form.schedule.trim(),
          prompt: form.prompt.trim(),
          sessionMode: form.sessionMode,
          approvalMode: form.approvalMode,
          ...(modelId ? { modelId } : {}),
          enabled: form.enabled,
        });
        if (created) setFormOpen(false);
      }
    } finally {
      setSaving(false);
    }
  };

  const handleRunNow = async (job: ScheduledJob) => {
    setRunningId(job.id);
    try {
      await actions.runSchedule(job.id);
    } finally {
      setRunningId(undefined);
    }
  };

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="text-xs text-muted-foreground">
          共 {schedules.length} 个定时任务 · 已启用 {enabledCount} 个（随常驻 core 在后台持久化运行）
        </p>
        <Button type="button" size="sm" disabled={!connected} onClick={openNewForm}>
          <Plus />
          新建定时任务
        </Button>
      </div>

      {formOpen ? (
        <form onSubmit={(e) => void handleSubmit(e)} className="surface-enter flex flex-col gap-3.5 rounded-xl border border-border bg-muted/20 p-4">
          <div className="flex items-center justify-between">
            <h3 className="text-sm font-medium">{form.id ? "编辑定时任务" : "新建定时任务"}</h3>
            <Button type="button" size="xs" variant="ghost" onClick={() => setFormOpen(false)}>
              取消
            </Button>
          </div>

          <div className="grid gap-3 sm:grid-cols-2">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="sched-name">任务名称</Label>
              <Input
                id="sched-name"
                value={form.name}
                onChange={(e) => setForm((curr) => ({ ...curr, name: e.target.value }))}
                placeholder="例如：每日需求巡检 / PR 状态汇报"
                autoFocus
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label>运行项目</Label>
              <Select
                value={form.projectId}
                onValueChange={(value) => {
                  if (value) setForm((curr) => ({ ...curr, projectId: value }));
                }}
                items={projectItems}
              >
                <SelectTrigger size="sm">
                  <SelectValue placeholder="选择项目" />
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

            <div className="flex flex-col gap-1.5 sm:col-span-2">
              <Label htmlFor="sched-expr">调度表达式（支持 5/6 段 Cron、周期 15m/1h/1d、单次 +30m 或 ISO 时间）</Label>
              <Input
                id="sched-expr"
                value={form.schedule}
                onChange={(e) => setForm((curr) => ({ ...curr, schedule: e.target.value }))}
                placeholder="0 9 * * 1-5"
                className="font-mono text-xs"
              />
              <div className="flex flex-wrap gap-1">
                {PRESETS.map((preset) => (
                  <Button
                    key={preset.value}
                    type="button"
                    size="xs"
                    variant={form.schedule === preset.value ? "secondary" : "outline"}
                    onClick={() => setForm((curr) => ({ ...curr, schedule: preset.value }))}
                    className="h-6 px-2 text-[11px] font-normal"
                  >
                    {preset.label}
                  </Button>
                ))}
              </div>
            </div>

            <div className="flex flex-col gap-1.5 sm:col-span-2">
              <Label htmlFor="sched-prompt">触发指令</Label>
              <Textarea
                id="sched-prompt"
                rows={4}
                value={form.prompt}
                onChange={(e) => setForm((curr) => ({ ...curr, prompt: e.target.value }))}
                placeholder="例如：检查仓库最近提交、未处理 Issue 与依赖安全状态，整理一份简报并把需要我处理的问题用 task_upsert 写入任务收件箱。"
              />
            </div>

            <div className="flex flex-col gap-1.5">
              <Label>会话模式</Label>
              <Select
                value={form.sessionMode}
                onValueChange={(value) => {
                  if (value) setForm((curr) => ({ ...curr, sessionMode: value as ScheduleSessionMode }));
                }}
                items={SESSION_MODE_OPTIONS}
              >
                <SelectTrigger size="sm">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {SESSION_MODE_OPTIONS.map((opt) => (
                    <SelectItem key={opt.value} value={opt.value}>
                      {opt.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className="flex flex-col gap-1.5">
              <Label>模型</Label>
              <Select
                value={form.modelId}
                onValueChange={(value) => {
                  if (value) setForm((curr) => ({ ...curr, modelId: value }));
                }}
                items={modelItems}
              >
                <SelectTrigger size="sm">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent className="max-h-64">
                  {modelItems.map((item) => (
                    <SelectItem key={item.value} value={item.value}>
                      {item.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className="flex flex-col gap-1.5 sm:col-span-2">
              <Label>审批方式</Label>
              <div className="flex flex-wrap items-center gap-3">
                <ApprovalSelect
                  value={form.approvalMode}
                  onChange={(approvalMode) => setForm((curr) => ({ ...curr, approvalMode }))}
                  label="定时任务审批方式"
                  inSettings
                />
                {form.approvalMode === "auto" ? (
                  <span className="text-xs text-warning-foreground">完全访问会在无人值守时跳过 Lila 审批，请仅用于可信的只读/巡检任务。</span>
                ) : (
                  <span className="text-xs text-muted-foreground">遇到需确认操作时会暂停并进入侧栏「需要关注」。</span>
                )}
              </div>
            </div>
          </div>

          <div className="flex items-center justify-between pt-1">
            <label className="flex items-center gap-2 text-xs text-muted-foreground">
              <Switch
                checked={form.enabled}
                onCheckedChange={(enabled) => setForm((curr) => ({ ...curr, enabled }))}
                aria-label="创建后立即启用"
              />
              立即启用调度
            </label>
            <div className="flex gap-2">
              <Button type="button" size="sm" variant="outline" onClick={() => setFormOpen(false)}>
                取消
              </Button>
              <Button type="submit" size="sm" disabled={saving || !connected}>
                {form.id ? "保存修改" : "创建定时任务"}
              </Button>
            </div>
          </div>
        </form>
      ) : null}

      {schedules.length === 0 ? (
        <Empty className="border border-dashed py-10">
          <EmptyHeader>
            <EmptyMedia variant="icon">
              <Clock />
            </EmptyMedia>
            <EmptyTitle>还没有定时任务</EmptyTitle>
            <EmptyDescription>
              创建周期巡检、日报汇总或提醒任务，也可以直接跟 Lila 说“工作日早上 9 点帮我检查一次仓库状态”由它自动创建。
            </EmptyDescription>
          </EmptyHeader>
        </Empty>
      ) : (
        <div className="flex flex-col gap-3">
          {schedules.map((job) => {
            const project = projectById.get(job.projectId);
            const status = STATUS_BADGE[job.lastStatus];
            const lastSession = job.lastSessionId ? sessions[job.lastSessionId] : undefined;
            const isManualRunning = runningId === job.id;
            return (
              <div key={job.id} className={cn("flex flex-col gap-2.5 rounded-xl border border-border/80 bg-card/50 p-3.5", !job.enabled && "opacity-75")}>
                <div className="flex items-start justify-between gap-2">
                  <div className="flex min-w-0 flex-1 items-start gap-2.5">
                    <Switch
                      className="mt-0.5 shrink-0"
                      checked={job.enabled}
                      disabled={!connected}
                      onCheckedChange={(enabled) => void actions.updateSchedule(job.id, { enabled })}
                      aria-label={`切换定时任务 ${job.name}`}
                    />
                    <div className="flex min-w-0 flex-1 flex-col gap-1">
                      <div className="flex flex-wrap items-center gap-1.5">
                        <span className="text-sm font-medium text-foreground">{job.name}</span>
                        <Badge variant="outline" className="h-5 px-1.5 font-mono text-[11px]">
                          {job.schedule}
                        </Badge>
                        <Badge variant="secondary" className="h-5 px-1.5 text-[11px] font-normal">
                          {job.sessionMode === "reuse" ? "复用会话" : "每次新建"}
                        </Badge>
                        <Badge variant="outline" className={cn("h-5 px-1.5 text-[11px]", status.className)}>
                          {status.label}
                        </Badge>
                      </div>
                      <p className="line-clamp-2 text-xs text-muted-foreground">{job.prompt}</p>
                    </div>
                  </div>
                  <div className="flex shrink-0 items-center gap-1">
                    <Button
                      type="button"
                      size="icon-xs"
                      variant="ghost"
                      aria-label="编辑定时任务"
                      onClick={() => openEditForm(job)}
                    >
                      <Pencil className="size-3.5" />
                    </Button>
                    <Button
                      type="button"
                      size="icon-xs"
                      variant="ghost"
                      aria-label="删除定时任务"
                      className="text-muted-foreground hover:text-destructive-foreground"
                      onClick={() => setDeleting(job)}
                    >
                      <Trash2 className="size-3.5" />
                    </Button>
                  </div>
                </div>

                {job.lastError ? (
                  <p className="rounded-md bg-destructive/10 px-2.5 py-1.5 text-xs text-destructive-foreground">{job.lastError}</p>
                ) : null}

                <div className="flex flex-wrap items-center justify-between gap-2 border-t border-border/50 pt-2 text-[11px] text-muted-foreground">
                  <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                    {project ? <span>项目：{project.name}</span> : null}
                    <span>下次：{job.enabled ? formatDateTime(job.nextRunAt) : "已停用"}</span>
                    <span>上次：{formatDateTime(job.lastRunAt)}</span>
                    <span className="font-mono tabular-nums">已执行 {job.runCount} 次</span>
                  </div>
                  <div className="flex items-center gap-1.5">
                    {lastSession ? (
                      <Button
                        type="button"
                        size="xs"
                        variant="outline"
                        onClick={() => {
                          actions.setAutomationOpen(false);
                          actions.select(lastSession.id);
                        }}
                      >
                        <MessageSquare className="size-3.5" />
                        最近会话
                      </Button>
                    ) : null}
                    <Button
                      type="button"
                      size="xs"
                      variant="secondary"
                      disabled={!connected || isManualRunning || job.lastStatus === "running"}
                      onClick={() => void handleRunNow(job)}
                    >
                      <Play className="size-3.5" />
                      {isManualRunning ? "触发中…" : "立即运行"}
                    </Button>
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      )}

      <AlertDialog open={Boolean(deleting)} onOpenChange={(open) => { if (!open) setDeleting(undefined); }}>
        <AlertDialogContent size="sm">
          <AlertDialogHeader>
            <AlertDialogTitle>删除该定时任务？</AlertDialogTitle>
            <AlertDialogDescription>
              将停止并移除「{deleting?.name}」，已生成的历史会话记录会保留。
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>取消</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                if (deleting) void actions.deleteSchedule(deleting.id);
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
