import * as React from "react";
import { ExternalLink, Image as ImageIcon, ImageOff } from "lucide-react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { toast } from "sonner";
import { Code } from "@/components/code";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";

const MarkdownBasePathContext = React.createContext<string | undefined>(undefined);

export const MarkdownBasePathProvider = MarkdownBasePathContext.Provider;

const SCHEME_RE = /^[a-zA-Z][a-zA-Z0-9+.-]*:/;
const SAFE_EXTERNAL_SCHEME_RE = /^(https?:|mailto:|ircs?:|xmpp:|lila-media:)/i;

function markdownUrlTransform(url: string, key: string): string {
  const trimmed = url.trim();
  if (!trimmed) return "";
  if (SAFE_EXTERNAL_SCHEME_RE.test(trimmed) || /^file:/i.test(trimmed)) return trimmed;
  if (key === "src" && /^data:image\/[a-z0-9.+-]+(?:;|,)/i.test(trimmed)) return trimmed;
  if (SCHEME_RE.test(trimmed)) return "";
  return trimmed;
}

function stripQueryAndHash(raw: string): string {
  return raw.split(/[?#]/, 1)[0] ?? raw;
}

function decodeSafe(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

function normalizePosixPath(path: string): string {
  const parts = path.split("/");
  const out: string[] = [];
  for (const part of parts) {
    if (!part || part === ".") continue;
    if (part === "..") out.pop();
    else out.push(part);
  }
  return `/${out.join("/")}`;
}

export function resolveLocalFilePath(raw: string | undefined, basePath?: string): string | undefined {
  if (!raw) return undefined;
  const trimmed = raw.trim();
  if (!trimmed || trimmed.startsWith("#") || trimmed.startsWith("//")) return undefined;

  if (/^file:/i.test(trimmed)) {
    try {
      const parsed = new URL(trimmed);
      if (parsed.pathname) return normalizePosixPath(decodeSafe(parsed.pathname));
    } catch {
      return undefined;
    }
    return undefined;
  }

  if (SCHEME_RE.test(trimmed)) return undefined;

  const clean = decodeSafe(stripQueryAndHash(trimmed));
  if (!clean) return undefined;
  if (clean.startsWith("/")) return normalizePosixPath(clean);
  if (!basePath) return undefined;
  return normalizePosixPath(`${basePath.replace(/\/+$/, "")}/${clean}`);
}

function toMediaUrl(localPath: string, basePath?: string): string {
  const params = new URLSearchParams({ path: localPath });
  if (basePath) params.set("base", basePath);
  return `lila-media://local?${params.toString()}`;
}

/** Remote http(s) URLs other than http://localhost or http://127.0.0.1 are never auto-fetched to prevent prompt-injection exfiltration. */
function parseExternalRemoteImage(raw: string | undefined): { url: string; host: string } | undefined {
  if (!raw) return undefined;
  const trimmed = raw.trim();
  if (!/^https?:\/\//i.test(trimmed)) return undefined;
  try {
    const parsed = new URL(trimmed);
    const isLocalHttp = parsed.protocol === "http:" && (parsed.hostname === "localhost" || parsed.hostname === "127.0.0.1");
    if (isLocalHttp) return undefined;
    return { url: parsed.href, host: parsed.host || parsed.href };
  } catch {
    return undefined;
  }
}

/** If a local dev-server URL (localhost / 127.0.0.1) fails to load, try resolving its pathname in the project directory. */
function localhostFallbackUrl(raw: string | undefined, basePath?: string): string | undefined {
  if (!raw || !basePath || !/^http:\/\//i.test(raw)) return undefined;
  try {
    const parsed = new URL(raw);
    if (parsed.hostname !== "localhost" && parsed.hostname !== "127.0.0.1") return undefined;
    const pathname = decodeSafe(parsed.pathname);
    if (!pathname || pathname === "/") return undefined;
    return toMediaUrl(normalizePosixPath(pathname), basePath);
  } catch {
    return undefined;
  }
}

function MarkdownImage({ src, alt, title }: { src?: string | undefined; alt?: string | undefined; title?: string | undefined }) {
  const basePath = React.useContext(MarkdownBasePathContext);
  const localPath = resolveLocalFilePath(src, basePath);
  const externalRemote = React.useMemo(() => (localPath ? undefined : parseExternalRemoteImage(src)), [localPath, src]);
  const primarySrc = localPath ? toMediaUrl(localPath, basePath) : externalRemote ? undefined : src;
  const fallbackSrc = React.useMemo(() => (localPath || externalRemote ? undefined : localhostFallbackUrl(src, basePath)), [localPath, externalRemote, src, basePath]);

  const [useFallback, setUseFallback] = React.useState(false);
  const [failed, setFailed] = React.useState(false);
  const [previewOpen, setPreviewOpen] = React.useState(false);

  React.useEffect(() => {
    setUseFallback(false);
    setFailed(false);
  }, [primarySrc, fallbackSrc]);

  if (externalRemote) {
    const caption = alt?.trim();
    return (
      <span
        data-not-typeset
        title={externalRemote.url}
        className="my-1 inline-flex max-w-full items-center gap-2 rounded-lg border bg-muted/40 px-2.5 py-1.5 align-middle text-xs text-muted-foreground"
      >
        <ImageIcon className="size-3.5 shrink-0" />
        <span className="min-w-0 truncate">
          {caption ? (
            <>
              <span className="font-medium text-foreground">{caption}</span>
              <span className="ms-1.5 text-muted-foreground">({externalRemote.host})</span>
            </>
          ) : (
            externalRemote.host
          )}
        </span>
        <a
          href={externalRemote.url}
          target="_blank"
          rel="noreferrer"
          className="inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-foreground no-underline hover:bg-muted"
        >
          <ExternalLink className="size-3" />
          在浏览器中打开
        </a>
      </span>
    );
  }

  const activeSrc = useFallback ? fallbackSrc : primarySrc;

  if (!activeSrc || failed) {
    const label = alt || localPath?.split("/").pop() || src || "图片加载失败";
    return (
      <span
        data-not-typeset
        className="my-1 inline-flex max-w-full items-center gap-2 rounded-lg border bg-muted/40 px-2.5 py-1.5 align-middle text-xs text-muted-foreground"
      >
        <ImageOff className="size-3.5 shrink-0" />
        <span className="truncate">{label}</span>
        {localPath ? (
          <button
            type="button"
            className="inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-foreground hover:bg-muted"
            onClick={() => void window.lila.openPath(localPath).catch((error: unknown) => toast.error(error instanceof Error ? error.message : String(error)))}
          >
            <ExternalLink className="size-3" />
            打开
          </button>
        ) : null}
      </span>
    );
  }

  return (
    <>
      <button
        type="button"
        data-not-typeset
        aria-label={alt ? `放大查看图片：${alt}` : "放大查看图片"}
        onClick={() => setPreviewOpen(true)}
        className="my-1.5 inline-block max-w-full cursor-zoom-in overflow-hidden rounded-lg border bg-muted/20 align-middle transition-opacity hover:opacity-95 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
      >
        <img
          src={activeSrc}
          alt={alt ?? ""}
          title={title ?? alt}
          loading="lazy"
          onError={() => {
            if (!useFallback && fallbackSrc) setUseFallback(true);
            else setFailed(true);
          }}
          className="block max-h-[420px] max-w-full object-contain"
        />
      </button>
      <Dialog open={previewOpen} onOpenChange={setPreviewOpen}>
        <DialogContent className="w-fit max-w-[92vw] p-2 sm:max-w-[92vw]">
          <DialogTitle className="sr-only">{alt || "图片预览"}</DialogTitle>
          <img src={activeSrc} alt={alt ?? ""} className="max-h-[85vh] max-w-[88vw] rounded-lg object-contain" />
        </DialogContent>
      </Dialog>
    </>
  );
}

function MarkdownLink({ href, children }: { href?: string | undefined; children?: React.ReactNode }) {
  const basePath = React.useContext(MarkdownBasePathContext);
  const localPath = resolveLocalFilePath(href, basePath);
  if (localPath) {
    return (
      <a
        href={href}
        onClick={(event) => {
          event.preventDefault();
          void window.lila.openPath(localPath).catch((error: unknown) => toast.error(error instanceof Error ? error.message : String(error)));
        }}
      >
        {children}
      </a>
    );
  }
  return (
    <a href={href} target="_blank" rel="noreferrer">
      {children}
    </a>
  );
}

function components(streaming: boolean): Components {
  return {
    pre: ({ children }) => <>{children}</>,
    code: ({ className, children }) => {
      const language = /language-([^\s]+)/.exec(className ?? "")?.[1];
      const value = String(children);
      if (!language && !value.includes("\n")) return <code>{children}</code>;
      return (
        <Code
          code={value}
          {...(language ? { language, title: <span className="font-mono text-muted-foreground">{language}</span> } : {})}
          streaming={streaming}
        />
      );
    },
    a: ({ href, children }) => <MarkdownLink href={href}>{children}</MarkdownLink>,
    img: ({ src, alt, title }) => <MarkdownImage src={src} alt={alt} title={title} />,
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

const LIST_ITEM_RE = /^\s*(?:[-*+]|\d+[.)])\s+/;
const BLOCKQUOTE_RE = /^\s*>/;
const INDENTED_CONTINUATION_RE = /^(?: {2,}|\t)\S/;

function nextNonBlankLine(lines: string[], from: number): string | undefined {
  for (let i = from; i < lines.length; i++) {
    const candidate = lines[i]!;
    if (candidate.trim() !== "") return candidate;
  }
  return undefined;
}

/**
 * Splits streaming markdown into settled top-level blocks and the growing tail.
 * Avoids splitting inside fenced code blocks, loose lists, or multi-paragraph blockquotes.
 */
function splitStreamingBlocks(text: string): { settled: string[]; active: string } {
  const lines = text.split("\n");
  const settled: string[] = [];
  let current: string[] = [];
  let fenceChar = "";
  let fenceLen = 0;

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]!;
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
        const next = nextNonBlankLine(lines, i + 1);
        const currentIsList = LIST_ITEM_RE.test(current[0] ?? "");
        const keepsTogether =
          next !== undefined &&
          (LIST_ITEM_RE.test(next) || BLOCKQUOTE_RE.test(next) || (currentIsList && INDENTED_CONTINUATION_RE.test(next)));
        if (keepsTogether) {
          current.push(line);
        } else {
          settled.push(current.join("\n"));
          current = [];
        }
      }
      continue;
    }

    current.push(line);
  }

  return { settled, active: current.join("\n") };
}

const MarkdownBlock = React.memo(function MarkdownBlock({ text, streaming }: { text: string; streaming: boolean }) {
  return (
    <ReactMarkdown remarkPlugins={plugins} urlTransform={markdownUrlTransform} components={streaming ? streamingComponents : settledComponents}>
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
