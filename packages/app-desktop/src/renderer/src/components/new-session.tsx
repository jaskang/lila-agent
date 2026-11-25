import { useMemo } from "react";
import { Folder, FolderOpen, FolderPlus } from "lucide-react";
import { Composer } from "@/components/composer";
import { ApprovalSelect, COMPOSER_CHIP, ModelPicker, ModelSetup } from "@/components/session-controls";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectSeparator, SelectTrigger, SelectValue } from "@/components/ui/select";
import { SidebarTrigger, useSidebar } from "@/components/ui/sidebar";
import { cn } from "cn";
import { actions, NEW_DRAFT, useLila } from "@/lib/store";

const ADD_PROJECT = "__add__";

export function NewSession() {
  const projects = useLila((state) => state.projects);
  const sessions = useLila((state) => state.sessions);
  const models = useLila((state) => state.models);
  const defaultModelId = useLila((state) => state.defaultModelId);
  const modelsError = useLila((state) => state.modelsError);
  const modelsLoading = useLila((state) => state.modelsLoading);
  const connected = useLila((state) => state.connection === "connected");
  const options = useLila((state) => state.newSession);
  const { open, isMobile } = useSidebar();

  const recentProjectId = useMemo(() => {
    const projectIds = new Set(projects.map((p) => p.id));
    let best: { id: string; updatedAt: string } | undefined;
    for (const session of Object.values(sessions)) {
      if (session.parentId || !projectIds.has(session.projectId)) continue;
      if (!best || session.updatedAt > best.updatedAt) {
        best = { id: session.projectId, updatedAt: session.updatedAt };
      }
    }
    return best?.id ?? projects.at(-1)?.id;
  }, [projects, sessions]);

  const projectId = projects.some((project) => project.id === options.projectId) ? options.projectId : recentProjectId;
  const enabledModels = useMemo(() => {
    const enabled = models.filter((item) => item.enabled);
    return enabled.length ? enabled : models;
  }, [models]);
  const model =
    enabledModels.find((item) => item.id === options.modelId) ??
    enabledModels.find((item) => item.id === defaultModelId) ??
    enabledModels[0];

  const thinkingLevel = model?.thinkingLevels.find((level) => level === options.thinkingLevel) ?? model?.defaultThinkingLevel;

  const showTrigger = !open || isMobile;
  const header = (
    <header className={cn("flex h-11 shrink-0 items-center px-4 [-webkit-app-region:drag]", showTrigger && "pl-[74px]")}>
      {showTrigger ? <SidebarTrigger className="[-webkit-app-region:no-drag]" title="切换侧边栏（⌘B）" /> : null}
    </header>
  );

  if (!projects.length) {
    return (
      <div className="flex min-h-0 flex-1 flex-col">
        {header}
        <div className="flex flex-1 items-center justify-center px-6">
          <Button
            disabled={!connected}
            onClick={() => void actions.addProject().then((project) => project && actions.setNewSession({ projectId: project.id }))}
          >
            <FolderOpen />
            选择项目目录
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {header}
      <div className="flex flex-1 flex-col items-center justify-center px-6 pb-[10vh]">
        <div className="flex w-full max-w-[46rem] flex-col gap-3">
          {(!modelsLoading && !model) || modelsError ? <ModelSetup /> : null}
          <Composer
            draftKey={NEW_DRAFT}
            projectId={projectId}
            disabled={!connected || !projectId}
            sendDisabled={!model}
            placeholder="输入消息…"
            onSubmit={async () => {
              if (projectId && model) await actions.createAndPrompt(projectId, model.id, thinkingLevel);
            }}
            footer={
              <>
                <Select
                  value={projectId ?? null}
                  onValueChange={(value) => {
                    if (value === ADD_PROJECT) {
                      void actions.addProject().then((project) => project && actions.setNewSession({ projectId: project.id }));
                    } else if (typeof value === "string") {
                      actions.setNewSession({ projectId: value });
                    }
                  }}
                  items={projects.map((project) => ({ value: project.id, label: project.name }))}
                >
                  <SelectTrigger size="sm" className={cn(COMPOSER_CHIP, "max-w-44")} aria-label="项目">
                    <Folder />
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent alignItemWithTrigger={false} align="start" side="top" className="min-w-72">
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
                <ModelPicker
                  models={models}
                  modelId={model?.id}
                  thinkingLevel={thinkingLevel}
                  onApply={(modelId, thinkingLevel) => {
                    actions.setNewSession({ modelId, thinkingLevel });
                    return true;
                  }}
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
