import * as React from "react";
import { RotateCw } from "lucide-react";
import type { GitFileStatus } from "@lila/protocol";
import { FileTree } from "@/components/file-tree";
import { IconButton } from "@/components/icon-button";
import { SearchInput } from "@/components/search-input";
import { Button } from "@/components/ui/button";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "cn";

const GIT_BADGE: Record<GitFileStatus, { letter: string; className: string; label: string }> = {
  modified: { letter: "M", className: "text-warning-foreground", label: "已修改" },
  added: { letter: "A", className: "text-success-foreground", label: "已新增" },
  deleted: { letter: "D", className: "text-destructive-foreground", label: "已删除" },
  renamed: { letter: "R", className: "text-info-foreground", label: "已重命名" },
  copied: { letter: "C", className: "text-info-foreground", label: "已复制" },
  untracked: { letter: "U", className: "text-success-foreground", label: "未跟踪" },
  unmerged: { letter: "!", className: "text-destructive-foreground", label: "冲突" },
};

export function FileTreePane({
  files,
  filteredFiles,
  gitStatusMap,
  treeTruncated,
  treeLoading,
  searching,
  treeError,
  filter,
  selectedPath,
  showRefresh,
  onFilterChange,
  onRefresh,
  onSelect,
  className,
}: {
  files: string[] | undefined;
  filteredFiles: string[];
  gitStatusMap: Map<string, GitFileStatus>;
  treeTruncated: boolean;
  treeLoading: boolean;
  searching: boolean;
  treeError: string | undefined;
  filter: string;
  selectedPath: string | undefined;
  showRefresh: boolean;
  onFilterChange: (value: string) => void;
  onRefresh: () => void;
  onSelect: (path: string) => void;
  className?: string;
}) {
  return (
    <div className={cn("flex min-h-0 flex-col", className)}>
      <div className="flex shrink-0 items-center gap-1 border-b border-border/60 p-2">
        <SearchInput
          value={filter}
          onChange={onFilterChange}
          placeholder="筛选文件"
          ariaLabel="筛选项目文件"
          stopEscapePropagation
        />
        {showRefresh ? (
          <IconButton label="刷新文件树" disabled={treeLoading} onClick={onRefresh}>
            {treeLoading ? <Spinner /> : <RotateCw />}
          </IconButton>
        ) : null}
      </div>

      {(treeLoading && !files) || searching ? (
        <div className="flex flex-1 items-center justify-center gap-2 text-xs text-muted-foreground">
          <Spinner />
          {searching ? "正在搜索…" : "正在加载…"}
        </div>
      ) : treeError ? (
        <Empty className="flex-1">
          <EmptyHeader>
            <EmptyTitle>读取失败</EmptyTitle>
            <EmptyDescription>{treeError}</EmptyDescription>
          </EmptyHeader>
          <Button variant="outline" size="sm" onClick={onRefresh}>
            <RotateCw />
            重试
          </Button>
        </Empty>
      ) : !filteredFiles.length ? (
        <Empty className="flex-1">
          <EmptyHeader>
            <EmptyTitle>{filter ? "无匹配文件" : "目录为空"}</EmptyTitle>
          </EmptyHeader>
        </Empty>
      ) : (
        <ScrollArea className="min-h-0 flex-1">
          <FileTree
            className="p-2"
            entries={filteredFiles.map((path) => {
              const gitStatus = gitStatusMap.get(path);
              const badge = gitStatus ? GIT_BADGE[gitStatus] : undefined;
              return {
                path,
                ...(badge
                  ? {
                      trailing: (
                        <span className={cn("shrink-0 font-mono text-[10px] font-semibold", badge.className)} title={badge.label}>
                          {badge.letter}
                        </span>
                      ),
                    }
                  : {}),
              };
            })}
            selectedPath={selectedPath}
            expandAll={Boolean(filter.trim())}
            onSelect={onSelect}
          />
          {treeTruncated && !filter ? (
            <p className="px-3 pb-3 text-[11px] text-muted-foreground">已显示前 {files?.length} 项，输入关键字搜索全部</p>
          ) : null}
        </ScrollArea>
      )}
    </div>
  );
}
