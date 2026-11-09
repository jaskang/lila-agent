//! Worktree slots (`crate::worktree`): which chain works where, chains taking a
//! free slot in queue order, and slots freed once their chain is over. A project
//! that is not the top of a git repository (or has no commit yet) has none: its
//! chains share the project folder (`held_for_folder`).

use super::*;
use crate::worktree;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Slot {
    pub path: PathBuf,
    /// The chain (root task) it holds.
    pub chain: Option<String>,
}

fn root_of(tasks: &[Task], id: &str) -> String {
    route::ancestors(tasks, id).last().map(|t| t.id.clone()).unwrap_or_else(|| id.to_owned())
}

impl<T: Terminals> Daemon<T> {
    /// A project's slots, from git: a slot's branch says which chain it holds.
    /// Slots beyond `parallel` (it was lowered) stay while they hold a chain.
    pub(super) fn read_slots(&self, pid: &str, cwd: &Path, team: &Team) -> Vec<Slot> {
        let n = team.settings.parallel;
        if n == 0 || !worktree::is_repo_root(cwd, &self.path_env) {
            return Vec::new();
        }
        let dir = self.paths.worktrees(pid);
        let mut slots: Vec<Slot> = (1..=n).map(|i| dir.join(i.to_string())).map(|path| Slot { chain: worktree::occupant(&path, &self.path_env), path }).collect();
        for path in worktree::existing(&dir) {
            let beyond = path.file_name().and_then(|f| f.to_str()).and_then(|f| f.parse::<u32>().ok()).is_some_and(|i| i > n);
            if beyond && let Some(chain) = worktree::occupant(&path, &self.path_env) {
                slots.push(Slot { path, chain: Some(chain) });
            }
        }
        slots
    }

    /// Chains take slots: the project is a git repository with a commit to start from.
    pub(super) fn uses_slots(&mut self, pid: &str) -> bool {
        let p = &self.projects[pid];
        if p.slots.is_empty() {
            return false;
        }
        if !p.based {
            let based = worktree::out(&p.cwd, &self.path_env, &["rev-parse", "--verify", "-q", "HEAD"]).is_ok();
            self.projects.get_mut(pid).unwrap().based = based;
        }
        self.projects[pid].based
    }

    pub(super) fn slot_of(&self, pid: &str, chain: &str) -> Option<usize> {
        self.projects[pid].slots.iter().position(|s| s.chain.as_deref() == Some(chain))
    }

    /// Where a chain's work is: its slot, else the project folder.
    pub(super) fn chain_dir(&self, pid: &str, chain: &str) -> PathBuf {
        let p = &self.projects[pid];
        self.slot_of(pid, chain).map(|i| p.slots[i].path.clone()).unwrap_or_else(|| p.cwd.clone())
    }

    /// Where a member works on `chain`: the PM always in the project folder.
    pub(super) fn member_dir(&self, pid: &str, mid: &str, chain: Option<&str>) -> PathBuf {
        let p = &self.projects[pid];
        match chain {
            Some(c) if p.team.member(mid).is_some_and(|m| !m.is_owner) => self.chain_dir(pid, c),
            _ => p.cwd.clone(),
        }
    }

    /// Where a member starts when nothing is being delivered to it: in the slot
    /// of the chain it was last on, if that chain still has one.
    pub(super) fn home_dir(&self, pid: &str, mid: &str) -> PathBuf {
        let chain = self.projects[pid].live.get(mid).and_then(|l| l.chain.clone());
        self.member_dir(pid, mid, chain.as_deref())
    }

    /// Every folder a project's members work in.
    pub(super) fn dirs(&self, pid: &str) -> Vec<PathBuf> {
        let p = &self.projects[pid];
        std::iter::once(p.cwd.clone()).chain(p.slots.iter().map(|s| s.path.clone())).collect()
    }

    pub(super) fn dir_strings(&self, pid: &str) -> Vec<String> {
        self.dirs(pid).into_iter().map(|p| p.to_string_lossy().to_string()).collect()
    }

    /// Chains that are not over: open work or flows, or something waiting for the
    /// person (a review card, a merge that did not go through), or their commit is under way.
    pub(super) fn busy_chains(&self, pid: &str, tasks: &[Task]) -> BTreeSet<String> {
        let mut busy: BTreeSet<String> = tasks.iter().filter(|t| !matches!(t.state, TaskState::Done | TaskState::Cancelled)).map(|t| root_of(tasks, &t.id)).collect();
        busy.extend(self.store.runs(pid).unwrap_or_default().into_iter().filter(|r| r.state.is_open()).map(|r| root_of(tasks, &r.anchor)));
        busy.extend(self.store.inbox(pid, false).unwrap_or_default().into_iter().filter(|i| matches!(i.kind.as_str(), "review" | "merge")).filter_map(|i| i.task).map(|t| root_of(tasks, &t)));
        busy.extend(self.committing.iter().filter(|(p, _)| p == pid).map(|(_, c)| c.clone()));
        busy
    }

    /// Queued tasks that wait for a slot. A chain that has one goes on, and so
    /// does one that started in the project folder before it had slots; a new
    /// chain takes a free slot, in the order its work was queued. The PM's work never waits.
    pub(super) fn held_for_slots(&mut self, pid: &str, tasks: &[(Task, lila_store::TaskMeta)], now: i64) -> BTreeSet<String> {
        let plain: Vec<Task> = tasks.iter().map(|(t, _)| t.clone()).collect();
        let owner = self.projects[pid].team.owner.clone();
        let parallel = self.projects[pid].team.settings.parallel as usize;
        let started: BTreeSet<String> = plain.iter().filter(|t| t.to != owner && t.state != TaskState::Queued).map(|t| root_of(&plain, &t.id)).collect();
        let mut queued: Vec<&(Task, lila_store::TaskMeta)> = tasks.iter().filter(|(t, _)| t.state == TaskState::Queued && t.to != owner).collect();
        queued.sort_by_key(|(_, m)| m.seq);
        let mut held = BTreeSet::new();
        for (t, _) in queued {
            let chain = root_of(&plain, &t.id);
            if self.slot_of(pid, &chain).is_some() || started.contains(&chain) {
                continue;
            }
            if self.held_for_review(pid, t) || self.slot_failed.get(&(pid.to_owned(), chain.clone())).is_some_and(|at| now - at < 60_000) {
                held.insert(t.id.clone());
                continue;
            }
            let free = self.projects[pid].slots.iter().take(parallel).position(|s| s.chain.is_none());
            match free {
                Some(i) if self.take_slot(pid, i, &chain, now) => {}
                _ => {
                    held.insert(t.id.clone());
                }
            }
        }
        held
    }

    /// `chain` takes slot `i`: reset to the project's current commit on its own branch.
    fn take_slot(&mut self, pid: &str, i: usize, chain: &str, now: i64) -> bool {
        let (main, slot) = (self.projects[pid].cwd.clone(), self.projects[pid].slots[i].path.clone());
        let ready = worktree::prepare(&main, &slot, chain, &self.path_env).and_then(|()| {
            std::fs::create_dir_all(slot.join(".lila").join("tasks"))?;
            std::fs::create_dir_all(spool_dir(&slot))?;
            Ok(())
        });
        let key = (pid.to_owned(), chain.to_owned());
        match ready {
            Ok(()) => {
                self.slot_failed.remove(&key);
                self.projects.get_mut(pid).unwrap().slots[i].chain = Some(chain.to_owned());
                self.live_version += 1;
                let _ = self.store.event(pid, Some(chain), LILAD, "worktree", &format!("{chain} works in worktree {} ({})", i + 1, slot.display()), now);
                self.say(format!("{pid}: {chain} takes worktree {}", i + 1));
                true
            }
            Err(e) => {
                let why = format!("{e:#}");
                if self.slot_failed.insert(key, now).is_none() {
                    let _ = self.store.add_inbox(pid, "report", None, Some(chain), &inbox::worktree_failed(chain, &why), &[], now);
                }
                self.say(format!("{pid}: no worktree for {chain}: {why}"));
                false
            }
        }
    }

    /// Frees slot `i`. Its branch goes when its work is in the project's branch (`merged`).
    pub(super) fn free_slot(&mut self, pid: &str, i: usize, merged: bool, now: i64) {
        let (main, slot) = (self.projects[pid].cwd.clone(), self.projects[pid].slots[i].clone());
        let Some(chain) = slot.chain else { return };
        if let Err(e) = worktree::release(&main, &slot.path, &chain, merged, &self.path_env) {
            self.say(format!("{pid}: freeing worktree {} of {chain}: {e:#}", i + 1));
        }
        self.projects.get_mut(pid).unwrap().slots[i].chain = None;
        self.live_version += 1;
        let _ = self.store.event(pid, Some(&chain), LILAD, "worktree", &format!("worktree {} is free", i + 1), now);
    }

    /// Chains that are over without passing through a commit (tick): a cancelled
    /// one keeps its work on its branch; one that finished with changes nobody
    /// has accepted gets a review card (everything that changes the code passes
    /// the person); one that changed nothing frees its slot.
    pub(super) fn tend_slots(&mut self, pid: &str, now: i64) {
        if self.projects[pid].slots.iter().all(|s| s.chain.is_none()) {
            return;
        }
        let tasks: Vec<Task> = self.store.tasks(pid).unwrap_or_default().into_iter().map(|(t, _)| t).collect();
        let busy = self.busy_chains(pid, &tasks);
        let main = self.projects[pid].cwd.clone();
        let mut freed = false;
        for i in 0..self.projects[pid].slots.len() {
            let Slot { path, chain: Some(chain) } = self.projects[pid].slots[i].clone() else { continue };
            if busy.contains(&chain) {
                continue;
            }
            let root = tasks.iter().find(|t| t.id == chain);
            if root.is_none_or(|r| r.state == TaskState::Cancelled) {
                if let Err(e) = worktree::park(&path, &chain, &self.path_env) {
                    self.say(format!("{pid}: keeping {chain}'s work: {e:#}"));
                    continue;
                }
                let _ = self.store.event(pid, Some(&chain), LILAD, "worktree", &format!("{chain} was cancelled; its work is kept on branch {}", worktree::branch(&chain)), now);
                self.projects.get_mut(pid).unwrap().slots[i].chain = None;
                self.live_version += 1;
                freed = true;
                continue;
            }
            match worktree::has_changes(&main, &path, &self.path_env) {
                Ok(true) => {
                    let root = root.unwrap().clone();
                    if let Err(e) = self.add_review(pid, &root, now) {
                        self.say(format!("{pid}: review of {chain}: {e:#}"));
                    }
                }
                Ok(false) => {
                    self.free_slot(pid, i, true, now);
                    freed = true;
                }
                Err(e) => self.say(format!("{pid}: worktree {} of {chain}: {e:#}", i + 1)),
            }
        }
        if freed {
            self.pump(pid, now);
        }
    }

    /// The commit of an accepted chain that worked in a slot came back.
    pub(super) fn slot_commit_done(&mut self, pid: &str, i: usize, chain: &str, task: &str, r: &CommitResult, now: i64) {
        let retry = vec!["重试".to_owned()];
        let dir = self.projects[pid].slots[i].path.to_string_lossy().to_string();
        let item = match r {
            CommitResult::Committed(_) | CommitResult::Nothing => return self.free_slot(pid, i, true, now),
            CommitResult::NotRepo => return self.free_slot(pid, i, false, now),
            CommitResult::Conflict(files) => {
                self.conflicts.insert((pid.to_owned(), chain.to_owned()), files.clone());
                (inbox::merge_conflict(chain, files), vec!["交给助理".to_owned(), "重试".to_owned()])
            }
            CommitResult::NotMerged(why) => (inbox::not_merged(chain, why), retry),
            CommitResult::Failed(why) => (inbox::slot_commit_failed(chain, &dir, why), retry),
        };
        let _ = self.store.add_inbox(pid, "merge", None, Some(task), &item.0, &item.1, now);
    }

    /// 重试: commit and merge again. 交给助理 (or any other words): the PM has the conflict resolved
    /// in the chain, and the person is asked to accept again. No answer: it stays as it is.
    pub(super) fn merge_answer(&mut self, pid: &str, item: &lila_store::InboxItem, text: Option<&str>, now: i64) -> Result<()> {
        let id = item.task.clone().context("a merge item without its task")?;
        let Some(text) = text.map(str::trim).filter(|t| !t.is_empty()) else { return Ok(()) };
        if matches!(text.to_lowercase().as_str(), "重试" | "retry") {
            self.queue_commit(pid, &id);
            return Ok(());
        }
        let task = self.store.task(pid, &id)?.context("the accepted task is gone")?;
        let tasks: Vec<Task> = self.store.tasks(pid)?.into_iter().map(|(t, _)| t).collect();
        let chain = root_of(&tasks, &id);
        let files = self.conflicts.remove(&(pid.to_owned(), chain.clone())).unwrap_or_default();
        let target = worktree::target(&self.projects[pid].cwd, &self.path_env).unwrap_or_else(|_| "the project's branch".to_owned());
        let comment = (text != "交给助理").then_some(text);
        let owner = self.projects[pid].team.owner.clone();
        let new = NewTask {
            title: format!("Resolve conflicts: {}", task.title),
            body: message::resolve_conflicts(&task, &chain, &target, &files, comment),
            from: Actor::Human,
            to: owner,
            parent: Some(id.clone()),
            priority: 0,
            brief: Brief::default(),
            run: None,
        };
        let t = self.create_task(pid, new, now)?;
        self.store.event(pid, Some(&id), "human", "resolve", &format!("conflicts with {target} → {}", t.id), now)?;
        Ok(())
    }

    /// The slots for `lila status` and the window.
    pub(super) fn slots_json(&self, pid: &str) -> Value {
        json!(self.projects[pid].slots.iter().enumerate().map(|(i, s)| json!({"slot": i + 1, "chain": s.chain, "path": s.path})).collect::<Vec<_>>())
    }
}
