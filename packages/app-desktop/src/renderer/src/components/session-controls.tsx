import * as React from "react";
import { Radio } from "@base-ui/react/radio";
import { RadioGroup } from "@base-ui/react/radio-group";
import {
  BarChart3,
  Check,
  ChevronDown,
  KeyRound,
  Minimize2,
  RotateCw,
  Settings2,
  ShieldAlert,
  ShieldCheck,
  ShieldQuestion,
  SlidersHorizontal,
  X,
} from "lucide-react";
import type { ApprovalMode, ModelInfo, SessionState, ThinkingLevel } from "@lila/protocol";
import { SearchInput } from "@/components/search-input";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTitle, PopoverTrigger } from "@/components/ui/popover";
import { Progress } from "@/components/ui/progress";
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";
import { actions, APPROVAL_LABEL, useLila } from "@/lib/store";

export function ModelSetup() {
  const connected = useLila((state) => state.connection === "connected");
  const loading = useLila((state) => state.modelsLoading);
  const error = useLila((state) => state.modelsError);
  const hasModels = useLila((state) => state.models.length > 0);
  const status = loading ? "正在读取模型…" : error ? "读取模型失败" : hasModels ? "请选择模型" : "尚未配置模型";
  return (
    <div className="flex flex-wrap items-center justify-between gap-2 rounded-xl border bg-muted/30 px-3 py-2 text-sm">
      <p role="status" className="min-w-0 flex-1 text-muted-foreground">
        {status}
      </p>
      <div className="flex items-center gap-1.5">
        {!hasModels ? (
          <Button type="button" size="sm" disabled={!connected} onClick={() => actions.openSettings("providers")}>
            <KeyRound />
            配置模型
          </Button>
        ) : null}
        <Button type="button" size="sm" variant="outline" disabled={!connected || loading} onClick={() => void actions.retryModels()}>
          <RotateCw />
          刷新
        </Button>
      </div>
    </div>
  );
}

/** The quiet chip look shared by every control in the composer's bottom bar. */
export const COMPOSER_CHIP =
  "h-7 gap-1 rounded-md border-transparent bg-transparent px-2 text-xs font-normal text-muted-foreground shadow-none hover:bg-muted hover:text-foreground dark:bg-transparent dark:hover:bg-muted aria-expanded:bg-muted aria-expanded:text-foreground [&_svg]:size-3.5 [&>svg:last-child]:size-3 [&>svg:last-child]:opacity-60";

/** Provider-grouped Model Selector with instant selection and live reasoning-effort control. */
export function ModelPicker({
  models,
  modelId,
  thinkingLevel,
  currentLevels,
  onApply,
}: {
  models: ModelInfo[];
  modelId: string | undefined;
  thinkingLevel: ThinkingLevel | undefined;
  currentLevels?: ThinkingLevel[] | undefined;
  onApply: (modelId: string, thinkingLevel: ThinkingLevel) => boolean | Promise<boolean>;
}) {
  const [open, setOpen] = React.useState(false);
  const [query, setQuery] = React.useState("");
  const [draftEffort, setDraftEffort] = React.useState<ThinkingLevel | undefined>(thinkingLevel);
  const [saving, setSaving] = React.useState(false);

  React.useEffect(() => {
    setDraftEffort(thinkingLevel);
  }, [thinkingLevel, modelId]);

  const visibleModels = React.useMemo(() => {
    const enabled = models.filter((item) => item.enabled || item.id === modelId);
    return enabled.length ? enabled : models;
  }, [models, modelId]);

  const filteredModels = React.useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return visibleModels;
    return visibleModels.filter(
      (model) =>
        model.label.toLowerCase().includes(needle) ||
        model.id.toLowerCase().includes(needle) ||
        model.providerName.toLowerCase().includes(needle),
    );
  }, [visibleModels, query]);

  const groups = React.useMemo(() => {
    const map = new Map<string, { providerName: string; items: ModelInfo[] }>();
    for (const model of filteredModels) {
      let group = map.get(model.provider);
      if (!group) {
        group = { providerName: model.providerName || model.provider, items: [] };
        map.set(model.provider, group);
      }
      group.items.push(model);
    }
    return [...map.entries()];
  }, [filteredModels]);

  const current = models.find((model) => model.id === modelId);
  const levels = currentLevels ?? current?.thinkingLevels ?? [];
  const currentEffort = draftEffort && levels.includes(draftEffort) ? draftEffort : current?.defaultThinkingLevel ?? levels[0];
  const showSearch = visibleModels.length > 8;

  async function selectModel(nextId: string): Promise<void> {
    if (!nextId || saving) return;
    if (nextId === modelId) {
      setOpen(false);
      return;
    }
    const target = models.find((item) => item.id === nextId);
    const targetLevels = target?.thinkingLevels ?? [];
    const nextEffort =
      thinkingLevel && targetLevels.includes(thinkingLevel)
        ? thinkingLevel
        : target?.defaultThinkingLevel ?? targetLevels[0] ?? "off";
    setSaving(true);
    try {
      if (await onApply(nextId, nextEffort)) setOpen(false);
    } finally {
      setSaving(false);
    }
  }

  async function selectEffort(nextEffort: ThinkingLevel): Promise<void> {
    const targetId = modelId ?? visibleModels[0]?.id;
    if (!targetId || saving || nextEffort === thinkingLevel) return;
    const prev = draftEffort;
    setDraftEffort(nextEffort);
    setSaving(true);
    try {
      const ok = await onApply(targetId, nextEffort);
      if (!ok) setDraftEffort(prev);
    } finally {
      setSaving(false);
    }
  }

  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (next) {
          setQuery("");
          setDraftEffort(thinkingLevel);
        }
      }}
    >
      <PopoverTrigger
        render={<Button type="button" variant="ghost" size="sm" className={cn(COMPOSER_CHIP, "max-w-full sm:max-w-[19rem]")} />}
        disabled={!modelId && !visibleModels.length}
        aria-label="模型与推理等级"
      >
        <span className="truncate">{current?.label ?? modelId ?? (visibleModels.length ? "选择模型" : "无可用模型")}</span>
        {currentEffort && currentEffort !== "off" ? (
          <span className="shrink-0 text-muted-foreground">· {currentEffort}</span>
        ) : null}
        <ChevronDown className="transition-transform duration-150 ease-(--ease-ui-out) motion-reduce:transition-none group-aria-expanded/button:rotate-180" />
      </PopoverTrigger>
      <PopoverContent side="top" align="start" className="w-[min(22rem,calc(100vw-2rem))] gap-0 overflow-hidden p-0">
        <PopoverTitle className="sr-only">模型与推理等级</PopoverTitle>
        {showSearch ? (
          <div className="border-b border-border/60 p-2">
            <SearchInput autoFocus value={query} onChange={setQuery} placeholder="搜索模型" ariaLabel="搜索模型" inputClassName="h-7" />
          </div>
        ) : null}

        <RadioGroup
          aria-label="模型"
          value={modelId ?? ""}
          disabled={saving}
          onValueChange={(next) => void selectModel(next)}
          className="flex max-h-[min(18rem,40vh)] flex-col gap-1.5 overflow-y-auto p-1.5"
        >
          {groups.length ? (
            groups.map(([providerId, group]) => (
              <div key={providerId} className="flex flex-col gap-0.5">
                <div className="px-2 pt-1 pb-0.5 text-[11px] font-medium text-muted-foreground">{group.providerName}</div>
                {group.items.map((model) => (
                  <Radio.Root
                    key={model.id}
                    value={model.id}
                    nativeButton
                    render={<Button type="button" variant="ghost" size="sm" />}
                    className="h-7.5 w-full justify-start gap-2 px-2 font-normal data-checked:bg-accent data-checked:font-medium data-checked:text-accent-foreground"
                  >
                    <Radio.Indicator keepMounted className="shrink-0 data-unchecked:invisible">
                      <Check aria-hidden className="size-3.5" />
                    </Radio.Indicator>
                    <span className="min-w-0 flex-1 truncate text-start text-xs" title={model.id}>
                      {model.label}
                    </span>
                  </Radio.Root>
                ))}
              </div>
            ))
          ) : (
            <p className="px-3 py-5 text-center text-xs text-muted-foreground">无匹配模型</p>
          )}
        </RadioGroup>

        <div className="flex flex-col gap-2 border-t border-border/60 px-2.5 py-2">
          {levels.length > 1 ? (
            <div className="flex items-center justify-between gap-2">
              <span className="shrink-0 text-[11px] text-muted-foreground">推理</span>
              <div className="inline-flex flex-wrap justify-end gap-0.5 rounded-md bg-muted/50 p-0.5">
                {levels.map((level) => (
                  <Button
                    key={level}
                    type="button"
                    size="xs"
                    variant="ghost"
                    disabled={saving}
                    aria-pressed={currentEffort === level}
                    className="h-6 px-1.5 text-[11px] font-normal text-muted-foreground aria-pressed:bg-background aria-pressed:font-medium aria-pressed:text-foreground aria-pressed:shadow-2xs"
                    onClick={() => void selectEffort(level)}
                  >
                    {level}
                  </Button>
                ))}
              </div>
            </div>
          ) : null}
          <div className="flex items-center justify-between gap-2">
            {saving ? (
              <span className="inline-flex items-center gap-1.5 text-[11px] text-muted-foreground">
                <Spinner className="size-3" />
                切换中…
              </span>
            ) : (
              <span />
            )}
            <Button
              type="button"
              size="xs"
              variant="ghost"
              className="h-6 px-1.5 text-[11px] font-normal text-muted-foreground hover:text-foreground"
              onClick={() => {
                setOpen(false);
                actions.openSettings("providers");
              }}
            >
              <Settings2 className="size-3" />
              管理模型
            </Button>
          </div>
        </div>
      </PopoverContent>
    </Popover>
  );
}

const APPROVAL_MODES = Object.keys(APPROVAL_LABEL) as ApprovalMode[];
const APPROVAL_ICON = { auto: ShieldAlert, "ask-exec": ShieldCheck, "ask-all": ShieldQuestion } as const;
const APPROVAL_DESCRIPTION: Record<ApprovalMode, string> = {
  "ask-all": "写入项目外文件、联网或影响范围不明确时询问",
  "ask-exec": "自动放行只读命令、低风险 MCP 与可撤销记忆更新，其余询问",
  auto: "工具以当前系统账号权限运行，跳过 Lila 审批",
};

function formatRuleLabel(pattern: string): string {
  if (pattern.startsWith("bash:")) return `${pattern.slice(5)} *`;
  if (pattern.startsWith("mcp:")) {
    const raw = pattern.slice(4);
    return `MCP ${raw.startsWith("mcp__") ? raw.slice(5).replace("__", " / ") : raw}`;
  }
  if (pattern.startsWith("tool:")) return `工具 ${pattern.slice(5)}`;
  return pattern;
}

/** Compact popover listing granted session/project approval rules with one-click revoke. */
export function ApprovalRulesBadge({ sessionId, state }: { sessionId: string; state: SessionState }) {
  const sessionRules = state.sessionRules ?? [];
  const projectRules = state.projectRules ?? [];
  const total = sessionRules.length + projectRules.length;
  const [busy, setBusy] = React.useState(false);
  if (!total || state.approvalMode === "auto") return null;

  const mutate = async (scope: "session" | "project", action: "remove" | "clear", pattern?: string) => {
    if (busy) return;
    setBusy(true);
    try {
      await actions.updateApprovalRules(sessionId, scope, action, pattern);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Popover>
      <PopoverTrigger
        render={
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className={cn(COMPOSER_CHIP, "px-1.5 font-mono tabular-nums")}
            aria-label={`已放行 ${total} 条审批规则`}
            title={`已放行 ${total} 条审批规则`}
          />
        }
      >
        <span>+{total} 规则</span>
      </PopoverTrigger>
      <PopoverContent side="top" align="start" className="flex w-72 flex-col gap-3">
        <PopoverTitle className="text-xs font-medium">已放行的审批规则</PopoverTitle>
        {sessionRules.length ? (
          <div className="flex flex-col gap-1">
            <div className="flex items-center justify-between text-[11px] text-muted-foreground">
              <span>仅当前会话</span>
              <button
                type="button"
                disabled={busy}
                onClick={() => void mutate("session", "clear")}
                className="text-muted-foreground transition-colors hover:text-destructive"
              >
                清空
              </button>
            </div>
            {sessionRules.map((pattern) => (
              <div key={`s:${pattern}`} className="flex items-center justify-between gap-2 rounded-md bg-muted/40 px-2 py-1 text-xs">
                <span className="min-w-0 truncate font-mono">{formatRuleLabel(pattern)}</span>
                <Button
                  type="button"
                  size="icon-xs"
                  variant="ghost"
                  disabled={busy}
                  aria-label={`撤销 ${pattern}`}
                  className="size-5 shrink-0 text-muted-foreground hover:text-destructive"
                  onClick={() => void mutate("session", "remove", pattern)}
                >
                  <X className="size-3" />
                </Button>
              </div>
            ))}
          </div>
        ) : null}
        {projectRules.length ? (
          <div className="flex flex-col gap-1">
            <div className="flex items-center justify-between text-[11px] text-muted-foreground">
              <span>本项目永久有效</span>
              <button
                type="button"
                disabled={busy}
                onClick={() => void mutate("project", "clear")}
                className="text-muted-foreground transition-colors hover:text-destructive"
              >
                清空
              </button>
            </div>
            {projectRules.map((pattern) => (
              <div key={`p:${pattern}`} className="flex items-center justify-between gap-2 rounded-md bg-muted/40 px-2 py-1 text-xs">
                <span className="min-w-0 truncate font-mono">{formatRuleLabel(pattern)}</span>
                <Button
                  type="button"
                  size="icon-xs"
                  variant="ghost"
                  disabled={busy}
                  aria-label={`撤销 ${pattern}`}
                  className="size-5 shrink-0 text-muted-foreground hover:text-destructive"
                  onClick={() => void mutate("project", "remove", pattern)}
                >
                  <X className="size-3" />
                </Button>
              </div>
            ))}
          </div>
        ) : null}
      </PopoverContent>
    </Popover>
  );
}

export function ApprovalSelect({
  value,
  disabled,
  onChange,
  label = "审批方式",
  inSettings = false,
}: {
  value: ApprovalMode;
  disabled?: boolean;
  onChange: (mode: ApprovalMode) => void;
  label?: string;
  inSettings?: boolean;
}) {
  const Icon = APPROVAL_ICON[value];
  return (
    <Select
      value={value}
      disabled={disabled}
      onValueChange={(next) => {
        if (next && next !== value) onChange(next as ApprovalMode);
      }}
      items={APPROVAL_MODES.map((mode) => ({ value: mode, label: APPROVAL_LABEL[mode] }))}
    >
      <SelectTrigger
        size="sm"
        className={inSettings ? "w-44" : COMPOSER_CHIP}
        aria-label={label}
        title={APPROVAL_DESCRIPTION[value]}
      >
        <Icon />
        <SelectValue />
      </SelectTrigger>
      <SelectContent
        alignItemWithTrigger={false}
        align={inSettings ? "end" : "start"}
        side={inSettings ? "bottom" : "top"}
        className="w-[min(23rem,calc(100vw-2rem))]"
      >
        <SelectGroup>
          {APPROVAL_MODES.map((mode) => (
            <SelectItem key={mode} value={mode}>
              <div className="flex min-w-0 flex-col gap-0.5 whitespace-normal">
                <span>{APPROVAL_LABEL[mode]}</span>
                <span className="text-xs text-muted-foreground">{APPROVAL_DESCRIPTION[mode]}</span>
              </div>
            </SelectItem>
          ))}
        </SelectGroup>
      </SelectContent>
    </Select>
  );
}

const TOKENS = new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 1 });
const RING = 2 * Math.PI * 7;

/** Context window use as a small ring next to the send button; X-Ray breakdown, guided compaction and usage shortcut on click. */
export function ContextUsage({ sessionId, state }: { sessionId: string; state: SessionState }) {
  const [open, setOpen] = React.useState(false);
  const [guideOpen, setGuideOpen] = React.useState(false);
  const [instructions, setInstructions] = React.useState("");
  const context = state.context;
  if (!context) return null;
  const percent = context.percent ?? (context.tokens !== null && context.window ? (context.tokens / context.window) * 100 : null);
  const filled = Math.min(100, Math.max(0, percent ?? 0));
  const busy = state.access !== "ready" || state.runActive || state.compacting;
  const tone = filled >= 80 ? "text-warning-foreground" : "text-muted-foreground";
  const breakdown = context.breakdown;
  const breakdownTotal = breakdown ? breakdown.system + breakdown.conversation + breakdown.toolOutputs + breakdown.thinking : 0;
  const segments = breakdown && breakdownTotal > 0
    ? [
        { key: "system", label: "系统与工具", value: breakdown.system, color: "bg-info" },
        { key: "conversation", label: "对话文本", value: breakdown.conversation, color: "bg-primary" },
        { key: "toolOutputs", label: "工具输出", value: breakdown.toolOutputs, color: "bg-warning" },
        { key: "thinking", label: "思考过程", value: breakdown.thinking, color: "bg-muted-foreground/50" },
      ].filter((item) => item.value > 0)
    : [];

  const runCompact = () => {
    const trimmed = instructions.trim();
    setOpen(false);
    setGuideOpen(false);
    setInstructions("");
    void actions.compact(sessionId, trimmed || undefined);
  };

  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) setGuideOpen(false);
      }}
    >
      <PopoverTrigger
        render={
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            className={tone}
            title={`上下文已用 ${Math.round(filled)}%`}
            aria-label={`上下文已用 ${Math.round(filled)}%`}
          />
        }
      >
        <svg viewBox="0 0 18 18" className="size-4 -rotate-90" aria-hidden>
          <circle cx="9" cy="9" r="7" fill="none" stroke="currentColor" strokeWidth="2" opacity="0.2" />
          <circle
            cx="9"
            cy="9"
            r="7"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeDasharray={RING}
            strokeDashoffset={RING * (1 - filled / 100)}
            className="transition-[stroke-dashoffset] duration-200 ease-(--ease-ui-out) motion-reduce:transition-none"
          />
        </svg>
      </PopoverTrigger>
      <PopoverContent align="end" side="top" className="flex w-72 flex-col gap-3">
        <PopoverTitle className="sr-only">上下文与用量</PopoverTitle>
        <div className="flex flex-col gap-2">
          <div className="flex items-baseline justify-between text-xs">
            <span className="font-medium">上下文 {percent === null ? "" : `${Math.round(percent)}%`}</span>
            <span className="text-muted-foreground tabular-nums">
              {context.tokens === null ? "未知" : TOKENS.format(context.tokens)} / {TOKENS.format(context.window)}
            </span>
          </div>
          {segments.length > 0 ? (
            <>
              <div className="flex h-1.5 w-full overflow-hidden rounded-full bg-muted">
                {segments.map((seg) => (
                  <div
                    key={seg.key}
                    style={{ width: `${(seg.value / breakdownTotal) * 100}%` }}
                    className={cn("h-full transition-[width] duration-200 ease-(--ease-ui-out)", seg.color)}
                    title={`${seg.label}：约 ${TOKENS.format(seg.value)}`}
                  />
                ))}
              </div>
              <div className="grid grid-cols-2 gap-x-3 gap-y-1 text-[11px] text-muted-foreground tabular-nums">
                {segments.map((seg) => (
                  <div key={seg.key} className="flex items-center justify-between gap-1.5">
                    <span className="inline-flex min-w-0 items-center gap-1.5">
                      <span className={cn("size-1.5 shrink-0 rounded-full", seg.color)} />
                      <span className="truncate">{seg.label}</span>
                    </span>
                    <span className="shrink-0 font-mono">{TOKENS.format(seg.value)}</span>
                  </div>
                ))}
              </div>
            </>
          ) : (
            <Progress value={filled} />
          )}
        </div>
        <div className="flex items-center justify-between text-xs">
          <span className="font-medium">花费</span>
          <span className="text-muted-foreground tabular-nums">${state.cost.toFixed(4)}</span>
        </div>
        {guideOpen ? (
          <form
            className="flex flex-col gap-1.5"
            onSubmit={(event) => {
              event.preventDefault();
              runCompact();
            }}
          >
            <Input
              autoFocus
              value={instructions}
              disabled={busy}
              placeholder="重点保留什么？（可选）"
              aria-label="定向压缩保留重点"
              className="h-7 text-xs"
              onChange={(event) => setInstructions(event.target.value)}
            />
          </form>
        ) : null}
        <div className="flex items-center gap-1.5">
          <Button
            type="button"
            size="sm"
            variant="outline"
            className="flex-1"
            disabled={busy}
            onClick={runCompact}
          >
            {state.compacting ? <Spinner /> : <Minimize2 />}
            {state.compacting ? "压缩中…" : guideOpen && instructions.trim() ? "定向压缩" : "压缩上下文"}
          </Button>
          <Button
            type="button"
            size="icon-sm"
            variant={guideOpen ? "secondary" : "ghost"}
            disabled={busy}
            title="指定压缩时重点保留的内容"
            aria-label="指定压缩时重点保留的内容"
            aria-pressed={guideOpen}
            className="shrink-0 text-muted-foreground hover:text-foreground"
            onClick={() => setGuideOpen((v) => !v)}
          >
            <SlidersHorizontal className="size-3.5" />
          </Button>
          <Button
            type="button"
            size="sm"
            variant="ghost"
            className="shrink-0 text-muted-foreground hover:text-foreground"
            onClick={() => {
              setOpen(false);
              actions.openSettings("usage");
            }}
          >
            <BarChart3 />
            用量
          </Button>
        </div>
      </PopoverContent>
    </Popover>
  );
}
