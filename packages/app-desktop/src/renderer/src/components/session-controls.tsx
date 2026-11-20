import * as React from "react";
import { Radio } from "@base-ui/react/radio";
import { RadioGroup } from "@base-ui/react/radio-group";
import { Check, ChevronDown, Minimize2, RotateCw, ShieldAlert, ShieldCheck, ShieldQuestion } from "lucide-react";
import type { ApprovalMode, ModelInfo, SessionState, ThinkingLevel } from "@lila/protocol";
import { Button } from "@/components/ui/button";
import { HoverCard, HoverCardContent, HoverCardTrigger } from "@/components/ui/hover-card";
import { Popover, PopoverContent, PopoverTitle, PopoverTrigger } from "@/components/ui/popover";
import { Slider } from "@/components/ui/slider";
import { Progress } from "@/components/ui/progress";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";
import { actions, APPROVAL_HINT, APPROVAL_LABEL, useLila } from "@/lib/store";

export function ModelSetup() {
  const connected = useLila((state) => state.connection === "connected");
  const loading = useLila((state) => state.modelsLoading);
  const error = useLila((state) => state.modelsError);
  const hasModels = useLila((state) => state.models.length > 0);
  const guidance = <>没有可用模型。请在终端运行 <code>pi</code>，在 Pi 中输入 <code>/login</code>，然后回来刷新。</>;
  const status = loading ? "正在读取模型…" : error ? "读取模型失败，请刷新后重试。" : hasModels ? "请选择一个可用模型。" : guidance;
  return (
    <div className="flex flex-wrap items-center justify-between gap-2 rounded-xl border bg-muted/30 px-3 py-2 text-sm">
      <p role="status" className="min-w-0 flex-1">{status}</p>
      <Button type="button" size="sm" variant="outline" disabled={!connected || loading} onClick={() => void actions.retryModels()}>
        <RotateCw />
        刷新模型
      </Button>
    </div>
  );
}

/** The quiet chip look shared by every control in the composer's bottom bar. */
const CHIP =
  "h-7 gap-1 rounded-md border-transparent bg-transparent px-2 text-xs font-normal text-muted-foreground shadow-none hover:bg-muted hover:text-foreground dark:bg-transparent dark:hover:bg-muted aria-expanded:bg-muted aria-expanded:text-foreground [&_svg]:size-3.5 [&>svg:last-child]:size-3 [&>svg:last-child]:opacity-60";

/** The Model Selector and its effort control share one focused Base UI popover. */
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
  const [selection, setSelection] = React.useState(modelId);
  const [effort, setEffort] = React.useState(thinkingLevel);
  const [saving, setSaving] = React.useState(false);

  const current = models.find((model) => model.id === modelId);
  const selected = models.find((model) => model.id === selection);
  const levels = (selection && selection === modelId ? currentLevels : undefined) ?? selected?.thinkingLevels ?? [];
  const chosenEffort = effort && levels.includes(effort) ? effort : selected?.defaultThinkingLevel ?? levels[0];
  const currentEffort = thinkingLevel ?? current?.defaultThinkingLevel;
  const sliderIndex = Math.max(0, levels.indexOf(chosenEffort ?? "off"));

  async function apply(): Promise<void> {
    if (!selection || !chosenEffort || saving) return;
    setSaving(true);
    try {
      if (await onApply(selection, chosenEffort)) setOpen(false);
    } finally {
      setSaving(false);
    }
  }

  return (
    <Popover open={open} onOpenChange={(next) => {
      if (next) {
        setSelection(modelId ?? models[0]?.id);
        setEffort(thinkingLevel);
      }
      setOpen(next);
    }}>
      <PopoverTrigger
        render={<Button type="button" variant="ghost" size="sm" className={cn(CHIP, "max-w-full sm:max-w-[19rem]")} />}
        disabled={!modelId && !models.length}
        aria-label="模型与推理等级"
      >
        <span className="truncate">{current?.label ?? modelId ?? (models.length ? "选择模型" : "没有可用模型")}</span>
        {currentEffort && currentEffort !== "off" ? (
          <span className="shrink-0 text-muted-foreground">· {currentEffort}</span>
        ) : null}
        <ChevronDown />
      </PopoverTrigger>
      <PopoverContent side="top" align="start" className="w-[min(21rem,calc(100vw-2rem))] gap-0 overflow-hidden p-0">
        <PopoverTitle className="sr-only">模型与推理等级</PopoverTitle>
        <RadioGroup
          aria-label="模型"
          value={selection ?? ""}
          disabled={saving}
          onValueChange={(next) => {
            setSelection(next);
            setEffort(next === modelId ? thinkingLevel : undefined);
          }}
          className="flex max-h-[min(15rem,35vh)] flex-col overflow-y-auto p-1"
        >
          {models.map((model) => (
            <Radio.Root
              key={model.id}
              value={model.id}
              nativeButton
              render={<Button type="button" variant="ghost" />}
              className="w-full justify-start gap-2 px-2 font-normal data-checked:bg-accent data-checked:text-accent-foreground"
            >
              <Radio.Indicator keepMounted className="shrink-0 data-unchecked:invisible">
                <Check aria-hidden />
              </Radio.Indicator>
              <span className="min-w-0 flex-1 truncate text-start" title={model.label}>{model.label}</span>
              <span className="max-w-[35%] truncate text-xs text-muted-foreground" title={model.provider}>{model.provider}</span>
            </Radio.Root>
          ))}
        </RadioGroup>
        <div className="flex flex-col gap-1 border-t px-2.5 py-2">
          {levels.length > 1 ? (
            <>
              <Slider
                value={[sliderIndex]}
                max={levels.length - 1}
                step={1}
                disabled={saving}
                thumbLabel="推理等级"
                getAriaValueText={(_formatted, value) => levels[value] ?? "off"}
                onValueChange={(value) => setEffort(levels[typeof value === "number" ? value : value[0] ?? 0])}
                className="py-1"
              />
              <div className="flex flex-wrap justify-between gap-0.5">
                {levels.map((level) => (
                  <Button
                    key={level}
                    type="button"
                    size="xs"
                    variant="ghost"
                    disabled={saving}
                    aria-pressed={chosenEffort === level}
                    className="px-1 text-xs font-normal text-muted-foreground aria-pressed:bg-accent aria-pressed:font-medium aria-pressed:text-foreground"
                    onClick={() => setEffort(level)}
                  >
                    {level}
                  </Button>
                ))}
              </div>
            </>
          ) : null}
          <div className="flex items-center justify-end gap-2">
            {levels.length === 1 ? (
              <span className="me-auto text-xs text-muted-foreground"><span className="sr-only">推理等级 </span>{chosenEffort}</span>
            ) : null}
            <Button type="button" size="sm" disabled={!selection || !chosenEffort || saving} onClick={() => void apply()}>
              {saving ? <Spinner /> : null}
              应用
            </Button>
          </div>
        </div>
      </PopoverContent>
    </Popover>
  );
}

const APPROVAL_MODES = Object.keys(APPROVAL_LABEL) as ApprovalMode[];
const APPROVAL_SHORT: Record<ApprovalMode, string> = { auto: "自动", "ask-exec": "命令前询问", "ask-all": "都询问" };
const APPROVAL_ICON = { auto: ShieldCheck, "ask-exec": ShieldQuestion, "ask-all": ShieldAlert } as const;

export function ApprovalSelect({ value, disabled, onChange }: { value: ApprovalMode; disabled?: boolean; onChange: (mode: ApprovalMode) => void }) {
  const Icon = APPROVAL_ICON[value];
  return (
    <Select
      value={value}
      disabled={disabled}
      onValueChange={(next) => {
        if (next && next !== value) onChange(next as ApprovalMode);
      }}
      items={APPROVAL_MODES.map((mode) => ({ value: mode, label: APPROVAL_SHORT[mode] }))}
    >
      <SelectTrigger size="sm" className={CHIP} aria-label="审批方式" title={APPROVAL_LABEL[value]}>
        <Icon />
        <SelectValue />
      </SelectTrigger>
      <SelectContent alignItemWithTrigger={false} align="start" side="top" className="w-72">
        {APPROVAL_MODES.map((mode) => (
          <SelectItem key={mode} value={mode} className="[&>*:first-child]:min-w-0 [&>*:first-child]:shrink [&>*:first-child]:whitespace-normal">
            <div className="flex flex-col whitespace-normal">
              <span>{APPROVAL_LABEL[mode]}</span>
              <span className="text-xs text-muted-foreground">{APPROVAL_HINT[mode]}</span>
            </div>
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

const TOKENS = new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 1 });
const RING = 2 * Math.PI * 7;

/** Context window use as a small ring next to the send button; details and compaction on hover. */
export function ContextUsage({ sessionId, state }: { sessionId: string; state: SessionState }) {
  const context = state.context;
  if (!context) return null;
  const percent = context.percent ?? (context.tokens !== null && context.window ? (context.tokens / context.window) * 100 : null);
  const filled = Math.min(100, Math.max(0, percent ?? 0));
  const busy = state.access !== "ready" || state.runActive || state.compacting;
  const tone = filled >= 80 ? "text-amber-600 dark:text-amber-400" : "text-muted-foreground";
  return (
    <HoverCard>
      <HoverCardTrigger render={<Button variant="ghost" size="icon-sm" className={tone} aria-label={`上下文已用 ${Math.round(filled)}%`} />}>
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
          />
        </svg>
      </HoverCardTrigger>
      <HoverCardContent align="end" side="top" className="flex w-64 flex-col gap-3">
        <div className="flex flex-col gap-1.5">
          <div className="flex items-baseline justify-between text-xs">
            <span className="font-medium">上下文 {percent === null ? "" : `${Math.round(percent)}%`}</span>
            <span className="text-muted-foreground tabular-nums">
              {context.tokens === null ? "未知" : TOKENS.format(context.tokens)} / {TOKENS.format(context.window)}
            </span>
          </div>
          <Progress value={filled} />
        </div>
        <div className="flex items-baseline justify-between text-xs">
          <span className="font-medium">花费</span>
          <span className="text-muted-foreground tabular-nums">${state.cost.toFixed(4)}</span>
        </div>
        <Button size="sm" variant="outline" disabled={busy} onClick={() => void actions.compact(sessionId)}>
          {state.compacting ? <Spinner /> : <Minimize2 />}
          {state.compacting ? "正在压缩…" : "压缩上下文"}
        </Button>
      </HoverCardContent>
    </HoverCard>
  );
}
