//! Questions (docs/architecture-v4.md §6): what an agent asks the person, which is not a permission — omp's `ask` tool,
//! or anything else that opens a dialog. A question is a card on the thread, like a permission card: the thread waits for
//! the person's answer (`thread.answer`), which the host hands to omp.

use super::*;
use lila_agent::{Command, Question, QuestionKind, Reply};

/// An open question card.
#[derive(Debug, Clone)]
pub(super) struct Asked {
    /// lilad's id for the card; the window answers with it.
    pub id: String,
    pub thread: String,
    pub question: Question,
    pub at_ms: i64,
}

pub(super) fn card_json(a: &Asked) -> Value {
    let q = &a.question;
    json!({
        "id": a.id, "thread": a.thread, "ask": "question", "question": q.id, "kind": q.kind, "title": q.title, "detail": q.detail,
        "options": q.options, "prefill": q.prefill, "secret": q.secret, "at_ms": a.at_ms,
    })
}

impl<T: Terminals> Daemon<T> {
    /// A question of thread `tid`: a card for the person.
    pub(super) fn open_question(&mut self, tid: &str, question: Question, now: i64) {
        // Asked again (lilad came back, the host repeated it): the card already open.
        if let Some(a) = self.questions.iter_mut().find(|a| a.thread == tid && a.question.id == question.id) {
            a.question = question;
        } else {
            self.next_card += 1;
            let first = question.title.lines().next().unwrap_or_default().to_owned();
            let _ = self.store.event(tid, "lilad", "asked", &format!("question · {first}"), now);
            let card = Asked { id: format!("q{}", self.next_card), thread: tid.to_owned(), question, at_ms: now };
            self.questions.push(card);
        }
        self.mark(tid);
    }

    /// omp took a question back (it timed out, or went away with its turn).
    pub(super) fn question_resolved(&mut self, tid: &str, question: &str) {
        let before = self.questions.len();
        self.questions.retain(|a| !(a.thread == tid && a.question.id == question));
        if self.questions.len() != before {
            self.mark(tid);
        }
    }

    /// The person's answer to a question card: `thread.answer {thread, card, value | yes | cancel}`. A choice is given by
    /// its words or by its number in the card; a yes or no by `yes`, or in words.
    pub(super) fn answer_question(&mut self, t: &Thread, params: &Value, now: i64) -> Result<Value> {
        let at = self.questions.iter().position(|a| a.thread == t.id && s(params, "card") == Some(a.id.as_str())).ok_or_else(|| anyhow!("no open question {} on {}", s(params, "card").unwrap_or("?"), t.id))?;
        let q = self.questions[at].question.clone();
        let value = s(params, "value");
        let reply = if params["cancel"].as_bool() == Some(true) {
            Reply::Cancel
        } else {
            match q.kind {
                QuestionKind::Choice => {
                    let value = value.ok_or_else(|| anyhow!("give value: one of {:?}", q.options))?;
                    let picked = q.options.iter().find(|o| *o == value).or_else(|| value.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).and_then(|n| q.options.get(n)));
                    Reply::Choice { value: picked.ok_or_else(|| anyhow!("{value:?} is not one of the choices: {:?}", q.options))?.clone() }
                }
                QuestionKind::Confirm => {
                    let yes = match (params["yes"].as_bool(), value.map(str::to_ascii_lowercase).as_deref()) {
                        (Some(yes), _) => yes,
                        (None, Some("yes" | "y" | "true" | "是" | "确认")) => true,
                        (None, Some("no" | "n" | "false" | "否" | "取消")) => false,
                        _ => bail!("give yes or no"),
                    };
                    Reply::Confirm { yes }
                }
                QuestionKind::Text => Reply::Text { value: value.ok_or_else(|| anyhow!("give value: the answer, or cancel"))?.to_owned() },
            }
        };
        self.host_command(&t.id, &Command::Reply { question: q.id.clone(), reply: reply.clone() })?;
        let card = self.questions.remove(at);
        let said = match &reply {
            Reply::Choice { value } | Reply::Text { value } => value.clone(),
            Reply::Confirm { yes } => if *yes { "yes" } else { "no" }.to_owned(),
            Reply::Cancel => "cancelled".to_owned(),
        };
        // A secret does not go in the log.
        let said = if q.secret && matches!(reply, Reply::Text { .. }) { "…".to_owned() } else { said };
        self.store.event(&t.id, "human", "answered", &format!("{} · {said}", card.question.title.lines().next().unwrap_or_default()), now)?;
        self.mark(&t.id);
        Ok(json!({"thread": t.id, "card": card.id, "answered": true}))
    }
}
