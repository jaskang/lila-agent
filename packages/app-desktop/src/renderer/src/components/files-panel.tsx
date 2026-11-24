import * as React from "react";
import { ArrowLeft, AtSign, Code2, ExternalLink, Eye, FileText, FolderOpen, FolderTree, RotateCw, Search, X } from "lucide-react";
import { toast } from "sonner";
import type { ProjectFileContent } from "@lila/protocol";
import { Code, languageOf } from "@/components/code";
import { focusComposer } from "@/components/composer";
import { FileTree } from "@/components/file-tree";
import { Markdown, MarkdownBasePathProvider } from "@/components/markdown";
import { Button } from "@/components/ui/button";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Spinner } from "@/components/ui/spinner";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { actions, useLila } from "@/lib/store";

function isMarkdownPath(path: string): boolean {
  const lower = path.toLowerCase();
  return lower.endsWith(".md") || lower.endsWith(".mdx") || lower.endsWith(".markdown");
}

function fileNameOf(path: string): string {
  return path.split(/[/\\]/).at(-1) ?? path;
}

function dirNameOf(path: string): string | undefined {
  const slash = path.lastIndexOf("/");
  if (slash <= 0) return undefined;
  return path.slice(0, slash);
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function toMediaUrl(localPath: string, basePath?: string): string {
  const params = new URLSearchParams({ path: localPath });
  if (basePath) params.set("base", basePath);
  return `lila-media://local?${params.toString()}`;
}

function HeaderIconButton({
  label,
  disabled,
  onClick,
  children,
}: {
  label: string;
  disabled?: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <Tooltip>
      <TooltipTrigger render={<Button type="button" size="icon-sm" variant="ghost" aria-label={label} disabled={disabled} className="text-muted-foreground hover:text-foreground" onClick={onClick} />}>
        {children}
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

export function FilesPanel({ projectId }: { projectId: string }) {
  const project = useLila((state) => state.projects.find((item) => item.id === projectId));
  const connected = useLila((state) => state.connection === "connected");
  const filesView = useLila((state) => state.filesView);
  const activeFile = useLila((state) => state.activeFile);
  const refreshSeq = useLila((state) => state.workspaceRefreshSeq);

  const [files, setFiles] = React.useState<string[] | undefined>();
  const [treeTruncated, setTreeTruncated] = React.useState(false);
  const [treeLoading, setTreeLoading] = React.useState(false);
  const [treeError, setTreeError] = React.useState<string | undefined>();
  const [filter, setFilter] = React.useState("");

  const [fileContent, setFileContent] = React.useState<ProjectFileContent | undefined>();
  const [fileLoading, setFileLoading] = React.useState(false);
  const [fileError, setFileError] = React.useState<string | undefined>();
  const [markdownMode, setMarkdownMode] = React.useState<"preview" | "source">("preview");

  const treeReqRef = React.useRef(0);
  const fileReqRef = React.useRef(0);

  const loadTree = React.useCallback(
    async (refresh = false) => {
      if (!connected) return;
      const reqId = ++treeReqRef.current;
      setTreeLoading(true);
      setTreeError(undefined);
      try {
        const result = await actions.listFiles(projectId, refresh);
        if (treeReqRef.current !== reqId) return;
        setFiles(result.files);
        setTreeTruncated(result.truncated);
      } catch (error) {
        if (treeReqRef.current !== reqId) return;
        setTreeError(error instanceof Error ? error.message : String(error));
      } finally {
        if (treeReqRef.current === reqId) setTreeLoading(false);
      }
    },
    [connected, projectId],
  );

  const loadFile = React.useCallback(
    async (targetPath: string) => {
      if (!connected) return;
      const reqId = ++fileReqRef.current;
      setFileLoading(true);
      setFileError(undefined);
      try {
        const loaded = await actions.readFile(projectId, targetPath);
        if (fileReqRef.current !== reqId) return;
        setFileContent(loaded);
      } catch (error) {
        if (fileReqRef.current !== reqId) return;
        setFileContent(undefined);
        setFileError(error instanceof Error ? error.message : String(error));
      } finally {
        if (fileReqRef.current === reqId) setFileLoading(false);
      }
    },
    [connected, projectId],
  );

  React.useEffect(() => {
    setFiles(undefined);
    setFilter("");
  }, [projectId]);

  React.useEffect(() => {
    if (filesView === "tree" && files === undefined && connected) {
      void loadTree(false);
    }
  }, [filesView, files, connected, loadTree]);

  React.useEffect(() => {
    if (filesView !== "file" || !activeFile?.path) return;
    setMarkdownMode("preview");
    void loadFile(activeFile.path);
  }, [filesView, activeFile?.path, loadFile]);

  const prevRefreshSeq = React.useRef(refreshSeq);
  React.useEffect(() => {
    if (prevRefreshSeq.current === refreshSeq) return;
    prevRefreshSeq.current = refreshSeq;
    if (filesView === "file" && activeFile?.path) void loadFile(activeFile.path);
    else if (filesView === "tree") void loadTree(true);
  }, [refreshSeq, filesView, activeFile?.path, loadFile, loadTree]);

  const filteredFiles = React.useMemo(() => {
    if (!files) return [];
    const needle = filter.trim().toLowerCase();
    if (!needle) return files;
    return files.filter((path) => path.toLowerCase().includes(needle));
  }, [files, filter]);

  const isMarkdown = activeFile?.path ? isMarkdownPath(activeFile.path) : false;
  const displayPath = fileContent?.path ?? activeFile?.path ?? "";
  const fullPath = fileContent?.fullPath;
  const markdownBasePath = React.useMemo(() => {
    if (fileContent?.fullPath) return dirNameOf(fileContent.fullPath) ?? project?.path;
    return project?.path;
  }, [fileContent?.fullPath, project?.path]);

  const highlightRange = React.useMemo(() => {
    if (!activeFile?.line) return undefined;
    return { start: activeFile.line, end: activeFile.endLine ?? activeFile.line };
  }, [activeFile?.line, activeFile?.endLine]);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <header className="flex h-11 shrink-0 items-center gap-1 border-b border-border/60 px-2.5">
        {filesView === "file" ? (
          <>
            <HeaderIconButton label="查看项目文件树" onClick={() => actions.openFilesTree()}>
              <FolderTree />
            </HeaderIconButton>
            {activeFile?.history.length ? (
              <HeaderIconButton label="返回上一个文件" onClick={() => actions.backFilePreview()}>
                <ArrowLeft />
              </HeaderIconButton>
            ) : null}
            <div className="flex min-w-0 flex-1 flex-col px-1">
              <span className="truncate text-sm font-medium" title={displayPath}>
                {fileNameOf(displayPath)}
              </span>
              {displayPath.includes("/") ? (
                <span className="truncate font-mono text-[11px] leading-tight text-muted-foreground" title={displayPath}>
                  {displayPath}
                </span>
              ) : null}
            </div>
            {isMarkdown && fileContent && !fileContent.isBinary ? (
              <HeaderIconButton
                label={markdownMode === "preview" ? "查看源码" : "查看预览"}
                onClick={() => setMarkdownMode((current) => (current === "preview" ? "source" : "preview"))}
              >
                {markdownMode === "preview" ? <Code2 /> : <Eye />}
              </HeaderIconButton>
            ) : null}
            {displayPath ? (
              <HeaderIconButton
                label="引用到对话"
                onClick={() => {
                  actions.mentionFileInDraft(displayPath);
                  focusComposer();
                }}
              >
                <AtSign />
              </HeaderIconButton>
            ) : null}
            {fullPath ? (
              <>
                <HeaderIconButton
                  label="用默认应用打开"
                  onClick={() => void window.lila.openPath(fullPath).catch((error: unknown) => toast.error(error instanceof Error ? error.message : String(error)))}
                >
                  <ExternalLink />
                </HeaderIconButton>
                <HeaderIconButton label="在访达中显示" onClick={() => window.lila.revealPath(fullPath)}>
                  <FolderOpen />
                </HeaderIconButton>
              </>
            ) : null}
            <HeaderIconButton
              label="重新加载"
              disabled={fileLoading || !activeFile?.path}
              onClick={() => {
                if (activeFile?.path) void loadFile(activeFile.path);
              }}
            >
              {fileLoading ? <Spinner /> : <RotateCw />}
            </HeaderIconButton>
          </>
        ) : (
          <>
            <div className="flex min-w-0 flex-1 items-center gap-2 px-1.5">
              <FolderTree className="size-4 shrink-0 text-muted-foreground" />
              <h2 className="truncate text-sm font-medium">项目文件</h2>
              {files ? <span className="text-xs text-muted-foreground tabular-nums">{files.length}{treeTruncated ? "+" : ""}</span> : null}
            </div>
            {activeFile?.path ? (
              <Button
                type="button"
                size="xs"
                variant="ghost"
                className="max-w-36 text-muted-foreground hover:text-foreground"
                title={`回到 ${activeFile.path}`}
                onClick={() => actions.openFilePreview(activeFile.path)}
              >
                <FileText />
                <span className="truncate">{fileNameOf(activeFile.path)}</span>
              </Button>
            ) : null}
            <HeaderIconButton label="刷新文件树" disabled={treeLoading} onClick={() => void loadTree(true)}>
              {treeLoading ? <Spinner /> : <RotateCw />}
            </HeaderIconButton>
          </>
        )}
        <HeaderIconButton label="关闭文件面板" onClick={() => actions.closeRightPanel()}>
          <X />
        </HeaderIconButton>
      </header>

      {filesView === "tree" ? (
        <div className="flex min-h-0 flex-1 flex-col">
          <div className="border-b border-border/60 p-2.5">
            <div className="relative">
              <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
              <Input
                value={filter}
                placeholder="筛选文件…"
                aria-label="筛选项目文件"
                className="h-8 pl-8 pr-7 text-xs"
                onChange={(event) => setFilter(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Escape" && filter) {
                    event.preventDefault();
                    setFilter("");
                  }
                }}
              />
              {filter ? (
                <button
                  type="button"
                  aria-label="清空筛选"
                  onClick={() => setFilter("")}
                  className="absolute top-1/2 right-2 -translate-y-1/2 rounded-sm p-0.5 text-muted-foreground hover:bg-muted hover:text-foreground"
                >
                  <X className="size-3.5" />
                </button>
              ) : null}
            </div>
          </div>
          {treeLoading && !files ? (
            <div className="flex flex-1 items-center justify-center gap-2 text-sm text-muted-foreground">
              <Spinner />
              正在读取项目文件…
            </div>
          ) : treeError ? (
            <Empty className="flex-1">
              <EmptyHeader>
                <EmptyTitle>读取文件列表失败</EmptyTitle>
                <EmptyDescription>{treeError}</EmptyDescription>
              </EmptyHeader>
              <Button variant="outline" size="sm" onClick={() => void loadTree(true)}>
                <RotateCw />
                重试
              </Button>
            </Empty>
          ) : !filteredFiles.length ? (
            <Empty className="flex-1">
              <EmptyHeader>
                <EmptyTitle>{filter ? "没有匹配的文件" : "目录为空"}</EmptyTitle>
                <EmptyDescription>{filter ? "试试换一个文件名或路径关键字。" : project?.path}</EmptyDescription>
              </EmptyHeader>
            </Empty>
          ) : (
            <ScrollArea className="min-h-0 flex-1">
              <FileTree
                className="p-2"
                entries={filteredFiles.map((path) => ({ path }))}
                selectedPath={activeFile?.path}
                expandAll={Boolean(filter.trim())}
                onSelect={(path) => actions.openFilePreview(path)}
              />
              {treeTruncated && !filter ? (
                <p className="px-3 pb-3 text-xs text-muted-foreground">仅显示前 {files?.length} 个文件，可输入关键字筛选。</p>
              ) : null}
            </ScrollArea>
          )}
        </div>
      ) : fileLoading && !fileContent ? (
        <div className="flex flex-1 items-center justify-center gap-2 text-sm text-muted-foreground">
          <Spinner />
          正在加载文件…
        </div>
      ) : fileError ? (
        <Empty className="flex-1">
          <EmptyHeader>
            <EmptyTitle>无法预览该文件</EmptyTitle>
            <EmptyDescription className="max-w-sm break-words">{fileError}</EmptyDescription>
          </EmptyHeader>
          <div className="flex items-center gap-2">
            {activeFile?.path ? (
              <Button variant="outline" size="sm" onClick={() => void loadFile(activeFile.path)}>
                <RotateCw />
                重试
              </Button>
            ) : null}
            <Button variant="ghost" size="sm" onClick={() => actions.openFilesTree()}>
              <FolderTree />
              查看文件树
            </Button>
          </div>
        </Empty>
      ) : fileContent ? (
        fileContent.isImage ? (
          <ScrollArea className="min-h-0 flex-1">
            <div className="flex flex-col items-center gap-3 p-5">
              <img
                src={toMediaUrl(fileContent.fullPath, project?.path)}
                alt={fileNameOf(fileContent.path)}
                className="max-h-[75vh] max-w-full rounded-xl border bg-muted/20 object-contain"
              />
              <span className="font-mono text-xs text-muted-foreground tabular-nums">{formatBytes(fileContent.size)}</span>
            </div>
          </ScrollArea>
        ) : fileContent.isBinary ? (
          <Empty className="flex-1">
            <EmptyHeader>
              <EmptyTitle>二进制文件</EmptyTitle>
              <EmptyDescription>
                {fileNameOf(fileContent.path)}（{formatBytes(fileContent.size)}）暂不支持内联预览。
              </EmptyDescription>
            </EmptyHeader>
            <div className="flex items-center gap-2">
              <Button
                variant="outline"
                size="sm"
                onClick={() => void window.lila.openPath(fileContent.fullPath).catch((error: unknown) => toast.error(error instanceof Error ? error.message : String(error)))}
              >
                <ExternalLink />
                用默认应用打开
              </Button>
              <Button variant="ghost" size="sm" onClick={() => window.lila.revealPath(fileContent.fullPath)}>
                <FolderOpen />
                在访达中显示
              </Button>
            </div>
          </Empty>
        ) : isMarkdown && markdownMode === "preview" ? (
          <ScrollArea className="min-h-0 flex-1">
            <div className="px-5 py-4">
              {fileContent.truncated ? (
                <p className="mb-3 rounded-lg border bg-muted/40 px-3 py-1.5 text-xs text-muted-foreground">
                  文件较大（{formatBytes(fileContent.size)}），仅预览前 512 KB 内容。
                </p>
              ) : null}
              <MarkdownBasePathProvider value={markdownBasePath}>
                <Markdown text={fileContent.content} />
              </MarkdownBasePathProvider>
            </div>
          </ScrollArea>
        ) : (
          <ScrollArea className="min-h-0 flex-1">
            <div className="p-3">
              {fileContent.truncated ? (
                <p className="mb-2 rounded-lg border bg-muted/40 px-3 py-1.5 text-xs text-muted-foreground">
                  文件较大（{formatBytes(fileContent.size)}），仅显示前 512 KB 内容。
                </p>
              ) : null}
              <Code
                code={fileContent.content}
                language={languageOf(fileContent.path)}
                maxHeight={undefined}
                numbered
                highlightRange={highlightRange}
              />
            </div>
          </ScrollArea>
        )
      ) : null}
    </div>
  );
}
