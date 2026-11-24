import * as React from "react";
import { FileDiff as FileDiffIcon } from "lucide-react";
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
import { useHighlighted } from "@/lib/highlight";

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
  className,
}: {
  code: string;
  language?: string | undefined;
  title?: React.ReactNode;
  streaming?: boolean;
  maxHeight?: string | undefined;
  numbered?: boolean;
  highlightRange?: { start: number; end: number } | undefined;
  className?: string;
}) {
  const highlighted = useHighlighted(code, language, !streaming);
  const text = code.replace(/\n$/, "");
  const rootRef = React.useRef<HTMLDivElement>(null);

  React.useEffect(() => {
    if (!highlightRange?.start) return;
    const frame = requestAnimationFrame(() => {
      const target = rootRef.current?.querySelector<HTMLElement>(`[data-line="${highlightRange.start}"]`);
      target?.scrollIntoView({ block: "center", inline: "nearest" });
    });
    return () => cancelAnimationFrame(frame);
  }, [highlightRange?.start, highlightRange?.end, code]);

  const lines = highlighted ?? text.split("\n");
  return (
    <CodeBlock ref={rootRef} size="sm" className={cn("not-typeset", className)}>
      {title ? (
        <CodeBlockHeader>
          <CodeBlockTitle>{title}</CodeBlockTitle>
          <CodeBlockActions>
            <CodeBlockCopyButton value={code} />
          </CodeBlockActions>
        </CodeBlockHeader>
      ) : (
        <CodeBlockActions>
          <CodeBlockCopyButton value={code} />
        </CodeBlockActions>
      )}
      <CodeBlockContent lineNumbers={numbered} style={maxHeight ? { maxHeight } : undefined}>
        {lines.map((line, index) => {
          const lineNumber = index + 1;
          const isHighlighted =
            highlightRange !== undefined &&
            lineNumber >= highlightRange.start &&
            lineNumber <= highlightRange.end;
          return (
            <CodeBlockLine key={index} data-line={lineNumber} variant={isHighlighted ? "highlighted" : "default"}>
              {line}
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
type DiffFile = { path: string; lines: DiffLine[]; added: number; removed: number; maxLine: number };

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
  "rename from ",
  "rename to ",
  "copy from ",
  "copy to ",
  "Binary files ",
  "GIT binary patch",
];

function isExtendedHeader(row: string): boolean {
  return EXTENDED_HEADER_PREFIXES.some((prefix) => row.startsWith(prefix));
}

const HUNK_RE = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@\s?(.*)$/;

/** Splits a unified patch into files; skips Git extended headers and tracks old/new line numbers. */
export function parsePatch(patch: string, fallbackPath: string): DiffFile[] {
  const files: DiffFile[] = [];
  let file: DiffFile | undefined;
  let oldLine: number | undefined;
  let newLine: number | undefined;
  let inHunk = false;

  const open = (path: string): DiffFile => {
    const next: DiffFile = { path, lines: [], added: 0, removed: 0, maxLine: 0 };
    files.push(next);
    oldLine = undefined;
    newLine = undefined;
    inHunk = false;
    return next;
  };

  for (const row of patch.split("\n")) {
    if (isExtendedHeader(row)) continue;
    if (row.startsWith("--- ")) continue;
    if (row.startsWith("+++ ")) {
      const path = stripPrefix(row.slice(4).trim());
      file = open(path === "/dev/null" ? fallbackPath : path);
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
  return files.filter((item) => item.lines.length);
}

export function DiffView({ patch, path, maxHeight = "24rem" }: { patch: string; path: string; maxHeight?: string }) {
  const files = React.useMemo(() => parsePatch(patch, path), [patch, path]);
  // A pure rename or mode change has Git headers and no line hunks.
  if (!files.length && patch.trim()) return <Code code={patch.trimEnd()} language="diff" title={path} maxHeight={maxHeight} />;
  return (
    <>
      {files.map((file, fileIndex) => {
        const hasNumbers = file.maxLine > 0;
        const digits = Math.max(2, String(file.maxLine).length);
        const colStyle = { width: `${digits}ch` } as React.CSSProperties;
        return (
          <CodeBlock key={`${file.path}-${fileIndex}`} size="sm" className="not-typeset">
            <CodeBlockHeader>
              <CodeBlockTitle>
                <FileDiffIcon />
                {file.path}
              </CodeBlockTitle>
              <span className="ms-auto shrink-0 font-mono text-xs tabular-nums">
                <span className="text-success-foreground">+{file.added}</span>{" "}
                <span className="text-destructive-foreground">−{file.removed}</span>
              </span>
            </CodeBlockHeader>
            <CodeBlockContent style={{ maxHeight }}>
              {file.lines.map((line, index) => (
                <CodeBlockLine
                  key={index}
                  variant={line.variant}
                  className={cn(
                    "ps-5 group-data-[size=sm]/code-block:ps-5",
                    line.variant === "highlighted" && "text-muted-foreground",
                  )}
                >
                  {hasNumbers ? (
                    <>
                      <span
                        aria-hidden="true"
                        className="me-3 inline-flex shrink-0 select-none gap-1.5 text-right text-[11px] text-muted-foreground/55 tabular-nums"
                      >
                        <span className="inline-block" style={colStyle}>
                          {line.variant === "highlighted" ? "" : (line.oldLine ?? "")}
                        </span>
                        <span className="inline-block" style={colStyle}>
                          {line.variant === "highlighted" ? "" : (line.newLine ?? "")}
                        </span>
                      </span>
                      {line.text}
                    </>
                  ) : (
                    line.text
                  )}
                </CodeBlockLine>
              ))}
            </CodeBlockContent>
          </CodeBlock>
        );
      })}
    </>
  );
}
