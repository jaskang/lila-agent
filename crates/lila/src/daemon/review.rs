//! The person's side: review cards, one chain in the folder at a time (without worktrees), the commit on accept, budgets.

use super::*;

impl<T: Terminals> Daemon<T> {
    // ---------------- the person's review ----------------

    /// Results waiting for the person to sign off.
    pub(super) fn unreviewed(&self, pid: &str) -> usize {
        self.store.inbox(pid, false).map(|v| v.iter().filter(|i| i.kind == "review").count()).unwrap_or(0)
    }

    /// New work the PM starts waits while `max_unreviewed` results wait for the
    /// person: more output than anyone checks is hoping, not reviewing. The
    /// person's own requests and work within a chain go on.
    pub(super) fn held_for_review(&self, pid: &str, task: &Task) -> bool {
        let max = self.projects[pid].team.settings.max_unreviewed as usize;
        max > 0 && task.parent.is_none() && task.from != Actor::Human && self.unreviewed(pid) >= max
    }

    // ---------------- one chain in the folder at a time ----------------

    /// Chains with work in the project folder: a member other than the PM has had
    /// work in them, and they are not signed off yet (work or a flow is open, a
    /// review card waits, or their commit is being made). By root task.
    pub(super) fn open_chains(&self, pid: &str, tasks: &[Task]) -> BTreeSet<String> {
        let owner = &self.projects[pid].team.owner;
        let root = |id: &str| route::ancestors(tasks, id).last().map(|t| t.id.clone()).unwrap_or_else(|| id.to_owned());
        let started: BTreeSet<String> = tasks.iter().filter(|t| t.to != *owner && t.state != TaskState::Queued).map(|t| root(&t.id)).collect();
        started.intersection(&self.busy_chains(pid, tasks)).cloned().collect()
    }

    /// Queued tasks that wait for the folder: members share one, so at most
    /// `max_chains` chains have work in it. Chains already in it go on; new ones
    /// come in the order their work was queued. The PM's own tasks never wait.
    pub(super) fn held_for_folder(&self, pid: &str, tasks: &[(Task, lila_store::TaskMeta)]) -> BTreeSet<String> {
        let team = &self.projects[pid].team;
        let max = team.settings.max_chains as usize;
        if max == 0 {
            return BTreeSet::new();
        }
        let plain: Vec<Task> = tasks.iter().map(|(t, _)| t.clone()).collect();
        let root = |id: &str| route::ancestors(&plain, id).last().map(|t| t.id.clone()).unwrap_or_else(|| id.to_owned());
        let mut admitted = self.open_chains(pid, &plain);
        let mut held = BTreeSet::new();
        let mut queued: Vec<&(Task, lila_store::TaskMeta)> = tasks.iter().filter(|(t, _)| t.state == TaskState::Queued && t.to != team.owner).collect();
        queued.sort_by_key(|(_, m)| m.seq);
        for (t, _) in queued {
            let chain = root(&t.id);
            if admitted.contains(&chain) {
                continue;
            }
            if admitted.len() < max && !self.held_for_review(pid, t) {
                admitted.insert(chain);
            } else {
                held.insert(t.id.clone());
            }
        }
        held
    }

    /// The person accepted `id`: its chain's changes become one commit (`commit_on_accept`).
    pub(super) fn queue_commit(&mut self, pid: &str, id: &str) {
        let p = &self.projects[pid];
        let tasks: Vec<Task> = self.store.tasks(pid).unwrap_or_default().into_iter().map(|(t, _)| t).collect();
        let Some(task) = tasks.iter().find(|t| t.id == id) else { return };
        // The chain's title and the person's words; what came of it from the accepted task (a rework's).
        let mut what = route::ancestors(&tasks, id).last().copied().unwrap_or(task).clone();
        what.brief = what.chain_brief();
        what.summary = task.summary.clone();
        what.outcome = task.outcome.clone();
        let chain = what.id.clone();
        // From a worktree it always goes: that is how the work reaches the project folder.
        let slot = self.slot_of(pid, &chain).map(|i| p.slots[i].path.clone());
        if slot.is_none() && !p.team.settings.commit_on_accept {
            return;
        }
        if !self.committing.insert((pid.to_owned(), chain.clone())) {
            return;
        }
        self.commit_task.insert((pid.to_owned(), chain.clone()), id.to_owned());
        let merge_into = slot.is_some().then(|| p.cwd.clone());
        let cwd = slot.unwrap_or_else(|| p.cwd.clone());
        self.commits.push(CommitJob { project: pid.to_owned(), chain, cwd, message: message::commit_message(&what), path_env: self.path_env.clone(), merge_into });
    }

    /// Commits to make now; each result comes back through `on_commit_done`.
    /// One per project at a time: two accepted chains merging at once would each
    /// rebase onto the branch before the other lands on it.
    pub fn take_commits(&mut self) -> Vec<CommitJob> {
        let mut out = Vec::new();
        let mut left = Vec::new();
        for job in std::mem::take(&mut self.commits) {
            if self.merging.insert(job.project.clone()) {
                out.push(job);
            } else {
                left.push(job);
            }
        }
        self.commits = left;
        out
    }

    /// A chain's commit is made (or not): the folder is free for the next chain.
    pub fn on_commit_done(&mut self, pid: &str, chain: &str, r: &CommitResult, now: i64) {
        self.committing.remove(&(pid.to_owned(), chain.to_owned()));
        self.merging.remove(pid);
        let task = self.commit_task.remove(&(pid.to_owned(), chain.to_owned())).unwrap_or_else(|| chain.to_owned());
        if !self.projects.contains_key(pid) {
            return;
        }
        let (kind, detail) = match r {
            CommitResult::Committed(sha) => ("committed", sha.clone()),
            CommitResult::Nothing => ("committed", format!("{chain} changed nothing to commit")),
            CommitResult::NotRepo => ("committed", "the project folder is not a git repository; nothing committed".to_owned()),
            CommitResult::Failed(why) => ("commit_failed", format!("{chain}'s changes could not be committed: {why}")),
            CommitResult::Conflict(files) => ("commit_failed", format!("{chain}'s commit conflicts with the project's branch in {}", files.join(", "))),
            CommitResult::NotMerged(why) => ("commit_failed", format!("{chain} is committed on its branch, but the project folder did not move to it: {why}")),
        };
        let _ = self.store.event(pid, Some(chain), LILAD, kind, &detail, now);
        if let Some(i) = self.slot_of(pid, chain) {
            self.slot_commit_done(pid, i, chain, &task, r, now);
        } else if let CommitResult::Failed(why) = r {
            let _ = self.store.add_inbox(pid, "report", None, Some(chain), &inbox::commit_failed(chain, why), &[], now);
        }
        self.say(format!("{pid}: {chain} {kind} {detail}"));
        self.pump(pid, now);
    }

    /// The person's work is finished: a card for them to sign off, with what each
    /// step reported and every check Lila ran under it.
    pub(super) fn add_review(&mut self, pid: &str, task: &Task, now: i64) -> Result<()> {
        let tasks: Vec<Task> = self.store.tasks(pid)?.into_iter().map(|(t, _)| t).collect();
        let under = |id: &str| id == task.id || route::ancestors(&tasks, id).iter().any(|a| a.id == task.id);
        let mut lines: Vec<(i64, String)> = Vec::new();
        for t in tasks.iter().filter(|t| t.id != task.id && t.state == TaskState::Done && under(&t.id)) {
            let files = self.store.artifacts(pid, &t.id)?;
            lines.push((t.updated_ms, inbox::step_done(t, &files)));
        }
        for e in self.store.events(pid, None, 1000)? {
            if e.kind == "check" && !e.detail.starts_with("running") && e.task.as_deref().is_some_and(under) {
                lines.push((e.at_ms, inbox::check(&e.detail)));
            }
        }
        lines.sort_by_key(|(at, _)| *at);
        let steps: Vec<String> = lines.into_iter().map(|(_, l)| l).collect();
        let text = inbox::review(task, &steps);
        self.store.add_inbox(pid, "review", Some(&task.to), Some(&task.id), &text, &["通过".to_owned()], now)?;
        Ok(())
    }

    /// Accept (no text, or 通过/accept/ok), or anything else: what to do again,
    /// which goes to the PM as a new task in the same chain.
    pub(super) fn review_answer(&mut self, pid: &str, item: &lila_store::InboxItem, text: Option<&str>, now: i64) -> Result<()> {
        let id = item.task.clone().context("a review without its task")?;
        let accepted = text.is_none_or(|t| matches!(t.trim().to_lowercase().as_str(), "通过" | "accept" | "accepted" | "ok" | "pass" | "yes"));
        if accepted {
            self.store.event(pid, Some(&id), "human", "accepted", &format!("{id} signed off"), now)?;
            self.queue_commit(pid, &id);
            return Ok(());
        }
        let task = self.store.task(pid, &id)?.context("the reviewed task is gone")?;
        let owner = self.projects[pid].team.owner.clone();
        let comment = text.unwrap_or_default().trim().to_owned();
        let new = NewTask {
            title: format!("Rework: {}", task.title),
            body: format!("The person did not accept {id} \"{}\":\n\n{comment}", task.title),
            from: Actor::Human,
            to: owner,
            parent: Some(id.clone()),
            priority: 0,
            brief: Brief::default(),
            run: None,
        };
        let rework = self.create_task(pid, new, now)?;
        self.store.event(pid, Some(&id), "human", "rejected", &format!("{comment} → {}", rework.id), now)?;
        Ok(())
    }

    // ---------------- budgets ----------------

    /// Tokens a run's step tasks used: each member's use while it held its task.
    pub(super) fn run_tokens(&mut self, pid: &str, run: &Run, now: i64) -> u64 {
        let cwds = self.dir_strings(pid);
        let tasks: Vec<Task> = self.store.tasks(pid).unwrap_or_default().into_iter().map(|(t, _)| t).filter(|t| t.run.as_ref().is_some_and(|(a, _)| *a == run.anchor)).collect();
        let mut spent = 0;
        let src = self.live_sources();
        for t in tasks {
            let events = self.store.events(pid, Some(&t.id), 200).unwrap_or_default();
            let Some(from) = events.iter().filter(|e| e.kind == "delivered").map(|e| e.at_ms).min() else { continue };
            let to = events.iter().filter(|e| matches!(e.kind.as_str(), "done" | "cancelled")).map(|e| e.at_ms).max().unwrap_or(now);
            spent += self.scanner.tokens_between(&src, pid, &cwds, &t.to, from, to);
        }
        spent
    }

    /// Over budget before its next step: the run holds what it would do next until the person decides.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn hold_run(&mut self, pid: &str, run: &Run, anchor: &Task, next: (usize, bool, &str, &str), spent: u64, budget: u64, now: i64) -> Result<()> {
        let (at, passed, last, failure) = next;
        let hold = json!({"at": at, "passed": passed, "last": last, "failure": failure}).to_string();
        self.store.set_run_hold(pid, &run.anchor, run.budget, Some(&hold))?;
        self.store.set_run(pid, &run.anchor, at, run.round, RunState::Held, now)?;
        let text = message::over_budget(anchor, &run.flow, spent, budget);
        self.store.event(pid, Some(&run.anchor), LILAD, "flow_held", &text, now)?;
        let person = inbox::over_budget(anchor, &message::short_tokens(spent), &message::short_tokens(budget));
        self.store.add_inbox(pid, "budget", Some(&anchor.to), Some(&run.anchor), &person, &["继续".to_owned(), "取消".to_owned()], now)?;
        let owner = self.projects[pid].team.owner.clone();
        self.store.push_outbox(pid, &owner, &message::note(&run.anchor, &format!("{text} You hear when they do.")), now)?;
        Ok(())
    }

    /// Go on (with twice the budget, or more than it spent) or cancel the run.
    pub(super) fn budget_answer(&mut self, pid: &str, item: &lila_store::InboxItem, text: Option<&str>, now: i64) -> Result<()> {
        let anchor = item.task.clone().context("a budget item without its run")?;
        let Some(run) = self.store.run(pid, &anchor)? else { return Ok(()) };
        if run.state != RunState::Held {
            return Ok(());
        }
        let owner = self.projects[pid].team.owner.clone();
        if text.is_some_and(|t| matches!(t.trim().to_lowercase().as_str(), "取消" | "cancel" | "stop")) {
            self.store.set_run_hold(pid, &anchor, run.budget, None)?;
            self.store.set_run(pid, &anchor, run.step, run.round, RunState::Cancelled, now)?;
            self.store.event(pid, Some(&anchor), "human", "flow_cancelled", "over budget", now)?;
            self.store.push_outbox(pid, &owner, &message::note(&anchor, &format!("The person stopped the {} flow of {anchor} over its budget.", run.flow)), now)?;
            return Ok(());
        }
        let spent = self.run_tokens(pid, &run, now);
        let old = run.budget.unwrap_or(0);
        let budget = (old * 2).max(spent + old.max(1));
        let hold: Value = serde_json::from_str(run.hold.as_deref().unwrap_or("{}")).unwrap_or_default();
        self.store.set_run_hold(pid, &anchor, Some(budget), None)?;
        self.store.set_run(pid, &anchor, run.step, run.round, RunState::Running, now)?;
        self.store.event(pid, Some(&anchor), "human", "budget", &format!("raised to {}", message::short_tokens(budget)), now)?;
        let last = hold["last"].as_str().and_then(|id| self.store.task(pid, id).ok().flatten()).context("the held run lost its last step")?;
        let at = hold["at"].as_u64().unwrap_or(run.step as u64) as usize;
        self.advance(pid, &anchor, at, hold["passed"].as_bool().unwrap_or(true), &last, hold["failure"].as_str().unwrap_or(""), now)
    }
}
