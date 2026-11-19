import { FolderOpen, FolderPlus, RotateCw } from "lucide-react";
import type { ThinkingLevel } from "@lila/protocol";
import { Composer } from "@/components/composer";
import { ApprovalSelect, ModelPicker, ThinkingSelect } from "@/components/session-controls";
import { Button } from "@/components/ui/button";
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Select, SelectContent, SelectItem, SelectSeparator, SelectTrigger, SelectValue } from "@/components/ui/select";
import { SidebarTrigger, useSidebar } from "@/components/ui/sidebar";
import { cn } from "cn";
import { actions, NEW_DRAFT, useLila } from "@/lib/store";

/** Before a session exists core cannot tell the exact levels; these are the ones every reasoning model accepts. */
const REASONING_LEVELS: ThinkingLevel[] = ["off", "minimal", "low", "medium", "high"];
const ADD_PROJECT = "__add__";

export function NewSession() {
  const projects = useLila((state) => state.projects);
  const models = useLila((state) => state.models);
  const defaultModelId = useLila((state) => state.defaultModelId);
  const modelsError = useLila((state) => state.modelsError);
  const connected = useLila((state) => state.connection === "connected");
  const options = useLila((state) => state.newSession);
  const { open, isMobile } = useSidebar();

  const projectId = projects.some((project) => project.id === options.projectId) ? options.projectId : projects.at(-1)?.id;
  const modelId = options.modelId && models.some((model) => model.id === options.modelId) ? options.modelId : defaultModelId;
  const model = models.find((item) => item.id === modelId);

  const header = (
    <header className={cn("flex h-12 shrink-0 items-center px-4 [-webkit-app-region:drag]", !open && !isMobile && "pl-20")}>
      <SidebarTrigger className="[-webkit-app-region:no-drag]" title="切换侧边栏（⌘B）" />
    </header>
  );

  if (!projects.length) {
    return (
      <div className="flex min-h-0 flex-1 flex-col">
        {header}
        <Empty className="flex-1 [-webkit-app-region:drag]">
          <EmptyHeader>
            <EmptyTitle>先添加一个项目</EmptyTitle>
            <EmptyDescription>选择一个本地代码目录，Agent 会在这个目录里工作。</EmptyDescription>
          </EmptyHeader>
          <EmptyContent className="[-webkit-app-region:no-drag]">
            <Button disabled={!connected} onClick={() => void actions.addProject().then((project) => project && actions.setNewSession({ projectId: project.id }))}>
              <FolderOpen />
              选择目录
            </Button>
          </EmptyContent>
        </Empty>
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {header}
      <div className="flex flex-1 flex-col items-center justify-center px-6 pb-[12vh]">
        <div className="flex w-full max-w-3xl flex-col gap-4">
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1 px-1">
            <h1 className="text-2xl font-semibold tracking-tight">要在</h1>
            <Select
              value={projectId ?? null}
              onValueChange={(value) => {
                if (value === ADD_PROJECT) void actions.addProject().then((project) => project && actions.setNewSession({ projectId: project.id }));
                else if (typeof value === "string") actions.setNewSession({ projectId: value });
              }}
              items={projects.map((project) => ({ value: project.id, label: project.name }))}
            >
              <SelectTrigger className="h-9 border-dashed text-2xl font-semibold tracking-tight" aria-label="项目">
                <SelectValue />
              </SelectTrigger>
              <SelectContent alignItemWithTrigger={false} align="start" className="min-w-72">
                {projects.map((project) => (
                  <SelectItem key={project.id} value={project.id}>
                    <div className="flex min-w-0 flex-col">
                      <span>{project.name}</span>
                      <span className="truncate text-xs text-muted-foreground">{project.path}</span>
                    </div>
                  </SelectItem>
                ))}
                <SelectSeparator />
                <SelectItem value={ADD_PROJECT}>
                  <FolderPlus />
                  添加项目…
                </SelectItem>
              </SelectContent>
            </Select>
            <h1 className="text-2xl font-semibold tracking-tight">里做什么？</h1>
          </div>
          <Composer
            draftKey={NEW_DRAFT}
            projectId={projectId}
            disabled={!connected || !projectId}
            placeholder="描述你要完成的任务，@ 引用文件，可以粘贴或拖入图片"
            onSubmit={(text, images) => {
              if (projectId !== options.projectId) actions.setNewSession({ projectId });
              return actions.createAndPrompt(text, images);
            }}
            footer={
              <>
                {modelsError ? (
                  <Button size="sm" variant="ghost" className="text-destructive" title={modelsError} onClick={() => void actions.retryModels()}>
                    <RotateCw />
                    读取模型失败，重试
                  </Button>
                ) : (
                  <ModelPicker models={models} value={modelId} onChange={(next) => actions.setNewSession({ modelId: next })} />
                )}
                <ThinkingSelect
                  levels={model?.reasoning ? REASONING_LEVELS : []}
                  value={options.thinkingLevel}
                  onChange={(thinkingLevel) => actions.setNewSession({ thinkingLevel })}
                />
                <ApprovalSelect value={options.approvalMode} onChange={(approvalMode) => actions.setNewSession({ approvalMode })} />
              </>
            }
          />
        </div>
      </div>
    </div>
  );
}
