import * as React from "react";
import {
  ArrowLeft,
  AtSign,
  Code2,
  Copy,
  ExternalLink,
  Eye,
  FolderOpen,
  FolderTree,
  MoreHorizontal,
  PanelLeftClose,
  PanelLeftOpen,
  RotateCw,
  Search,
  X,
} from "lucide-react";
import { toast } from "sonner";
import type { GitFileStatus, ProjectFileContent } from "@lila/protocol";
import { focusComposer } from "@/components/composer";
import { dirNameOf, fileNameOf, FilePreviewPane, isMarkdownPath, isSvgPath } from "@/components/files/file-preview-pane";
import { FileTreePane } from "@/components/files/file-tree-pane";
import { IconButton } from "@/components/icon-button";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "cn";
import { actions, useLila } from "@/lib/store";

const TREE_OPEN_STORAGE_KEY = "lila.filesTreeOpen";
const SPLIT_MIN_WIDTH_PX = 608;

function readTreeOpen(): boolean {
  try {
    const saved = localStorage.getItem(TREE_OPEN_STORAGE_KEY);
    if (saved === "false") return false;
    if (saved === "true") return true;
  } catch {
    // Ignore storage read errors.
  }
  return true;
}

export function FilesPanel({ projectId }: { projectId: string }) {
  const project = useLila((state) => state.projects.find((item) => item.id === projectId));
  const connected = useLila((state) => state.connection === "connected");
  const activeFile = useLila((state) => state.activeFile);
  const returnToPreviewId = useLila((state) => state.returnToPreviewId);
  const refreshSeq = useLila((state) => state.workspaceRefreshSeq);

  const panelRef = React.useRef<HTMLDivElement>(null);
  const [wide, setWide] = React.useState(false);
  const [wideTreeOpen, setWideTreeOpen] = React.useState(readTreeOpen);
  const [narrowTreeOpen, setNarrowTreeOpen] = React.useState(false);

  const [files, setFiles] = React.useState<string[] | undefined>();
  const [gitStatusMap, setGitStatusMap] = React.useState<Map<string, GitFileStatus>>(() => new Map());
  const [treeTruncated, setTreeTruncated] = React.useState(false);
  const [treeLoading, setTreeLoading] = React.useState(false);
  const [treeError, setTreeError] = React.useState<string | undefined>();
  const [filter, setFilter] = React.useState("");
  const [remoteSearch, setRemoteSearch] = React.useState<{ query: string; files: string[] }>();
  const [remoteSearching, setRemoteSearching] = React.useState(false);

  const [fileContent, setFileContent] = React.useState<ProjectFileContent | undefined>();
  const [fileLoading, setFileLoading] = React.useState(false);
  const [fileError, setFileError] = React.useState<string | undefined>();
  const [markdownMode, setMarkdownMode] = React.useState<"preview" | "source">("preview");
  const [svgMode, setSvgMode] = React.useState<"preview" | "source">("preview");
  const [imageZoomActual, setImageZoomActual] = React.useState(false);

  const [findOpen, setFindOpen] = React.useState(false);
  const [findQuery, setFindQuery] = React.useState("");
  const [findIndex, setFindIndex] = React.useState(0);
  const findInputRef = React.useRef<HTMLInputElement>(null);
  const [selectedLines, setSelectedLines] = React.useState<{ anchor: number; start: number; end: number }>();

  const treeReqRef = React.useRef(0);
  const fileReqRef = React.useRef(0);

  const hasFile = Boolean(activeFile?.path);
  const treeVisible = wide ? wideTreeOpen : !hasFile || narrowTreeOpen;
  const narrowOverlayOpen = !wide && hasFile && narrowTreeOpen;

  React.useLayoutEffect(() => {
    const element = panelRef.current;
    if (!element) return;
    const syncWidth = () => setWide(element.clientWidth >= SPLIT_MIN_WIDTH_PX);
    syncWidth();
    const observer = new ResizeObserver(syncWidth);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  React.useEffect(() => {
    if (wide) setNarrowTreeOpen(false);
  }, [wide]);

  const setPersistedWideTreeOpen = React.useCallback((next: boolean) => {
    setWideTreeOpen(next);
    try {
      localStorage.setItem(TREE_OPEN_STORAGE_KEY, String(next));
    } catch {
      // Ignore storage write errors.
    }
  }, []);

  const toggleTree = React.useCallback(() => {
    if (wide) setPersistedWideTreeOpen(!wideTreeOpen);
    else setNarrowTreeOpen((current) => !current);
  }, [wide, wideTreeOpen, setPersistedWideTreeOpen]);

  const openTree = React.useCallback(() => {
    if (wide) setPersistedWideTreeOpen(true);
    else setNarrowTreeOpen(true);
  }, [wide, setPersistedWideTreeOpen]);

  const loadTree = React.useCallback(
    async (refresh = false) => {
      if (!connected) return;
      const reqId = ++treeReqRef.current;
      setTreeLoading(true);
      setTreeError(undefined);
      try {
        const [result, git] = await Promise.all([
          actions.listFiles(projectId, refresh),
          actions.gitStatus(projectId).catch(() => undefined),
        ]);
        if (treeReqRef.current !== reqId) return;
        setFiles(result.files);
        setTreeTruncated(result.truncated);
        setGitStatusMap(git?.isRepo ? new Map(git.files.map((item) => [item.path, item.status])) : new Map());
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
    setGitStatusMap(new Map());
    setFilter("");
    setRemoteSearch(undefined);
    setFileContent(undefined);
    setFileError(undefined);
    setNarrowTreeOpen(false);
    setFindOpen(false);
    setFindQuery("");
    setSelectedLines(undefined);
  }, [projectId]);

  React.useEffect(() => {
    const trimmed = filter.trim();
    if (!treeTruncated || !trimmed || !connected) {
      setRemoteSearch(undefined);
      setRemoteSearching(false);
      return;
    }
    let cancelled = false;
    setRemoteSearching(true);
    const timer = window.setTimeout(() => {
      void actions.searchFiles(projectId, trimmed).then((found) => {
        if (!cancelled) {
          setRemoteSearch({ query: trimmed, files: found });
          setRemoteSearching(false);
        }
      });
    }, 180);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [treeTruncated, filter, connected, projectId]);

  React.useEffect(() => {
    if (treeVisible && files === undefined && connected) void loadTree(false);
  }, [treeVisible, files, connected, loadTree]);

  React.useEffect(() => {
    setMarkdownMode("preview");
    setSvgMode("preview");
    setImageZoomActual(false);
    setNarrowTreeOpen(false);
    setFindOpen(false);
    setFindQuery("");
    setFindIndex(0);
    setSelectedLines(undefined);
  }, [activeFile?.path]);

  React.useEffect(() => {
    if (!activeFile?.path) {
      setFileContent(undefined);
      setFileError(undefined);
      return;
    }
    if (!connected) return;
    void loadFile(activeFile.path);
  }, [activeFile?.path, connected, loadFile]);

  const prevRefreshSeq = React.useRef(refreshSeq);
  React.useEffect(() => {
    if (prevRefreshSeq.current === refreshSeq) return;
    prevRefreshSeq.current = refreshSeq;
    if (activeFile?.path) void loadFile(activeFile.path);
    if (treeVisible) void loadTree(true);
    else setFiles(undefined);
  }, [refreshSeq, activeFile?.path, treeVisible, loadFile, loadTree]);

  const filteredFiles = React.useMemo(() => {
    const needle = filter.trim();
    if (treeTruncated && needle && remoteSearch?.query === needle) return remoteSearch.files;
    if (!files) return [];
    if (!needle) return files;
    const lower = needle.toLowerCase();
    return files.filter((path) => path.toLowerCase().includes(lower));
  }, [files, filter, treeTruncated, remoteSearch]);

  const displayPath = fileContent?.path ?? activeFile?.path ?? "";
  const isMarkdown = displayPath ? isMarkdownPath(displayPath) : false;
  const isSvg = displayPath ? isSvgPath(displayPath) : false;
  const fullPath = fileContent?.fullPath;
  const markdownBasePath = React.useMemo(
    () => (fileContent?.fullPath ? dirNameOf(fileContent.fullPath) ?? project?.path : project?.path),
    [fileContent?.fullPath, project?.path],
  );

  const isCodeView = Boolean(
    fileContent &&
      !fileContent.isImage &&
      !fileContent.isBinary &&
      !(isMarkdown && markdownMode === "preview") &&
      !(isSvg && svgMode === "preview"),
  );

  const findMatchLines = React.useMemo(() => {
    const q = findQuery.trim().toLowerCase();
    if (!q || !fileContent?.content || !isCodeView) return [] as number[];
    const matches: number[] = [];
    const lines = fileContent.content.replace(/\n$/, "").split("\n");
    for (let i = 0; i < lines.length; i++) {
      if (lines[i]!.toLowerCase().includes(q)) matches.push(i + 1);
    }
    return matches;
  }, [findQuery, fileContent?.content, isCodeView]);

  const findMatchSet = React.useMemo(() => (findMatchLines.length ? new Set(findMatchLines) : undefined), [findMatchLines]);
  const activeSearchLine = findMatchLines.length ? findMatchLines[findIndex % findMatchLines.length] : undefined;

  React.useEffect(() => setFindIndex(0), [findQuery]);

  const highlightRange = React.useMemo(() => {
    if (selectedLines) return { start: selectedLines.start, end: selectedLines.end };
    if (!activeFile?.line) return undefined;
    return { start: activeFile.line, end: activeFile.endLine ?? activeFile.line };
  }, [selectedLines, activeFile?.line, activeFile?.endLine]);

  const handleLineClick = React.useCallback((lineNumber: number, shiftKey: boolean) => {
    setSelectedLines((current) => {
      if (shiftKey && current) {
        return { anchor: current.anchor, start: Math.min(current.anchor, lineNumber), end: Math.max(current.anchor, lineNumber) };
      }
      if (current && current.start === lineNumber && current.end === lineNumber) return undefined;
      return { anchor: lineNumber, start: lineNumber, end: lineNumber };
    });
  }, []);

  const mentionWithRange = React.useCallback(() => {
    if (!displayPath) return;
    const suffix = selectedLines
      ? selectedLines.start === selectedLines.end
        ? `#L${selectedLines.start}`
        : `#L${selectedLines.start}-L${selectedLines.end}`
      : "";
    actions.mentionFileInDraft(`${displayPath}${suffix}`);
    focusComposer();
  }, [displayPath, selectedLines]);

  const handleSelectFile = React.useCallback(
    (path: string) => {
      actions.openFilePreview(path);
      if (!wide) setNarrowTreeOpen(false);
    },
    [wide],
  );

  const previewNode = (
    <FilePreviewPane
      hasFile={hasFile}
      treeVisible={treeVisible}
      activeFilePath={activeFile?.path}
      fileContent={fileContent}
      fileLoading={fileLoading}
      fileError={fileError}
      projectPath={project?.path}
      markdownBasePath={markdownBasePath}
      isMarkdown={isMarkdown}
      isSvg={isSvg}
      markdownMode={markdownMode}
      svgMode={svgMode}
      imageZoomActual={imageZoomActual}
      findOpen={findOpen}
      findQuery={findQuery}
      findIndex={findIndex}
      findMatchLines={findMatchLines}
      findMatchSet={findMatchSet}
      activeSearchLine={activeSearchLine}
      findInputRef={findInputRef}
      selectedLines={selectedLines}
      highlightRange={highlightRange}
      onOpenTree={openTree}
      onReloadFile={() => {
        if (activeFile?.path) void loadFile(activeFile.path);
      }}
      onToggleZoom={() => setImageZoomActual((z) => !z)}
      onFindQueryChange={setFindQuery}
      onFindIndexChange={setFindIndex}
      onCloseFind={() => {
        setFindOpen(false);
        setFindQuery("");
      }}
      onLineClick={handleLineClick}
      onMentionWithRange={mentionWithRange}
      onClearSelectedLines={() => setSelectedLines(undefined)}
    />
  );

  return (
    <div
      ref={panelRef}
      className="flex min-h-0 flex-1 flex-col"
      onKeyDown={(event) => {
        if (event.key !== "Escape" || !narrowOverlayOpen) return;
        if (event.target instanceof HTMLInputElement && event.target.value) return;
        event.preventDefault();
        event.stopPropagation();
        setNarrowTreeOpen(false);
      }}
    >
      <header className="flex h-11 shrink-0 items-center gap-1 border-b border-border/60 px-2.5">
        {returnToPreviewId ? (
          <Button type="button" variant="ghost" size="xs" className="shrink-0 text-muted-foreground hover:text-foreground" onClick={() => actions.returnToSubsession()}>
            <ArrowLeft />
            返回子会话
          </Button>
        ) : null}
        {wide || hasFile ? (
          <IconButton label={treeVisible ? "收起文件树" : "展开文件树"} onClick={toggleTree}>
            {treeVisible ? <PanelLeftClose /> : <PanelLeftOpen />}
          </IconButton>
        ) : null}

        {hasFile ? (
          <>
            {activeFile?.history.length ? (
              <IconButton label="返回上一个文件" onClick={() => actions.backFilePreview()}>
                <ArrowLeft />
              </IconButton>
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
            {isCodeView ? (
              <IconButton
                label="在文件内查找"
                onClick={() => {
                  setFindOpen(true);
                  requestAnimationFrame(() => {
                    findInputRef.current?.focus();
                    findInputRef.current?.select();
                  });
                }}
              >
                <Search />
              </IconButton>
            ) : null}
            {isMarkdown && fileContent && !fileContent.isBinary ? (
              <IconButton
                label={markdownMode === "preview" ? "查看源码" : "查看预览"}
                onClick={() => setMarkdownMode((current) => (current === "preview" ? "source" : "preview"))}
              >
                {markdownMode === "preview" ? <Code2 /> : <Eye />}
              </IconButton>
            ) : isSvg && fileContent ? (
              <IconButton
                label={svgMode === "preview" ? "查看源码" : "预览图像"}
                onClick={() => setSvgMode((current) => (current === "preview" ? "source" : "preview"))}
              >
                {svgMode === "preview" ? <Code2 /> : <Eye />}
              </IconButton>
            ) : null}
            {displayPath ? (
              <IconButton label={selectedLines ? "引用选中行" : "引用到对话"} onClick={mentionWithRange}>
                <AtSign />
              </IconButton>
            ) : null}
            <DropdownMenu>
              <DropdownMenuTrigger
                render={
                  <Button
                    type="button"
                    size="icon-sm"
                    variant="ghost"
                    aria-label="更多操作"
                    title="更多操作"
                    className="text-muted-foreground hover:text-foreground"
                  />
                }
              >
                <MoreHorizontal />
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-44">
                <DropdownMenuItem
                  disabled={fileLoading || !activeFile?.path}
                  onClick={() => {
                    if (activeFile?.path) void loadFile(activeFile.path);
                  }}
                >
                  <RotateCw />
                  重新加载
                </DropdownMenuItem>
                {displayPath ? (
                  <DropdownMenuItem
                    onClick={() =>
                      void navigator.clipboard
                        .writeText(displayPath)
                        .then(() => toast.success("已复制相对路径"))
                        .catch(() => toast.error("复制失败"))
                    }
                  >
                    <Copy />
                    复制相对路径
                  </DropdownMenuItem>
                ) : null}
                {fullPath ? (
                  <>
                    <DropdownMenuItem
                      onClick={() =>
                        void navigator.clipboard
                          .writeText(fullPath)
                          .then(() => toast.success("已复制绝对路径"))
                          .catch(() => toast.error("复制失败"))
                      }
                    >
                      <Copy />
                      复制绝对路径
                    </DropdownMenuItem>
                    <DropdownMenuSeparator />
                    <DropdownMenuItem
                      onClick={() =>
                        void window.lila
                          .openPath(fullPath)
                          .catch((error: unknown) => toast.error(error instanceof Error ? error.message : String(error)))
                      }
                    >
                      <ExternalLink />
                      用默认应用打开
                    </DropdownMenuItem>
                    <DropdownMenuItem onClick={() => void actions.revealPath(fullPath)}>
                      <FolderOpen />
                      在访达中显示
                    </DropdownMenuItem>
                  </>
                ) : null}
              </DropdownMenuContent>
            </DropdownMenu>
          </>
        ) : (
          <>
            <div className="flex min-w-0 flex-1 items-center gap-2 px-1.5">
              <FolderTree className="size-4 shrink-0 text-muted-foreground" />
              <h2 className="truncate text-sm font-medium">项目文件</h2>
            </div>
            {!wide ? (
              <IconButton label="刷新文件树" disabled={treeLoading} onClick={() => void loadTree(true)}>
                {treeLoading ? <Spinner /> : <RotateCw />}
              </IconButton>
            ) : null}
          </>
        )}

        <IconButton label="关闭文件面板" onClick={() => actions.closeRightPanel()}>
          <X />
        </IconButton>
      </header>

      <div className="relative flex min-h-0 flex-1 overflow-hidden">
        {wide ? (
          <>
            {wideTreeOpen ? (
              <FileTreePane
                files={files}
                filteredFiles={filteredFiles}
                gitStatusMap={gitStatusMap}
                treeTruncated={treeTruncated}
                treeLoading={treeLoading}
                searching={remoteSearching}
                treeError={treeError}
                filter={filter}
                selectedPath={displayPath || undefined}
                showRefresh
                onFilterChange={setFilter}
                onRefresh={() => void loadTree(true)}
                onSelect={handleSelectFile}
                className="w-[14.5rem] shrink-0 border-e border-border/60"
              />
            ) : null}
            <div className="flex min-h-0 min-w-0 flex-1 flex-col">{previewNode}</div>
          </>
        ) : !hasFile ? (
          <FileTreePane
            files={files}
            filteredFiles={filteredFiles}
            gitStatusMap={gitStatusMap}
            treeTruncated={treeTruncated}
            treeLoading={treeLoading}
            searching={remoteSearching}
            treeError={treeError}
            filter={filter}
            selectedPath={displayPath || undefined}
            showRefresh={false}
            onFilterChange={setFilter}
            onRefresh={() => void loadTree(true)}
            onSelect={handleSelectFile}
            className="min-w-0 flex-1"
          />
        ) : (
          <>
            <div className="flex min-h-0 min-w-0 flex-1 flex-col">{previewNode}</div>
            <div
              aria-hidden
              className={cn(
                "absolute inset-0 z-10 bg-background/55 backdrop-blur-[1px] transition-opacity ease-(--ease-ui-drawer)",
                narrowOverlayOpen ? "opacity-100 duration-200" : "pointer-events-none opacity-0 duration-150",
              )}
              onClick={() => setNarrowTreeOpen(false)}
            />
            <div
              inert={!narrowOverlayOpen}
              className={cn(
                "absolute inset-y-0 start-0 z-20 flex w-[min(17rem,85%)] transition-[transform,opacity] ease-(--ease-ui-drawer) motion-reduce:transition-opacity",
                narrowOverlayOpen
                  ? "translate-x-0 opacity-100 duration-[240ms]"
                  : "pointer-events-none opacity-0 duration-180 motion-safe:-translate-x-full",
              )}
            >
              <FileTreePane
                files={files}
                filteredFiles={filteredFiles}
                gitStatusMap={gitStatusMap}
                treeTruncated={treeTruncated}
                treeLoading={treeLoading}
                searching={remoteSearching}
                treeError={treeError}
                filter={filter}
                selectedPath={displayPath || undefined}
                showRefresh
                onFilterChange={setFilter}
                onRefresh={() => void loadTree(true)}
                onSelect={handleSelectFile}
                className="min-w-0 flex-1 border-e border-border/80 bg-background shadow-lg"
              />
            </div>
          </>
        )}
      </div>
    </div>
  );
}
