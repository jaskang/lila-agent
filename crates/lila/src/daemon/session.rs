//! Sessions: a member taking work from another chain, and the PM starting afresh from a digest.

use super::*;

impl<T: Terminals> Daemon<T> {
    /// How `mid` (its session on another chain, or in another folder) takes
    /// work from `chain` in `dir`: back to the session that did this chain
    /// before (in that folder), the same session while its context is small
    /// (`fresh_after_tokens`) and it stays in its folder, else a new one.
    pub(super) fn switch_plan(&mut self, pid: &str, mid: &str, live: &Live, chain: &str, dir: &Path, moved: bool) -> Switch {
        let dir_s = dir.to_string_lossy();
        if let Ok(Some(s)) = self.store.session_for_chain(pid, mid, chain)
            && live.session.as_ref() != Some(&s.id)
            && s.file.as_deref().is_some_and(|f| Path::new(f).exists() && (!moved || sessions::recorded_in(f, &s.runtime, &dir_s)))
        {
            return Switch::Resume(s.id);
        }
        if moved {
            return Switch::Fresh;
        }
        let limit = self.projects[pid].team.settings.fresh_after_tokens;
        match self.context_of(pid, live.session.as_deref()) {
            Some(n) if limit > 0 && n < limit => Switch::Keep,
            _ => Switch::Fresh,
        }
    }

    /// The context of a member's session's latest turn, in tokens, when its record is readable.
    pub(super) fn context_of(&mut self, pid: &str, session: Option<&str>) -> Option<u64> {
        let file = self.store.session(pid, session?).ok()??.file?;
        self.contexts.of(Path::new(&file))
    }

    /// Starts a free member's CLI again for `tid` (from another chain): in the
    /// session that did that chain before, or an empty one. It holds no open task,
    /// so nothing goes back to the queue; `tid` goes out once the session reports idle.
    pub(super) fn new_session(&mut self, pid: &str, mid: &str, tid: &str, plan: Switch, now: i64) {
        let Some(old) = self.projects[pid].live[mid].terminal.clone() else { return };
        if let Err(e) = self.term.kill(&old) {
            self.say(format!("{pid}: kill {mid} ({old}): {e:#}"));
        }
        self.member_gone(pid, mid, &format!("new session for {tid}"), now, false);
        let (session, kind, what) = match plan {
            Switch::Resume(id) => (runtime::Session::Resume(id.clone()), "resumed_session", format!("{mid} goes back to session {id} for {tid}")),
            _ => (runtime::Session::New(None), "fresh_session", format!("{mid} starts a new session for {tid}")),
        };
        let _ = self.store.event(pid, Some(tid), "lilad", kind, &what, now);
        let chain = self.store.ancestors(pid, tid).unwrap_or_default().last().map(|t| t.id.clone()).unwrap_or_else(|| tid.to_owned());
        let dir = self.member_dir(pid, mid, Some(&chain));
        if let Err(e) = self.start_session(pid, mid, session, &dir, now) {
            self.say(format!("{pid}: {mid} could not be started again for {tid}: {e:#}"));
            let _ = self.store.add_inbox(pid, "lost", Some(mid), None, &inbox::restart_failed(mid, &format!("{e:#}")), &[], now);
        }
    }

    /// The PM in a new session: its context grew past `pm_refresh_tokens`. The
    /// digest is what it needs to carry on; the details stay a `lila task show` away.
    pub(super) fn refresh_pm(&mut self, pid: &str, tokens: u64, now: i64) {
        let owner = self.projects[pid].team.owner.clone();
        let Some(old) = self.projects[pid].live[&owner].terminal.clone() else { return };
        let digest = match self.digest(pid) {
            Ok(d) => d,
            Err(e) => return self.say(format!("{pid}: no digest for the PM: {e:#}")),
        };
        if let Err(e) = self.term.kill(&old) {
            self.say(format!("{pid}: kill {owner} ({old}): {e:#}"));
        }
        self.member_gone(pid, &owner, "new session", now, false);
        let detail = format!("its context reached {} tokens; it starts again with a digest of the project", crate::usage::short(tokens));
        let _ = self.store.event(pid, None, "lilad", "pm_refreshed", &detail, now);
        self.say(format!("{pid}: {owner}: {detail}"));
        if let Err(e) = self.start_member(pid, &owner, now) {
            let _ = self.store.add_inbox(pid, "lost", Some(&owner), None, &inbox::restart_failed(&owner, &format!("{e:#}")), &[], now);
            return;
        }
        let _ = self.store.push_outbox(pid, &owner, &digest, now);
    }

    /// What the PM needs in a new session, from the store: no model summarizes it,
    /// so nothing the person decided is lost on the way.
    pub(super) fn digest(&self, pid: &str) -> Result<String> {
        let tasks: Vec<Task> = self.store.tasks(pid)?.into_iter().map(|(t, _)| t).collect();
        let runs: Vec<Run> = self.store.runs(pid)?.into_iter().filter(|r| r.state.is_open()).collect();
        let team = &self.projects[pid].team;
        let decided: Vec<String> = self
            .store
            .inbox(pid, true)?
            .into_iter()
            .filter(|i| i.kind == "question" && i.answer.is_some())
            .map(|i| format!("{} {} → {}", i.id, i.text.trim(), i.answer.unwrap_or_default().trim()))
            .collect();
        let under = |root: &str, id: &str| id == root || route::ancestors(&tasks, id).iter().any(|a| a.id == root);
        let mut open = Vec::new();
        let mut closed = Vec::new();
        for r in tasks.iter().filter(|t| t.parent.is_none()) {
            let below = route::open_under(&tasks, &r.id);
            let flows: Vec<&Run> = runs.iter().filter(|x| under(&r.id, &x.anchor)).collect();
            if r.state.is_open() || !below.is_empty() || !flows.is_empty() {
                let mut line = format!("{} from {} → {} [{}] \"{}\"", r.id, r.from, r.to, r.state, r.title);
                for t in &below {
                    line.push_str(&format!("\n    {} → {} [{}] {}", t.id, t.to, t.state, t.title));
                }
                for x in flows {
                    let step = team.flows.get(&x.flow).and_then(|f| f.steps.get(x.step)).map(|s| s.action.as_str()).unwrap_or("?");
                    line.push_str(&format!("\n    {} flow of {}: {step} ({}, fix round {})", x.flow, x.anchor, x.state.as_str(), x.round));
                }
                open.push(line);
            } else if r.state == TaskState::Done {
                closed.push(format!("{} \"{}\": {}", r.id, r.title, r.summary.as_deref().unwrap_or("").chars().take(160).collect::<String>()));
            }
        }
        let waiting = self.store.inbox(pid, false)?.len();
        Ok(message::digest(&decided, &open, &closed[closed.len().saturating_sub(8)..], waiting))
    }
}
