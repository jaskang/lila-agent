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
  css: "css",
  html: "html",
  md: "markdown",
  py: "python",
  sh: "bash",
  sql: "sql",
  yml: "yaml",
  yaml: "yaml",
};

export function languageOf(path: string): string | undefined {
  return EXTENSION_LANGUAGE[path.split(".").pop()?.toLowerCase() ?? ""];
}

export function Code({
  code,
  language,
  title,
  streaming = false,
  maxHeight = "20rem",
}: {
  code: string;
  language?: string | undefined;
  title?: React.ReactNode;
  streaming?: boolean;
  maxHeight?: string;
}) {
  const highlighted = useHighlighted(code, language, !streaming);
  const text = code.replace(/\n$/, "");
  return (
    <CodeBlock size="sm" className="not-typeset">
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
      <CodeBlockContent style={{ maxHeight }}>
        {highlighted
          ? highlighted.map((line, index) => <CodeBlockLine key={index}>{line}</CodeBlockLine>)
          : text.split("\n").map((line, index) => <CodeBlockLine key={index}>{line}</CodeBlockLine>)}
      </CodeBlockContent>
    </CodeBlock>
  );
}

type DiffLine = { variant: "added" | "removed" | "default" | "highlighted"; text: string };
type DiffFile = { path: string; lines: DiffLine[]; added: number; removed: number };

function stripPrefix(path: string): string {
  return path.replace(/^[ab]\//, "").split("\t")[0] ?? path;
}

/** Splits a unified patch into files; hunk headers become highlighted separator lines. */
export function parsePatch(patch: string, fallbackPath: string): DiffFile[] {
  const files: DiffFile[] = [];
  let file: DiffFile | undefined;
  const open = (path: string): DiffFile => {
    const next = { path, lines: [], added: 0, removed: 0 };
    files.push(next);
    return next;
  };
  for (const row of patch.split("\n")) {
    if (row.startsWith("diff --git ") || row.startsWith("index ") || row.startsWith("Index: ") || row.startsWith("===")) continue;
    if (row.startsWith("--- ")) continue;
    if (row.startsWith("+++ ")) {
      const path = stripPrefix(row.slice(4).trim());
      file = open(path === "/dev/null" ? fallbackPath : path);
      continue;
    }
    file ??= open(fallbackPath);
    if (row.startsWith("@@")) {
      const context = row.replace(/^@@[^@]*@@\s?/, "");
      file.lines.push({ variant: "highlighted", text: context ? `⋯ ${context}` : "⋯" });
    } else if (row.startsWith("+")) {
      file.added += 1;
      file.lines.push({ variant: "added", text: row.slice(1) });
    } else if (row.startsWith("-")) {
      file.removed += 1;
      file.lines.push({ variant: "removed", text: row.slice(1) });
    } else if (row.startsWith("\\")) {
      continue;
    } else {
      file.lines.push({ variant: "default", text: row.slice(1) });
    }
  }
  for (const item of files) {
    while (item.lines.at(-1)?.variant === "default" && !item.lines.at(-1)?.text) item.lines.pop();
  }
  return files.filter((item) => item.lines.length);
}

export function DiffView({ patch, path }: { patch: string; path: string }) {
  const files = React.useMemo(() => parsePatch(patch, path), [patch, path]);
  return (
    <>
      {files.map((file, fileIndex) => (
        <CodeBlock key={`${file.path}-${fileIndex}`} size="sm" className="not-typeset">
          <CodeBlockHeader>
            <CodeBlockTitle>
              <FileDiffIcon />
              {file.path}
            </CodeBlockTitle>
            <span className="ms-auto shrink-0 font-mono text-xs tabular-nums">
              <span className="text-emerald-600 dark:text-emerald-400">+{file.added}</span>{" "}
              <span className="text-destructive">−{file.removed}</span>
            </span>
          </CodeBlockHeader>
          <CodeBlockContent style={{ maxHeight: "24rem" }}>
            {file.lines.map((line, index) => (
              <CodeBlockLine
                key={index}
                variant={line.variant}
                className={line.variant === "highlighted" ? "text-muted-foreground" : undefined}
              >
                {line.text}
              </CodeBlockLine>
            ))}
          </CodeBlockContent>
        </CodeBlock>
      ))}
    </>
  );
}
