import { useMemo } from "react";
import { Folder, FolderOpen, FolderPlus } from "lucide-react";
import { Composer } from "@/components/composer";
import { DEFAULT_SANDBOX_MODE } from "@lila/protocol";
import { ApprovalSelect, COMPOSER_CHIP, ModelPicker, ModelSetup, SandboxSelect } from "@/components/session-controls";
import { Button } from "@/components/ui/button";
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Kbd } from "@/components/ui/kbd";
import { useNewSessionProjectId } from "@/hooks/use-current-project";
import { Select, SelectContent, SelectItem, SelectSeparator, SelectTrigger, SelectValue } from "@/components/ui/select";
import { SidebarTrigger, useSidebar } from "@/components/ui/sidebar";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "cn";
import { actions, NEW_DRAFT, useLila } from "@/lib/store";

const ADD_PROJECT = "__add__";

export function NewSession() {
  const projects = useLila((state) => state.projects);
  const projectId = useNewSessionProjectId();
  const approvalMode = useLila((state) => state.newSessionApprovalMode ?? state.defaultApprovalMode);
  const sandboxMode = useLila((state) => state.newSessionSandboxMode ?? DEFAULT_SANDBOX_MODE);
  const models = useLila((state) => state.models);
  const defaultModelId = useLila((state) => state.defaultModelId);
  const modelsError = useLila((state) => state.modelsError);
  const modelsLoading = useLila((state) => state.modelsLoading);
  const connected = useLila((state) => state.connection === "connected");
  const options = useLila((state) => state.newSession);
  const { open, isMobile } = useSidebar();

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
      {showTrigger ? (
        <Tooltip>
          <TooltipTrigger render={<SidebarTrigger className="[-webkit-app-region:no-drag]" />} />
          <TooltipContent side="bottom">
            切换侧边栏
            <Kbd>⌘B</Kbd>
          </TooltipContent>
        </Tooltip>
      ) : null}
    </header>
  );

  if (!projects.length) {
    return (
      <div className="flex min-h-0 flex-1 flex-col">
        {header}
        <div className="flex flex-1 items-center justify-center px-6">
          <Empty>
            <EmptyHeader>
              <EmptyTitle>开始新会话</EmptyTitle>
              <EmptyDescription>选择 Agent 要工作的目录。添加时，你需要确认是否信任项目中的 .pi 配置和扩展。</EmptyDescription>
            </EmptyHeader>
            <EmptyContent className="flex-row flex-wrap justify-center">
              <Button
                disabled={!connected}
                onClick={() => void actions.addProject().then((project) => project && actions.setNewSession({ projectId: project.id }))}
              >
                <FolderOpen data-icon="inline-start" />
                选择项目目录
              </Button>
              {!modelsLoading && !model ? (
                <Button variant="outline" disabled={!connected} onClick={() => actions.openSettings("providers")}>
                  配置模型
                </Button>
              ) : null}
            </EmptyContent>
          </Empty>
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
                <ApprovalSelect value={approvalMode} onChange={actions.setNewSessionApprovalMode} />
                <SandboxSelect value={sandboxMode} onChange={actions.setNewSessionSandboxMode} />
              </>
            }
          />
        </div>
      </div>
    </div>
  );
}
