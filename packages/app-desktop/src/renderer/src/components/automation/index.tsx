import * as React from "react";
import { Clock, Inbox } from "lucide-react";
import { SchedulesPanel } from "@/components/automation/schedules-panel";
import { TasksPanel } from "@/components/automation/tasks-panel";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { cn } from "cn";
import { useNewSessionProjectId } from "@/hooks/use-current-project";
import { actions, type AutomationTab, useLila } from "@/lib/store";

const NAV_ITEMS: Array<{ id: AutomationTab; label: string; icon: React.ComponentType<{ className?: string }> }> = [
  { id: "tasks", label: "任务收件箱", icon: Inbox },
  { id: "schedules", label: "定时任务", icon: Clock },
];

export function AutomationDialog() {
  const open = useLila((state) => state.automationOpen);
  const tab = useLila((state) => state.automationTab);
  const createToken = useLila((state) => state.automationCreateToken);
  const selectedId = useLila((state) => state.selectedId);
  const sessionProjectId = useLila((state) => (state.selectedId ? state.sessions[state.selectedId]?.projectId : undefined));
  const newSessionProjectId = useNewSessionProjectId();
  const activeProjectId = selectedId ? sessionProjectId : newSessionProjectId;
  const openTasksCount = useLila((state) => state.tasks.filter((task) => task.status !== "done").length);
  const enabledSchedulesCount = useLila((state) => state.schedules.filter((job) => job.enabled).length);

  const activeNav = NAV_ITEMS.find((item) => item.id === tab) ?? NAV_ITEMS[0]!;

  return (
    <Dialog open={open} onOpenChange={actions.setAutomationOpen}>
      <DialogContent className="flex h-[min(44rem,calc(100vh-3.5rem))] w-[min(58rem,calc(100vw-3rem))] flex-col gap-0 overflow-hidden p-0 sm:max-w-[58rem] sm:flex-row">
        <aside className="flex shrink-0 flex-col border-b border-border/60 bg-muted/25 p-3 sm:w-48 sm:border-e sm:border-b-0">
          <nav aria-label="自动化与任务" className="flex flex-row gap-1 overflow-x-auto sm:flex-col">
            {NAV_ITEMS.map(({ id, label, icon: Icon }) => {
              const active = id === tab;
              const badge = id === "tasks" ? openTasksCount : enabledSchedulesCount;
              return (
                <Button
                  key={id}
                  type="button"
                  size="sm"
                  variant="ghost"
                  aria-current={active ? "page" : undefined}
                  onClick={() => actions.setAutomationTab(id)}
                  className={cn(
                    "justify-start gap-2.5 px-2.5 font-normal text-muted-foreground hover:text-foreground",
                    active && "bg-accent font-medium text-accent-foreground",
                  )}
                >
                  <Icon className="size-4 shrink-0" />
                  <span className="truncate">{label}</span>
                  {badge > 0 ? (
                    <span className="ms-auto font-mono text-[11px] text-muted-foreground tabular-nums">{badge}</span>
                  ) : null}
                </Button>
              );
            })}
          </nav>
        </aside>

        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
          <header className="flex h-12 shrink-0 items-center px-6 pr-12">
            <DialogTitle className="text-sm font-medium">{activeNav.label}</DialogTitle>
          </header>
          <Separator />
          <ScrollArea className="min-h-0 flex-1">
            <div className="px-6 py-5">
              {tab === "tasks" ? (
                <TasksPanel defaultProjectId={activeProjectId} createToken={tab === "tasks" ? createToken : 0} />
              ) : (
                <SchedulesPanel defaultProjectId={activeProjectId} createToken={tab === "schedules" ? createToken : 0} />
              )}
            </div>
          </ScrollArea>
        </div>
      </DialogContent>
    </Dialog>
  );
}
