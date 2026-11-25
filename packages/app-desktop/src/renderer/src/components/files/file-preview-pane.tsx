import * as React from "react";
import { AtSign, ChevronDown, ChevronUp, ExternalLink, FolderOpen, FolderTree, PanelLeftOpen, RotateCw, Search, X, ZoomIn, ZoomOut } from "lucide-react";
import { toast } from "sonner";
import type { ProjectFileContent } from "@lila/protocol";
import { Code, languageOf } from "@/components/code";
import { Markdown, MarkdownBasePathProvider } from "@/components/markdown";
import { Button } from "@/components/ui/button";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "cn";
import { actions } from "@/lib/store";

export function fileNameOf(path: string): string {
  return path.split(/[/\\]/).at(-1) ?? path;
}

export function dirNameOf(path: string): string | undefined {
  const slash = path.lastIndexOf("/");
  if (slash <= 0) return undefined;
  return path.slice(0, slash);
}

export function isMarkdownPath(path: string): boolean {
  const lower = path.toLowerCase();
  return lower.endsWith(".md") || lower.endsWith(".mdx") || lower.endsWith(".markdown");
}

export function isSvgPath(path: string): boolean {
  return path.toLowerCase().endsWith(".svg");
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

export function FilePreviewPane({
  hasFile,
  treeVisible,
  activeFilePath,
  fileContent,
  fileLoading,
  fileError,
  projectPath,
  markdownBasePath,
  isMarkdown,
  isSvg,
  markdownMode,
  svgMode,
  imageZoomActual,
  findOpen,
  findQuery,
  findIndex,
  findMatchLines,
  findMatchSet,
  activeSearchLine,
  findInputRef,
  selectedLines,
  highlightRange,
  onOpenTree,
  onReloadFile,
  onToggleZoom,
  onFindQueryChange,
  onFindIndexChange,
  onCloseFind,
  onLineClick,
  onMentionWithRange,
  onClearSelectedLines,
}: {
  hasFile: boolean;
  treeVisible: boolean;
  activeFilePath: string | undefined;
  fileContent: ProjectFileContent | undefined;
  fileLoading: boolean;
  fileError: string | undefined;
  projectPath: string | undefined;
  markdownBasePath: string | undefined;
  isMarkdown: boolean;
  isSvg: boolean;
  markdownMode: "preview" | "source";
  svgMode: "preview" | "source";
  imageZoomActual: boolean;
  findOpen: boolean;
  findQuery: string;
  findIndex: number;
  findMatchLines: number[];
  findMatchSet: Set<number> | undefined;
  activeSearchLine: number | undefined;
  findInputRef: React.RefObject<HTMLInputElement | null>;
  selectedLines: { anchor: number; start: number; end: number } | undefined;
  highlightRange: { start: number; end: number } | undefined;
  onOpenTree: () => void;
  onReloadFile: () => void;
  onToggleZoom: () => void;
  onFindQueryChange: (value: string) => void;
  onFindIndexChange: (updater: (current: number) => number) => void;
  onCloseFind: () => void;
  onLineClick: (lineNumber: number, shiftKey: boolean) => void;
  onMentionWithRange: () => void;
  onClearSelectedLines: () => void;
}) {
  if (!hasFile) {
    return (
      <Empty className="flex-1">
        <EmptyHeader>
          <EmptyTitle>选择文件预览</EmptyTitle>
        </EmptyHeader>
        {!treeVisible ? (
          <Button variant="outline" size="sm" onClick={onOpenTree}>
            <PanelLeftOpen />
            展开文件树
          </Button>
        ) : null}
      </Empty>
    );
  }

  if (fileLoading && !fileContent) {
    return (
      <div className="flex flex-1 items-center justify-center gap-2 text-xs text-muted-foreground">
        <Spinner />
        正在加载…
      </div>
    );
  }

  if (fileError) {
    return (
      <Empty className="flex-1">
        <EmptyHeader>
          <EmptyTitle>无法预览</EmptyTitle>
          <EmptyDescription className="max-w-sm break-words">{fileError}</EmptyDescription>
        </EmptyHeader>
        <div className="flex items-center gap-2">
          {activeFilePath ? (
            <Button variant="outline" size="sm" onClick={onReloadFile}>
              <RotateCw />
              重试
            </Button>
          ) : null}
          {!treeVisible ? (
            <Button variant="ghost" size="sm" onClick={onOpenTree}>
              <FolderTree />
              展开文件树
            </Button>
          ) : null}
        </div>
      </Empty>
    );
  }

  if (!fileContent) return null;

  if (fileContent.isImage || (isSvg && svgMode === "preview")) {
    return (
      <ScrollArea className="min-h-0 flex-1">
        <div className="flex flex-col items-center gap-3 p-5">
          <div
            className="overflow-auto rounded-xl border p-3"
            style={{
              backgroundImage:
                "linear-gradient(45deg, color-mix(in oklch, var(--foreground) 6%, transparent) 25%, transparent 25%), linear-gradient(-45deg, color-mix(in oklch, var(--foreground) 6%, transparent) 25%, transparent 25%), linear-gradient(45deg, transparent 75%, color-mix(in oklch, var(--foreground) 6%, transparent) 75%), linear-gradient(-45deg, transparent 75%, color-mix(in oklch, var(--foreground) 6%, transparent) 75%)",
              backgroundSize: "16px 16px",
              backgroundPosition: "0 0, 0 8px, 8px -8px, -8px 0px",
            }}
          >
            <img
              src={toMediaUrl(fileContent.fullPath, projectPath)}
              alt={fileNameOf(fileContent.path)}
              className={cn(imageZoomActual ? "max-w-none" : "max-h-[72vh] max-w-full object-contain")}
            />
          </div>
          <div className="flex items-center gap-2">
            <span className="font-mono text-xs text-muted-foreground tabular-nums">{formatBytes(fileContent.size)}</span>
            <Button type="button" size="xs" variant="outline" onClick={onToggleZoom}>
              {imageZoomActual ? <ZoomOut /> : <ZoomIn />}
              {imageZoomActual ? "适应窗口" : "1:1 原图"}
            </Button>
          </div>
        </div>
      </ScrollArea>
    );
  }

  if (fileContent.isBinary) {
    return (
      <Empty className="flex-1">
        <EmptyHeader>
          <EmptyTitle>二进制文件</EmptyTitle>
          <EmptyDescription>{formatBytes(fileContent.size)}</EmptyDescription>
        </EmptyHeader>
        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={() =>
              void window.lila
                .openPath(fileContent.fullPath)
                .catch((error: unknown) => toast.error(error instanceof Error ? error.message : String(error)))
            }
          >
            <ExternalLink />
            用默认应用打开
          </Button>
          <Button variant="ghost" size="sm" onClick={() => void actions.revealPath(fileContent.fullPath)}>
            <FolderOpen />
            在访达中显示
          </Button>
        </div>
      </Empty>
    );
  }

  if (isMarkdown && markdownMode === "preview") {
    return (
      <ScrollArea className="min-h-0 flex-1">
        <div className="px-5 py-4">
          {fileContent.truncated ? (
            <p className="mb-3 text-xs text-muted-foreground">已截断（{formatBytes(fileContent.size)}，仅显示前 512 KB）</p>
          ) : null}
          <MarkdownBasePathProvider value={markdownBasePath}>
            <Markdown text={fileContent.content} />
          </MarkdownBasePathProvider>
        </div>
      </ScrollArea>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {findOpen ? (
        <div className="flex shrink-0 items-center gap-1.5 border-b border-border/60 bg-muted/25 px-3 py-1.5">
          <Search className="size-3.5 shrink-0 text-muted-foreground" />
          <Input
            ref={findInputRef}
            autoFocus
            value={findQuery}
            placeholder="查找…"
            aria-label="在文件内查找"
            className="h-7 min-w-0 flex-1 bg-background text-xs"
            onChange={(event) => onFindQueryChange(event.target.value)}
            onKeyDown={(event) => {
              if (event.nativeEvent.isComposing || event.keyCode === 229) return;
              if (event.key === "Escape") {
                event.preventDefault();
                event.stopPropagation();
                onCloseFind();
              } else if (event.key === "Enter" && findMatchLines.length) {
                event.preventDefault();
                onFindIndexChange((cur) => (cur + (event.shiftKey ? -1 : 1) + findMatchLines.length) % findMatchLines.length);
              }
            }}
          />
          {findQuery.trim() ? (
            <span className="shrink-0 font-mono text-[11px] text-muted-foreground tabular-nums">
              {findMatchLines.length ? `${(findIndex % findMatchLines.length) + 1} / ${findMatchLines.length}` : "0 / 0"}
            </span>
          ) : null}
          <Button
            type="button"
            size="icon-xs"
            variant="ghost"
            disabled={!findMatchLines.length}
            aria-label="上一处"
            onClick={() => onFindIndexChange((cur) => (cur - 1 + findMatchLines.length) % findMatchLines.length)}
          >
            <ChevronUp className="size-3.5" />
          </Button>
          <Button
            type="button"
            size="icon-xs"
            variant="ghost"
            disabled={!findMatchLines.length}
            aria-label="下一处"
            onClick={() => onFindIndexChange((cur) => (cur + 1) % findMatchLines.length)}
          >
            <ChevronDown className="size-3.5" />
          </Button>
          <Button type="button" size="icon-xs" variant="ghost" aria-label="关闭查找" onClick={onCloseFind}>
            <X className="size-3.5" />
          </Button>
        </div>
      ) : null}
      {selectedLines ? (
        <div className="flex shrink-0 items-center justify-between gap-2 border-b border-border/60 bg-info/10 px-3 py-1 text-xs">
          <span className="font-mono text-info-foreground tabular-nums">
            {selectedLines.start === selectedLines.end ? `L${selectedLines.start}` : `L${selectedLines.start}–L${selectedLines.end}`}
          </span>
          <div className="flex items-center gap-1">
            <Button type="button" size="xs" variant="secondary" className="h-6 text-[11px]" onClick={onMentionWithRange}>
              <AtSign className="size-3" />
              引用到对话
            </Button>
            <Button type="button" size="xs" variant="ghost" className="h-6 text-[11px]" onClick={onClearSelectedLines}>
              取消
            </Button>
          </div>
        </div>
      ) : null}
      <ScrollArea className="min-h-0 flex-1">
        <div className="p-3">
          {fileContent.truncated ? (
            <p className="mb-2 text-xs text-muted-foreground">已截断（{formatBytes(fileContent.size)}，仅显示前 512 KB）</p>
          ) : null}
          <Code
            code={fileContent.content}
            language={languageOf(fileContent.path)}
            maxHeight={undefined}
            numbered
            highlightRange={highlightRange}
            searchMatches={findMatchSet}
            activeSearchLine={activeSearchLine}
            onLineClick={onLineClick}
          />
        </div>
      </ScrollArea>
    </div>
  );
}
