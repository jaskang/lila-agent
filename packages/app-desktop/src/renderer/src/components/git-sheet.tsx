import * as React from "react";
import { ArrowDown, ArrowUp, ExternalLink, FileCode, FolderOpen, GitBranch, RotateCw, TriangleAlert } from "lucide-react";
import { toast } from "sonner";
import type { GitFileChange, GitFileDiff, GitFileStatus, GitStatus } from "@lila/protocol";
import { DiffView } from "@/components/code";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { cn } from "cn";
import { actions, useLila } from "@/lib/store";

type Scope = "all" | "staged" | "unstaged";

const STATUS_BADGE: Record<GitFileStatus, { letter: string; label: string; className: string }> = {
  modified: { letter: "M", label: "已修改", className: "text-amber-600 dark:text-amber-400" },
  added: { letter: "A", label: "新增", className: "text-emerald-600 dark:text-emerald-400" },
  deleted: { letter: "D", label: "删除", className: "text-destructive" },
  renamed: { letter: "R", label: "重命名", className: "text-sky-600 dark:text-sky-400" },
  copied: { letter: "C", label: "复制", className: "text-sky-600 dark:text-sky-400" },
  untracked: { letter: "?", label: "未跟踪", className: "text-emerald-600 dark:text-emerald-400" },
  unmerged: { letter: "U", label: "冲突", className: "text-destructive" },
};

function splitPath(path: string): { name: string; dir: string } {
  const slash = path.lastIndexOf("/");
  if (slash < 0) return { name: path, dir: "" };
  return { name: path.slice(slash + 1), dir: path.slice(0, slash) };
}

function joinPath(root: string, rel: string): string {
  return `${root.replace(/\/+$/, "")}/${rel.replace(/^\.\//, "")}`;
}

export function GitSheet({
  projectId,
  open,
  onOpenChange,
  onStatusLoaded,
}: {
  projectId: string | undefined;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onStatusLoaded?: (status: GitStatus) => void;
}) {
  const project = useLila((state) => state.projects.find((item) => item.id === projectId));
  const [status, setStatus] = React.useState<GitStatus>();
  const [error, setError] = React.useState<string>();
  const [loading, setLoading] = React.useState(false);
  const [scope, setScope] = React.useState<Scope>("all");
  const [selectedPath, setSelectedPath] = React.useState<string>();
  const [diff, setDiff] = React.useState<GitFileDiff>();
  const [diffLoading, setDiffLoading] = React.useState(false);
  const [diffError, setDiffError] = React.useState<string>();
  const [refreshNonce, setRefreshNonce] = React.useState(0);

  const onStatusLoadedRef = React.useRef(onStatusLoaded);
  onStatusLoadedRef.current = onStatusLoaded;
  const statusReqRef = React.useRef(0);

  React.useEffect(() => {
    setStatus(undefined);
    setError(undefined);
    setSelectedPath(undefined);
    setDiff(undefined);
    setDiffError(undefined);
    setScope("all");
  }, [projectId]);

  const loadStatus = React.useCallback(async () => {
    if (!projectId) return;
    const reqId = ++statusReqRef.current;
    setLoading(true);
    setError(undefined);
    try {
      const next = await actions.gitStatus(projectId);
      if (statusReqRef.current !== reqId) return;
      setStatus(next);
      onStatusLoadedRef.current?.(next);
      setRefreshNonce((nonce) => nonce + 1);
    } catch (failure) {
      if (statusReqRef.current !== reqId) return;
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      if (statusReqRef.current === reqId) setLoading(false);
    }
  }, [projectId]);

  React.useEffect(() => {
    if (open) void loadStatus();
  }, [open, loadStatus]);

  const files = React.useMemo<GitFileChange[]>(() => {
    const all = status?.files ?? [];
    if (scope === "staged") return all.filter((file) => file.staged);
    if (scope === "unstaged") return all.filter((file) => file.unstaged);
    return all;
  }, [status, scope]);

  const hasStaged = Boolean(status?.files.some((file) => file.staged));
  const hasUnstaged = Boolean(status?.files.some((file) => file.unstaged));

  React.useEffect(() => {
    if ((scope === "staged" && !hasStaged) || (scope === "unstaged" && !hasUnstaged)) {
      setScope("all");
    }
  }, [scope, hasStaged, hasUnstaged]);

  React.useEffect(() => {
    if (!files.length) {
      setSelectedPath(undefined);
      setDiff(undefined);
      return;
    }
    if (!selectedPath || !files.some((file) => file.path === selectedPath)) {
      setSelectedPath(files[0]?.path);
    }
  }, [files, selectedPath]);

  const selectedFile = files.find((file) => file.path === selectedPath);

  React.useEffect(() => {
    if (!open || !projectId || !selectedPath) return;
    let cancelled = false;
    setDiffLoading(true);
    setDiffError(undefined);
    const staged = scope === "staged" ? true : scope === "unstaged" ? false : undefined;
    void actions
      .gitDiff(projectId, selectedPath, staged)
      .then((result) => {
        if (!cancelled) setDiff(result);
      })
      .catch((failure: unknown) => {
        if (!cancelled) {
          setDiff(undefined);
          setDiffError(failure instanceof Error ? failure.message : String(failure));
        }
      })
      .finally(() => {
        if (!cancelled) setDiffLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, projectId, selectedPath, scope, refreshNonce]);

  const fullPath = project && selectedFile ? joinPath(project.path, selectedFile.path) : undefined;

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="gap-0 data-[side=right]:w-[min(56rem,92vw)] data-[side=right]:sm:max-w-none">
        <SheetHeader className="border-b pr-12">
          <div className="flex flex-wrap items-center gap-2">
            <SheetTitle>工作区改动</SheetTitle>
            {status?.branch ? (
              <Badge variant="secondary" className="gap-1 font-mono text-xs font-normal">
                <GitBranch className="size-3" />
                {status.branch}
              </Badge>
            ) : null}
            {status?.ahead ? (
              <Badge variant="outline" className="gap-0.5 font-mono text-xs tabular-nums">
                <ArrowUp className="size-3" />
                {status.ahead}
              </Badge>
            ) : null}
            {status?.behind ? (
              <Badge variant="outline" className="gap-0.5 font-mono text-xs tabular-nums">
                <ArrowDown className="size-3" />
                {status.behind}
              </Badge>
            ) : null}
            <Button type="button" size="xs" variant="ghost" className="ms-auto text-muted-foreground" disabled={loading} onClick={() => void loadStatus()}>
              {loading ? <Spinner /> : <RotateCw />}
              刷新
            </Button>
          </div>
          <SheetDescription className="truncate font-mono text-xs" title={project?.path}>
            {project?.path ?? "未选择项目"}
          </SheetDescription>
        </SheetHeader>

        {!status ? (
          <Empty className="flex-1">
            <EmptyHeader>
              <EmptyTitle>{error ? "读取失败" : "正在读取 Git 状态…"}</EmptyTitle>
              {error ? <EmptyDescription>{error}</EmptyDescription> : null}
            </EmptyHeader>
            {error ? (
              <Button variant="outline" onClick={() => void loadStatus()}>
                <RotateCw />
                重试
              </Button>
            ) : (
              <Spinner />
            )}
          </Empty>
        ) : !status.isRepo ? (
          <Empty className="flex-1">
            <EmptyHeader>
              <EmptyTitle>当前目录不是 Git 仓库</EmptyTitle>
              <EmptyDescription>在 Git 仓库里工作时，可以在这里随时查看未提交的改动和差异。</EmptyDescription>
            </EmptyHeader>
          </Empty>
        ) : status.files.length === 0 ? (
          <Empty className="flex-1">
            <EmptyHeader>
              <EmptyTitle>工作区很干净</EmptyTitle>
              <EmptyDescription>当前没有未提交的文件改动。</EmptyDescription>
            </EmptyHeader>
          </Empty>
        ) : (
          <div className="flex min-h-0 flex-1 flex-col">
            {hasStaged && hasUnstaged ? (
              <div className="flex items-center gap-2 border-b px-4 py-2">
                <Tabs value={scope} onValueChange={(value) => setScope(value as Scope)}>
                  <TabsList>
                    <TabsTrigger value="all">全部 · {status.files.length}</TabsTrigger>
                    <TabsTrigger value="staged">已暂存 · {status.files.filter((file) => file.staged).length}</TabsTrigger>
                    <TabsTrigger value="unstaged">未暂存 · {status.files.filter((file) => file.unstaged).length}</TabsTrigger>
                  </TabsList>
                </Tabs>
              </div>
            ) : null}
            <div className="grid min-h-0 flex-1 grid-cols-1 md:grid-cols-[17rem_minmax(0,1fr)]">
              <ScrollArea className="max-h-56 min-h-0 border-b md:max-h-none md:border-r md:border-b-0">
                <div className="flex flex-col gap-0.5 p-2">
                  {files.map((file) => {
                    const badge = STATUS_BADGE[file.status];
                    const { name, dir } = splitPath(file.path);
                    const active = file.path === selectedPath;
                    return (
                      <button
                        key={file.path}
                        type="button"
                        onClick={() => setSelectedPath(file.path)}
                        className={cn(
                          "flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left transition-colors hover:bg-muted/70",
                          active && "bg-muted text-foreground",
                        )}
                      >
                        <span className={cn("w-3.5 shrink-0 text-center font-mono text-xs font-semibold", badge.className)} title={badge.label}>
                          {badge.letter}
                        </span>
                        <div className="flex min-w-0 flex-1 flex-col">
                          <span className="truncate font-mono text-xs font-medium">{name}</span>
                          {dir ? <span className="truncate font-mono text-[11px] text-muted-foreground">{dir}</span> : null}
                        </div>
                        {file.added !== undefined || file.removed !== undefined ? (
                          <span className="shrink-0 font-mono text-[11px] tabular-nums">
                            {file.added ? <span className="text-emerald-600 dark:text-emerald-400">+{file.added}</span> : null}
                            {file.added && file.removed ? " " : null}
                            {file.removed ? <span className="text-destructive">−{file.removed}</span> : null}
                          </span>
                        ) : null}
                      </button>
                    );
                  })}
                </div>
              </ScrollArea>

              <ScrollArea className="min-h-0">
                <div className="flex flex-col gap-3 p-4">
                  {selectedFile ? (
                    <div className="flex flex-wrap items-center gap-2">
                      <FileCode className="size-4 shrink-0 text-muted-foreground" />
                      <span className="min-w-0 flex-1 truncate font-mono text-xs font-medium" title={selectedFile.path}>
                        {selectedFile.from ? `${selectedFile.from} → ${selectedFile.path}` : selectedFile.path}
                      </span>
                      <Badge variant="outline" className={cn("text-xs font-normal", STATUS_BADGE[selectedFile.status].className)}>
                        {STATUS_BADGE[selectedFile.status].label}
                      </Badge>
                      {fullPath && selectedFile.status !== "deleted" ? (
                        <Button
                          type="button"
                          size="xs"
                          variant="ghost"
                          className="h-6 text-muted-foreground"
                          onClick={() => void window.lila.openPath(fullPath).catch((err: unknown) => toast.error(err instanceof Error ? err.message : String(err)))}
                        >
                          <ExternalLink />
                          打开
                        </Button>
                      ) : null}
                      {fullPath ? (
                        <Button type="button" size="xs" variant="ghost" className="h-6 text-muted-foreground" onClick={() => window.lila.revealPath(fullPath)}>
                          <FolderOpen />
                          访达
                        </Button>
                      ) : null}
                    </div>
                  ) : null}

                  {diffLoading ? (
                    <div className="flex items-center justify-center py-16">
                      <Spinner className="text-muted-foreground" />
                    </div>
                  ) : diffError ? (
                    <Alert variant="destructive">
                      <TriangleAlert />
                      <AlertTitle>读取差异失败</AlertTitle>
                      <AlertDescription>{diffError}</AlertDescription>
                    </Alert>
                  ) : diff?.binary ? (
                    <p className="py-12 text-center text-xs text-muted-foreground">二进制文件，无法显示文本差异。</p>
                  ) : diff?.patch ? (
                    <>
                      {diff.truncated ? (
                        <Alert>
                          <TriangleAlert />
                          <AlertTitle>差异内容过大</AlertTitle>
                          <AlertDescription>仅展示前部内容，完整改动可在外部编辑器中查看。</AlertDescription>
                        </Alert>
                      ) : null}
                      <DiffView patch={diff.patch} path={selectedFile?.path ?? diff.path} maxHeight="calc(100vh - 13rem)" />
                    </>
                  ) : (
                    <p className="py-12 text-center text-xs text-muted-foreground">该范围下没有文本差异。</p>
                  )}
                </div>
              </ScrollArea>
            </div>
          </div>
        )}
      </SheetContent>
    </Sheet>
  );
}
