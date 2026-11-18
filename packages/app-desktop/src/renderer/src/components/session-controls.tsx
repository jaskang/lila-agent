import { Brain, Minimize2, ShieldAlert, ShieldCheck, ShieldQuestion } from "lucide-react";
import type { ApprovalMode, ModelInfo, SessionState, ThinkingLevel } from "@lila/protocol";
import { Button } from "@/components/ui/button";
import { Combobox, ComboboxContent, ComboboxEmpty, ComboboxInput, ComboboxItem, ComboboxList, ComboboxTrigger } from "@/components/ui/combobox";
import { HoverCard, HoverCardContent, HoverCardTrigger } from "@/components/ui/hover-card";
import { Progress } from "@/components/ui/progress";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";
import { actions, APPROVAL_HINT, APPROVAL_LABEL, THINKING_LABEL } from "@/lib/store";

/** The quiet chip look shared by every control in the composer's bottom bar. */
const CHIP =
  "h-7 gap-1 rounded-md border-transparent bg-transparent px-2 text-xs font-normal text-muted-foreground shadow-none hover:bg-muted hover:text-foreground dark:bg-transparent dark:hover:bg-muted aria-expanded:bg-muted aria-expanded:text-foreground [&_svg]:size-3.5 [&>svg:last-child]:size-3 [&>svg:last-child]:opacity-60";

export function ModelPicker({
  models,
  value,
  disabled,
  onChange,
}: {
  models: ModelInfo[];
  value: string | undefined;
  disabled?: boolean;
  onChange: (modelId: string) => void;
}) {
  const byId = new Map(models.map((model) => [model.id, model]));
  const label = (id: string) => byId.get(id)?.label ?? id;
  return (
    <Combobox<string>
      items={models.map((model) => model.id)}
      value={value ?? null}
      disabled={disabled || !models.length}
      itemToStringLabel={label}
      filter={(id, query) => {
        const q = query.trim().toLowerCase();
        return !q || label(id).toLowerCase().includes(q) || id.toLowerCase().includes(q);
      }}
      onValueChange={(next) => {
        if (next && next !== value) onChange(next);
      }}
    >
      <ComboboxTrigger render={<Button variant="ghost" size="sm" className={cn(CHIP, "max-w-48")} />} aria-label="模型">
        <span className="truncate">{value ? label(value) : models.length ? "默认模型" : "没有可用模型"}</span>
      </ComboboxTrigger>
      <ComboboxContent className="w-80" align="start" side="top">
        <ComboboxInput showTrigger={false} placeholder="搜索模型…" />
        <ComboboxEmpty>没有匹配的模型</ComboboxEmpty>
        <ComboboxList>
          {(id: string) => (
            <ComboboxItem key={id} value={id}>
              <span className="min-w-0 flex-1 truncate">{label(id)}</span>
              <span className="shrink-0 text-xs text-muted-foreground">{byId.get(id)?.provider}</span>
            </ComboboxItem>
          )}
        </ComboboxList>
      </ComboboxContent>
    </Combobox>
  );
}

export function ThinkingSelect({
  levels,
  value,
  disabled,
  onChange,
}: {
  levels: ThinkingLevel[];
  value: ThinkingLevel | undefined;
  disabled?: boolean;
  onChange: (level: ThinkingLevel) => void;
}) {
  if (levels.length <= 1) return null;
  return (
    <Select
      value={value ?? null}
      disabled={disabled}
      onValueChange={(next) => {
        if (next && next !== value) onChange(next as ThinkingLevel);
      }}
      items={levels.map((level) => ({ value: level, label: THINKING_LABEL[level] }))}
    >
      <SelectTrigger size="sm" className={CHIP} aria-label="思考强度" title="思考强度">
        <Brain />
        <SelectValue placeholder="默认思考" />
      </SelectTrigger>
      <SelectContent alignItemWithTrigger={false} align="start" side="top">
        {levels.map((level) => (
          <SelectItem key={level} value={level}>
            {THINKING_LABEL[level]}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
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
  const busy = state.status === "running" || state.compacting;
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
