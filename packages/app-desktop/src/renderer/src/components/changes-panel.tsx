import * as React from "react";
import { ArrowLeft, AtSign, ExternalLink, FileCode, FolderOpen, FolderTree, GitBranch, List, RotateCw, TriangleAlert, X } from "lucide-react";
import { toast } from "sonner";
import type { GitFileChange, GitFileDiff, GitFileStatus, GitStatus } from "@lila/protocol";
import { DiffView } from "@/components/code";
import { focusComposer } from "@/components/composer";
import { FileTree } from "@/components/file-tree";
import { IconButton } from "@/components/icon-button";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { cn } from "cn";
import { actions, useLila } from "@/lib/store";

type Scope = "all" | "staged" | "unstaged";
type ViewMode = "tree" | "list";
const VIEW_MODE_KEY = "lila.gitViewMode";

function readViewMode(): ViewMode {
  try {
    return localStorage.getItem(VIEW_MODE_KEY) === "list" ? "list" : "tree";
  } catch {
    return "tree";
  }
}

const STATUS_BADGE: Record<GitFileStatus, { letter: string; label: string; className: string }> = {
  modified: { letter: "M", label: "已修改", className: "text-warning-foreground" },
  added: { letter: "A", label: "新增", className: "text-success-foreground" },
  deleted: { letter: "D", label: "删除", className: "text-destructive-foreground" },
  renamed: { letter: "R", label: "重命名", className: "text-info-foreground" },
  copied: { letter: "C", label: "复制", className: "text-info-foreground" },
  untracked: { letter: "?", label: "未跟踪", className: "text-success-foreground" },
  unmerged: { letter: "U", label: "冲突", className: "text-destructive-foreground" },
};

function splitPath(path: string): { name: string; dir: string } {
  const slash = path.lastIndexOf("/");
  if (slash < 0) return { name: path, dir: "" };
  return { name: path.slice(slash + 1), dir: path.slice(0, slash) };
}

function joinPath(root: string, rel: string): string {
  return `${root.replace(/\/+$/, "")}/${rel.replace(/^\.\//, "")}`;
}

/**
 * Workspace Git changes panel inside the unified right card.
 * Uses @container queries to show a clean single-column drill-down at default width
 * and automatically expand to a split file-tree + diff view when dragged wider.
 */
export function ChangesPanel({ projectId }: { projectId: string }) {
  const project = useLila((state) => state.projects.find((item) => item.id === projectId));
  const connected = useLila((state) => state.connection === "connected");
  const refreshSeq = useLila((state) => state.workspaceRefreshSeq);
  const activeChangePath = useLila((state) => state.activeChangePath);
  const returnToPreviewId = useLila((state) => state.returnToPreviewId);
  const [status, setStatus] = React.useState<GitStatus>();
  const [error, setError] = React.useState<string>();
  const [loading, setLoading] = React.useState(false);
  const [scope, setScope] = React.useState<Scope>("all");
  const [viewMode, setViewMode] = React.useState<ViewMode>(readViewMode);
  const [selectedPath, setSelectedPath] = React.useState<string>();
  const [showDiff, setShowDiff] = React.useState(false);
  const [diff, setDiff] = React.useState<GitFileDiff>();
  const [diffLoading, setDiffLoading] = React.useState(false);
  const [diffError, setDiffError] = React.useState<string>();
  const [refreshNonce, setRefreshNonce] = React.useState(0);

  const statusReqRef = React.useRef(0);
  const diffReqRef = React.useRef(0);

  const toggleViewMode = React.useCallback(() => {
    setViewMode((current) => {
      const next: ViewMode = current === "tree" ? "list" : "tree";
      try {
        localStorage.setItem(VIEW_MODE_KEY, next);
      } catch {
        // Ignore storage write errors.
      }
      return next;
    });
  }, []);

  React.useEffect(() => {
    setStatus(undefined);
    setError(undefined);
    setSelectedPath(undefined);
    setShowDiff(false);
    setDiff(undefined);
    setDiffError(undefined);
    setScope("all");
  }, [projectId]);

  const loadStatus = React.useCallback(async () => {
    if (!connected) return;
    const reqId = ++statusReqRef.current;
    setLoading(true);
    setError(undefined);
    try {
      const next = await actions.gitStatus(projectId);
      if (statusReqRef.current !== reqId) return;
      setStatus(next);
      actions.setGitChangeCount(projectId, next.isRepo ? next.files.length : undefined);
      setRefreshNonce((nonce) => nonce + 1);
    } catch (failure) {
      if (statusReqRef.current !== reqId) return;
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      if (statusReqRef.current === reqId) setLoading(false);
    }
  }, [connected, projectId]);

  React.useEffect(() => {
    void loadStatus();
  }, [loadStatus, refreshSeq]);

  const files = React.useMemo(() => {
    const all = status?.files ?? [];
    if (scope === "staged") return all.filter((file) => file.staged);
    if (scope === "unstaged") return all.filter((file) => file.unstaged);
    return all;
  }, [status, scope]);

  React.useEffect(() => {
    if (!files.length) {
      setSelectedPath(undefined);
      setShowDiff(false);
      return;
    }
    if (activeChangePath && files.some((file) => file.path === activeChangePath)) {
      setSelectedPath(activeChangePath);
      setShowDiff(true);
      actions.setActiveChangePath(undefined);
      return;
    }
    if (!selectedPath || !files.some((file) => file.path === selectedPath)) {
      setSelectedPath(files[0]!.path);
    }
  }, [files, selectedPath, activeChangePath]);

  const selectedFile: GitFileChange | undefined = files.find((file) => file.path === selectedPath);
  const effectiveStaged = scope === "staged" ? true : scope === "unstaged" ? false : undefined;

  React.useEffect(() => {
    if (!selectedFile || !connected) {
      setDiff(undefined);
      setDiffError(undefined);
      return;
    }
    const reqId = ++diffReqRef.current;
    setDiffLoading(true);
    setDiffError(undefined);
    void actions
      .gitDiff(projectId, selectedFile.path, effectiveStaged)
      .then((next) => {
        if (diffReqRef.current !== reqId) return;
        setDiff(next);
      })
      .catch((failure: unknown) => {
        if (diffReqRef.current !== reqId) return;
        setDiff(undefined);
        setDiffError(failure instanceof Error ? failure.message : String(failure));
      })
      .finally(() => {
        if (diffReqRef.current === reqId) setDiffLoading(false);
      });
  }, [projectId, selectedFile, effectiveStaged, refreshNonce, connected]);

  const hasStaged = Boolean(status?.files.some((file) => file.staged));
  const hasUnstaged = Boolean(status?.files.some((file) => file.unstaged));
  const fullPath = project && selectedFile ? joinPath(project.path, selectedFile.path) : undefined;

  const handleSelectFile = (path: string) => {
    setSelectedPath(path);
    setShowDiff(true);
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <header className="flex h-11 shrink-0 items-center gap-1 border-b border-border/60 px-2.5">
        {returnToPreviewId ? (
          <Button
            type="button"
            variant="ghost"
            size="xs"
            className="text-muted-foreground hover:text-foreground"
            onClick={() => actions.returnToSubsession()}
          >
            <ArrowLeft />
            返回子会话
          </Button>
        ) : showDiff && selectedFile ? (
          <span className="@min-[42rem]:hidden">
            <IconButton label="返回改动列表" onClick={() => setShowDiff(false)}>
              <ArrowLeft />
            </IconButton>
          </span>
        ) : null}
        <div className="flex min-w-0 flex-1 items-baseline gap-2 px-1">
          <GitBranch className="size-4 shrink-0 self-center text-muted-foreground" />
          <h2 className="truncate text-sm font-medium">工作区改动</h2>
          {status?.isRepo && status.branch ? (
            <span className="hidden truncate font-mono text-xs text-muted-foreground @min-[24rem]:inline">
              {status.branch}
              {status.ahead ? ` ↑${status.ahead}` : ""}
              {status.behind ? ` ↓${status.behind}` : ""}
            </span>
          ) : null}
        </div>
        <IconButton label={viewMode === "tree" ? "切换为列表视图" : "切换为目录树视图"} onClick={toggleViewMode}>
          {viewMode === "tree" ? <List /> : <FolderTree />}
        </IconButton>
        <IconButton label="刷新改动" disabled={loading} onClick={() => void loadStatus()}>
          {loading ? <Spinner /> : <RotateCw />}
        </IconButton>
        <IconButton label="关闭改动面板" onClick={() => actions.closeRightPanel()}>
          <X />
        </IconButton>
      </header>

      {!status ? (
        <Empty className="flex-1">
          <EmptyHeader>
            <EmptyTitle>{error ? "读取失败" : "正在读取 Git 状态…"}</EmptyTitle>
            {error ? <EmptyDescription>{error}</EmptyDescription> : null}
          </EmptyHeader>
          {error ? (
            <Button variant="outline" size="sm" onClick={() => void loadStatus()}>
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
            <EmptyTitle>非 Git 仓库</EmptyTitle>
          </EmptyHeader>
        </Empty>
      ) : status.files.length === 0 ? (
        <Empty className="flex-1">
          <EmptyHeader>
            <EmptyTitle>无未提交改动</EmptyTitle>
          </EmptyHeader>
        </Empty>
      ) : (
        <div className="flex min-h-0 flex-1 flex-col">
          {hasStaged && hasUnstaged ? (
            <div className={cn("flex items-center gap-2 border-b border-border/60 px-3 py-2", showDiff && "hidden @min-[42rem]:flex")}>
              <Tabs value={scope} onValueChange={(value) => setScope(value as Scope)}>
                <TabsList>
                  <TabsTrigger value="all">全部 · {status.files.length}</TabsTrigger>
                  <TabsTrigger value="staged">已暂存 · {status.files.filter((file) => file.staged).length}</TabsTrigger>
                  <TabsTrigger value="unstaged">未暂存 · {status.files.filter((file) => file.unstaged).length}</TabsTrigger>
                </TabsList>
              </Tabs>
            </div>
          ) : null}

          <div className="grid min-h-0 flex-1 grid-cols-1 @min-[42rem]:grid-cols-[16rem_minmax(0,1fr)]">
            <ScrollArea className={cn("min-h-0 @min-[42rem]:border-e", showDiff ? "hidden @min-[42rem]:block" : "block")}>
              {viewMode === "tree" ? (
                <FileTree
                  className="p-2"
                  entries={files.map((file) => {
                    const badge = STATUS_BADGE[file.status];
                    return {
                      path: file.path,
                      title: `${badge.label} · ${file.path}`,
                      leading: (
                        <span className={cn("w-3.5 shrink-0 text-center font-mono text-xs font-semibold", badge.className)} title={badge.label}>
                          {badge.letter}
                        </span>
                      ),
                      trailing:
                        file.added !== undefined || file.removed !== undefined ? (
                          <span className="shrink-0 font-mono text-[11px] tabular-nums">
                            {file.added ? <span className="text-success-foreground">+{file.added}</span> : null}
                            {file.added && file.removed ? " " : null}
                            {file.removed ? <span className="text-destructive-foreground">−{file.removed}</span> : null}
                          </span>
                        ) : null,
                    };
                  })}
                  selectedPath={selectedPath}
                  defaultOpen={files.length <= 20}
                  onSelect={handleSelectFile}
                />
              ) : (
                <div className="flex flex-col gap-0.5 p-2">
                  {files.map((file) => {
                    const badge = STATUS_BADGE[file.status];
                    const { name, dir } = splitPath(file.path);
                    const active = file.path === selectedPath;
                    return (
                      <button
                        key={file.path}
                        type="button"
                        onClick={() => handleSelectFile(file.path)}
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
                            {file.added ? <span className="text-success-foreground">+{file.added}</span> : null}
                            {file.added && file.removed ? " " : null}
                            {file.removed ? <span className="text-destructive-foreground">−{file.removed}</span> : null}
                          </span>
                        ) : null}
                      </button>
                    );
                  })}
                </div>
              )}
            </ScrollArea>

            <ScrollArea className={cn("min-h-0", showDiff ? "block" : "hidden @min-[42rem]:block")}>
              <div className="flex flex-col gap-3 p-3.5">
                {selectedFile ? (
                  <>
                    <div className="flex flex-wrap items-center gap-1.5 border-b border-border/60 pb-2.5">
                      <FileCode className="size-4 shrink-0 text-muted-foreground" />
                      <span className="min-w-0 flex-1 truncate font-mono text-xs font-medium" title={selectedFile.path}>
                        {selectedFile.path}
                      </span>
                      {selectedFile.from ? (
                        <span className="truncate font-mono text-[11px] text-muted-foreground">← {selectedFile.from}</span>
                      ) : null}
                      {fullPath && selectedFile.status !== "deleted" ? (
                        <div className="flex items-center gap-0.5">
                          <IconButton
                            label="引用到对话"
                            onClick={() => {
                              actions.mentionFileInDraft(selectedFile.path);
                              focusComposer();
                            }}
                          >
                            <AtSign />
                          </IconButton>
                          <IconButton label="在文件面板预览" onClick={() => actions.openFilePreview(fullPath)}>
                            <FileCode />
                          </IconButton>
                          <IconButton
                            label="用默认应用打开"
                            onClick={() =>
                              void window.lila.openPath(fullPath).catch((err: unknown) => toast.error(err instanceof Error ? err.message : String(err)))
                            }
                          >
                            <ExternalLink />
                          </IconButton>
                          <IconButton label="在文件夹中显示" onClick={() => void actions.revealPath(fullPath)}>
                            <FolderOpen />
                          </IconButton>
                        </div>
                      ) : null}
                    </div>

                    {diffLoading ? (
                      <div className="flex items-center justify-center gap-2 py-12 text-sm text-muted-foreground">
                        <Spinner />
                        正在加载差异…
                      </div>
                    ) : diffError ? (
                      <Alert variant="destructive">
                        <TriangleAlert />
                        <AlertTitle>读取差异失败</AlertTitle>
                        <AlertDescription>{diffError}</AlertDescription>
                      </Alert>
                    ) : diff?.binary ? (
                      <Empty className="py-12">
                        <EmptyHeader>
                          <EmptyTitle>二进制文件</EmptyTitle>
                        </EmptyHeader>
                      </Empty>
                    ) : diff?.patch ? (
                      <>
                        {diff.truncated ? (
                          <p className="text-xs text-muted-foreground">差异已截断</p>
                        ) : null}
                        <DiffView patch={diff.patch} path={selectedFile.path} maxHeight="none" />
                      </>
                    ) : (
                      <Empty className="py-12">
                        <EmptyHeader>
                          <EmptyTitle>无文本差异</EmptyTitle>
                        </EmptyHeader>
                      </Empty>
                    )}
                  </>
                ) : null}
              </div>
            </ScrollArea>
          </div>
        </div>
      )}
    </div>
  );
}
