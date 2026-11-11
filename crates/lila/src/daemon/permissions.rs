//! Permission requests (architecture v3 D4, tiers in docs/product-v4.md §6). Every engine's
//! request is a `PermissionRequest`; lilad answers it by the tier of the thread (`policy`), and what is
//! left becomes a card on the thread for the person.

use super::*;
use crate::policy::{self, Danger, Verdict};
use crate::rules::{self, Rule};
use lila_agent::{Command, PermKind, PermOption, PermissionRequest};

/// The id of the option on a card that allows "always" (`rules`): Lila's own, whatever the engine offers.
pub const ALWAYS: &str = "always";

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
    /// A Claude Code hook waits for this card and asks after it every half second; when it was last heard from.
    /// `None` for the hosted engines' cards, which the engine itself takes back.
    pub polled_ms: Option<i64>,
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
        match verdict {
            Verdict::Allow => {
                if let Some(o) = request.pick(true) {
                    self.auto_event(tid, t.tier, "auto_allowed", &request, now)?;
                    return Ok(Decided::Answer { option: o.id.clone(), message: None });
                }
                self.open_card(tid, request, "没有可用的自动回答".to_owned(), None, now)
            }
            Verdict::Deny(message) => {
                if let Some(o) = request.pick(false) {
                    self.auto_event(tid, t.tier, "auto_denied", &request, now)?;
                    return Ok(Decided::Answer { option: o.id.clone(), message: Some(message) });
                }
                self.open_card(tid, request, "没有可用的自动回答".to_owned(), None, now)
            }
            Verdict::Ask { why, danger } => {
                // What the tier itself asks about may have been allowed "always" for this thread; a danger never is.
                if danger.is_none()
                    && t.tier == Tier::Confirm
                    && let Some(by) = self.rules_covering(&t, &request)
                    && let Some(o) = request.pick(true)
                {
                    self.rule_event(tid, t.tier, &request, &by, now)?;
                    return Ok(Decided::Answer { option: o.id.clone(), message: None });
                }
                self.open_card(tid, request, why, danger, now)
            }
        }
    }

    /// The thread's rules, as `rules` knows them.
    pub(super) fn rules_of(&self, tid: &str) -> Vec<Rule> {
        self.store.rules(tid).unwrap_or_default().into_iter().filter_map(|r| rules::Kind::parse(&r.kind).map(|kind| Rule { kind, key: r.key })).collect()
    }

    /// `thread.read`'s and `thread.show`'s `rules`: `[{id, kind, key, label, at_ms}]`.
    pub(super) fn rules_json(&self, tid: &str) -> Value {
        Value::Array(
            self.store
                .rules(tid)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|r| {
                    let rule = Rule { kind: rules::Kind::parse(&r.kind)?, key: r.key.clone() };
                    Some(json!({"id": r.id, "kind": r.kind, "key": r.key, "label": rule.text(), "at_ms": r.created_ms}))
                })
                .collect(),
        )
    }

    /// The rules of the thread that answer `request`, if it is asked only because the tier asks (`confirm`) and they do.
    pub(super) fn rules_covering(&self, t: &Thread, request: &PermissionRequest) -> Option<Vec<Rule>> {
        if t.tier != Tier::Confirm {
            return None;
        }
        rules::covers(Path::new(&t.cwd), request, &self.rules_of(&t.id))
    }

    /// Logs an answer a rule gave, naming the rule.
    pub(super) fn rule_event(&self, tid: &str, tier: Tier, request: &PermissionRequest, by: &[Rule], now: i64) -> Result<()> {
        let names: Vec<String> = by.iter().map(Rule::text).collect();
        self.store.event(tid, "lilad", "auto_allowed", &format!("{} · {} · rule: {}", tier.as_str(), what(request), names.join(", ")), now)
    }

    /// The card's options as the person sees them: the engines' own "always" answers (Codex's for the session, ACP's) go,
    /// because what they cover is the engine's to say and Lila cannot show it, nor take it back; in their place, when
    /// the tier itself is asking and a rule can say exactly what would be allowed, Lila's own (`ALWAYS`), whose label says it.
    pub(super) fn with_always(&self, t: &Thread, mut request: PermissionRequest, danger: Option<Danger>) -> PermissionRequest {
        let has = |request: &PermissionRequest, kind: PermKind| request.options.iter().any(|o| o.kind == kind);
        if has(&request, PermKind::Allow) {
            request.options.retain(|o| o.kind != PermKind::AllowAlways);
        }
        if has(&request, PermKind::Reject) {
            request.options.retain(|o| o.kind != PermKind::RejectAlways);
        }
        request.options.retain(|o| o.id != ALWAYS);
        if t.tier == Tier::Confirm
            && danger.is_none()
            && let Some(at) = request.options.iter().position(|o| o.kind == PermKind::Allow)
            && let Some(new) = rules::derive(Path::new(&t.cwd), &request, &self.rules_of(&t.id))
        {
            request.options.insert(at + 1, PermOption { id: ALWAYS.into(), label: rules::label(&new), kind: PermKind::AllowAlways });
        }
        request
    }

    /// What a thread's tier answered by itself, for `thread.log`.
    pub(super) fn auto_event(&self, tid: &str, tier: Tier, kind: &str, request: &PermissionRequest, now: i64) -> Result<()> {
        self.store.event(tid, "lilad", kind, &format!("{} · {}", tier.as_str(), what(request)), now)
    }

    /// Opens a card on the thread for the person to answer.
    pub(super) fn open_card(&mut self, tid: &str, request: PermissionRequest, why: String, danger: Option<Danger>, now: i64) -> Result<Decided> {
        let t = self.store.thread(tid)?.ok_or_else(|| anyhow!("no thread {tid}"))?;
        let request = self.with_always(&t, request, danger);
        // Asked again (lilad came back, the host repeated it): the card already open.
        if let Some(a) = self.asking.iter_mut().find(|a| a.thread == tid && a.request.id == request.id) {
            a.request = request;
        } else {
            self.next_card += 1;
            self.store.event(tid, "lilad", "asked", &format!("{} · {why}", what(&request)), now)?;
            let card = Asking { id: format!("p{}", self.next_card), thread: tid.to_owned(), request, why, danger, at_ms: now, polled_ms: None };
            self.asking.push(card);
        }
        self.mark(tid);
        Ok(Decided::Card)
    }

    /// The person's answer to a permission card: `thread.answer {thread, card | permission, option | allow, dismiss?}`.
    /// A host gets it as a command; a Claude Code hook picks it up (`hook.answer`). `dismiss` closes the card
    /// without an answer, which leaves a Claude Code question to its terminal. The option `always` adds the rules the card
    /// said (`with_always`) to the thread and answers with the engine's plain allow; the thread's other cards that those
    /// rules answer too are answered with it.
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
        let mut picked = match (s(params, "option"), params["allow"].as_bool()) {
            _ if dismiss => None,
            (Some(o), _) => Some(card.request.options.iter().find(|x| x.id == o || x.label == o).ok_or_else(|| anyhow!("{o:?} is not one of the options"))?.id.clone()),
            (None, Some(allow)) => Some(card.request.pick(allow).ok_or_else(|| anyhow!("the request has no such option"))?.id.clone()),
            (None, None) => bail!("give option, allow or dismiss"),
        };
        let mut note = None;
        if picked.as_deref() == Some(ALWAYS) {
            // Asked again now, as the tier or the rules may have changed since the card was shown.
            let new = if card.danger.is_none() && t.tier == Tier::Confirm { rules::derive(Path::new(&t.cwd), &card.request, &self.rules_of(&t.id)) } else { None };
            let Some(new) = new else { bail!("\"always\" is not offered for this any more; allow it once") };
            for r in &new {
                self.store.add_rule(&t.id, r.kind.as_str(), &r.key, now)?;
            }
            self.store.event(&t.id, "human", "rule_added", &new.iter().map(Rule::text).collect::<Vec<_>>().join(", "), now)?;
            picked = card.request.options.iter().find(|o| o.kind == PermKind::Allow).map(|o| o.id.clone());
            note = Some("always");
        }
        self.deliver_answer(&t, &card, picked, dismiss, note, now)?;
        if note.is_some() {
            self.answer_covered(&t.id, now);
        }
        Ok(json!({"thread": t.id, "card": card.id, "answered": true}))
    }

    /// Hands the person's answer to the engine (a host as a command, a hook through `hook.answer`) and closes the card.
    fn deliver_answer(&mut self, t: &Thread, card: &Asking, picked: Option<String>, dismiss: bool, note: Option<&str>, now: i64) -> Result<()> {
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
        self.asking.retain(|a| a.id != card.id);
        let detail = format!("{} · {}{}", card.request.title.lines().next().unwrap_or_default(), picked.as_deref().unwrap_or("left to the terminal"), note.map(|n| format!(" ({n})")).unwrap_or_default());
        self.store.event(&t.id, "human", if dismiss { "dismissed" } else { "answered" }, &detail, now)?;
        self.mark(&t.id);
        Ok(())
    }

    /// The thread's open cards that its rules answer, answered: the person clicked "always" on one of several asked at once.
    fn answer_covered(&mut self, tid: &str, now: i64) {
        let Ok(Some(t)) = self.store.thread(tid) else { return };
        for card in self.asking.iter().filter(|a| a.thread == tid && a.danger.is_none()).cloned().collect::<Vec<_>>() {
            let Some(by) = self.rules_covering(&t, &card.request) else { continue };
            let Some(option) = card.request.options.iter().find(|o| o.kind == PermKind::Allow).map(|o| o.id.clone()) else { continue };
            match self.deliver_answer(&t, &card, Some(option), false, None, now) {
                Ok(()) => {
                    let _ = self.rule_event(tid, t.tier, &card.request, &by, now);
                }
                Err(e) => self.say(format!("{tid}: could not answer a card by its rules: {e:#}")),
            }
        }
    }

    /// `thread.rule.remove {thread, rule}`: the person takes a rule back; what it allowed is asked about again.
    pub(super) fn thread_rule_remove(&mut self, params: &Value, now: i64) -> Result<Value> {
        let t = self.thread_of(params)?;
        let id = params["rule"].as_i64().ok_or_else(|| anyhow!("give rule: a number from the thread's rules"))?;
        let row = self.store.remove_rule(&t.id, id)?.ok_or_else(|| anyhow!("{} has no rule {id}", t.id))?;
        self.store.event(&t.id, "human", "rule_removed", &row.key, now)?;
        self.mark(&t.id);
        Ok(json!({"thread": t.id, "rule": id, "removed": true, "rules": self.rules_json(&t.id)}))
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

/// "Bash: cargo test": the tool and what it does, on one line.
fn what(request: &PermissionRequest) -> String {
    format!("{}: {}", request.name_or_tool(), request.title.lines().next().unwrap_or_default())
}

trait Named {
    fn name_or_tool(&self) -> String;
}

impl Named for PermissionRequest {
    fn name_or_tool(&self) -> String {
        if self.name.is_empty() { serde_json::to_value(self.tool).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default() } else { self.name.clone() }
    }
}
