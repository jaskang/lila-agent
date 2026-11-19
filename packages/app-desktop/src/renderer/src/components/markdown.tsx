import * as React from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { Code } from "@/components/code";

function components(streaming: boolean): Components {
  return {
    pre: ({ children }) => <>{children}</>,
    code: ({ className, children }) => {
      const language = /language-(\w+)/.exec(className ?? "")?.[1];
      const value = String(children);
      if (!language && !value.includes("\n")) return <code>{children}</code>;
      return <Code code={value} {...(language ? { language } : {})} streaming={streaming} />;
    },
    a: ({ href, children }) => (
      <a href={href} target="_blank" rel="noreferrer">
        {children}
      </a>
    ),
    table: ({ children }) => (
      <div className="typeset-scroll">
        <table>{children}</table>
      </div>
    ),
  };
}

const streamingComponents = components(true);
const settledComponents = components(false);
const plugins = [remarkGfm];

/**
 * Splits streaming markdown into settled top-level blocks and the growing tail.
 * Only splits on blank lines outside fenced code blocks so code fences stay intact.
 */
function splitStreamingBlocks(text: string): { settled: string[]; active: string } {
  const lines = text.split("\n");
  const settled: string[] = [];
  let current: string[] = [];
  let fenceChar = "";
  let fenceLen = 0;

  for (const line of lines) {
    const fence = /^\s*(`{3,}|~{3,})(.*)$/.exec(line);
    if (fence) {
      const marker = fence[1]!;
      const rest = fence[2]!;
      if (!fenceChar) {
        fenceChar = marker[0]!;
        fenceLen = marker.length;
      } else if (marker[0] === fenceChar && marker.length >= fenceLen && rest.trim() === "") {
        fenceChar = "";
        fenceLen = 0;
      }
      current.push(line);
      continue;
    }

    if (!fenceChar && line.trim() === "") {
      if (current.length > 0) {
        settled.push(current.join("\n"));
        current = [];
      }
      continue;
    }

    current.push(line);
  }

  return { settled, active: current.join("\n") };
}

const MarkdownBlock = React.memo(function MarkdownBlock({ text, streaming }: { text: string; streaming: boolean }) {
  return (
    <ReactMarkdown remarkPlugins={plugins} components={streaming ? streamingComponents : settledComponents}>
      {text}
    </ReactMarkdown>
  );
});

export const Markdown = React.memo(function Markdown({ text, streaming = false }: { text: string; streaming?: boolean }) {
  if (!streaming) {
    return (
      <div className="typeset typeset-chat">
        <MarkdownBlock text={text} streaming={false} />
      </div>
    );
  }
  const { settled, active } = splitStreamingBlocks(text);
  return (
    <div className="typeset typeset-chat">
      {settled.map((block, index) => (
        <MarkdownBlock key={index} text={block} streaming={false} />
      ))}
      {active ? <MarkdownBlock text={active} streaming /> : null}
    </div>
  );
});
