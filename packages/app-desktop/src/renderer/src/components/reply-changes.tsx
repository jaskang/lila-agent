import * as React from "react";
import { ArrowLeft, ChevronDown, FileCode2, FileDiff, FileText, GitBranch, X } from "lucide-react";
import { Code, DiffView, type DiffLineComment, languageOf, parsePatch } from "@/components/code";
import { focusComposer } from "@/components/composer";
import { FileTree } from "@/components/file-tree";
import { IconButton } from "@/components/icon-button";
import { FileActions } from "@/components/tool-call";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "cn";
import { actions, useLila, type ReplyReviewState } from "@/lib/store";
import type { FileChange } from "@/lib/transcript-layout";

export type ChangeSelection = { changes: FileChange[]; path: string };

type FileChangeSummary = {
  path: string;
  name: string;
  dir: string;
  added: number;
  removed: number;
  changes: FileChange[];
};

function splitPath(path: string): { name: string; dir: string } {
  const normalized = path.replaceAll("\\", "/");
  const slash = normalized.lastIndexOf("/");
  if (slash < 0) return { name: normalized, dir: "" };
  return { name: normalized.slice(slash + 1), dir: normalized.slice(0, slash) };
}

function countWrittenLines(content: string): number {
  if (!content) return 0;
  return content.replace(/\n$/, "").split("\n").length;
}

function summarizeChanges(changes: readonly FileChange[]): {
  files: FileChangeSummary[];
  totalAdded: number;
  totalRemoved: number;
} {
  const byPath = new Map<string, FileChangeSummary>();
  for (const change of changes) {
    let entry = byPath.get(change.path);
    if (!entry) {
      const { name, dir } = splitPath(change.path);
      entry = { path: change.path, name, dir, added: 0, removed: 0, changes: [] };
      byPath.set(change.path, entry);
    }
    entry.changes.push(change);
    if (change.kind === "patch") {
      for (const parsed of parsePatch(change.patch, change.path)) {
        entry.added += parsed.added;
        entry.removed += parsed.removed;
      }
    } else {
      entry.added += countWrittenLines(change.content);
    }
  }
  const files = [...byPath.values()];
  const totalAdded = files.reduce((sum, file) => sum + file.added, 0);
  const totalRemoved = files.reduce((sum, file) => sum + file.removed, 0);
  return { files, totalAdded, totalRemoved };
}

function DiffStat({ added, removed, className }: { added: number; removed: number; className?: string }) {
  if (!added && !removed) return null;
  return (
    <span className={cn("inline-flex shrink-0 items-center gap-1.5 font-mono text-xs tabular-nums", className)}>
      {added > 0 ? <span className="text-success-foreground">+{added}</span> : null}
      {removed > 0 ? <span className="text-destructive-foreground">-{removed}</span> : null}
    </span>
  );
}

/** Per-turn changed files summary card rendered at the end of a turn when files were edited or written. */
export const ReplyChanges = React.memo(function ReplyChanges({
  changes,
  onOpen,
}: {
  changes: FileChange[];
  onOpen: (selection: ChangeSelection) => void;
}) {
  const { files, totalAdded, totalRemoved } = React.useMemo(() => summarizeChanges(changes), [changes]);
  const [open, setOpen] = React.useState(true);
  if (!files.length) return null;

  return (
    <Collapsible open={open} onOpenChange={setOpen} className="not-typeset mt-2 overflow-hidden rounded-xl border border-border/80 bg-card/80">
      <div className="flex items-center gap-2 bg-muted/35 px-3 py-2">
        <CollapsibleTrigger
          className="flex min-w-0 flex-1 cursor-pointer items-center gap-2 text-left text-[13px] outline-none focus-visible:text-foreground"
        >
          <span className="font-medium text-foreground">{files.length} 个文件已改动</span>
          <DiffStat added={totalAdded} removed={totalRemoved} />
          <ChevronDown
            aria-hidden
            className={cn(
              "size-3.5 shrink-0 text-muted-foreground transition-transform duration-150 ease-(--ease-ui-out) motion-reduce:transition-none",
              !open && "-rotate-90",
            )}
          />
        </CollapsibleTrigger>
        <Button
          type="button"
          variant="outline"
          size="xs"
          className="h-6 shrink-0 bg-background px-2 font-normal text-foreground shadow-2xs"
          onClick={() => onOpen({ changes, path: files[0]!.path })}
        >
          <FileDiff className="size-3.5 text-muted-foreground" />
          查看
        </Button>
      </div>
      <CollapsibleContent className="h-(--collapsible-panel-height) overflow-hidden transition-[height,opacity] duration-200 ease-(--ease-ui-out) data-ending-style:duration-150 data-ending-style:h-0 data-ending-style:opacity-0 data-starting-style:h-0 data-starting-style:opacity-0 motion-reduce:transition-none">
        <div className="max-h-64 divide-y divide-border/50 overflow-y-auto border-t border-border/60">
          {files.map((file) => (
            <button
              key={file.path}
              type="button"
              title={file.path}
              onClick={() => onOpen({ changes, path: file.path })}
              className="flex w-full min-w-0 cursor-pointer items-center gap-2 px-3 py-1.5 text-left text-[13px] transition-colors hover:bg-muted/40 focus-visible:bg-muted/50 focus-visible:outline-none"
            >
              <FileCode2 aria-hidden className="size-3.5 shrink-0 text-info-foreground/80" />
              <span className="shrink-0 font-medium text-foreground">{file.name}</span>
              {file.dir ? <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground/75">{file.dir}</span> : <span className="flex-1" />}
              <DiffStat added={file.added} removed={file.removed} className="ms-auto" />
            </button>
          ))}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
});

/** Right-panel card view for inspecting a turn's recorded edits and writes without an overlay sheet. */
export function ReplyChangesPanel({ review }: { review: ReplyReviewState }) {
  const { files, totalAdded, totalRemoved } = React.useMemo(() => summarizeChanges(review.changes), [review.changes]);
  const [narrowView, setNarrowView] = React.useState<"list" | "diff">("diff");
  const selected = files.find((file) => file.path === review.path) ?? files[0];

  React.useEffect(() => {
    setNarrowView("diff");
  }, [review.path, review.changes]);

  const returnToPreviewId = useLila((state) => state.returnToPreviewId);
  const selectFile = (path: string) => {
    actions.selectReplyReviewPath(path);
    setNarrowView("diff");
  };

  const handleCommentLine = React.useCallback((comment: DiffLineComment) => {
    const loc = comment.lineNumber ? `${comment.path}#L${comment.lineNumber}` : comment.path;
    const prefix = comment.variant === "added" ? "+" : comment.variant === "removed" ? "-" : " ";
    actions.appendToDraft(`关于 @${loc}：\n\`\`\`\n${prefix}${comment.lineText}\n\`\`\`\n${comment.note}`);
    focusComposer();
  }, []);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <header className="flex h-11 shrink-0 items-center gap-1.5 border-b border-border/60 px-3">
        {returnToPreviewId ? (
          <Button
            type="button"
            variant="ghost"
            size="xs"
            className="-ms-1 text-muted-foreground hover:text-foreground"
            onClick={() => actions.returnToSubsession()}
          >
            <ArrowLeft />
            返回子会话
          </Button>
        ) : files.length > 1 && narrowView === "diff" ? (
          <span className="@min-[42rem]:hidden">
            <IconButton label="返回文件列表" onClick={() => setNarrowView("list")}>
              <ArrowLeft />
            </IconButton>
          </span>
        ) : (
          <FileDiff aria-hidden className="size-4 shrink-0 text-muted-foreground" />
        )}
        <div className="flex min-w-0 flex-1 items-center gap-2">
          <h2 className="truncate text-sm font-medium">
            {narrowView === "diff" && selected ? (
              <>
                <span className="@min-[42rem]:hidden">{selected.name}</span>
                <span className="hidden @min-[42rem]:inline">本轮改动 · {files.length} 个文件</span>
              </>
            ) : (
              `本轮改动 · ${files.length} 个文件`
            )}
          </h2>
          <DiffStat added={totalAdded} removed={totalRemoved} className="hidden @min-[26rem]:inline-flex" />
        </div>
        {selected ? (
          <IconButton label="在文件面板打开" onClick={() => actions.openFilePreview(selected.path)}>
            <FileText />
          </IconButton>
        ) : null}
        <IconButton
          label="查看完整 Git 改动"
          onClick={() => (selected ? actions.openChangesForFile(selected.path) : actions.openRightPanel("changes"))}
        >
          <GitBranch />
        </IconButton>
        <IconButton label="关闭本轮改动" onClick={() => actions.closeRightPanel()}>
          <X />
        </IconButton>
      </header>

      <div className="grid min-h-0 flex-1 grid-cols-1 @min-[42rem]:grid-cols-[minmax(11rem,15rem)_minmax(0,1fr)]">
        <ScrollArea className={cn("min-h-0 min-w-0 @min-[42rem]:block @min-[42rem]:border-e", narrowView === "list" ? "block" : "hidden")}>
          <FileTree
            className="p-2"
            entries={files.map((file) => ({
              path: file.path,
              leading: <FileCode2 className="size-3.5 shrink-0 text-info-foreground/80" />,
              trailing: <DiffStat added={file.added} removed={file.removed} className="text-[11px]" />,
            }))}
            selectedPath={selected?.path}
            expandAll
            onSelect={selectFile}
          />
        </ScrollArea>

        <ScrollArea
          key={selected?.path}
          className={cn("min-h-0 min-w-0 @min-[42rem]:block", narrowView === "diff" ? "block" : "hidden")}
        >
          {selected ? (
            <div className="flex min-w-0 flex-col gap-3 p-3.5">
              <div className="flex flex-wrap items-center gap-2">
                <p className="min-w-0 flex-1 break-all font-mono text-xs text-muted-foreground">{selected.path}</p>
                <FileActions sessionId={review.sessionId} path={selected.path} />
              </div>
              {selected.changes.map((change) =>
                change.kind === "patch" ? (
                  <DiffView key={change.itemId} patch={change.patch} path={change.path} maxHeight="none" onCommentLine={handleCommentLine} />
                ) : (
                  <Code key={change.itemId} code={change.content} language={languageOf(change.path)} title="写入内容" maxHeight="none" />
                ),
              )}
            </div>
          ) : null}
        </ScrollArea>
      </div>
    </div>
  );
}
