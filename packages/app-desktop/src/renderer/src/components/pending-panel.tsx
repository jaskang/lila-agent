import * as React from "react";
import { Check, ChevronDown, Copy, MessageCircleQuestion, MessageSquareText, ShieldAlert } from "lucide-react";
import { toast } from "sonner";
import type { PendingAnswer, PendingRequest } from "@lila/protocol";
import { Button } from "@/components/ui/button";
import { Item, ItemContent, ItemMedia, ItemTitle } from "@/components/ui/item";
import { Input } from "@/components/ui/input";
import {
  Questionnaire,
  QuestionnaireActions,
  QuestionnaireChoice,
  QuestionnaireChoices,
  QuestionnaireInput,
  QuestionnaireItem,
  QuestionnaireNext,
  QuestionnairePrevious,
  QuestionnaireProgress,
  QuestionnaireSubmit,
  QuestionnaireTitle,
} from "@/components/ui/questionnaire";
import { Spinner } from "@/components/ui/spinner";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "cn";
import { actions } from "@/lib/store";

type Answer = (answer: PendingAnswer) => void;
type Choice = { label: string; answer: PendingAnswer; variant: "default" | "secondary" | "outline" };

const APPROVAL_CHOICES: Choice[] = [
  { label: "允许本次", answer: { type: "option", value: "allow" }, variant: "default" },
  { label: "拒绝", answer: { type: "option", value: "deny" }, variant: "outline" },
];

const CONFIRM_CHOICES: Choice[] = [
  { label: "确认", answer: { type: "confirm", confirmed: true }, variant: "default" },
  { label: "取消", answer: { type: "confirm", confirmed: false }, variant: "outline" },
];

function Shell({
  icon,
  title,
  message,
  queuedItems,
  busy,
  children,
}: {
  icon: React.ReactNode;
  title: string;
  message?: string | undefined;
  queuedItems: PendingRequest[];
  busy: boolean;
  children: React.ReactNode;
}) {
  const [expandedMsg, setExpandedMsg] = React.useState(false);
  const [showQueue, setShowQueue] = React.useState(false);
  const [copied, setCopied] = React.useState(false);
  const isLong = Boolean(message && (message.length > 360 || message.split("\n").length > 7));

  const copyMessage = () => {
    if (!message) return;
    void navigator.clipboard
      .writeText(message)
      .then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1_500);
      })
      .catch(() => toast.error("复制失败"));
  };

  return (
    <Item
      variant="outline"
      size="sm"
      role="region"
      aria-label={title}
      data-pending-panel=""
      className="surface-enter items-start gap-2 border-warning/35 bg-warning/8 dark:bg-warning/10"
    >
      <ItemMedia variant="icon" className="mt-0.5 text-warning-foreground">
        {busy ? <Spinner className="size-4" /> : icon}
      </ItemMedia>
      <ItemContent className="min-w-0 gap-2">
        <div className="flex items-center gap-2">
          <ItemTitle className="min-w-0 flex-1">{title}</ItemTitle>
          {queuedItems.length > 0 ? (
            <button
              type="button"
              aria-expanded={showQueue}
              onClick={() => setShowQueue((open) => !open)}
              className="inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-xs text-muted-foreground transition-[color,background-color,scale] duration-150 ease-(--ease-ui-out) outline-none hover:bg-background/60 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 motion-safe:active:scale-[0.97]"
            >
              还有 {queuedItems.length} 个
              <ChevronDown
                aria-hidden
                className={cn(
                  "size-3 transition-transform duration-150 ease-(--ease-ui-out) motion-reduce:transition-none",
                  showQueue && "rotate-180",
                )}
              />
            </button>
          ) : null}
        </div>
        {showQueue && queuedItems.length > 0 ? (
          <div className="flex flex-col gap-1 rounded-md border border-border/60 bg-background/60 px-2.5 py-1.5 text-xs text-muted-foreground">
            {queuedItems.map((req, idx) => (
              <div key={req.id} className="truncate">
                {idx + 2}. {req.title || (req.kind === "approval" ? `运行 ${req.toolName ?? "工具"}` : "待回答问题")}
              </div>
            ))}
          </div>
        ) : null}
        {message ? (
          <div className="relative">
            <pre
              className={cn(
                "overflow-auto rounded-md bg-background/70 p-2 pr-14 font-mono text-xs whitespace-pre-wrap break-words",
                expandedMsg ? "max-h-96" : "max-h-40",
              )}
            >
              {message}
            </pre>
            <div className="absolute top-1.5 right-1.5 flex items-center gap-0.5">
              {isLong ? (
                <Button
                  type="button"
                  size="icon-xs"
                  variant="ghost"
                  className="size-6 bg-background/80 text-muted-foreground hover:text-foreground"
                  title={expandedMsg ? "收起" : "展开完整内容"}
                  aria-label={expandedMsg ? "收起" : "展开完整内容"}
                  onClick={() => setExpandedMsg((v) => !v)}
                >
                  <ChevronDown
                    aria-hidden
                    className={cn(
                      "size-3 transition-transform duration-150 ease-(--ease-ui-out) motion-reduce:transition-none",
                      expandedMsg && "rotate-180",
                    )}
                  />
                </Button>
              ) : null}
              <Button
                type="button"
                size="icon-xs"
                variant="ghost"
                className="size-6 bg-background/80 text-muted-foreground hover:text-foreground"
                title="复制内容"
                aria-label="复制内容"
                onClick={copyMessage}
              >
                <span className="swap-icon" aria-hidden="true">
                  <Copy className="size-3" data-state={copied ? "hidden" : "visible"} />
                  <Check className="size-3" data-state={copied ? "visible" : "hidden"} />
                </span>
              </Button>
            </div>
          </div>
        ) : null}
        {children}
      </ItemContent>
    </Item>
  );
}

function Choices({
  choices,
  busy,
  onAnswer,
  onCancel,
  allowDenyNote,
}: {
  choices: Choice[];
  busy: boolean;
  onAnswer: Answer;
  onCancel?: () => void;
  allowDenyNote?: boolean;
}) {
  const [denyNoteOpen, setDenyNoteOpen] = React.useState(false);
  const [denyNote, setDenyNote] = React.useState("");

  const handleChoiceKey = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (busy || denyNoteOpen || event.repeat || event.nativeEvent.isComposing || event.keyCode === 229) return;
    if (event.defaultPrevented || event.altKey || event.shiftKey || !/^[1-9]$/.test(event.key)) return;
    const choice = choices[Number(event.key) - 1];
    if (!choice) return;
    event.preventDefault();
    onAnswer(choice.answer);
  };

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap gap-2" onKeyDown={handleChoiceKey}>
        {choices.map((choice, index) => (
          <Button
            key={`${choice.label}-${index}`}
            size="sm"
            variant={choice.variant}
            disabled={busy}
            title={index < 9 ? `聚焦此组按钮时按 ${index + 1}` : undefined}
            aria-keyshortcuts={index < 9 ? String(index + 1) : undefined}
            onClick={() => onAnswer(choice.answer)}
          >
            {choice.label}
          </Button>
        ))}
        {allowDenyNote ? (
          <Button
            type="button"
            size="sm"
            variant="ghost"
            disabled={busy}
            className="text-muted-foreground hover:text-foreground"
            onClick={() => setDenyNoteOpen((open) => !open)}
          >
            <MessageSquareText />
            拒绝并说明
          </Button>
        ) : null}
        {onCancel ? (
          <Button size="sm" variant="ghost" disabled={busy} onClick={onCancel}>
            取消
          </Button>
        ) : null}
      </div>
      {allowDenyNote && denyNoteOpen ? (
        <form
          className="flex items-center gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            onAnswer({ type: "option", value: "deny", ...(denyNote.trim() ? { note: denyNote.trim() } : {}) });
          }}
        >
          <Input
            autoFocus
            value={denyNote}
            placeholder="拒绝原因（可选）"
            className="h-8 flex-1 bg-background text-xs"
            onChange={(event) => setDenyNote(event.target.value)}
            onKeyDown={(event) => {
              if (event.nativeEvent.isComposing || event.keyCode === 229) return;
              if (event.key === "Escape") {
                event.preventDefault();
                setDenyNoteOpen(false);
              }
            }}
          />
          <Button type="submit" size="sm" variant="destructive" disabled={busy}>
            拒绝并发送
          </Button>
        </form>
      ) : null}
    </div>
  );
}

function TextAnswer({ item, multiline, onAnswer }: { item: PendingRequest; multiline: boolean; onAnswer: Answer }) {
  const [value, setValue] = React.useState(item.prefill ?? "");
  return (
    <form
      className="flex flex-col gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        onAnswer({ type: "text", value });
      }}
    >
      {multiline ? (
        <Textarea
          autoFocus
          value={value}
          placeholder={item.placeholder}
          onChange={(event) => setValue(event.target.value)}
          onKeyDown={(event) => {
            if (event.nativeEvent.isComposing || event.keyCode === 229) return;
            if (event.key === "Escape") {
              event.preventDefault();
              onAnswer({ type: "cancel" });
              return;
            }
            if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
              event.preventDefault();
              event.currentTarget.form?.requestSubmit();
            }
          }}
          className="max-h-80 min-h-32 bg-background font-mono text-xs"
        />
      ) : (
        <Input
          autoFocus
          value={value}
          placeholder={item.placeholder}
          onChange={(event) => setValue(event.target.value)}
          onKeyDown={(event) => {
            if (event.nativeEvent.isComposing || event.keyCode === 229) {
              if (event.key === "Enter") event.preventDefault();
              return;
            }
            if (event.key === "Escape") {
              event.preventDefault();
              onAnswer({ type: "cancel" });
            }
          }}
          className="bg-background"
        />
      )}
      <div className="flex gap-2">
        <Button type="submit" size="sm" title={multiline ? "⌘↵" : "↵"}>
          提交
        </Button>
        <Button type="button" size="sm" variant="ghost" title="Esc" onClick={() => onAnswer({ type: "cancel" })}>
          取消
        </Button>
      </div>
    </form>
  );
}

function AskAnswer({ item, onAnswer }: { item: PendingRequest; onAnswer: Answer }) {
  const questions = item.questions ?? [];
  return (
    <Questionnaire
      shortcuts="numbers"
      onSubmit={(event) => {
        event.preventDefault();
        const data = new FormData(event.currentTarget);
        const answers = questions.map((question) => {
          const values = data
            .getAll(question.id)
            .map((v) => String(v).trim())
            .filter(Boolean);
          const custom = values.find((value) => !question.options.includes(value));
          return { id: question.id, selected: values.filter((value) => question.options.includes(value)), ...(custom ? { custom } : {}) };
        });
        if (answers.some((ans) => !ans.selected.length && !ans.custom)) {
          toast.error("请完成所有必答问题，或点击取消");
          return;
        }
        onAnswer({ type: "ask", answers });
      }}
    >
      {questions.length > 1 ? <QuestionnaireProgress /> : null}
      {questions.map((question) => (
        <QuestionnaireItem key={question.id} name={question.id} multiple={question.multiple} required>
          <QuestionnaireTitle>{question.question}</QuestionnaireTitle>
          <QuestionnaireChoices>
            {question.options.map((option) => (
              <QuestionnaireChoice key={option} value={option}>
                {option}
              </QuestionnaireChoice>
            ))}
            <QuestionnaireInput aria-label="其他回答" placeholder="其他回答…" />
          </QuestionnaireChoices>
        </QuestionnaireItem>
      ))}
      <QuestionnaireActions>
        <Button type="button" size="sm" variant="ghost" className="col-start-2 row-start-1 justify-self-end" onClick={() => onAnswer({ type: "cancel" })}>
          {questions.length > 1 ? "取消全部" : "跳过"}
        </Button>
        <QuestionnairePrevious size="sm">上一题</QuestionnairePrevious>
        <QuestionnaireNext size="sm">下一题</QuestionnaireNext>
        <QuestionnaireSubmit size="sm">提交</QuestionnaireSubmit>
      </QuestionnaireActions>
    </Questionnaire>
  );
}

function PendingCard({ sessionId, item, queuedItems }: { sessionId: string; item: PendingRequest; queuedItems: PendingRequest[] }) {
  const [busy, setBusy] = React.useState(false);
  const onAnswer = React.useCallback<Answer>(
    (answer) => {
      if (busy) return;
      setBusy(true);
      void actions.respond(sessionId, item.id, answer).finally(() => setBusy(false));
    },
    [busy, sessionId, item.id],
  );
  const cancel = React.useCallback(() => onAnswer({ type: "cancel" }), [onAnswer]);
  const selectChoices = React.useMemo<Choice[]>(
    () => item.options.map((option, index) => ({ label: option, answer: { type: "option", value: option }, variant: index === 0 ? "default" : "secondary" })),
    [item.options],
  );
  const approvalChoices = React.useMemo<Choice[]>(() => {
    if (!item.ruleSuggestion) return APPROVAL_CHOICES;
    return [
      { label: "允许本次", answer: { type: "option", value: "allow" }, variant: "default" },
      { label: `本会话允许 ${item.ruleSuggestion.label}`, answer: { type: "option", value: "allow-session" }, variant: "secondary" },
      { label: "项目永久允许", answer: { type: "option", value: "allow-project" }, variant: "outline" },
      { label: "拒绝", answer: { type: "option", value: "deny" }, variant: "outline" },
    ];
  }, [item.ruleSuggestion]);
  const approval = item.kind === "approval";
  const icon = approval ? <ShieldAlert className="size-4" /> : <MessageCircleQuestion className="size-4" />;
  const title = item.title || (approval ? `Agent 想要运行 ${item.toolName ?? "工具"}` : "Agent 有个问题");
  return (
    <div className={cn("transition-opacity duration-150", busy && "pointer-events-none opacity-60")}>
      <Shell icon={icon} title={title} message={item.message} queuedItems={queuedItems} busy={busy}>
        {item.kind === "approval" ? <Choices choices={approvalChoices} busy={busy} onAnswer={onAnswer} allowDenyNote /> : null}
        {item.kind === "select" ? <Choices choices={selectChoices} busy={busy} onAnswer={onAnswer} onCancel={cancel} /> : null}
        {item.kind === "confirm" ? <Choices choices={CONFIRM_CHOICES} busy={busy} onAnswer={onAnswer} /> : null}
        {item.kind === "input" || item.kind === "editor" ? <TextAnswer item={item} multiline={item.kind === "editor"} onAnswer={onAnswer} /> : null}
        {item.kind === "ask" ? <AskAnswer item={item} onAnswer={onAnswer} /> : null}
      </Shell>
    </div>
  );
}

/** Only the oldest request is answerable; Pi waits for one answer at a time. */
export function PendingPanel({ sessionId, pending }: { sessionId: string; pending: PendingRequest[] }) {
  const [current, ...rest] = pending;
  if (!current) return null;
  return <PendingCard key={current.id} sessionId={sessionId} item={current} queuedItems={rest} />;
}
