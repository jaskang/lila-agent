import * as React from "react";
import { MessageCircleQuestion, ShieldAlert } from "lucide-react";
import type { PendingAnswer, PendingRequest } from "@lila/protocol";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Kbd } from "@/components/ui/kbd";
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
import { Textarea } from "@/components/ui/textarea";
import { actions } from "@/lib/store";

type Answer = (answer: PendingAnswer) => void;
type Choice = { label: string; answer: PendingAnswer; variant: "default" | "secondary" | "outline" };

const APPROVAL_CHOICES: Choice[] = [
  { label: "允许", answer: { type: "option", value: "allow" }, variant: "default" },
  { label: "本会话始终允许", answer: { type: "option", value: "always" }, variant: "secondary" },
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
  extra,
  children,
}: {
  icon: React.ReactNode;
  title: string;
  message?: string | undefined;
  extra?: number;
  children: React.ReactNode;
}) {
  return (
    <section aria-live="polite" className="rounded-xl border border-amber-500/40 bg-amber-50/80 p-4 shadow-sm dark:bg-amber-950/30">
      <div className="flex items-start gap-3">
        <span className="mt-0.5 text-amber-600 dark:text-amber-400">{icon}</span>
        <div className="flex min-w-0 flex-1 flex-col gap-3">
          <div className="flex min-w-0 flex-col gap-1">
            <div className="flex items-baseline gap-2">
              <p className="min-w-0 flex-1 text-sm font-medium">{title}</p>
              {extra ? <span className="shrink-0 text-xs text-muted-foreground">还有 {extra} 个</span> : null}
            </div>
            {message ? (
              <pre className="max-h-48 overflow-auto rounded-md bg-background/70 p-2 font-mono text-xs whitespace-pre-wrap">{message}</pre>
            ) : null}
          </div>
          {children}
        </div>
      </div>
    </section>
  );
}

/** Buttons answered with number keys while focus is not in a text field. */
function Choices({ choices, onAnswer, onCancel }: { choices: Choice[]; onAnswer: Answer; onCancel?: () => void }) {
  const first = React.useRef<HTMLButtonElement>(null);
  React.useEffect(() => first.current?.focus(), []);
  React.useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (target?.closest("textarea, input, [contenteditable=true]") || event.metaKey || event.ctrlKey || event.altKey) return;
      const choice = choices[Number(event.key) - 1];
      if (choice) {
        event.preventDefault();
        onAnswer(choice.answer);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [choices, onAnswer]);
  return (
    <div className="flex flex-wrap gap-2">
      {choices.map((choice, index) => (
        <Button key={choice.label} ref={index === 0 ? first : undefined} size="sm" variant={choice.variant} onClick={() => onAnswer(choice.answer)}>
          {choice.label}
          {index < 9 ? <Kbd className="ms-1">{index + 1}</Kbd> : null}
        </Button>
      ))}
      {onCancel ? (
        <Button size="sm" variant="ghost" onClick={onCancel}>
          跳过
        </Button>
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
            if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
              event.preventDefault();
              event.currentTarget.form?.requestSubmit();
            }
          }}
          className="max-h-80 min-h-32 bg-background font-mono text-xs"
        />
      ) : (
        <Input autoFocus value={value} placeholder={item.placeholder} onChange={(event) => setValue(event.target.value)} className="bg-background" />
      )}
      <div className="flex gap-2">
        <Button type="submit" size="sm">
          提交 <Kbd className="ms-1">{multiline ? "⌘↵" : "↵"}</Kbd>
        </Button>
        <Button type="button" size="sm" variant="ghost" onClick={() => onAnswer({ type: "cancel" })}>
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
        onAnswer({
          type: "ask",
          answers: questions.map((question) => {
            const values = data.getAll(question.id).map(String).filter(Boolean);
            const custom = values.find((value) => !question.options.includes(value));
            return { id: question.id, selected: values.filter((value) => question.options.includes(value)), ...(custom ? { custom } : {}) };
          }),
        });
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
            <QuestionnaireInput aria-label="其他回答" placeholder="或者直接写下你的回答…" />
          </QuestionnaireChoices>
        </QuestionnaireItem>
      ))}
      <QuestionnaireActions>
        <Button type="button" size="sm" variant="ghost" className="justify-self-start" onClick={() => onAnswer({ type: "cancel" })}>
          跳过
        </Button>
        <QuestionnairePrevious size="sm">上一题</QuestionnairePrevious>
        <QuestionnaireNext size="sm">下一题</QuestionnaireNext>
        <QuestionnaireSubmit size="sm">提交</QuestionnaireSubmit>
      </QuestionnaireActions>
    </Questionnaire>
  );
}

function PendingCard({ sessionId, item, extra }: { sessionId: string; item: PendingRequest; extra: number }) {
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
  const approval = item.kind === "approval";
  const icon = approval ? <ShieldAlert className="size-4" /> : <MessageCircleQuestion className="size-4" />;
  const title = item.title || (approval ? `Agent 想要运行 ${item.toolName ?? "工具"}` : "Agent 有个问题");
  return (
    <div className={busy ? "pointer-events-none opacity-60" : undefined}>
      <Shell icon={icon} title={title} message={item.message} extra={extra}>
        {item.kind === "approval" ? <Choices choices={APPROVAL_CHOICES} onAnswer={onAnswer} /> : null}
        {item.kind === "select" ? <Choices choices={selectChoices} onAnswer={onAnswer} onCancel={cancel} /> : null}
        {item.kind === "confirm" ? <Choices choices={CONFIRM_CHOICES} onAnswer={onAnswer} /> : null}
        {item.kind === "input" || item.kind === "editor" ? <TextAnswer item={item} multiline={item.kind === "editor"} onAnswer={onAnswer} /> : null}
        {item.kind === "ask" ? <AskAnswer item={item} onAnswer={onAnswer} /> : null}
      </Shell>
    </div>
  );
}

/** Only the oldest request is answerable; Pi waits for one answer at a time. */
export function PendingPanel({ sessionId, pending }: { sessionId: string; pending: PendingRequest[] }) {
  const [current] = pending;
  if (!current) return null;
  return <PendingCard key={current.id} sessionId={sessionId} item={current} extra={pending.length - 1} />;
}
