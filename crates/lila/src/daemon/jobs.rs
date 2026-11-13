//! Work that blocks, so lilad does it off its loop: what git does for a thread (a worktree made, its
//! changes and diffs, a merge with its commit hooks, a checkpoint of its folder and putting its files
//! back). A request that needs one answers when it is done:
//! `begin` says what to run, the server runs it on a thread of its own, and `finish` (back on the loop)
//! turns its result into the answer and does what follows from it. The summaries of changes that threads
//! show are refreshed the same way, without anyone asking (`refresh_changes`), one at a time.

use super::*;
use crate::changes::{self, Summary};
use crate::checkpoint::{self, Prev, Restored, Taken};
use crate::{merge, worktree};
use lila_agent::Attachment;
use lila_store::{Base, Checkpoint, NewThread};

/// How long the list of models is kept before omp is asked again.
const MODELS_FOR_MS: i64 = 10 * 60 * 1000;

/// What a request comes to: its answer, or work to run first.
pub enum Begin {
    Done(Value),
    Job(Job),
}

/// A thread about to be made, once its worktree is.
pub(super) struct Plan {
    pub new: NewThread,
    pub text: String,
    pub files: Vec<Attachment>,
}

pub(super) enum Work {
    /// `thread.new` with a worktree: make it, then start the thread.
    Worktree { plan: Plan, main: PathBuf, path: PathBuf, branch: String },
    /// A thread's changes. `quiet`: lilad asked, for the summary, not a request.
    Changes { thread: String, cwd: PathBuf, worktree: bool, base_branch: Option<String>, quiet: bool },
    Diff { cwd: PathBuf, path: Option<String> },
    /// `thread.commit`: is there anything to commit?
    CommitCheck { thread: String, cwd: PathBuf },
    Merge { thread: String, job: merge::Job },
    /// A merged thread's worktree and branch, or those of a thread that did not start.
    Remove { thread: String, main: PathBuf, path: PathBuf, branch: String },
    /// A checkpoint of the folder before a turn (`daemon/checkpoints.rs`). The thread's words wait for it and go when it is done.
    Checkpoint { thread: String, n: i64, cwd: PathBuf, text: String, prev: Option<Prev>, at_ms: i64 },
    /// `thread.restore`: the folder's files go back to `target` (a dry run only says what would be done).
    Restore { thread: String, cwd: PathBuf, target: Box<Checkpoint>, undo_n: i64, prev: Option<Prev>, dry_run: bool },
    /// The checkpoint refs of threads that are gone, in the repository `dir` is in.
    DropRefs { dir: PathBuf, threads: Vec<String> },
    /// `models.list`: the models omp offers, asked of an omp of its own.
    Models { program: PathBuf, cwd: PathBuf },
}

/// Work to run off the loop. `run` blocks; `Daemon::finish` takes what it gives.
pub struct Job {
    work: Box<Work>,
    path_env: String,
}

pub enum Output {
    Worktree(Base),
    Changes(changes::Changes),
    Diffs(Vec<changes::ChangeDiff>, bool),
    Dirty { repo: bool, dirty: bool },
    Merged(merge::Merged),
    Removed,
    /// `None`: the folder is not in a git repository.
    Checkpoint(Option<Taken>),
    Restored(Restored),
    Dropped,
    Models(Vec<lila_agent::Choice>),
}

impl Job {
    /// Runs it; what went wrong, as words.
    pub fn run(&self) -> std::result::Result<Output, String> {
        let env = self.path_env.as_str();
        let words = |e: anyhow::Error| format!("{e:#}");
        match &*self.work {
            Work::Worktree { main, path, branch, .. } => worktree::create(main, path, branch, env).map(Output::Worktree).map_err(words),
            Work::Changes { cwd, base_branch, .. } => changes::changes(cwd, env, base_branch.as_deref()).map(Output::Changes).map_err(words),
            Work::Diff { cwd, path } => changes::diffs(cwd, env, path.as_deref()).map(|(d, cut)| Output::Diffs(d, cut)).map_err(words),
            Work::CommitCheck { cwd, .. } => changes::changes(cwd, env, None).map(|c| Output::Dirty { repo: c.repo, dirty: c.count > 0 }).map_err(words),
            Work::Merge { job, .. } => merge::run(job).map(Output::Merged).map_err(words),
            Work::Remove { main, path, branch, .. } => worktree::remove(main, path, branch, env).map(|_| Output::Removed).map_err(words),
            Work::Checkpoint { thread, n, cwd, prev, .. } => checkpoint::take(cwd, env, &checkpoint::Spec { thread, n: *n, prev: prev.as_ref(), keep_below: n - checkpoint::KEEP }).map(Output::Checkpoint).map_err(words),
            Work::Restore { thread, cwd, target, undo_n, prev, dry_run } => {
                let goal = checkpoint::Target { sha: &target.sha, tree: &target.tree, head: target.head.as_deref() };
                let undo = checkpoint::Spec { thread, n: *undo_n, prev: prev.as_ref(), keep_below: undo_n - checkpoint::KEEP };
                checkpoint::restore(cwd, env, &goal, (!dry_run).then_some(&undo)).map(Output::Restored).map_err(words)
            }
            Work::Models { program, cwd } => crate::models::list(program, env, cwd).map(Output::Models).map_err(words),
            Work::DropRefs { dir, threads } => threads.iter().try_for_each(|t| checkpoint::drop_all(dir, env, t).map(|_| ())).map(|_| Output::Dropped).map_err(words),
        }
    }

    /// Takes a checkpoint (tests settle these first, as lilad's server runs them at once).
    pub fn is_checkpoint(&self) -> bool {
        matches!(*self.work, Work::Checkpoint { .. })
    }

    /// Runs on the one thread that counts changes for the summaries, not on a thread of its own: many at once (a restart) would be too much git.
    pub fn serial(&self) -> bool {
        matches!(*self.work, Work::Changes { quiet: true, .. })
    }
}

/// What a thread is asked when the person asks it to commit. In the conversation it shows as Lila's quiet line.
const COMMIT_REQUEST: &str = "<lila note about=\"commit\">\n提交当前所有改动：先看 git status 和 git diff，按这个仓库已有提交信息的风格写一条提交信息，提交全部改动（包括新文件）。只提交，不要推送，不要改历史。\n</lila>";

impl<A: Agents> Daemon<A> {
    /// What a request needs: its answer now, or a job to run first. Jobs are for what runs git on a folder that may be big.
    pub fn begin(&mut self, method: &str, params: &Value, now: i64) -> Result<Begin> {
        match method {
            "thread.new" => self.thread_new(params, now),
            "thread.changes" => {
                let t = self.thread_of(params)?;
                Ok(Begin::Job(self.job(Work::Changes { thread: t.id.clone(), cwd: PathBuf::from(&t.cwd), worktree: t.worktree, base_branch: t.base_branch.clone().filter(|_| t.worktree), quiet: false })))
            }
            "thread.diff" => {
                let t = self.thread_of(params)?;
                Ok(Begin::Job(self.job(Work::Diff { cwd: PathBuf::from(&t.cwd), path: s(params, "path").map(String::from) })))
            }
            "thread.commit" => self.commit_begin(params),
            "thread.merge" => self.merge_begin(params),
            "thread.restore" => self.restore_begin(params),
            "models.list" => self.models_begin(params, now),
            other => Ok(Begin::Done(self.dispatch(other, params, now)?)),
        }
    }

    pub(super) fn job(&self, work: Work) -> Job {
        Job { work: Box::new(work), path_env: self.path_env.clone() }
    }

    /// Jobs lilad wants run (the summaries, a worktree to remove, …): the server runs them and calls `finish`.
    pub fn take_jobs(&mut self) -> Vec<Job> {
        std::mem::take(&mut self.jobs)
    }

    /// A job is done: what it gives becomes the answer to the request that wanted it (a job lilad started itself has none), and what follows from it happens.
    pub fn finish(&mut self, job: Job, result: std::result::Result<Output, String>, now: i64) -> Result<Value> {
        match *job.work {
            Work::Worktree { mut plan, path, branch, main } => {
                let started = match result {
                    Ok(Output::Worktree(base)) if self.projects.contains_key(&plan.new.project) => base,
                    Ok(Output::Worktree(_)) => {
                        self.remove_later(&plan.new.id, main, path, branch);
                        bail!("the project {} was removed", plan.new.project);
                    }
                    Err(e) => return Err(anyhow!(e)),
                    Ok(_) => unreachable!("a worktree job gives a worktree"),
                };
                plan.new.cwd = path.to_string_lossy().to_string();
                plan.new.worktree = Some(started);
                let id = plan.new.id.clone();
                self.start_new(plan, now).inspect_err(|_| {
                    // The thread did not start: nor does its worktree stay.
                    self.remove_later(&id, main, path, branch);
                })
            }
            Work::Changes { thread, worktree, base_branch, quiet, .. } => {
                if quiet {
                    self.refreshing.remove(&thread);
                }
                let c = match result {
                    Ok(Output::Changes(c)) => c,
                    Err(e) if quiet => {
                        self.say(format!("changes of {thread}: {e}"));
                        return Ok(Value::Null);
                    }
                    Err(e) => return Err(anyhow!(e)),
                    Ok(_) => unreachable!("a changes job gives changes"),
                };
                self.set_summary(&thread, c.summary());
                if quiet && self.refresh_again.remove(&thread) {
                    self.refresh_changes(&thread);
                }
                Ok(c.json(worktree, base_branch.as_deref()))
            }
            Work::Diff { .. } => match result {
                Ok(Output::Diffs(diffs, cut)) => Ok(json!({"diffs": diffs, "truncated": cut})),
                Err(e) => Err(anyhow!(e)),
                Ok(_) => unreachable!("a diff job gives diffs"),
            },
            Work::CommitCheck { thread, cwd } => {
                let (repo, dirty) = match result {
                    Ok(Output::Dirty { repo, dirty }) => (repo, dirty),
                    Err(e) => return Err(anyhow!(e)),
                    Ok(_) => unreachable!("a commit check gives what is dirty"),
                };
                if !repo {
                    bail!("{} is not in a git repository", cwd.display());
                }
                if !dirty {
                    bail!("{thread} has nothing to commit");
                }
                let t = self.store.thread(&thread)?.ok_or_else(|| anyhow!("no thread {thread}"))?;
                self.check_free(&t, false)?;
                self.send_to(&t, COMMIT_REQUEST, Vec::new(), now)?;
                self.store.event(&thread, "human", "commit", "asked the agent to commit", now)?;
                self.refresh_changes(&thread);
                Ok(json!({"sent": true, "thread": thread}))
            }
            Work::Merge { thread, job: merge_job } => {
                self.merging.remove(&thread);
                match result {
                    Ok(Output::Merged(m)) => {
                        let t = self.store.thread(&thread)?.ok_or_else(|| anyhow!("no thread {thread}"))?;
                        // omp ends first: its folder goes.
                        self.stop_thread(&t, "merged", now);
                        self.store.set_merged(&thread, now)?;
                        self.store.set_archived(&thread, true)?;
                        for m in self.store.outbox(&thread)? {
                            self.store.pop_outbox(m.id)?;
                        }
                        let detail = format!("{} commit(s) into {} as {}", m.commits, merge_job.base_branch, m.sha);
                        self.store.event(&thread, "lilad", "merged", &detail, now)?;
                        self.say(format!("{thread}: merged, {detail}"));
                        self.remove_later(&thread, merge_job.main.clone(), merge_job.worktree.clone(), merge_job.branch.clone());
                        self.drop_checkpoints_later(std::slice::from_ref(&thread), merge_job.main.clone())?;
                        self.set_summary(&thread, None);
                        self.mark(&thread);
                        let t = self.store.thread(&thread)?.ok_or_else(|| anyhow!("no thread {thread}"))?;
                        Ok(json!({"merged": true, "thread": self.thread_json(&t), "commit": m.sha, "commits": m.commits, "files": m.files, "branch": merge_job.base_branch}))
                    }
                    Err(e) => {
                        let _ = self.store.event(&thread, "lilad", "merge_failed", &e, now);
                        self.mark(&thread);
                        Err(anyhow!(e))
                    }
                    Ok(_) => unreachable!("a merge job gives a merge"),
                }
            }
            Work::Checkpoint { thread, n, cwd, text, at_ms, .. } => self.checkpoint_done(&thread, n, cwd, text, at_ms, result, now),
            Work::Restore { thread, target, undo_n, dry_run, .. } => self.restore_done(&thread, *target, undo_n, dry_run, result, now),
            Work::DropRefs { threads, .. } => {
                if let Err(e) = result {
                    self.say(format!("could not delete the checkpoints of {}: {e}", threads.join(", ")));
                }
                Ok(Value::Null)
            }
            Work::Models { .. } => match result {
                Ok(Output::Models(models)) => {
                    self.models = Some((now, models));
                    Ok(self.models_json())
                }
                Ok(_) => unreachable!("a models job gives models"),
                Err(e) => bail!("could not list omp's models: {e}"),
            },
            Work::Remove { thread, .. } => {
                if let Err(e) = result {
                    self.say(format!("{thread}: could not remove its worktree: {e}"));
                    if self.store.thread(&thread)?.is_some() {
                        self.store.event(&thread, "lilad", "notice", &format!("could not remove its worktree: {e}"), now)?;
                    }
                }
                Ok(Value::Null)
            }
        }
    }

    /// `models.list {refresh?}`: the models omp offers, `[{id, name}]`. Asked of a short-lived omp, and remembered for a while.
    fn models_begin(&mut self, params: &Value, now: i64) -> Result<Begin> {
        if let Some((at, _)) = &self.models
            && now - at < MODELS_FOR_MS
            && params["refresh"].as_bool() != Some(true)
        {
            return Ok(Begin::Done(self.models_json()));
        }
        let program = self.omp_program()?;
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir());
        Ok(Begin::Job(self.job(Work::Models { program, cwd })))
    }

    pub(super) fn models_json(&self) -> Value {
        json!({"models": self.models.as_ref().map(|(_, m)| m.clone()).unwrap_or_default()})
    }

    fn remove_later(&mut self, thread: &str, main: PathBuf, path: PathBuf, branch: String) {
        let job = self.job(Work::Remove { thread: thread.to_owned(), main, path, branch });
        self.jobs.push(job);
    }

    // ---------------- worktree threads ----------------

    /// A new thread in a worktree of its own: the work that makes the worktree, after which the thread starts (`finish`).
    pub(super) fn worktree_job(&self, plan: Plan, main: PathBuf) -> Job {
        let (path, branch) = (self.paths.worktree(&plan.new.project, &plan.new.id), worktree::branch(&plan.new.id));
        self.job(Work::Worktree { plan, main, path, branch })
    }

    /// Nothing is going on in the thread: its agent is idle (`or_ended`: or gone, which a merge does not mind; a commit needs the agent).
    pub(super) fn check_free(&self, t: &Thread, or_ended: bool) -> Result<()> {
        if self.merging.contains(&t.id) {
            bail!("{} is being merged", t.id);
        }
        if self.restoring.contains(&t.id) {
            bail!("{} is being restored", t.id);
        }
        match self.state_of(t).0 {
            ThreadState::Idle => Ok(()),
            ThreadState::Ended if or_ended => Ok(()),
            ThreadState::Ended => bail!("{} has ended; resume it first", t.id),
            ThreadState::Waiting => bail!("{} is waiting for an answer: give it first", t.id),
            ThreadState::Starting | ThreadState::Running => bail!("{} is working now; wait for its turn to end", t.id),
        }
    }

    /// `thread.commit {thread}`: asks the agent to commit what the folder has changed. The agent writes the message and commits (only that: no push, no history changes);
    /// what it may run is up to the thread's permission tier. The thread must be idle and have changes.
    fn commit_begin(&mut self, params: &Value) -> Result<Begin> {
        let t = self.thread_of(params)?;
        if t.merged_ms.is_some() {
            bail!("{} was merged; its worktree is gone", t.id);
        }
        self.check_free(&t, false)?;
        Ok(Begin::Job(self.job(Work::CommitCheck { thread: t.id.clone(), cwd: PathBuf::from(&t.cwd) })))
    }

    /// `thread.merge {thread}`: a worktree thread's work goes into the branch it started from (`merge.rs`).
    fn merge_begin(&mut self, params: &Value) -> Result<Begin> {
        let t = self.thread_of(params)?;
        if !t.worktree {
            bail!("{} does not work in a worktree of its own; there is nothing to merge", t.id);
        }
        if t.merged_ms.is_some() {
            bail!("{} was merged already", t.id);
        }
        self.check_free(&t, true)?;
        // Words an agent that is still up has not taken yet go to it first; an ended one will never take them.
        if self.state_of(&t).0 != ThreadState::Ended && self.has_words_waiting(&t.id)? {
            bail!("{} has a message waiting for its agent; let it be taken first", t.id);
        }
        let main = self.projects.get(&t.project).map(|p| p.cwd.clone()).ok_or_else(|| anyhow!("unknown project {}", t.project))?;
        let base_branch = t.base_branch.clone().ok_or_else(|| anyhow!("{} has no base branch recorded", t.id))?;
        self.merging.insert(t.id.clone());
        self.mark(&t.id);
        let job = merge::Job { title: t.title.clone(), main, worktree: PathBuf::from(&t.cwd), branch: worktree::branch(&t.id), base_branch, path_env: self.path_env.clone() };
        Ok(Begin::Job(self.job(Work::Merge { thread: t.id, job })))
    }

    // ---------------- the summary of changes ----------------

    /// Counts the thread's changes again, off the loop, and the window is told if they differ. Asked after a turn ends, a commit, a merge.
    pub(super) fn refresh_changes(&mut self, tid: &str) {
        let Ok(Some(t)) = self.store.thread(tid) else { return };
        if t.archived || t.merged_ms.is_some() {
            return;
        }
        // One at a time per thread; one more after it when asked meanwhile.
        if !self.refreshing.insert(tid.to_owned()) {
            self.refresh_again.insert(tid.to_owned());
            return;
        }
        let job = self.job(Work::Changes { thread: t.id.clone(), cwd: PathBuf::from(&t.cwd), worktree: t.worktree, base_branch: t.base_branch.clone().filter(|_| t.worktree), quiet: true });
        self.jobs.push(job);
    }

    fn set_summary(&mut self, tid: &str, summary: Option<Summary>) {
        let old = self.summaries.insert(tid.to_owned(), summary);
        if old.flatten() != summary {
            self.mark(tid);
        }
    }

    /// The summary the thread list carries: none until counted, or when the folder is not in a git repository.
    pub(super) fn summary_of(&self, tid: &str) -> Option<Summary> {
        self.summaries.get(tid).copied().flatten()
    }
}
