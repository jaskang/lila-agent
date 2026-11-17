import * as React from "react";
import { FolderOpen, RotateCw } from "lucide-react";
import type { ApprovalMode } from "@lila/protocol";
import { Button } from "@/components/ui/button";
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Field, FieldDescription, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { actions, APPROVAL_LABEL, useLila } from "@/lib/store";

const APPROVAL_HINT: Record<ApprovalMode, string> = {
  yolo: "所有工具直接执行，适合你信任的项目。",
  write: "改文件不问，运行命令前先问你。",
  "always-ask": "改文件和运行命令前都先问你。",
};

export function NewSession({ projectId: initialProjectId }: { projectId?: string }) {
  const projects = useLila((state) => state.projects);
  const models = useLila((state) => state.models);
  const defaultModelId = useLila((state) => state.defaultModelId);
  const modelsError = useLila((state) => state.modelsError);
  const connected = useLila((state) => state.connection === "connected");
  const [projectId, setProjectId] = React.useState(initialProjectId ?? projects.at(-1)?.id);
  const [modelId, setModelId] = React.useState<string>();
  const [approvalMode, setApprovalMode] = React.useState<ApprovalMode>("yolo");
  const [creating, setCreating] = React.useState(false);

  React.useEffect(() => {
    if (initialProjectId) setProjectId(initialProjectId);
  }, [initialProjectId]);
  React.useEffect(() => {
    if (!projectId && projects.length) setProjectId(projects.at(-1)?.id);
  }, [projectId, projects]);

  const model = modelId ?? defaultModelId;

  if (!projects.length) {
    return (
      <Empty className="[-webkit-app-region:drag]">
        <EmptyHeader>
          <EmptyTitle>先添加一个项目</EmptyTitle>
          <EmptyDescription>选择一个本地代码目录，Agent 会在这个目录里工作。</EmptyDescription>
        </EmptyHeader>
        <EmptyContent className="[-webkit-app-region:no-drag]">
          <Button disabled={!connected} onClick={() => void actions.addProject().then((project) => project && setProjectId(project.id))}>
            <FolderOpen />
            选择目录
          </Button>
        </EmptyContent>
      </Empty>
    );
  }

  async function create(): Promise<void> {
    if (!projectId) return;
    setCreating(true);
    await actions.createSession(projectId, model, approvalMode);
    setCreating(false);
  }

  return (
    <div className="flex flex-1 items-center justify-center p-6 [-webkit-app-region:drag]">
      <form
        className="flex w-full max-w-md flex-col gap-6 [-webkit-app-region:no-drag]"
        onSubmit={(event) => {
          event.preventDefault();
          void create();
        }}
      >
        <div>
          <h1 className="text-lg font-semibold">新会话</h1>
          <p className="text-sm text-muted-foreground">选好项目和模型，然后直接描述任务。</p>
        </div>
        <FieldGroup>
          <Field>
            <FieldLabel>项目</FieldLabel>
            <div className="flex gap-2">
              <Select
                value={projectId ?? null}
                onValueChange={(value) => typeof value === "string" && setProjectId(value)}
                items={projects.map((project) => ({ value: project.id, label: project.name }))}
              >
                <SelectTrigger className="flex-1" aria-label="项目">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {projects.map((project) => (
                    <SelectItem key={project.id} value={project.id}>
                      <span>{project.name}</span>
                      <span className="truncate text-xs text-muted-foreground">{project.path}</span>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <Button
                type="button"
                variant="outline"
                size="icon"
                title="添加项目"
                onClick={() => void actions.addProject().then((project) => project && setProjectId(project.id))}
              >
                <FolderOpen />
              </Button>
            </div>
          </Field>
          <Field>
            <FieldLabel>模型</FieldLabel>
            {modelsError ? (
              <div className="flex items-center gap-2 text-sm text-destructive">
                <span className="min-w-0 flex-1 truncate" title={modelsError}>
                  读取模型列表失败：{modelsError}
                </span>
                <Button type="button" size="sm" variant="outline" onClick={() => void actions.retryModels()}>
                  <RotateCw />
                  重试
                </Button>
              </div>
            ) : (
              <Select
                value={model ?? null}
                disabled={!models.length}
                onValueChange={(value) => typeof value === "string" && setModelId(value)}
                items={models.map((item) => ({ value: item.id, label: item.label }))}
              >
                <SelectTrigger className="w-full" aria-label="模型">
                  {models.length ? <SelectValue /> : <span className="flex items-center gap-2 text-muted-foreground"><Spinner />正在读取模型…</span>}
                </SelectTrigger>
                <SelectContent>
                  {models.map((item) => (
                    <SelectItem key={item.id} value={item.id}>
                      {item.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
          </Field>
          <Field>
            <FieldLabel>审批方式</FieldLabel>
            <Select
              value={approvalMode}
              onValueChange={(value) => value && setApprovalMode(value as ApprovalMode)}
              items={Object.entries(APPROVAL_LABEL).map(([value, label]) => ({ value, label }))}
            >
              <SelectTrigger className="w-full" aria-label="审批方式">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {Object.entries(APPROVAL_LABEL).map(([value, label]) => (
                  <SelectItem key={value} value={value}>
                    {label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <FieldDescription>{APPROVAL_HINT[approvalMode]}</FieldDescription>
          </Field>
        </FieldGroup>
        <Button type="submit" disabled={!connected || !projectId || creating}>
          {creating ? <Spinner /> : null}
          开始
        </Button>
      </form>
    </div>
  );
}
