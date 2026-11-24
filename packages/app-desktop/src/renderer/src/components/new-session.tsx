import { Folder, FolderOpen, FolderPlus } from "lucide-react";
import { Composer, focusComposer } from "@/components/composer";
import { ApprovalSelect, ModelPicker, ModelSetup } from "@/components/session-controls";
import { Button } from "@/components/ui/button";
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Kbd } from "@/components/ui/kbd";
import { Select, SelectContent, SelectItem, SelectSeparator, SelectTrigger, SelectValue } from "@/components/ui/select";
import { SidebarTrigger, useSidebar } from "@/components/ui/sidebar";
import { cn } from "cn";
import { actions, NEW_DRAFT, useLila } from "@/lib/store";

const ADD_PROJECT = "__add__";

const SUGGESTIONS = [
  { label: "查看未提交改动", prompt: "查看当前工作区的 Git 未提交改动，总结主要变化和潜在风险。" },
  { label: "梳理项目架构", prompt: "梳理这个项目的目录结构、核心模块和主要数据流。" },
  { label: "运行测试并排查问题", prompt: "运行项目的测试和类型检查，如果有失败项请帮我定位原因。" },
  { label: "审查最近改动", prompt: "检查最近改动的重点代码，给出清晰可落地的优化建议。" },
] as const;

export function NewSession() {
  const projects = useLila((state) => state.projects);
  const models = useLila((state) => state.models);
  const defaultModelId = useLila((state) => state.defaultModelId);
  const modelsError = useLila((state) => state.modelsError);
  const connected = useLila((state) => state.connection === "connected");
  const options = useLila((state) => state.newSession);
  const { open, isMobile } = useSidebar();

  const projectId = projects.some((project) => project.id === options.projectId) ? options.projectId : projects.at(-1)?.id;
  const model = models.find((item) => item.id === options.modelId) ?? models.find((item) => item.id === defaultModelId) ?? models[0];

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
        <Empty className="flex-1">
          <EmptyHeader>
            <EmptyTitle>先添加一个项目</EmptyTitle>
            <EmptyDescription>选择一个本地代码目录，Agent 会在这个目录里工作。</EmptyDescription>
          </EmptyHeader>
          <EmptyContent>
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
      <div className="flex flex-1 flex-col items-center justify-center px-6 pb-[10vh]">
        <div className="flex w-full max-w-[46rem] flex-col gap-5">
          <div className="flex flex-col items-center gap-2.5 text-center">
            <h1 className="font-heading text-2xl font-medium tracking-tight text-foreground">要做点什么？</h1>
            <Select
              value={projectId ?? null}
              onValueChange={(value) => {
                if (value === ADD_PROJECT) void actions.addProject().then((project) => project && actions.setNewSession({ projectId: project.id }));
                else if (typeof value === "string") actions.setNewSession({ projectId: value });
              }}
              items={projects.map((project) => ({ value: project.id, label: project.name }))}
            >
              <SelectTrigger
                size="sm"
                className="h-7 gap-1.5 rounded-full border-border/80 bg-muted/50 px-3 text-xs font-normal text-muted-foreground shadow-none hover:bg-muted hover:text-foreground dark:bg-muted/60 dark:hover:bg-muted"
                aria-label="项目"
              >
                <Folder className="size-3.5" />
                <SelectValue />
              </SelectTrigger>
              <SelectContent alignItemWithTrigger={false} align="center" className="min-w-72">
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
          </div>
          {!model || modelsError ? <ModelSetup /> : null}
          <div className="flex flex-col gap-2.5">
            <div className="flex scroll-fade-x scrollbar-none flex-wrap justify-center gap-1.5 overflow-x-auto overscroll-x-contain">
              {SUGGESTIONS.map((item) => (
                <Button
                  key={item.label}
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={!connected || !projectId}
                  className="h-7 shrink-0 rounded-full border-border/80 bg-card/70 px-3 text-xs font-normal text-muted-foreground shadow-none hover:bg-muted hover:text-foreground"
                  onClick={() => {
                    actions.setDraft(NEW_DRAFT, item.prompt);
                    if (connected && projectId && model) {
                      void actions.createAndPrompt(projectId, model.id, thinkingLevel);
                    } else {
                      focusComposer();
                    }
                  }}
                >
                  {item.label}
                </Button>
              ))}
            </div>
            <Composer
              draftKey={NEW_DRAFT}
              projectId={projectId}
              disabled={!connected || !projectId}
              sendDisabled={!model}
              placeholder="描述你要完成的任务，@ 引用文件，可以粘贴或拖入图片"
              onSubmit={async () => {
                if (projectId && model) await actions.createAndPrompt(projectId, model.id, thinkingLevel);
              }}
              footer={
                <>
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
          <p className="flex flex-wrap items-center justify-center gap-x-3 gap-y-1 text-xs text-muted-foreground/75">
            <span className="inline-flex items-center gap-1"><Kbd>↵</Kbd> 发送</span>
            <span className="inline-flex items-center gap-1"><Kbd>⇧↵</Kbd> 换行</span>
            <span className="inline-flex items-center gap-1"><Kbd>@</Kbd> 引用文件</span>
            <span className="inline-flex items-center gap-1"><Kbd>/</Kbd> 技能与命令</span>
            <span className="inline-flex items-center gap-1"><Kbd>⌘K</Kbd> 命令面板</span>
          </p>
        </div>
      </div>
    </div>
  );
}
