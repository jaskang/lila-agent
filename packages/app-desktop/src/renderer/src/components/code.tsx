import * as React from "react";
import { Columns2, FileDiff as FileDiffIcon, Maximize2, MessageSquarePlus, Minimize2, Rows2, Send, WrapText, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  CodeBlock,
  CodeBlockActions,
  CodeBlockContent,
  CodeBlockCopyButton,
  CodeBlockHeader,
  CodeBlockLine,
  CodeBlockTitle,
} from "@/components/ui/code-block";
import { cn } from "cn";
import { HIGHLIGHT_MAX_CHARS, useHighlighted } from "@/lib/highlight";

const EXTENSION_LANGUAGE: Record<string, string> = {
  ts: "typescript",
  mts: "typescript",
  cts: "typescript",
  tsx: "tsx",
  js: "javascript",
  jsx: "jsx",
  mjs: "javascript",
  cjs: "javascript",
  json: "json",
  jsonc: "jsonc",
  css: "css",
  scss: "scss",
  sass: "scss",
  less: "css",
  html: "html",
  htm: "html",
  xml: "xml",
  svg: "xml",
  plist: "xml",
  md: "markdown",
  markdown: "markdown",
  mdx: "markdown",
  py: "python",
  sh: "bash",
  bash: "bash",
  zsh: "bash",
  sql: "sql",
  yml: "yaml",
  yaml: "yaml",
  toml: "toml",
  go: "go",
  rs: "rust",
  c: "c",
  h: "c",
  cc: "cpp",
  cpp: "cpp",
  cxx: "cpp",
  hpp: "cpp",
  hxx: "cpp",
  hh: "cpp",
  cs: "csharp",
  java: "java",
  kt: "kotlin",
  kts: "kotlin",
  swift: "swift",
  rb: "ruby",
  php: "php",
  diff: "diff",
  patch: "diff",
  dockerfile: "dockerfile",
  graphql: "graphql",
  gql: "graphql",
  m: "objective-c",
  mm: "objective-c",
  vue: "vue",
  svelte: "svelte",
  lua: "lua",
  dart: "dart",
  proto: "proto",
  ini: "ini",
  env: "ini",
  conf: "ini",
  cfg: "ini",
};

export function languageOf(path: string): string | undefined {
  const base = path.split("/").pop()?.toLowerCase() ?? "";
  if (base === "dockerfile" || base.startsWith("dockerfile.")) return "dockerfile";
  const dot = base.lastIndexOf(".");
  if (dot < 0) return undefined;
  return EXTENSION_LANGUAGE[base.slice(dot + 1)];
}

export function Code({
  code,
  language,
  title,
  streaming = false,
  maxHeight = "20rem",
  numbered = false,
  highlightRange,
  searchMatches,
  activeSearchLine,
  onLineClick,
  className,
}: {
  code: string;
  language?: string | undefined;
  title?: React.ReactNode;
  streaming?: boolean;
  maxHeight?: string | undefined;
  numbered?: boolean;
  highlightRange?: { start: number; end: number } | undefined;
  searchMatches?: Set<number> | undefined;
  activeSearchLine?: number | undefined;
  onLineClick?: ((lineNumber: number, shiftKey: boolean) => void) | undefined;
  className?: string;
}) {
  const highlighted = useHighlighted(code, language, !streaming);
  const text = code.replace(/\n$/, "");
  const rootRef = React.useRef<HTMLDivElement>(null);
  const [wrap, setWrap] = React.useState(false);
  const [expanded, setExpanded] = React.useState(false);

  const lines = React.useMemo(() => highlighted ?? text.split("\n"), [highlighted, text]);
  const isLong = Boolean(maxHeight && maxHeight !== "none" && lines.length > 18);
  const skippedHighlight = Boolean(!streaming && language && code.length > HIGHLIGHT_MAX_CHARS);
  const digits = Math.max(2, String(lines.length).length);
  const lineColStyle = React.useMemo(() => ({ width: `${digits}ch` }) as React.CSSProperties, [digits]);

  React.useEffect(() => {
    const targetLine = activeSearchLine ?? highlightRange?.start;
    if (!targetLine) return;
    const frame = requestAnimationFrame(() => {
      const target = rootRef.current?.querySelector<HTMLElement>(`[data-line="${targetLine}"]`);
      target?.scrollIntoView({ block: "center", inline: "nearest" });
    });
    return () => cancelAnimationFrame(frame);
  }, [activeSearchLine, highlightRange?.start, highlightRange?.end, code, highlighted]);

  const effectiveMaxHeight = expanded ? "none" : maxHeight;
  const actionsNode = (
    <CodeBlockActions className="gap-0.5">
      <Button
        type="button"
        size="icon-xs"
        variant="ghost"
        aria-pressed={wrap}
        aria-label={wrap ? "关闭自动换行" : "开启自动换行"}
        title={wrap ? "关闭自动换行" : "开启自动换行"}
        onClick={() => setWrap((v) => !v)}
        className={cn("size-6 text-muted-foreground hover:text-foreground", wrap && "bg-accent text-foreground")}
      >
        <WrapText className="size-3" />
      </Button>
      {isLong ? (
        <Button
          type="button"
          size="icon-xs"
          variant="ghost"
          aria-label={expanded ? "收起代码块" : "展开完整代码块"}
          title={expanded ? "收起代码块" : "展开完整代码块"}
          onClick={() => setExpanded((v) => !v)}
          className="size-6 text-muted-foreground hover:text-foreground"
        >
          {expanded ? <Minimize2 className="size-3" /> : <Maximize2 className="size-3" />}
        </Button>
      ) : null}
      <CodeBlockCopyButton value={code} />
    </CodeBlockActions>
  );

  return (
    <CodeBlock ref={rootRef} size="sm" className={cn("not-typeset", className)}>
      {title || skippedHighlight ? (
        <CodeBlockHeader>
          <CodeBlockTitle className="flex items-center gap-2">
            {title}
            {skippedHighlight ? <span className="text-[11px] font-normal text-muted-foreground">文件较长，已跳过语法高亮</span> : null}
          </CodeBlockTitle>
          {actionsNode}
        </CodeBlockHeader>
      ) : (
        actionsNode
      )}
      <CodeBlockContent
        lineNumbers={numbered && !onLineClick}
        wrap={wrap}
        style={effectiveMaxHeight && effectiveMaxHeight !== "none" ? { maxHeight: effectiveMaxHeight } : undefined}
      >
        {lines.map((line, index) => {
          const lineNumber = index + 1;
          const isHighlighted =
            (highlightRange !== undefined && lineNumber >= highlightRange.start && lineNumber <= highlightRange.end) ||
            activeSearchLine === lineNumber;
          const isSearchHit = !isHighlighted && Boolean(searchMatches?.has(lineNumber));
          return (
            <CodeBlockLine
              key={index}
              data-line={lineNumber}
              variant={isHighlighted ? "highlighted" : "default"}
              className={cn(
                isSearchHit && "bg-warning/15",
                onLineClick && numbered && "ps-0! group-data-[size=sm]/code-block:ps-0!",
              )}
            >
              {onLineClick && numbered ? (
                <>
                  <button
                    type="button"
                    title={`点击选择第 ${lineNumber} 行（按住 Shift 选范围）`}
                    onClick={(event) => onLineClick(lineNumber, event.shiftKey)}
                    className={cn(
                      "sticky left-0 z-10 me-3 inline-block shrink-0 cursor-pointer select-none border-e border-border/40 bg-background/95 px-2.5 text-right font-mono text-[11px] text-muted-foreground/60 tabular-nums hover:text-foreground",
                      isHighlighted && "bg-info/15 font-medium text-info-foreground",
                    )}
                    style={{ minWidth: `calc(${lineColStyle.width} + 1.25rem)` }}
                  >
                    {lineNumber}
                  </button>
                  {line}
                </>
              ) : (
                line
              )}
            </CodeBlockLine>
          );
        })}
      </CodeBlockContent>
    </CodeBlock>
  );
}

type DiffLine = {
  variant: "added" | "removed" | "default" | "highlighted";
  text: string;
  oldLine?: number | undefined;
  newLine?: number | undefined;
};
type DiffFile = {
  path: string;
  fromPath?: string | undefined;
  binary?: boolean | undefined;
  lines: DiffLine[];
  added: number;
  removed: number;
  maxLine: number;
};

function stripPrefix(path: string): string {
  return path.replace(/^[ab]\//, "").split("\t")[0] ?? path;
}

const EXTENDED_HEADER_PREFIXES = [
  "diff --git ",
  "index ",
  "Index: ",
  "===",
  "new file mode ",
  "deleted file mode ",
  "old mode ",
  "new mode ",
  "similarity index ",
  "dissimilarity index ",
  "copy from ",
  "copy to ",
  "GIT binary patch",
];

function isExtendedHeader(row: string): boolean {
  return EXTENDED_HEADER_PREFIXES.some((prefix) => row.startsWith(prefix));
}

const HUNK_RE = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@\s?(.*)$/;

/** Splits a unified patch into files; preserves delete paths, renames, binary notices, and old/new line numbers. */
export function parsePatch(patch: string, fallbackPath: string): DiffFile[] {
  const files: DiffFile[] = [];
  let file: DiffFile | undefined;
  let oldLine: number | undefined;
  let newLine: number | undefined;
  let inHunk = false;
  let pendingOldPath: string | undefined;
  let pendingRenameFrom: string | undefined;

  const open = (path: string, fromPath?: string): DiffFile => {
    const next: DiffFile = { path, ...(fromPath ? { fromPath } : {}), lines: [], added: 0, removed: 0, maxLine: 0 };
    files.push(next);
    oldLine = undefined;
    newLine = undefined;
    inHunk = false;
    return next;
  };

  for (const row of patch.split("\n")) {
    if (row.startsWith("rename from ")) {
      pendingRenameFrom = row.slice("rename from ".length).trim();
      continue;
    }
    if (row.startsWith("rename to ")) {
      const to = row.slice("rename to ".length).trim();
      file = open(to, pendingRenameFrom);
      pendingRenameFrom = undefined;
      continue;
    }
    if (row.startsWith("Binary files ")) {
      const target = file ?? open(fallbackPath);
      target.binary = true;
      target.lines.push({ variant: "highlighted", text: row });
      continue;
    }
    if (isExtendedHeader(row)) {
      if (row.startsWith("diff --git ")) {
        pendingOldPath = undefined;
        pendingRenameFrom = undefined;
      }
      continue;
    }
    if (row.startsWith("--- ")) {
      const raw = stripPrefix(row.slice(4).trim());
      pendingOldPath = raw === "/dev/null" ? undefined : raw;
      continue;
    }
    if (row.startsWith("+++ ")) {
      const path = stripPrefix(row.slice(4).trim());
      const resolved = path === "/dev/null" ? (pendingOldPath ?? fallbackPath) : path;
      file = open(resolved, pendingRenameFrom);
      pendingRenameFrom = undefined;
      continue;
    }
    if (row.startsWith("\\")) continue;

    if (row.startsWith("@@")) {
      file ??= open(fallbackPath);
      inHunk = true;
      const match = HUNK_RE.exec(row);
      if (match) {
        oldLine = Number(match[1]);
        newLine = Number(match[2]);
        const context = match[3]?.trim() ?? "";
        file.lines.push({ variant: "highlighted", text: context ? `⋯ ${context}` : "⋯" });
      } else {
        const context = row.replace(/^@@[^@]*@@\s?/, "").trim();
        file.lines.push({ variant: "highlighted", text: context ? `⋯ ${context}` : "⋯" });
      }
      continue;
    }

    if (!inHunk && !row) continue;

    file ??= open(fallbackPath);
    if (row.startsWith("+")) {
      file.added += 1;
      const lineNum = newLine;
      if (newLine !== undefined) {
        file.maxLine = Math.max(file.maxLine, newLine);
        newLine += 1;
      }
      file.lines.push({ variant: "added", text: row.slice(1), newLine: lineNum });
    } else if (row.startsWith("-")) {
      file.removed += 1;
      const lineNum = oldLine;
      if (oldLine !== undefined) {
        file.maxLine = Math.max(file.maxLine, oldLine);
        oldLine += 1;
      }
      file.lines.push({ variant: "removed", text: row.slice(1), oldLine: lineNum });
    } else {
      const prevOld = oldLine;
      const prevNew = newLine;
      if (oldLine !== undefined) {
        file.maxLine = Math.max(file.maxLine, oldLine);
        oldLine += 1;
      }
      if (newLine !== undefined) {
        file.maxLine = Math.max(file.maxLine, newLine);
        newLine += 1;
      }
      file.lines.push({
        variant: "default",
        text: row.startsWith(" ") ? row.slice(1) : row,
        oldLine: prevOld,
        newLine: prevNew,
      });
    }
  }

  for (const item of files) {
    while (item.lines.at(-1)?.variant === "default" && !item.lines.at(-1)?.text) item.lines.pop();
  }
  return files.filter((item) => item.lines.length || item.fromPath || item.binary);
}

/** Computes inline changed span [start, end) for a paired removed/added line via common prefix & suffix. */
function computeInlineDiffSpans(oldText: string, newText: string): { oldRange: [number, number]; newRange: [number, number] } | undefined {
  if (oldText === newText || oldText.length > 600 || newText.length > 600) return undefined;
  let prefix = 0;
  const minLen = Math.min(oldText.length, newText.length);
  while (prefix < minLen && oldText[prefix] === newText[prefix]) prefix++;
  let suffix = 0;
  while (
    suffix < minLen - prefix &&
    oldText[oldText.length - 1 - suffix] === newText[newText.length - 1 - suffix]
  ) {
    suffix++;
  }
  if (prefix === 0 && suffix === 0) return undefined;
  return {
    oldRange: [prefix, oldText.length - suffix],
    newRange: [prefix, newText.length - suffix],
  };
}

function renderWithSpan(text: string, range: [number, number] | undefined, highlightClass: string): React.ReactNode {
  if (!range || range[0] >= range[1]) return text;
  return (
    <>
      {text.slice(0, range[0])}
      <mark className={cn("rounded-[2px] text-inherit", highlightClass)}>{text.slice(range[0], range[1])}</mark>
      {text.slice(range[1])}
    </>
  );
}

function computeLineInlineMap(lines: DiffLine[]): Map<number, [number, number]> {
  const spans = new Map<number, [number, number]>();
  let i = 0;
  while (i < lines.length) {
    if (lines[i]!.variant !== "removed") {
      i++;
      continue;
    }
    const remStart = i;
    while (i < lines.length && lines[i]!.variant === "removed") i++;
    const addStart = i;
    while (i < lines.length && lines[i]!.variant === "added") i++;
    const pairCount = Math.min(addStart - remStart, i - addStart);
    for (let k = 0; k < pairCount; k++) {
      const res = computeInlineDiffSpans(lines[remStart + k]!.text, lines[addStart + k]!.text);
      if (res) {
        spans.set(remStart + k, res.oldRange);
        spans.set(addStart + k, res.newRange);
      }
    }
  }
  return spans;
}

type SplitRow =
  | { kind: "hunk"; text: string }
  | { kind: "pair"; left?: { line: DiffLine; span?: [number, number] }; right?: { line: DiffLine; span?: [number, number] } };

function buildSplitRows(lines: DiffLine[], spans: Map<number, [number, number]>): SplitRow[] {
  const rows: SplitRow[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i]!;
    if (line.variant === "highlighted") {
      rows.push({ kind: "hunk", text: line.text });
      i++;
    } else if (line.variant === "default") {
      rows.push({ kind: "pair", left: { line }, right: { line } });
      i++;
    } else {
      const removed: Array<{ line: DiffLine; span?: [number, number] }> = [];
      const added: Array<{ line: DiffLine; span?: [number, number] }> = [];
      while (i < lines.length && lines[i]!.variant === "removed") {
        removed.push({ line: lines[i]!, span: spans.get(i) });
        i++;
      }
      while (i < lines.length && lines[i]!.variant === "added") {
        added.push({ line: lines[i]!, span: spans.get(i) });
        i++;
      }
      const max = Math.max(removed.length, added.length);
      for (let k = 0; k < max; k++) {
        rows.push({ kind: "pair", left: removed[k], right: added[k] });
      }
    }
  }
  return rows;
}

export type DiffLineComment = {
  path: string;
  lineNumber?: number | undefined;
  variant: "added" | "removed" | "default";
  lineText: string;
  note: string;
};

export function DiffView({
  patch,
  path,
  maxHeight = "24rem",
  onCommentLine,
}: {
  patch: string;
  path: string;
  maxHeight?: string;
  onCommentLine?: ((comment: DiffLineComment) => void) | undefined;
}) {
  const [split, setSplit] = React.useState(false);
  const [commentTarget, setCommentTarget] = React.useState<{
    fileKey: string;
    lineIndex: number;
    path: string;
    lineNumber?: number | undefined;
    variant: "added" | "removed" | "default";
    lineText: string;
  }>();
  const [commentText, setCommentText] = React.useState("");
  const files = React.useMemo(() => parsePatch(patch, path), [patch, path]);
  // A pure mode change has Git headers and no line hunks.
  if (!files.length && patch.trim()) return <Code code={patch.trimEnd()} language="diff" title={path} maxHeight={maxHeight} />;

  const submitComment = (event: React.FormEvent) => {
    event.preventDefault();
    const note = commentText.trim();
    if (!commentTarget || !note || !onCommentLine) return;
    onCommentLine({
      path: commentTarget.path,
      lineNumber: commentTarget.lineNumber,
      variant: commentTarget.variant,
      lineText: commentTarget.lineText,
      note,
    });
    setCommentTarget(undefined);
    setCommentText("");
  };

  return (
    <>
      {files.map((file, fileIndex) => {
        const fileKey = `${file.path}-${fileIndex}`;
        const hasNumbers = file.maxLine > 0;
        const digits = Math.max(2, String(file.maxLine).length);
        const colStyle = { width: `${digits}ch` } as React.CSSProperties;
        const spans = computeLineInlineMap(file.lines);
        const splitRows = split ? buildSplitRows(file.lines, spans) : undefined;
        return (
          <CodeBlock key={fileKey} size="sm" className="not-typeset">
            <CodeBlockHeader>
              <CodeBlockTitle className="min-w-0 flex-1">
                <FileDiffIcon />
                <span className="truncate">{file.path}</span>
                {file.fromPath ? <span className="text-[11px] font-normal text-muted-foreground">← {file.fromPath}</span> : null}
              </CodeBlockTitle>
              <div className="ms-auto flex shrink-0 items-center gap-2">
                <span className="font-mono text-xs tabular-nums">
                  <span className="text-success-foreground">+{file.added}</span>{" "}
                  <span className="text-destructive-foreground">−{file.removed}</span>
                </span>
                {!file.binary && file.lines.length > 0 ? (
                  <Button
                    type="button"
                    size="icon-xs"
                    variant="ghost"
                    aria-pressed={split}
                    title={split ? "切换为统一 Diff 视图" : "切换为并排 Diff 视图"}
                    aria-label={split ? "切换为统一 Diff 视图" : "切换为并排 Diff 视图"}
                    onClick={() => setSplit((v) => !v)}
                    className={cn("size-6 text-muted-foreground hover:text-foreground", split && "bg-accent text-foreground")}
                  >
                    {split ? <Rows2 className="size-3" /> : <Columns2 className="size-3" />}
                  </Button>
                ) : null}
                <CodeBlockCopyButton value={patch} title="复制 Diff" />
              </div>
            </CodeBlockHeader>
            {splitRows ? (
              <div
                className="overflow-auto font-mono text-xs leading-5"
                style={maxHeight && maxHeight !== "none" ? { maxHeight } : undefined}
              >
                <div className="min-w-[38rem]">
                  {splitRows.map((row, rowIndex) =>
                    row.kind === "hunk" ? (
                      <div key={rowIndex} className="bg-muted/40 px-3 py-0.5 text-[11px] text-muted-foreground">
                        {row.text}
                      </div>
                    ) : (
                      <div key={rowIndex} className="grid grid-cols-2 divide-x divide-border/50">
                        <div
                          className={cn(
                            "flex min-w-0 whitespace-pre px-2 py-px",
                            row.left?.line.variant === "removed" && "bg-destructive/10 text-destructive-foreground",
                            !row.left && "bg-muted/20",
                          )}
                        >
                          <span aria-hidden className="me-2 shrink-0 select-none text-right text-[11px] text-muted-foreground/55 tabular-nums" style={colStyle}>
                            {row.left?.line.oldLine ?? ""}
                          </span>
                          <span className="min-w-0 flex-1 overflow-hidden text-ellipsis">
                            {row.left
                              ? renderWithSpan(row.left.line.text, row.left.span, "bg-destructive/25")
                              : ""}
                          </span>
                        </div>
                        <div
                          className={cn(
                            "flex min-w-0 whitespace-pre px-2 py-px",
                            row.right?.line.variant === "added" && "bg-success/10 text-success-foreground",
                            !row.right && "bg-muted/20",
                          )}
                        >
                          <span aria-hidden className="me-2 shrink-0 select-none text-right text-[11px] text-muted-foreground/55 tabular-nums" style={colStyle}>
                            {row.right?.line.newLine ?? ""}
                          </span>
                          <span className="min-w-0 flex-1 overflow-hidden text-ellipsis">
                            {row.right
                              ? renderWithSpan(row.right.line.text, row.right.span, "bg-success/25")
                              : ""}
                          </span>
                        </div>
                      </div>
                    ),
                  )}
                </div>
              </div>
            ) : (
              <CodeBlockContent style={maxHeight && maxHeight !== "none" ? { maxHeight } : undefined}>
                {file.lines.map((line, index) => {
                  const sign = line.variant === "added" ? "+" : line.variant === "removed" ? "−" : " ";
                  const span = spans.get(index);
                  const canComment = Boolean(onCommentLine) && line.variant !== "highlighted";
                  const commenting = commentTarget?.fileKey === fileKey && commentTarget.lineIndex === index;
                  const lineNumber = line.newLine ?? line.oldLine;
                  return (
                    <React.Fragment key={index}>
                      <CodeBlockLine
                        variant={line.variant}
                        className={cn(
                          "group/diffline ps-0! group-data-[size=sm]/code-block:ps-0!",
                          line.variant === "highlighted" && "text-muted-foreground",
                        )}
                      >
                        <span
                          aria-hidden="true"
                          className={cn(
                            "sticky left-0 z-10 me-2.5 inline-flex shrink-0 select-none items-center gap-1.5 border-e border-border/35 bg-background/95 px-2 text-right text-[11px] text-muted-foreground/55 tabular-nums",
                            line.variant === "added" && "bg-success/12 text-success-foreground/80",
                            line.variant === "removed" && "bg-destructive/12 text-destructive-foreground/80",
                            line.variant === "highlighted" && "bg-muted/60",
                          )}
                        >
                          {canComment ? (
                            <button
                              type="button"
                              title="对此行写评审意见并引用到对话"
                              aria-label={`对第 ${lineNumber ?? index + 1} 行添加评审意见`}
                              onClick={() => {
                                if (commenting) {
                                  setCommentTarget(undefined);
                                  return;
                                }
                                setCommentTarget({
                                  fileKey,
                                  lineIndex: index,
                                  path: file.path,
                                  lineNumber,
                                  variant: line.variant as "added" | "removed" | "default",
                                  lineText: line.text,
                                });
                                setCommentText("");
                              }}
                              className={cn(
                                "inline-flex size-4 cursor-pointer items-center justify-center rounded text-muted-foreground opacity-0 transition-[opacity,color,background-color] duration-150 hover:bg-muted hover:text-foreground group-hover/diffline:opacity-100 focus-visible:opacity-100 pointer-coarse:opacity-100",
                                commenting && "bg-accent text-foreground opacity-100",
                              )}
                            >
                              <MessageSquarePlus className="size-3" />
                            </button>
                          ) : null}
                          {hasNumbers ? (
                            <>
                              <span className="inline-block" style={colStyle}>
                                {line.variant === "highlighted" ? "" : (line.oldLine ?? "")}
                              </span>
                              <span className="inline-block" style={colStyle}>
                                {line.variant === "highlighted" ? "" : (line.newLine ?? "")}
                              </span>
                            </>
                          ) : null}
                          <span className="inline-block w-2.5 text-center font-semibold">
                            {line.variant === "highlighted" ? "" : sign}
                          </span>
                        </span>
                        {line.variant === "added"
                          ? renderWithSpan(line.text, span, "bg-success/25")
                          : line.variant === "removed"
                            ? renderWithSpan(line.text, span, "bg-destructive/25")
                            : line.text}
                      </CodeBlockLine>
                      {commenting ? (
                        <form
                          onSubmit={submitComment}
                          className="surface-enter sticky left-0 z-10 flex items-center gap-1.5 border-y border-border/70 bg-card px-2.5 py-1.5 font-sans"
                        >
                          <Input
                            autoFocus
                            value={commentText}
                            placeholder={`针对 ${file.path}${lineNumber ? `:${lineNumber}` : ""} 的修改意见…`}
                            aria-label="行内评审意见"
                            className="h-7 min-w-0 flex-1 bg-background text-xs"
                            onChange={(event) => setCommentText(event.target.value)}
                            onKeyDown={(event) => {
                              if (event.key === "Escape") {
                                event.preventDefault();
                                setCommentTarget(undefined);
                              }
                            }}
                          />
                          <Button type="submit" size="xs" disabled={!commentText.trim()}>
                            <Send className="size-3" />
                            引用到对话
                          </Button>
                          <Button type="button" size="icon-xs" variant="ghost" aria-label="取消批注" onClick={() => setCommentTarget(undefined)}>
                            <X className="size-3" />
                          </Button>
                        </form>
                      ) : null}
                    </React.Fragment>
                  );
                })}
              </CodeBlockContent>
            )}
          </CodeBlock>
        );
      })}
    </>
  );
}
