import * as React from "react";
import { ArrowUp, Clock3, FileText, Paperclip, Puzzle, Sparkles, Square, SquareSlash, X } from "lucide-react";
import { toast } from "sonner";
import {
  MAX_PROMPT_IMAGE_BASE64_BYTES,
  MAX_PROMPT_IMAGE_BYTES,
  MAX_PROMPT_IMAGES,
  promptImageBytes,
  type SlashCommand,
} from "@lila/protocol";
import { Attachment, AttachmentAction, AttachmentActions, AttachmentGroup, AttachmentMedia } from "@/components/ui/attachment";
import { Command, CommandEmpty, CommandGroup, CommandItem, CommandList } from "@/components/ui/command";
import { Kbd } from "@/components/ui/kbd";
import { Spinner } from "@/components/ui/spinner";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupTextarea } from "@/components/ui/input-group";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "cn";
import { actions, type DraftImage, useLila } from "@/lib/store";

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
  if (slash) {
    const slashMatch = /^\/([^\s]*)$/.exec(before);
    if (slashMatch) return { kind: "slash", start: 0, query: slashMatch[1] ?? "" };
  }
  if (files) {
    // Allow `@` at start, after whitespace, or after CJK/punctuation, while avoiding email addresses like user@domain.
    const atMatch = /(^|[^A-Za-z0-9_.-])@([^\s]*)$/.exec(before);
    if (atMatch) {
      const query = atMatch[2] ?? "";
      return { kind: "file", start: before.length - query.length - 1, query };
    }
  }
  return null;
}

export function focusComposer(): void {
  const focus = () => document.querySelector<HTMLTextAreaElement>("textarea[data-composer]")?.focus();
  if (document.querySelector("[data-workspace-sheet][data-open]")) {
    actions.closeRightPanel();
    requestAnimationFrame(focus);
  } else {
    focus();
  }
}

export function Composer({
  draftKey,
  sessionId,
  projectId,
  running = false,
  disabled = false,
  sendDisabled = false,
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
  sendDisabled?: boolean;
  abortArmed?: boolean;
  placeholder: string;
  /** Controls at the start of the bottom bar (model, thinking, approval). */
  footer?: React.ReactNode;
  /** Shown next to the send button, for example the context ring. */
  trailing?: React.ReactNode;
  onSubmit: (mode?: "steer" | "followUp") => Promise<void>;
  onAbort?: (() => void) | undefined;
}) {
  const text = useLila((state) => state.drafts[draftKey] ?? "");
  const images = useLila((state) => state.draftImages[draftKey] ?? EMPTY_IMAGES);
  const connected = useLila((state) => state.connection === "connected");
  const sending = useLila((state) => Boolean(state.sendingDrafts[draftKey]));
  const commandsRefreshSeq = useLila((state) => state.commandsRefreshSeq);
  const [dragging, setDragging] = React.useState(false);
  const [suggest, setSuggest] = React.useState<Suggest | null>(null);
  const [highlight, setHighlight] = React.useState(0);
  const [commands, setCommands] = React.useState<SlashCommand[] | null>(null);
  const [files, setFiles] = React.useState<{ query: string; files: string[] } | null>(null);
  const ref = React.useRef<HTMLTextAreaElement>(null);
  const fileInput = React.useRef<HTMLInputElement>(null);
  const listRef = React.useRef<HTMLDivElement>(null);
  const composingRef = React.useRef(false);

  React.useEffect(() => {
    ref.current?.focus();
    setCommands(null);
    setFiles(null);
    setSuggest(null);
  }, [draftKey, sessionId, projectId]);

  React.useEffect(() => {
    setCommands(null);
  }, [commandsRefreshSeq]);

  // Slash commands are fetched when the menu first opens for this session or project.
  React.useEffect(() => {
    if (suggest?.kind !== "slash" || !projectId || commands !== null) return;
    let cancelled = false;
    void actions.commands(projectId, sessionId).then((list) => {
      if (!cancelled && list !== undefined) {
        setCommands(sessionId ? list : list.filter((command) => command.source !== "builtin"));
      }
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

  const filesStale = suggest?.kind === "file" && (files === null || files.query !== suggest.query);
  const suggestLoading = (suggest?.kind === "slash" && commands === null) || filesStale;

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
    if (filesStale) return { groups: [], flatOptions: [] };
    const fileItems: Option[] = (files?.files ?? []).map((path) => ({
      value: path,
      label: path,
      group: "file",
      insert: `@${path} `,
    }));
    const ordered: Array<[string, Option[]]> = fileItems.length ? [["file", fileItems]] : [];
    return { groups: ordered, flatOptions: fileItems };
  }, [suggest, commands, files, filesStale]);

  React.useEffect(() => setHighlight(0), [flatOptions]);

  const menuOpen = suggest !== null;

  React.useEffect(() => {
    if (!menuOpen || !flatOptions.length) return;
    const item = listRef.current?.querySelector<HTMLElement>(`[data-option-index="${highlight}"]`);
    item?.scrollIntoView({ block: "nearest" });
  }, [highlight, menuOpen, flatOptions]);

  const hasText = text.trim().length > 0;
  const canSend = connected && !disabled && !sendDisabled && !sending && (running ? hasText : hasText || images.length > 0);

  function syncSuggest(value: string, caret: number): void {
    if (composingRef.current) return;
    setSuggest(findSuggest(value, caret, Boolean(projectId), Boolean(projectId)));
  }

  function choose(option: Option): void {
    const textarea = ref.current;
    if (!textarea || !suggest) return;
    const caret = textarea.selectionStart;
    // Extend end to the end of the current non-whitespace token so editing mid-token replaces cleanly.
    let end = caret;
    while (end < text.length && !/\s/.test(text[end]!)) end += 1;
    const next = text.slice(0, suggest.start) + option.insert + text.slice(end);
    actions.setDraft(draftKey, next);
    setSuggest(null);
    const position = suggest.start + option.insert.length;
    requestAnimationFrame(() => {
      textarea.focus();
      textarea.setSelectionRange(position, position);
    });
  }

  function imageLimit(usedCount: number, usedBytes: number, size: number): string | undefined {
    if (usedCount >= MAX_PROMPT_IMAGES) return `单条消息最多添加 ${MAX_PROMPT_IMAGES} 张图片`;
    const remaining = MAX_PROMPT_IMAGE_BASE64_BYTES - usedBytes;
    if (size > remaining) {
      const remainingMiB = Math.max(0, Math.floor(remaining * 0.75 / (1024 * 1024) * 10) / 10);
      return `图片超出单条消息的附件上限，当前还可添加约 ${remainingMiB} MB`;
    }
    return undefined;
  }

  async function addFiles(list: FileList | File[]): Promise<void> {
    const draftTarget = actions.captureDraftTarget(draftKey);
    const incoming = Array.from(list);
    const added: DraftImage[] = [];
    const blocks: string[] = [];
    const existingImages = () => {
      const key = draftTarget();
      const state = useLila.getState();
      return [...(state.sendingDrafts[key]?.images ?? []), ...(state.draftImages[key] ?? [])];
    };
    for (const file of incoming) {
      try {
        if (file.type.startsWith("image/")) {
          if (file.size > MAX_PROMPT_IMAGE_BYTES) {
            toast.warning(`${file.name} 超过 10 MB，没有添加`);
            continue;
          }
          const existing = existingImages();
          const limit = imageLimit(existing.length + added.length, promptImageBytes(existing) + promptImageBytes(added), Math.ceil(file.size / 3) * 4);
          if (limit) {
            toast.warning(`${file.name || "图片"}：${limit}`);
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
    const target = draftTarget();
    if (added.length) {
      actions.setDraftImages(target, (current) => {
        const pending = useLila.getState().sendingDrafts[target]?.images ?? [];
        const next = [...current];
        let usedBytes = promptImageBytes(pending) + promptImageBytes(current);
        for (const image of added) {
          const limit = imageLimit(pending.length + next.length, usedBytes, image.data.length);
          if (limit) {
            toast.warning(`${image.name}：${limit}`);
            continue;
          }
          next.push(image);
          usedBytes += image.data.length;
        }
        return next;
      });
    }
    if (blocks.length) {
      const current = useLila.getState().drafts[target] ?? "";
      actions.setDraft(target, [current.trimEnd(), ...blocks].filter(Boolean).join("\n\n") + "\n");
    }
    ref.current?.focus();
  }

  async function submit(mode?: "steer" | "followUp"): Promise<void> {
    if (!canSend) return;
    setSuggest(null);
    await onSubmit(running ? (mode ?? "steer") : undefined);
  }

  function onKeyDown(event: React.KeyboardEvent<HTMLTextAreaElement>): void {
    if (composingRef.current || event.nativeEvent.isComposing || event.keyCode === 229) return;
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
      } else if (suggestLoading && (event.key === "Tab" || (event.key === "Enter" && !event.shiftKey))) {
        // Avoid sending a half-typed `@query` while debounced file search is still loading.
        event.preventDefault();
        return;
      }
    }
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      void submit(running && event.altKey ? "followUp" : "steer");
    }
  }

  let optionIndex = 0;

  return (
    <InputGroup
      className={cn(
        "rounded-[22px] border-border bg-card shadow-[0_2px_12px_-3px_rgb(0_0_0/0.07)] transition-[border-color,box-shadow] duration-150 ease-(--ease-ui-out) has-disabled:bg-card has-disabled:opacity-100 has-[[data-slot=input-group-control]:focus-visible]:border-foreground/20 has-[[data-slot=input-group-control]:focus-visible]:ring-2 has-[[data-slot=input-group-control]:focus-visible]:ring-ring/15 dark:bg-card dark:shadow-[0_4px_18px_-4px_rgb(0_0_0/0.4)] dark:has-disabled:bg-card",
        dragging && "border-primary/60 ring-2 ring-primary/20",
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
      {dragging ? (
        <div
          aria-hidden
          className="surface-enter pointer-events-none absolute inset-0 z-20 flex items-center justify-center gap-2 rounded-[22px] bg-card/90 text-sm font-medium text-foreground backdrop-blur-xs"
        >
          <Paperclip className="size-4 text-muted-foreground" />
          松开以添加图片或文件
        </div>
      ) : null}
      {images.length ? (
        <InputGroupAddon align="block-start" className="px-3.5 pt-3 pb-0">
          <AttachmentGroup>
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
        </InputGroupAddon>
      ) : null}
      <InputGroupTextarea
        ref={ref}
        data-composer
        aria-label="消息"
        value={text}
        disabled={disabled}
        placeholder={placeholder}
        onCompositionStart={() => {
          composingRef.current = true;
        }}
        onCompositionEnd={(event) => {
          composingRef.current = false;
          syncSuggest(event.currentTarget.value, event.currentTarget.selectionStart);
        }}
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
        className="max-h-72 min-h-12 w-full flex-none px-4 pt-3.5 pb-2 text-[14.5px] leading-relaxed"
      />
      <InputGroupAddon align="block-end" className="flex-wrap gap-1 px-2.5 pt-1 pb-2">
        <Tooltip>
          <TooltipTrigger
            render={
              <InputGroupButton
                type="button"
                size="icon-sm"
                variant="ghost"
                className="shrink-0 text-muted-foreground"
                disabled={disabled}
                aria-label="添加图片或文件"
                onClick={() => fileInput.current?.click()}
              />
            }
          >
            <Paperclip />
          </TooltipTrigger>
          <TooltipContent>添加图片或文件</TooltipContent>
        </Tooltip>
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
          {abortArmed ? <span className="surface-enter shrink-0 px-1 text-xs text-warning-foreground">再按一次 Esc 停止</span> : null}
          {running ? (
            <Tooltip>
              <TooltipTrigger
                render={
                  <InputGroupButton
                    size="xs"
                    variant="ghost"
                    aria-keyshortcuts="Alt+Enter"
                    disabled={!canSend}
                    onClick={() => void submit("followUp")}
                  />
                }
              >
                <Clock3 />
                排队
              </TooltipTrigger>
              <TooltipContent>
                当前 Run 结束后发送
                <Kbd>⌥↵</Kbd>
              </TooltipContent>
            </Tooltip>
          ) : null}
          {trailing}
          {onAbort ? (
            <Tooltip>
              <TooltipTrigger
                render={
                  <InputGroupButton
                    type="button"
                    size="icon-sm"
                    variant={hasText ? "secondary" : "default"}
                    className="rounded-full"
                    aria-label="停止"
                    onClick={onAbort}
                  />
                }
              >
                <Square className="size-3 fill-current" />
              </TooltipTrigger>
              <TooltipContent>
                停止
                <Kbd>Esc×2</Kbd>
              </TooltipContent>
            </Tooltip>
          ) : null}
          {!running || hasText || images.length > 0 ? (
            <Tooltip>
              <TooltipTrigger
                render={
                  <InputGroupButton
                    type="button"
                    size="icon-sm"
                    variant="default"
                    className="rounded-full"
                    disabled={!canSend}
                    aria-label={running ? "插话" : "发送"}
                    onClick={() => void submit("steer")}
                  />
                }
              >
                {sending ? <Spinner /> : <ArrowUp />}
              </TooltipTrigger>
              <TooltipContent>
                {sendDisabled ? (
                  "会话正在准备中…"
                ) : running ? (
                  hasText ? (
                    <>
                      插话（当前工具结束后送达）
                      <Kbd>↵</Kbd>
                    </>
                  ) : (
                    "运行中插话请先输入文字"
                  )
                ) : (
                  <>
                    发送
                    <Kbd>↵</Kbd>
                  </>
                )}
              </TooltipContent>
            </Tooltip>
          ) : null}
        </div>
      </InputGroupAddon>
    </InputGroup>
  );
}
