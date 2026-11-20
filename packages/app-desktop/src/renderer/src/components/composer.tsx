import * as React from "react";
import { ArrowUp, FileText, Paperclip, Puzzle, Sparkles, Square, SquareSlash, X } from "lucide-react";
import { toast } from "sonner";
import type { ImageInput, SlashCommand } from "@lila/protocol";
import { Attachment, AttachmentAction, AttachmentActions, AttachmentGroup, AttachmentMedia } from "@/components/ui/attachment";
import { Button } from "@/components/ui/button";
import { Command, CommandEmpty, CommandGroup, CommandItem, CommandList } from "@/components/ui/command";
import { Kbd } from "@/components/ui/kbd";
import { Spinner } from "@/components/ui/spinner";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "cn";
import { actions, type DraftImage, useLila } from "@/lib/store";

const MAX_IMAGES = 10;
const MAX_IMAGE_BYTES = 10 * 1024 * 1024;
const MAX_TEXT_BYTES = 200 * 1024;
const EMPTY_IMAGES: DraftImage[] = [];
const TEXT_EXTENSIONS =
  /\.(txt|md|markdown|json|jsonc|ya?ml|toml|ini|env|csv|tsv|log|xml|html?|css|scss|less|[cm]?[jt]sx?|py|rb|go|rs|java|kt|swift|c|cc|cpp|h|hpp|cs|php|sh|bash|zsh|fish|sql|graphql|proto|vue|svelte|lua|dart|ex|exs|r|diff|patch)$/i;

type Suggest = { kind: "slash" | "file"; start: number; query: string };
type Option = { value: string; label: string; description?: string; group: string; insert: string };

const SLASH_GROUP_ORDER: SlashCommand["source"][] = ["builtin", "skill", "prompt", "extension"];

const SOURCE_LABEL: Record<SlashCommand["source"], string> = {
  builtin: "内置",
  extension: "扩展命令",
  prompt: "提示模板",
  skill: "技能",
};
const SOURCE_ICON: Record<SlashCommand["source"], React.ComponentType> = {
  builtin: SquareSlash,
  extension: Puzzle,
  prompt: FileText,
  skill: Sparkles,
};

function readAs(file: File, mode: "dataURL" | "text"): Promise<string> {
  // Promise.withResolvers is not in the ES2023 lib this package targets.
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error ?? new Error(`无法读取 ${file.name}`));
    if (mode === "dataURL") reader.readAsDataURL(file);
    else reader.readAsText(file);
  });
}

function isTextFile(file: File): boolean {
  return file.type.startsWith("text/") || /json|xml|javascript|typescript|yaml|toml|x-sh/.test(file.type) || TEXT_EXTENSIONS.test(file.name);
}

function fenced(name: string, content: string): string {
  let fence = "```";
  while (content.includes(fence)) fence += "`";
  return `${fence}${name}\n${content.replace(/\n$/, "")}\n${fence}`;
}

/** The `/command` or `@path` token right before the caret, if any. */
function findSuggest(text: string, caret: number, slash: boolean, files: boolean): Suggest | null {
  const before = text.slice(0, caret);
  const match = /(^|\s)([/@])([^\s]*)$/.exec(before);
  if (!match) return null;
  const start = before.length - (match[3]?.length ?? 0) - 1;
  if (match[2] === "/" && slash && start === 0) return { kind: "slash", start, query: match[3] ?? "" };
  if (match[2] === "@" && files) return { kind: "file", start, query: match[3] ?? "" };
  return null;
}

export function focusComposer(): void {
  document.querySelector<HTMLTextAreaElement>("textarea[data-composer]")?.focus();
}

export function Composer({
  draftKey,
  sessionId,
  projectId,
  running = false,
  disabled = false,
  abortArmed = false,
  placeholder,
  footer,
  trailing,
  onSubmit,
  onAbort,
}: {
  draftKey: string;
  /** The session's slash commands; without it the project's (minus built-ins that need a session). */
  sessionId?: string | undefined;
  /** Enables slash commands and `@` file search. */
  projectId?: string | undefined;
  running?: boolean;
  disabled?: boolean;
  abortArmed?: boolean;
  placeholder: string;
  /** Controls at the start of the bottom bar (model, thinking, approval). */
  footer?: React.ReactNode;
  /** Shown next to the send button, for example the context ring. */
  trailing?: React.ReactNode;
  onSubmit: (text: string, images: ImageInput[], mode?: "steer" | "followUp") => Promise<boolean>;
  onAbort?: () => void;
}) {
  const text = useLila((state) => state.drafts[draftKey] ?? "");
  const images = useLila((state) => state.draftImages[draftKey] ?? EMPTY_IMAGES);
  const connected = useLila((state) => state.connection === "connected");
  const [sending, setSending] = React.useState(false);
  const [dragging, setDragging] = React.useState(false);
  const [suggest, setSuggest] = React.useState<Suggest | null>(null);
  const [highlight, setHighlight] = React.useState(0);
  const [commands, setCommands] = React.useState<SlashCommand[] | null>(null);
  const [files, setFiles] = React.useState<{ query: string; files: string[] } | null>(null);
  const ref = React.useRef<HTMLTextAreaElement>(null);
  const fileInput = React.useRef<HTMLInputElement>(null);
  const listRef = React.useRef<HTMLDivElement>(null);

  React.useEffect(() => {
    ref.current?.focus();
    setCommands(null);
    setFiles(null);
    setSuggest(null);
  }, [draftKey, sessionId, projectId]);

  // Slash commands are fetched when the menu first opens for this session or project.
  React.useEffect(() => {
    if (suggest?.kind !== "slash" || !projectId || commands) return;
    let cancelled = false;
    void actions.commands(projectId, sessionId).then((list) => {
      if (!cancelled) setCommands(sessionId ? list : list.filter((command) => command.source !== "builtin"));
    });
    return () => {
      cancelled = true;
    };
  }, [suggest?.kind, sessionId, projectId, commands]);

  // File search is debounced while typing after `@`.
  const fileQuery = suggest?.kind === "file" ? suggest.query : undefined;
  React.useEffect(() => {
    if (fileQuery === undefined || !projectId) return;
    let cancelled = false;
    const timer = setTimeout(() => {
      void actions.searchFiles(projectId, fileQuery).then((found) => {
        if (!cancelled) setFiles({ query: fileQuery, files: found });
      });
    }, 150);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [fileQuery, projectId]);

  // Group in fixed display order first, then flatten so keyboard navigation matches visual order 1:1.
  const { groups, flatOptions } = React.useMemo<{ groups: Array<[string, Option[]]>; flatOptions: Option[] }>(() => {
    if (!suggest) return { groups: [], flatOptions: [] };
    const query = suggest.query.toLowerCase();
    if (suggest.kind === "slash") {
      const buckets = new Map<SlashCommand["source"], Option[]>();
      for (const source of SLASH_GROUP_ORDER) buckets.set(source, []);
      const filtered = (commands ?? [])
        .filter((command) => command.name.toLowerCase().includes(query))
        .sort((a, b) => Number(!a.name.toLowerCase().startsWith(query)) - Number(!b.name.toLowerCase().startsWith(query)));
      for (const command of filtered) {
        const list = buckets.get(command.source);
        if (!list) continue;
        list.push({
          value: `${command.source}:${command.name}`,
          label: `/${command.name}`,
          description: command.description,
          group: command.source,
          insert: `/${command.name} `,
        });
      }
      const ordered: Array<[string, Option[]]> = [];
      for (const source of SLASH_GROUP_ORDER) {
        const items = buckets.get(source);
        if (items?.length) ordered.push([source, items]);
      }
      return { groups: ordered, flatOptions: ordered.flatMap(([, items]) => items) };
    }
    const fileItems: Option[] = (files?.files ?? []).map((path) => ({
      value: path,
      label: path,
      group: "file",
      insert: `@${path} `,
    }));
    const ordered: Array<[string, Option[]]> = fileItems.length ? [["file", fileItems]] : [];
    return { groups: ordered, flatOptions: fileItems };
  }, [suggest, commands, files]);

  React.useEffect(() => setHighlight(0), [flatOptions]);

  const menuOpen = suggest !== null;

  React.useEffect(() => {
    if (!menuOpen || !flatOptions.length) return;
    const item = listRef.current?.querySelector<HTMLElement>(`[data-option-index="${highlight}"]`);
    item?.scrollIntoView({ block: "nearest" });
  }, [highlight, menuOpen, flatOptions]);

  const hasText = text.trim().length > 0;
  const canSend = connected && !disabled && !sending && (running ? hasText : hasText || images.length > 0);

  function syncSuggest(value: string, caret: number): void {
    setSuggest(findSuggest(value, caret, Boolean(projectId), Boolean(projectId)));
  }

  function choose(option: Option): void {
    const textarea = ref.current;
    if (!textarea || !suggest) return;
    const caret = textarea.selectionStart;
    const next = text.slice(0, suggest.start) + option.insert + text.slice(caret);
    actions.setDraft(draftKey, next);
    setSuggest(null);
    const position = suggest.start + option.insert.length;
    requestAnimationFrame(() => {
      textarea.focus();
      textarea.setSelectionRange(position, position);
    });
  }

  async function addFiles(list: FileList | File[]): Promise<void> {
    const incoming = Array.from(list);
    const added: DraftImage[] = [];
    const blocks: string[] = [];
    for (const file of incoming) {
      try {
        if (file.type.startsWith("image/")) {
          if (file.size > MAX_IMAGE_BYTES) {
            toast.warning(`${file.name} 超过 10 MB，没有添加`);
            continue;
          }
          const url = await readAs(file, "dataURL");
          added.push({ id: crypto.randomUUID(), name: file.name || "图片", mimeType: file.type, data: url.slice(url.indexOf(",") + 1) });
        } else if (isTextFile(file)) {
          if (file.size > MAX_TEXT_BYTES) {
            toast.warning(`${file.name} 超过 200 KB，没有添加`);
            continue;
          }
          const content = await readAs(file, "text");
          if (content.includes("\u0000")) {
            toast.warning(`${file.name} 不是文本文件`);
            continue;
          }
          blocks.push(fenced(file.name, content));
        } else {
          toast.warning(`暂不支持 ${file.name}，只能添加图片或文本文件`);
        }
      } catch (error) {
        toast.error(error instanceof Error ? error.message : String(error));
      }
    }
    if (added.length) {
      actions.setDraftImages(draftKey, (current) => {
        const room = MAX_IMAGES - current.length;
        if (added.length > room) toast.warning(`最多添加 ${MAX_IMAGES} 张图片`);
        return [...current, ...added.slice(0, Math.max(room, 0))];
      });
    }
    if (blocks.length) {
      const current = useLila.getState().drafts[draftKey] ?? "";
      actions.setDraft(draftKey, [current.trimEnd(), ...blocks].filter(Boolean).join("\n\n") + "\n");
    }
    ref.current?.focus();
  }

  async function submit(mode?: "steer" | "followUp"): Promise<void> {
    if (!canSend) return;
    setSending(true);
    const ok = await onSubmit(text.trim(), images.map(({ mimeType, data }) => ({ mimeType, data })), running ? (mode ?? "steer") : undefined);
    setSending(false);
    if (ok) {
      actions.setDraft(draftKey, "");
      actions.setDraftImages(draftKey, []);
      setSuggest(null);
    }
  }

  function onKeyDown(event: React.KeyboardEvent<HTMLTextAreaElement>): void {
    if (event.nativeEvent.isComposing) return;
    if (menuOpen) {
      if (event.key === "Escape") {
        event.preventDefault();
        setSuggest(null);
        return;
      }
      if (flatOptions.length) {
        if (event.key === "ArrowDown" || event.key === "ArrowUp") {
          event.preventDefault();
          const step = event.key === "ArrowDown" ? 1 : -1;
          setHighlight((current) => (current + step + flatOptions.length) % flatOptions.length);
          return;
        }
        if (event.key === "Tab" || (event.key === "Enter" && !event.shiftKey)) {
          event.preventDefault();
          const option = flatOptions[highlight];
          if (option) choose(option);
          return;
        }
      }
    }
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      void submit(event.altKey ? "followUp" : "steer");
    }
  }

  let optionIndex = 0;

  return (
    <div
      className={cn(
        "relative rounded-2xl border bg-background shadow-xs transition-colors focus-within:border-ring",
        dragging && "border-primary ring-3 ring-primary/20",
      )}
      onDragOver={(event) => {
        if (!event.dataTransfer.types.includes("Files")) return;
        event.preventDefault();
        setDragging(true);
      }}
      onDragLeave={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDragging(false);
      }}
      onDrop={(event) => {
        if (!event.dataTransfer.files.length) return;
        event.preventDefault();
        setDragging(false);
        void addFiles(event.dataTransfer.files);
      }}
    >
      {menuOpen ? (
        <div data-composer-suggest="" className="absolute inset-x-0 bottom-full z-30 mb-2" onMouseDown={(event) => event.preventDefault()}>
          <Command
            shouldFilter={false}
            value={flatOptions[highlight]?.value ?? ""}
            onValueChange={(value) => {
              const found = flatOptions.findIndex((option) => option.value === value);
              if (found >= 0) setHighlight(found);
            }}
            className="h-auto rounded-xl shadow-lg ring-1 ring-foreground/10"
          >
            <CommandList ref={listRef} className="max-h-72">
              <CommandEmpty className="py-4">
                {suggest.kind === "slash"
                  ? commands === null
                    ? "正在读取命令…"
                    : "没有匹配的命令"
                  : files === null || files.query !== suggest.query
                    ? "正在搜索文件…"
                    : "没有匹配的文件"}
              </CommandEmpty>
              {groups.map(([group, items]) => (
                <CommandGroup key={group} heading={group === "file" ? "文件" : SOURCE_LABEL[group as SlashCommand["source"]]}>
                  {items.map((option) => {
                    const Icon = option.group === "file" ? FileText : SOURCE_ICON[option.group as SlashCommand["source"]];
                    const index = optionIndex++;
                    return (
                      <CommandItem
                        key={option.value}
                        value={option.value}
                        data-option-index={index}
                        onSelect={() => choose(option)}
                      >
                        <Icon />
                        <span className="shrink-0 font-mono text-xs">{option.label}</span>
                        {option.description ? <span className="min-w-0 truncate text-xs text-muted-foreground">{option.description}</span> : null}
                      </CommandItem>
                    );
                  })}
                </CommandGroup>
              ))}
            </CommandList>
          </Command>
        </div>
      ) : null}
      {images.length ? (
        <AttachmentGroup className="px-3 pt-3">
          {images.map((image) => (
            <Attachment key={image.id} size="xs" orientation="vertical" className="w-16" title={image.name}>
              <AttachmentMedia variant="image">
                <img src={`data:${image.mimeType};base64,${image.data}`} alt={image.name} />
              </AttachmentMedia>
              <AttachmentActions className="top-1! right-1!">
                <AttachmentAction
                  variant="secondary"
                  aria-label={`移除 ${image.name}`}
                  onClick={() => actions.setDraftImages(draftKey, (current) => current.filter((item) => item.id !== image.id))}
                >
                  <X />
                </AttachmentAction>
              </AttachmentActions>
            </Attachment>
          ))}
        </AttachmentGroup>
      ) : null}
      <Textarea
        ref={ref}
        data-composer
        value={text}
        disabled={disabled}
        placeholder={placeholder}
        onChange={(event) => {
          actions.setDraft(draftKey, event.target.value);
          syncSuggest(event.target.value, event.target.selectionStart);
        }}
        onSelect={(event) => syncSuggest(event.currentTarget.value, event.currentTarget.selectionStart)}
        onBlur={() => setSuggest(null)}
        onKeyDown={onKeyDown}
        onPaste={(event) => {
          const pasted = Array.from(event.clipboardData.files);
          if (!pasted.length) return;
          event.preventDefault();
          void addFiles(pasted);
        }}
        className="max-h-72 min-h-16 resize-none border-0 bg-transparent px-4 pt-3 text-sm shadow-none focus-visible:ring-0 dark:bg-transparent"
      />
      <div className="flex flex-wrap items-center gap-1 px-2 pb-2">
        <Button
          type="button"
          size="icon-sm"
          variant="ghost"
          className="shrink-0 text-muted-foreground"
          disabled={disabled}
          aria-label="添加图片或文件"
          title="添加图片或文件"
          onClick={() => fileInput.current?.click()}
        >
          <Paperclip />
        </Button>
        <input
          ref={fileInput}
          type="file"
          multiple
          hidden
          onChange={(event) => {
            if (event.target.files) void addFiles(event.target.files);
            event.target.value = "";
          }}
        />
        <div className="flex min-w-0 flex-1 flex-wrap items-center gap-0.5">{footer}</div>
        <div className="ms-auto flex shrink-0 items-center gap-1">
          {abortArmed ? (
            <span className="shrink-0 px-1 text-xs text-amber-600 dark:text-amber-400">
              再按一次 <Kbd>Esc</Kbd> 停止
            </span>
          ) : running ? (
            <span className="hidden shrink-0 px-1 text-xs text-muted-foreground md:inline">
              <Kbd>↵</Kbd> 插话 · <Kbd>⌥↵</Kbd> 排队
            </span>
          ) : null}
          {trailing}
          {running && onAbort ? (
            <Button
              type="button"
              size="icon-sm"
              variant={hasText ? "secondary" : "default"}
              className="rounded-full"
              aria-label="停止"
              title="停止（双击 Esc）"
              onClick={onAbort}
            >
              <Square className="size-3 fill-current" />
            </Button>
          ) : null}
          {!running || hasText || images.length > 0 ? (
            <Button
              type="button"
              size="icon-sm"
              className="rounded-full"
              disabled={!canSend}
              aria-label={running ? "插话" : "发送"}
              title={
                running
                  ? hasText
                    ? "插话：当前工具结束后送达（↵），⌥↵ 排到这一轮之后"
                    : "运行中插话请先输入文字"
                  : "发送（↵），⇧↵ 换行"
              }
              onClick={() => void submit("steer")}
            >
              {sending ? <Spinner /> : <ArrowUp />}
            </Button>
          ) : null}
        </div>
      </div>
    </div>
  );
}
