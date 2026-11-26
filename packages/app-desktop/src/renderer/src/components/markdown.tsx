import type { TooltipRootActions, TooltipRootProps } from "@base-ui/react/tooltip";
import * as React from "react";
import { Check, Code2, Copy, ExternalLink, Eye, FileText, Image as ImageIcon, ImageOff } from "lucide-react";
import ReactMarkdown, { type Components } from "react-markdown";
import rehypeKatex from "rehype-katex";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import { toast } from "sonner";
import { Code } from "@/components/code";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { actions, useLila } from "@/lib/store";
import { useTheme } from "@/lib/theme";

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

const COLON_LINE_SUFFIX_RE = /:(\d+)(?::(\d+))?(?:-(\d+))?$/;

function splitColonLineSuffix(path: string): { path: string; line?: number; endLine?: number } {
  const match = COLON_LINE_SUFFIX_RE.exec(path);
  if (!match) return { path };
  const base = path.slice(0, match.index);
  if (!base || base.endsWith("/")) return { path };
  const line = Number(match[1]);
  const endLine = match[3] ? Number(match[3]) : undefined;
  if (!Number.isInteger(line) || line <= 0) return { path };
  return { path: base, line, ...(endLine && Number.isInteger(endLine) && endLine >= line ? { endLine } : {}) };
}

export function resolveLocalFilePath(raw: string | undefined, basePath?: string): string | undefined {
  if (!raw) return undefined;
  const trimmed = raw.trim();
  if (!trimmed || trimmed.startsWith("#") || trimmed.startsWith("//")) return undefined;

  if (/^file:/i.test(trimmed)) {
    try {
      const parsed = new URL(trimmed);
      if (parsed.pathname) return normalizePosixPath(splitColonLineSuffix(decodeSafe(parsed.pathname)).path);
    } catch {
      return undefined;
    }
    return undefined;
  }

  // Strip optional `:line[:col]` before checking scheme so `src/a.ts:42` is not mistaken for a URI scheme.
  const withoutQueryHash = decodeSafe(stripQueryAndHash(trimmed));
  const stripped = splitColonLineSuffix(withoutQueryHash).path;
  if (SCHEME_RE.test(stripped)) return undefined;
  if (!stripped) return undefined;
  if (stripped.startsWith("/")) return normalizePosixPath(stripped);
  if (!basePath) return undefined;
  return normalizePosixPath(`${basePath.replace(/\/+$/, "")}/${stripped}`);
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

function parseLineRange(href: string | undefined): { line?: number; endLine?: number } | undefined {
  if (!href) return undefined;
  const hashIndex = href.indexOf("#");
  if (hashIndex >= 0) {
    const hash = href.slice(hashIndex + 1);
    const match = /^L(\d+)(?:-L?(\d+))?$/i.exec(hash);
    if (match) {
      const line = Number(match[1]);
      const endLine = match[2] ? Number(match[2]) : undefined;
      if (Number.isInteger(line) && line > 0) {
        return { line, ...(endLine && Number.isInteger(endLine) && endLine >= line ? { endLine } : {}) };
      }
    }
  }
  const colon = splitColonLineSuffix(decodeSafe(stripQueryAndHash(href.trim())));
  if (colon.line) return { line: colon.line, ...(colon.endLine ? { endLine: colon.endLine } : {}) };
  return undefined;
}

function displayLocalPath(localPath: string, projectPath: string | undefined, range?: { line?: number; endLine?: number }): string {
  const prefix = projectPath ? `${projectPath.replace(/\/+$/, "")}/` : undefined;
  const base = prefix && localPath.startsWith(prefix) ? localPath.slice(prefix.length) : localPath;
  if (!range?.line) return base;
  return range.endLine && range.endLine !== range.line ? `${base}#L${range.line}-L${range.endLine}` : `${base}#L${range.line}`;
}

const LINK_CLASS =
  "font-medium text-info-foreground underline decoration-info-foreground/40 underline-offset-2 transition-colors hover:decoration-info-foreground focus-visible:rounded-xs focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring";

function pickLineRect(element: HTMLElement, pointer: { x: number; y: number } | null): DOMRect {
  const rects = Array.from(element.getClientRects()).filter((rect) => rect.width > 0 && rect.height > 0);
  if (rects.length <= 1) return rects[0] ?? element.getBoundingClientRect();
  if (!pointer) return rects[0]!;
  for (const rect of rects) {
    if (pointer.x >= rect.left && pointer.x <= rect.right && pointer.y >= rect.top && pointer.y <= rect.bottom) {
      return rect;
    }
  }
  let closest = rects[0]!;
  let minDistance = Infinity;
  for (const rect of rects) {
    const dx = pointer.x < rect.left ? rect.left - pointer.x : pointer.x > rect.right ? pointer.x - rect.right : 0;
    const dy = pointer.y < rect.top ? rect.top - pointer.y : pointer.y > rect.bottom ? pointer.y - rect.bottom : 0;
    const distance = dx * dx + dy * dy;
    if (distance < minDistance) {
      minDistance = distance;
      closest = rect;
    }
  }
  return closest;
}

function MarkdownLink({ href, children }: { href?: string | undefined; children?: React.ReactNode }) {
  const basePath = React.useContext(MarkdownBasePathContext);
  const activeProjectPath = useLila((state) => {
    const projectId = state.selectedId ? state.sessions[state.selectedId]?.projectId : undefined;
    return projectId ? state.projects.find((project) => project.id === projectId)?.path : undefined;
  });
  const linkRef = React.useRef<HTMLAnchorElement>(null);
  const pointerRef = React.useRef<{ x: number; y: number } | null>(null);
  const [anchor, setAnchor] = React.useState<{ getBoundingClientRect: () => DOMRect } | undefined>(undefined);
  const tooltipActions = React.useRef<TooltipRootActions>(null);
  const [hoverTooltipOpen, setHoverTooltipOpen] = React.useState(false);

  const onTooltipOpenChange: TooltipRootProps["onOpenChange"] = (open, { reason }) => {
    setHoverTooltipOpen(open && reason === "trigger-hover");
  };

  React.useEffect(() => {
    const link = linkRef.current;
    if (!hoverTooltipOpen || !link) return;

    // Scrolling can move the link away without the mousemove that completes hover dismissal.
    const handleScroll = (event: Event) => {
      if (event.target instanceof Node && event.target.contains(link)) {
        tooltipActions.current?.close();
      }
    };
    link.ownerDocument.addEventListener("scroll", handleScroll, { capture: true, passive: true });
    return () => link.ownerDocument.removeEventListener("scroll", handleScroll, true);
  }, [hoverTooltipOpen]);

  const syncAnchor = React.useCallback((event?: React.PointerEvent<HTMLAnchorElement>) => {
    if (event) pointerRef.current = { x: event.clientX, y: event.clientY };
    const element = linkRef.current;
    if (!element) return;
    const pointer = pointerRef.current;
    setAnchor({
      getBoundingClientRect: () => pickLineRect(element, pointer),
    });
  }, []);

  const localPath = resolveLocalFilePath(href, basePath);
  if (localPath) {
    const range = parseLineRange(href);
    const tip = displayLocalPath(localPath, activeProjectPath ?? basePath, range);
    return (
      <Tooltip actionsRef={tooltipActions} onOpenChange={onTooltipOpenChange}>
        <TooltipTrigger
          render={
            <a
              ref={linkRef}
              href={href}
              className={LINK_CLASS}
              onPointerEnter={syncAnchor}
              onFocus={() => {
                pointerRef.current = null;
                syncAnchor();
              }}
              onClick={(event) => {
                event.preventDefault();
                if (activeProjectPath) {
                  actions.openFilePreview(localPath, range);
                } else {
                  void window.lila.openPath(localPath).catch((error: unknown) => toast.error(error instanceof Error ? error.message : String(error)));
                }
              }}
            />
          }
        >
          {children}
        </TooltipTrigger>
        <TooltipContent anchor={anchor} side="top" sideOffset={6} className="max-w-sm">
          <span className="inline-flex items-center gap-1.5 font-mono text-[11px] break-all">
            <FileText className="size-3 shrink-0 opacity-75" />
            {tip}
          </span>
        </TooltipContent>
      </Tooltip>
    );
  }
  return (
    <Tooltip actionsRef={tooltipActions} onOpenChange={onTooltipOpenChange}>
      <TooltipTrigger
        render={
          <a
            ref={linkRef}
            href={href}
            target="_blank"
            rel="noreferrer"
            className={LINK_CLASS}
            onPointerEnter={syncAnchor}
            onPointerMove={syncAnchor}
            onFocus={() => {
              pointerRef.current = null;
              syncAnchor();
            }}
          />
        }
      >
        {children}
      </TooltipTrigger>
      {href ? (
        <TooltipContent anchor={anchor} side="top" sideOffset={6} className="max-w-sm">
          <span className="inline-flex items-center gap-1.5 font-mono text-[11px] break-all">
            <ExternalLink className="size-3 shrink-0 opacity-75" />
            {href}
          </span>
        </TooltipContent>
      ) : null}
    </Tooltip>
  );
}

const FencedCodeContext = React.createContext(false);

let mermaidRenderSeq = 0;

function MermaidBlock({ code, streaming }: { code: string; streaming: boolean }) {
  const { resolvedTheme } = useTheme();
  const [svg, setSvg] = React.useState<string>();
  const [error, setError] = React.useState<string>();
  const [showSource, setShowSource] = React.useState(false);
  const [copied, setCopied] = React.useState(false);

  React.useEffect(() => {
    if (streaming || showSource) return;
    let cancelled = false;
    const renderId = `lila-mermaid-${++mermaidRenderSeq}`;
    void import("mermaid")
      .then(async (mod) => {
        const mermaid = mod.default;
        mermaid.initialize({
          startOnLoad: false,
          securityLevel: "strict",
          theme: resolvedTheme === "dark" ? "dark" : "default",
          fontFamily: "var(--font-sans)",
        });
        const result = await mermaid.render(renderId, code.trim());
        if (!cancelled) {
          setSvg(result.svg);
          setError(undefined);
        }
      })
      .catch((failure: unknown) => {
        if (!cancelled) {
          setSvg(undefined);
          setError(failure instanceof Error ? failure.message : String(failure));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [code, streaming, showSource, resolvedTheme]);

  if (streaming || showSource || error) {
    return (
      <div className="not-typeset my-2 flex flex-col gap-1.5">
        <Code
          code={code}
          language="mermaid"
          streaming={streaming}
          title={
            <div className="flex items-center gap-2">
              <span className="font-mono text-muted-foreground">mermaid</span>
              {!streaming && !error ? (
                <Button
                  type="button"
                  size="xs"
                  variant="ghost"
                  className="h-5 px-1.5 text-[11px]"
                  onClick={() => setShowSource(false)}
                >
                  <Eye className="size-3" />
                  查看图表
                </Button>
              ) : null}
            </div>
          }
        />
        {error ? <p className="px-1 text-[11px] text-muted-foreground">图表渲染失败，已回退为源码显示。</p> : null}
      </div>
    );
  }

  return (
    <div data-not-typeset className="not-typeset my-2 overflow-hidden rounded-xl border border-border/70 bg-card/50">
      <div className="flex items-center justify-between gap-2 border-b border-border/60 bg-muted/30 px-3 py-1.5 text-xs text-muted-foreground">
        <span className="font-mono">mermaid</span>
        <div className="flex items-center gap-1">
          <Button type="button" size="xs" variant="ghost" className="h-6 px-1.5 text-[11px]" onClick={() => setShowSource(true)}>
            <Code2 className="size-3" />
            源码
          </Button>
          <Button
            type="button"
            size="xs"
            variant="ghost"
            className="h-6 px-1.5 text-[11px]"
            onClick={() => {
              void navigator.clipboard
                .writeText(code)
                .then(() => {
                  setCopied(true);
                  window.setTimeout(() => setCopied(false), 1_500);
                })
                .catch(() => toast.error("复制失败"));
            }}
          >
            {copied ? <Check className="size-3" /> : <Copy className="size-3" />}
            {copied ? "已复制" : "复制"}
          </Button>
        </div>
      </div>
      <div className="overflow-x-auto p-4 [&_svg]:mx-auto [&_svg]:max-w-full">
        {svg ? (
          <div dangerouslySetInnerHTML={{ __html: svg }} />
        ) : (
          <div className="py-6 text-center text-xs text-muted-foreground">正在渲染图表…</div>
        )}
      </div>
    </div>
  );
}

function MarkdownTable({ children }: { children?: React.ReactNode }) {
  const tableRef = React.useRef<HTMLTableElement>(null);
  const [copied, setCopied] = React.useState(false);

  const copyTable = () => {
    const table = tableRef.current;
    if (!table) return;
    const rows = Array.from(table.querySelectorAll("tr")).map((tr) =>
      Array.from(tr.querySelectorAll("th, td"))
        .map((cell) => (cell.textContent ?? "").replace(/\s+/g, " ").trim())
        .join("\t"),
    );
    void navigator.clipboard
      .writeText(rows.join("\n"))
      .then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1_500);
      })
      .catch(() => toast.error("复制失败"));
  };

  return (
    <div className="group/table relative">
      <div className="typeset-scroll">
        <table ref={tableRef}>{children}</table>
      </div>
      <Button
        type="button"
        size="icon-xs"
        variant="secondary"
        aria-label="复制表格"
        title="复制表格（制表符分隔）"
        onClick={copyTable}
        className="absolute top-1 right-1 size-6 opacity-0 transition-opacity group-hover/table:opacity-100 focus-visible:opacity-100"
      >
        {copied ? <Check className="size-3" /> : <Copy className="size-3" />}
      </Button>
    </div>
  );
}

function MarkdownCode({
  className,
  children,
  streaming,
}: {
  className?: string | undefined;
  children?: React.ReactNode;
  streaming: boolean;
}) {
  const isFenced = React.useContext(FencedCodeContext);
  const language = /language-([^\s]+)/.exec(className ?? "")?.[1];
  const value = String(children);
  if (!isFenced && !language && !value.includes("\n")) return <code>{children}</code>;
  if (language?.toLowerCase() === "mermaid") {
    return <MermaidBlock code={value} streaming={streaming} />;
  }
  return (
    <Code
      code={value}
      {...(language ? { language, title: <span className="font-mono text-muted-foreground">{language}</span> } : {})}
      streaming={streaming}
    />
  );
}

function components(streaming: boolean): Components {
  return {
    pre: ({ children }) => <FencedCodeContext.Provider value={true}>{children}</FencedCodeContext.Provider>,
    code: ({ className, children }) => <MarkdownCode className={className} streaming={streaming}>{children}</MarkdownCode>,
    a: ({ href, children }) => <MarkdownLink href={href}>{children}</MarkdownLink>,
    img: ({ src, alt, title }) => <MarkdownImage src={src} alt={alt} title={title} />,
    table: ({ children }) => <MarkdownTable>{children}</MarkdownTable>,
  };
}

const streamingComponents = components(true);
const settledComponents = components(false);
const remarkPlugins = [remarkGfm, remarkMath];
const rehypePlugins = [[rehypeKatex, { strict: false, throwOnError: false }] as [typeof rehypeKatex, { strict: boolean; throwOnError: boolean }]];

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
    <ReactMarkdown
      remarkPlugins={remarkPlugins}
      rehypePlugins={rehypePlugins}
      urlTransform={markdownUrlTransform}
      components={streaming ? streamingComponents : settledComponents}
    >
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
