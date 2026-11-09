//! Flows: one step ends, lilad starts the next (a member's turn or the check command).

use super::*;

impl<T: Terminals> Daemon<T> {
    /// Step `at` of `anchor`'s run ended (`passed`: its verdict or check). Starts what
    /// comes next (`flow::after`). `last`: the newest step task, which the next hangs
    /// under. `failure`: what goes back to the member on a fail.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn advance(&mut self, pid: &str, anchor: &str, at: usize, passed: bool, last: &Task, failure: &str, now: i64) -> Result<()> {
        let Some(run) = self.store.run(pid, anchor)? else { return Ok(()) };
        let anchor_task = self.store.task(pid, anchor)?.context("a flow run without its task")?;
        let (f, max) = {
            let team = &self.projects[pid].team;
            (team.flows.get(&run.flow).filter(|f| at < f.steps.len()).cloned(), team.settings.max_rounds)
        };
        let Some(f) = f else {
            self.store.set_run(pid, anchor, at, run.round, RunState::Done, now)?;
            self.store.event(pid, Some(anchor), LILAD, "flow_done", &format!("the {} flow is gone from the project: stopped here", run.flow), now)?;
            let files = self.store.artifacts(pid, &last.id)?;
            return self.report_done(pid, last, &files, now);
        };
        let check = self.check_cmd(pid);
        let next = flow::after(&f, at, passed, run.round, max, check.is_some());
        if !matches!(next, Next::Finished | Next::GiveUp)
            && let Some(budget) = run.budget
        {
            let spent = self.run_tokens(pid, &run, now);
            if spent >= budget {
                return self.hold_run(pid, &run, &anchor_task, (at, passed, &last.id, failure), spent, budget, now);
            }
        }
        match next {
            Next::Finished => {
                self.store.set_run(pid, anchor, at, run.round, RunState::Done, now)?;
                self.store.event(pid, Some(anchor), LILAD, "flow_done", &format!("{} flow passed", run.flow), now)?;
                self.finish_run(pid, &anchor_task, &run.flow, last, now)?;
            }
            Next::GiveUp => {
                self.store.set_run(pid, anchor, at, run.round, RunState::Failed, now)?;
                self.store.event(pid, Some(anchor), LILAD, "flow_failed", &format!("still failing after {} rounds", run.round), now)?;
                let text = message::gave_up(&anchor_task, &run.flow, run.round, failure);
                let person = inbox::gave_up(&anchor_task, run.round, failure);
                self.tell_lead(pid, &anchor_task, &text, &person, now)?;
            }
            Next::Check { step } => {
                self.store.set_run(pid, anchor, step, run.round, RunState::Checking, now)?;
                self.store.event(pid, Some(anchor), LILAD, "check", &format!("running `{}`", check.as_deref().unwrap_or("")), now)?;
                self.queue_check(pid, anchor);
            }
            Next::Member { step } => {
                let st = f.steps[step].clone();
                let to = self.resolve_to(pid, &Actor::Member(LILAD.into()), st.by.as_deref().unwrap_or_default(), Some(&last.id))?;
                let files = self.store.artifacts(pid, &last.id)?;
                let ran = if f.steps[at].is_check() { check.as_deref() } else { None };
                let title = if st.wants_verdict() { format!("Verify: {}", anchor_task.title) } else { format!("{}: {}", capitalized(&st.action), anchor_task.title) };
                let body = message::step_body(last, &files, ran, st.wants_verdict());
                self.store.set_run(pid, anchor, step, run.round, RunState::Running, now)?;
                self.create_step(pid, anchor, step, &to, &last.id, title, body, now)?;
            }
            Next::Back { step, round } => {
                // To whoever did that step last: its session may still hold the work.
                let to = self
                    .store
                    .tasks(pid)?
                    .into_iter()
                    .rev()
                    .map(|(t, _)| t)
                    .find(|t| t.run.as_ref().is_some_and(|(a, s)| a == anchor && *s == step))
                    .map(|t| t.to)
                    .context("nobody did the step to send the work back to")?;
                self.store.set_run(pid, anchor, step, round, RunState::Running, now)?;
                self.store.event(pid, Some(anchor), LILAD, "sent_back", &format!("round {round} of {max}, to {to}"), now)?;
                self.create_step(pid, anchor, step, &to, &last.id, format!("Fix: {}", anchor_task.title), failure.to_owned(), now)?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn create_step(&mut self, pid: &str, anchor: &str, step: usize, to: &str, parent: &str, title: String, body: String, now: i64) -> Result<Task> {
        let new = NewTask {
            title,
            body,
            from: Actor::Member(LILAD.into()),
            to: to.to_owned(),
            parent: Some(parent.to_owned()),
            priority: 0,
            brief: Brief::default(),
            run: Some((anchor.to_owned(), step)),
        };
        self.create_task(pid, new, now)
    }

    /// Every step passed: whoever started the run hears once, like for any finished chain.
    pub(super) fn finish_run(&mut self, pid: &str, anchor: &Task, flow_name: &str, last: &Task, now: i64) -> Result<()> {
        if anchor.from == Actor::Human {
            let _ = flow_name;
            return self.add_review(pid, anchor, now);
        }
        let files = self.store.artifacts(pid, &last.id)?;
        self.report_done(pid, last, &files, now)
    }

    /// News about a run for whoever started it (`person`: what the person reads), and the PM.
    pub(super) fn tell_lead(&mut self, pid: &str, anchor: &Task, text: &str, person: &str, now: i64) -> Result<()> {
        let owner = self.projects[pid].team.owner.clone();
        if anchor.from == Actor::Human {
            self.store.add_inbox(pid, "report", Some(&anchor.to), Some(&anchor.id), person, &[], now)?;
        } else if anchor.from.as_str() != owner {
            self.store.push_outbox(pid, anchor.from.as_str(), text, now)?;
        }
        self.store.push_outbox(pid, &owner, text, now)?;
        Ok(())
    }

    pub(super) fn queue_check(&mut self, pid: &str, anchor: &str) {
        let Some(cmd) = self.check_cmd(pid) else { return };
        if !self.checking.insert((pid.to_owned(), anchor.to_owned())) {
            return;
        }
        // In the chain's worktree, where its work is.
        let chain = self.store.ancestors(pid, anchor).unwrap_or_default().last().map(|t| t.id.clone()).unwrap_or_else(|| anchor.to_owned());
        let cwd = self.chain_dir(pid, &chain);
        let p = &self.projects[pid];
        self.checks.push(CheckJob {
            project: pid.to_owned(),
            anchor: anchor.to_owned(),
            cwd,
            cmd,
            timeout: std::time::Duration::from_secs(p.team.settings.check_timeout_secs.max(1)),
            path_env: self.path_env.clone(),
        });
    }

    /// Checks to start now; each result comes back through `on_check_done`.
    pub fn take_checks(&mut self) -> Vec<CheckJob> {
        std::mem::take(&mut self.checks)
    }

    /// A check finished: on to the next step, or the work goes back.
    pub fn on_check_done(&mut self, pid: &str, anchor: &str, r: &CheckResult, now: i64) {
        self.checking.remove(&(pid.to_owned(), anchor.to_owned()));
        if !self.projects.contains_key(pid) {
            return;
        }
        let Ok(Some(run)) = self.store.run(pid, anchor) else { return };
        if run.state != RunState::Checking {
            return;
        }
        let detail = match r.exit {
            Some(0) => "passed".to_owned(),
            Some(c) => format!("failed (exit {c})"),
            None => "did not finish".to_owned(),
        };
        let _ = self.store.event(pid, Some(anchor), LILAD, "check", &detail, now);
        self.say(format!("{pid}: check for {anchor} {detail}"));
        let tasks = self.store.tasks(pid).unwrap_or_default();
        let Some(last) = tasks.into_iter().rev().map(|(t, _)| t).find(|t| t.run.as_ref().is_some_and(|(a, _)| a == anchor)) else { return };
        let failure = message::fix_from_check(&self.check_cmd(pid).unwrap_or_default(), r.exit, &r.tail);
        if let Err(e) = self.advance(pid, anchor, run.step, r.passed(), &last, &failure, now) {
            self.say(format!("{pid}: after the check for {anchor}: {e:#}"));
        }
        self.pump(pid, now);
    }
}
