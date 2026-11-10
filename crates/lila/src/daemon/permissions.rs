//! Permission requests (architecture v3 D4, tiers in docs/product-v4.md §6). Every engine's
//! request is a `PermissionRequest`; lilad answers it by the tier of the thread (`policy`), and what is
//! left becomes a card on the thread for the person.

use super::*;
use crate::policy::{self, Danger, Verdict};
use lila_agent::{Command, PermissionRequest};

/// An open permission card: what a thread asks the person.
#[derive(Debug, Clone)]
pub(super) struct Asking {
    /// lilad's id for the card; the window answers with it.
    pub id: String,
    pub thread: String,
    pub request: PermissionRequest,
    /// Why the person is asked ("改动前确认", "全自动档仍需确认：…").
    pub why: String,
    pub danger: Option<Danger>,
    pub at_ms: i64,
}

/// What became of a request.
pub(super) enum Decided {
    /// Answered at once: the option (the runtime's own id) and, for a refusal, what the agent is told.
    Answer { option: String, message: Option<String> },
    /// A card is open: the person decides (`thread.answer`).
    Card,
}

pub(super) fn card_json(a: &Asking) -> Value {
    json!({
        "id": a.id, "thread": a.thread, "request": a.request.id, "tool": a.request.tool, "name": a.request.name, "title": a.request.title,
        "detail": a.request.detail, "command": a.request.command, "paths": a.request.paths, "why": a.why, "danger": a.danger.map(Danger::as_str),
        "options": a.request.options, "at_ms": a.at_ms,
    })
}

impl<T: Terminals> Daemon<T> {
    /// Answers `request` of thread `tid` by its tier, or opens a card for the person.
    pub(super) fn ask_permission(&mut self, tid: &str, request: PermissionRequest, now: i64) -> Result<Decided> {
        let t = self.store.thread(tid)?.ok_or_else(|| anyhow!("no thread {tid}"))?;
        let verdict = policy::decide(t.tier, Path::new(&t.cwd), &request);
        let what = format!("{}: {}", request.name_or_tool(), request.title.lines().next().unwrap_or_default());
        let (why, danger) = match verdict {
            Verdict::Allow => {
                if let Some(o) = request.pick(true) {
                    self.store.event(tid, "lilad", "auto_allowed", &format!("{} · {what}", t.tier.as_str()), now)?;
                    return Ok(Decided::Answer { option: o.id.clone(), message: None });
                }
                ("没有可用的自动回答".to_owned(), None)
            }
            Verdict::Deny(message) => {
                if let Some(o) = request.pick(false) {
                    self.store.event(tid, "lilad", "auto_denied", &format!("{} · {what}", t.tier.as_str()), now)?;
                    return Ok(Decided::Answer { option: o.id.clone(), message: Some(message) });
                }
                ("没有可用的自动回答".to_owned(), None)
            }
            Verdict::Ask { why, danger } => (why, danger),
        };
        // Asked again (lilad came back, the host repeated it): the card already open.
        if let Some(a) = self.asking.iter_mut().find(|a| a.thread == tid && a.request.id == request.id) {
            a.request = request;
        } else {
            self.next_card += 1;
            let card = Asking { id: format!("p{}", self.next_card), thread: tid.to_owned(), request, why: why.clone(), danger, at_ms: now };
            self.store.event(tid, "lilad", "asked", &format!("{what} · {why}"), now)?;
            self.asking.push(card);
        }
        self.mark(tid);
        Ok(Decided::Card)
    }

    /// The person's answer to a permission card: `thread.answer {thread, card | permission, option | allow, dismiss?}`.
    /// A host gets it as a command; a Claude Code hook picks it up (`hook.answer`). `dismiss` closes the card
    /// without an answer, which leaves a Claude Code question to its terminal.
    pub(super) fn thread_answer(&mut self, params: &Value, now: i64) -> Result<Value> {
        let t = self.thread_of(params)?;
        let at = self
            .asking
            .iter()
            .position(|a| a.thread == t.id && (s(params, "card") == Some(a.id.as_str()) || s(params, "permission") == Some(a.request.id.as_str())))
            // Answered already (a second click, a second window, or the engine moved on): nothing to do.
            .ok_or_else(|| anyhow!("no open permission card {} on {}", s(params, "card").or(s(params, "permission")).unwrap_or("?"), t.id))?;
        let card = self.asking[at].clone();
        let dismiss = params["dismiss"].as_bool().unwrap_or(false);
        let picked = match (s(params, "option"), params["allow"].as_bool()) {
            _ if dismiss => None,
            (Some(o), _) => Some(card.request.options.iter().find(|x| x.id == o || x.label == o).ok_or_else(|| anyhow!("{o:?} is not one of the options"))?.id.clone()),
            (None, Some(allow)) => Some(card.request.pick(allow).ok_or_else(|| anyhow!("the request has no such option"))?.id.clone()),
            (None, None) => bail!("give option, allow or dismiss"),
        };
        if t.engine.hosted() {
            if let Some(option) = &picked {
                self.host_command(&t.id, &Command::Answer { permission: card.request.id.clone(), option: option.clone() })?;
            }
            // Dismissing a hosted request has no meaning: the engine waits for an answer.
            else {
                bail!("{} has no terminal to leave the question to: answer it", t.id);
            }
        } else {
            let allow = picked.as_ref().and_then(|o| card.request.options.iter().find(|x| &x.id == o)).is_some_and(|o| o.kind.allows());
            let message = (!allow && picked.is_some()).then(|| "The person said no in Lila.".to_owned());
            self.hook_answers.insert(card.request.id.clone(), picked.clone().map(|o| (o, message)));
        }
        self.asking.remove(at);
        let detail = format!("{} · {}", card.request.title.lines().next().unwrap_or_default(), picked.as_deref().unwrap_or("left to the terminal"));
        self.store.event(&t.id, "human", if dismiss { "dismissed" } else { "answered" }, &detail, now)?;
        self.mark(&t.id);
        Ok(json!({"thread": t.id, "card": card.id, "answered": true}))
    }

    /// The engine took a request back (answered in its terminal, or it went away with its turn).
    pub(super) fn request_resolved(&mut self, tid: &str, request: &str) {
        let before = self.asking.len();
        self.asking.retain(|a| !(a.thread == tid && a.request.id == request));
        if self.asking.len() != before {
            self.mark(tid);
        }
    }
}

trait Named {
    fn name_or_tool(&self) -> String;
}

impl Named for PermissionRequest {
    fn name_or_tool(&self) -> String {
        if self.name.is_empty() { serde_json::to_value(self.tool).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default() } else { self.name.clone() }
    }
}
