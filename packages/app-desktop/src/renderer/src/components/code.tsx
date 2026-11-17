import * as React from "react";
import { FileDiff as FileDiffIcon } from "lucide-react";
import type { FileDiff } from "@lila/protocol";
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
  language?: string;
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

type DiffLine = { variant: "added" | "removed" | "default"; text: string };

/** omp diffs are `+N|text`, `-N|text`, ` N|text` rows; other formats fall back to unified-diff markers. */
export function parseDiff(diff: string): DiffLine[] {
  return diff.split("\n").map((row) => {
    const match = /^([+\- ])\s*(\d+)?\|(.*)$/.exec(row);
    const marker = match ? match[1] : row[0];
    const text = match ? (match[3] ?? "") : row.slice(1);
    return { variant: marker === "+" ? "added" : marker === "-" ? "removed" : "default", text };
  });
}

export function DiffView({ file }: { file: FileDiff }) {
  const lines = React.useMemo(() => parseDiff(file.diff), [file.diff]);
  const added = lines.filter((line) => line.variant === "added").length;
  const removed = lines.filter((line) => line.variant === "removed").length;
  return (
    <CodeBlock size="sm" className="not-typeset">
      <CodeBlockHeader>
        <CodeBlockTitle>
          <FileDiffIcon />
          {file.path}
        </CodeBlockTitle>
        <span className="ms-auto shrink-0 font-mono text-xs tabular-nums">
          <span className="text-emerald-600 dark:text-emerald-400">+{added}</span>{" "}
          <span className="text-destructive">−{removed}</span>
        </span>
      </CodeBlockHeader>
      <CodeBlockContent style={{ maxHeight: "24rem" }}>
        {lines.map((line, index) => (
          <CodeBlockLine key={index} variant={line.variant}>
            {line.text}
          </CodeBlockLine>
        ))}
      </CodeBlockContent>
    </CodeBlock>
  );
}
