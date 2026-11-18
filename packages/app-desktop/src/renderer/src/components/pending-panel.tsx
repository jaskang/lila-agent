import * as React from "react";
import { MessageCircleQuestion, ShieldAlert } from "lucide-react";
import type { PendingAnswer } from "@lila/protocol";
import { Button } from "@/components/ui/button";
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
import { actions, type OpenPending } from "@/lib/store";

/** Pi 的审批选项是英文，界面显示中文但回传原始文本。 */
export function optionLabel(option: string): string {
  if (option === "Approve") return "允许";
  if (option === "Deny") return "拒绝";
  const session = /^Approve (.+) for this session$/.exec(option);
  if (session) return `本会话内始终允许 ${session[1]}`;
  if (option === "Other (type your own)") return "其他（自己填写）";
  return option;
}

function Shell({ icon, title, message, children }: { icon: React.ReactNode; title: string; message?: string; children: React.ReactNode }) {
  return (
    <section
      aria-live="polite"
      className="rounded-xl border border-amber-500/40 bg-amber-50/80 p-4 shadow-sm dark:bg-amber-950/30"
    >
      <div className="flex items-start gap-3">
        <span className="mt-0.5 text-amber-600 dark:text-amber-400">{icon}</span>
        <div className="flex min-w-0 flex-1 flex-col gap-3">
          <div className="flex min-w-0 flex-col gap-1">
            <p className="text-sm font-medium">{title}</p>
            {message ? (
              <pre className="max-h-40 overflow-auto rounded-md bg-background/70 p-2 font-mono text-xs whitespace-pre-wrap">{message}</pre>
            ) : null}
          </div>
          {children}
        </div>
      </div>
    </section>
  );
}

function Choices({ item, onAnswer }: { item: OpenPending; onAnswer: (answer: PendingAnswer) => void }) {
  const first = React.useRef<HTMLButtonElement>(null);
  React.useEffect(() => first.current?.focus(), []);
  React.useEffect(() => {
    // Number keys pick an option while focus is not in a text field.
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (target?.closest("textarea, input")) return;
      const index = Number(event.key) - 1;
      const option = item.options[index];
      if (option !== undefined && !event.metaKey && !event.ctrlKey) {
        event.preventDefault();
        onAnswer({ type: "option", value: option });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [item.options, onAnswer]);
  return (
    <div className="flex flex-wrap gap-2">
      {item.options.map((option, index) => (
        <Button
          key={option}
          ref={index === 0 ? first : undefined}
          size="sm"
          variant={option === "Deny" ? "outline" : index === 0 ? "default" : "secondary"}
          onClick={() => onAnswer({ type: "option", value: option })}
        >
          {optionLabel(option)}
          {index < 9 ? <Kbd className="ms-1">{index + 1}</Kbd> : null}
        </Button>
      ))}
    </div>
  );
}

function TextAnswer({ item, onAnswer }: { item: OpenPending; onAnswer: (answer: PendingAnswer) => void }) {
  const [value, setValue] = React.useState(item.defaultValue ?? "");
  return (
    <form
      className="flex flex-col gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        onAnswer({ type: "text", value });
      }}
    >
      <Textarea
        autoFocus
        value={value}
        placeholder={item.placeholder}
        onChange={(event) => setValue(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) event.currentTarget.form?.requestSubmit();
        }}
        className="bg-background"
      />
      <div className="flex gap-2">
        <Button type="submit" size="sm">
          提交 <Kbd className="ms-1">⌘↵</Kbd>
        </Button>
        <Button type="button" size="sm" variant="ghost" onClick={() => onAnswer({ type: "cancel" })}>
          跳过
        </Button>
      </div>
    </form>
  );
}

function AskAnswer({ item, onAnswer }: { item: OpenPending; onAnswer: (answer: PendingAnswer) => void }) {
  return (
    <Questionnaire
      shortcuts="numbers"
      onSubmit={(event) => {
        event.preventDefault();
        const data = new FormData(event.currentTarget);
        onAnswer({
          type: "ask",
          answers: item.questions.map((question) => {
            const values = data.getAll(question.id).map(String).filter(Boolean);
            const selectedOptions = values.filter((value) => question.options.includes(value));
            const customInput = values.find((value) => !question.options.includes(value));
            return { id: question.id, selectedOptions, ...(customInput ? { customInput } : {}) };
          }),
        });
      }}
    >
      {item.questions.length > 1 ? <QuestionnaireProgress /> : null}
      {item.questions.map((question) => (
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

function PendingCard({ sessionId, item }: { sessionId: string; item: OpenPending }) {
  const [busy, setBusy] = React.useState(false);
  const onAnswer = React.useCallback(
    (answer: PendingAnswer) => {
      if (busy) return;
      setBusy(true);
      void actions.respond(sessionId, item.pendingId, answer).finally(() => setBusy(false));
    },
    [busy, sessionId, item.pendingId],
  );
  const approval = item.kind === "approval";
  const icon = approval ? <ShieldAlert className="size-4" /> : <MessageCircleQuestion className="size-4" />;
  const title = approval ? `Agent 想要运行 ${item.title.replace(/^Allow tool:\s*/, "")}` : item.title || "Agent 有个问题";
  return (
    <div className={busy ? "pointer-events-none opacity-60" : undefined}>
      <Shell icon={icon} title={title} {...(item.message ? { message: item.message } : {})}>
        {item.kind === "approval" || item.kind === "select" ? <Choices item={item} onAnswer={onAnswer} /> : null}
        {item.kind === "confirm" ? (
          <div className="flex gap-2">
            <Button size="sm" autoFocus onClick={() => onAnswer({ type: "confirm", confirmed: true })}>
              确认
            </Button>
            <Button size="sm" variant="outline" onClick={() => onAnswer({ type: "confirm", confirmed: false })}>
              取消
            </Button>
          </div>
        ) : null}
        {item.kind === "input" ? <TextAnswer item={item} onAnswer={onAnswer} /> : null}
        {item.kind === "ask" ? <AskAnswer item={item} onAnswer={onAnswer} /> : null}
      </Shell>
    </div>
  );
}

/** 只允许处理最早的待处理项，Pi 一次等待一个界面回答。 */
export function PendingPanel({ sessionId, pending }: { sessionId: string; pending: OpenPending[] }) {
  const [current] = pending;
  if (!current) return null;
  return (
    <div className="flex flex-col gap-2">
      <PendingCard key={current.pendingId} sessionId={sessionId} item={current} />
      {pending.length > 1 ? <p className="px-1 text-xs text-muted-foreground">还有 {pending.length - 1} 个请求在排队</p> : null}
    </div>
  );
}
