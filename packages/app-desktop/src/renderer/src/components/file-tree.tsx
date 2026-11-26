import * as React from "react";
import { ChevronRight, FileText, Folder } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { cn } from "cn";

export type FileTreeEntry = {
  path: string;
  leading?: React.ReactNode;
  trailing?: React.ReactNode;
  title?: string | undefined;
};

type FileTreeNode =
  | { kind: "folder"; name: string; path: string; children: FileTreeNode[] }
  | { kind: "file"; name: string; entry: FileTreeEntry };

type MutableFolder = {
  name: string;
  path: string;
  folders: Map<string, MutableFolder>;
  files: Array<{ name: string; entry: FileTreeEntry }>;
};

function compressFolder(folder: MutableFolder): FileTreeNode {
  const folderChildren = [...folder.folders.values()]
    .sort((a, b) => a.name.localeCompare(b.name))
    .map(compressFolder);
  const fileChildren: FileTreeNode[] = [...folder.files]
    .sort((a, b) => a.name.localeCompare(b.name))
    .map(({ name, entry }) => ({ kind: "file", name, entry }));
  const children = [...folderChildren, ...fileChildren];

  // Compact single-folder chains (e.g. packages/app-desktop/src) so deep repos stay readable.
  if (children.length === 1 && children[0]!.kind === "folder") {
    const only = children[0]!;
    return {
      kind: "folder",
      name: `${folder.name}/${only.name}`,
      path: only.path,
      children: only.children,
    };
  }
  return { kind: "folder", name: folder.name, path: folder.path, children };
}

function buildTree(entries: readonly FileTreeEntry[]): FileTreeNode[] {
  const root: MutableFolder = { name: "", path: "", folders: new Map(), files: [] };

  for (const entry of entries) {
    const normalized = entry.path.replace(/\\/g, "/").replace(/^\/+/, "");
    const parts = normalized.split("/").filter(Boolean);
    if (!parts.length) continue;
    let current = root;
    for (let i = 0; i < parts.length - 1; i++) {
      const segment = parts[i]!;
      const folderPath = current.path ? `${current.path}/${segment}` : segment;
      let next = current.folders.get(segment);
      if (!next) {
        next = { name: segment, path: folderPath, folders: new Map(), files: [] };
        current.folders.set(segment, next);
      }
      current = next;
    }
    current.files.push({ name: parts[parts.length - 1]!, entry });
  }

  const folders = [...root.folders.values()]
    .sort((a, b) => a.name.localeCompare(b.name))
    .map(compressFolder);
  const files: FileTreeNode[] = [...root.files]
    .sort((a, b) => a.name.localeCompare(b.name))
    .map(({ name, entry }) => ({ kind: "file", name, entry }));
  return [...folders, ...files];
}

function TreeNodeView({
  node,
  selectedPath,
  onSelect,
  defaultOpen,
  expandAll,
}: {
  node: FileTreeNode;
  selectedPath: string | undefined;
  onSelect: (path: string) => void;
  defaultOpen: boolean;
  expandAll: boolean;
}) {
  const isAncestor = node.kind === "folder" && Boolean(selectedPath && selectedPath.startsWith(`${node.path}/`));
  const [open, setOpen] = React.useState(() => defaultOpen || expandAll || isAncestor);
  const prevExpandAllRef = React.useRef(expandAll);

  React.useEffect(() => {
    if (expandAll || isAncestor) {
      setOpen(true);
    } else if (prevExpandAllRef.current && !expandAll && !defaultOpen) {
      setOpen(false);
    }
    prevExpandAllRef.current = expandAll;
  }, [expandAll, isAncestor, defaultOpen]);

  if (node.kind === "folder") {
    return (
      <Collapsible open={open} onOpenChange={setOpen}>
        <CollapsibleTrigger
          render={
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 w-full justify-start gap-1.5 px-1.5 font-mono text-xs font-normal text-muted-foreground hover:text-foreground aria-expanded:bg-transparent aria-expanded:hover:bg-muted"
              title={node.path}
            />
          }
        >
          <ChevronRight className="size-3.5 shrink-0 transition-transform duration-150 ease-(--ease-ui-out) motion-reduce:transition-none group-data-panel-open/button:rotate-90" />
          <Folder className="size-3.5 shrink-0" />
          <span className="truncate">{node.name}</span>
        </CollapsibleTrigger>
        <CollapsibleContent>
          <div className="ml-3 flex flex-col border-l border-border/60 pl-1.5">
            {node.children.map((child) => (
              <TreeNodeView
                key={child.kind === "folder" ? `dir:${child.path}` : `file:${child.entry.path}`}
                node={child}
                selectedPath={selectedPath}
                onSelect={onSelect}
                defaultOpen={defaultOpen}
                expandAll={expandAll}
              />
            ))}
          </div>
        </CollapsibleContent>
      </Collapsible>
    );
  }

  const active = node.entry.path === selectedPath;
  return (
    <Button
      type="button"
      variant="ghost"
      size="sm"
      aria-pressed={active}
      title={node.entry.title ?? node.entry.path}
      onClick={() => onSelect(node.entry.path)}
      className={cn(
        "h-7 w-full justify-start gap-1.5 px-2 font-mono text-xs font-normal text-foreground",
        active && "bg-muted font-medium",
      )}
    >
      {node.entry.leading ?? <FileText className="size-3.5 shrink-0 text-muted-foreground" />}
      <span className="min-w-0 flex-1 truncate text-start">{node.name}</span>
      {node.entry.trailing}
    </Button>
  );
}

/** shadcn File Tree pattern: displays files as a collapsible folder tree. */
export function FileTree({
  entries,
  selectedPath,
  onSelect,
  defaultOpen = false,
  expandAll = false,
  className,
}: {
  entries: readonly FileTreeEntry[];
  selectedPath: string | undefined;
  onSelect: (path: string) => void;
  defaultOpen?: boolean;
  expandAll?: boolean;
  className?: string;
}) {
  const nodes = React.useMemo(() => buildTree(entries), [entries]);
  return (
    <div className={cn("flex flex-col gap-0.5", className)}>
      {nodes.map((node) => (
        <TreeNodeView
          key={node.kind === "folder" ? `dir:${node.path}` : `file:${node.entry.path}`}
          node={node}
          selectedPath={selectedPath}
          onSelect={onSelect}
          defaultOpen={defaultOpen}
          expandAll={expandAll}
        />
      ))}
    </div>
  );
}
