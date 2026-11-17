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

export const Markdown = React.memo(function Markdown({ text, streaming = false }: { text: string; streaming?: boolean }) {
  return (
    <div className="typeset typeset-chat">
      <ReactMarkdown remarkPlugins={plugins} components={streaming ? streamingComponents : settledComponents}>
        {text}
      </ReactMarkdown>
    </div>
  );
});
