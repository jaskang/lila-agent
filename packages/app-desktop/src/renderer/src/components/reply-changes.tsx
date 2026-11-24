import { FileDiff, Files } from "lucide-react";
import { Code, DiffView, languageOf } from "@/components/code";
import { FileActions } from "@/components/tool-call";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { cn } from "cn";
import type { FileChange } from "@/lib/transcript-layout";

export type ChangeSelection = { changes: FileChange[]; path: string };

function fileName(path: string): string {
  return path.split(/[/\\]/).at(-1) ?? path;
}

export function ReplyChanges({ changes, onOpen }: { changes: FileChange[]; onOpen: (selection: ChangeSelection) => void }) {
  const paths = [...new Set(changes.map((change) => change.path))];
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-1.5 border-l border-border pl-2">
      <Button type="button" variant="ghost" size="xs" onClick={() => onOpen({ changes, path: paths[0]! })}>
        <Files />
        查看改动 · {paths.length} 个文件
      </Button>
      {paths.slice(0, 2).map((path) => (
        <Button key={path} type="button" variant="outline" size="xs" className="max-w-52 text-muted-foreground" title={path} onClick={() => onOpen({ changes, path })}>
          <FileDiff />
          <span className="truncate">{fileName(path)}</span>
        </Button>
      ))}
    </div>
  );
}

export function ReplyChangesSheet({
  sessionId,
  selection,
  onChange,
}: {
  sessionId: string;
  selection: ChangeSelection | undefined;
  onChange: (selection: ChangeSelection | undefined) => void;
}) {
  const paths = [...new Set(selection?.changes.map((change) => change.path))];
  const changes = selection?.changes.filter((change) => change.path === selection.path) ?? [];
  return (
    <Sheet open={Boolean(selection)} onOpenChange={(open) => { if (!open) onChange(undefined); }}>
      <SheetContent className="gap-0 data-[side=right]:w-[min(56rem,94vw)] data-[side=right]:sm:max-w-none">
        <SheetHeader className="border-b">
          <SheetTitle>回复中的文件改动</SheetTitle>
          <SheetDescription>按执行顺序显示这段会话记录的补丁和写入内容。</SheetDescription>
        </SheetHeader>
        {selection ? (
          <div className="grid min-h-0 flex-1 grid-cols-[minmax(7rem,1fr)_minmax(0,3fr)]">
            <ScrollArea className="min-w-0 border-e">
              <div className="flex flex-col gap-0.5 p-2">
                {paths.map((path) => (
                  <Button
                    key={path}
                    type="button"
                    size="sm"
                    variant="ghost"
                    className={cn("w-full justify-start font-mono text-xs", selection.path === path && "bg-muted")}
                    aria-pressed={selection.path === path}
                    title={path}
                    onClick={() => onChange({ ...selection, path })}
                  >
                    <FileDiff />
                    <span className="truncate">{fileName(path)}</span>
                  </Button>
                ))}
              </div>
            </ScrollArea>
            <ScrollArea key={selection.path} className="min-w-0">
              <div className="flex min-w-0 flex-col gap-3 p-4">
                <div className="flex flex-wrap items-center gap-2">
                  <p className="min-w-0 flex-1 break-all font-mono text-xs">{selection.path}</p>
                  <FileActions sessionId={sessionId} path={selection.path} />
                </div>
                {changes.map((change) => change.kind === "patch" ? (
                  <DiffView key={change.itemId} patch={change.patch} path={change.path} maxHeight="32rem" />
                ) : (
                  <Code key={change.itemId} code={change.content} language={languageOf(change.path)} title="写入内容" maxHeight="32rem" />
                ))}
              </div>
            </ScrollArea>
          </div>
        ) : null}
      </SheetContent>
    </Sheet>
  );
}
