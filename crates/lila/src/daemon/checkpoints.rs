//! Checkpoints (architecture v4 §11): a snapshot of a thread's folder before each turn, and putting the files
//! back to one. The git work is `checkpoint.rs`, run off the loop as jobs (`jobs.rs`); this is what lilad decides
//! around it: when one is taken, how a checkpoint finds the message of its turn in the conversation, what a
//! restore needs before it may run and what follows from it.
//!
//! A turn's checkpoint is taken in `deliver`, before the words are sent to the host, so that it is of the folder before
//! omp can act and never twice: the words wait for the job. omp takes words one turn at a time, so they are not
//! delivered until it is free, and every message begins a turn.
//!
//! A checkpoint that cannot be taken is recorded (`checkpoint_failed`) and the turn goes on.

use super::jobs::{Begin, Job, Output, Work};
use super::*;
use crate::checkpoint::{self, Diff, Prev, Restored};
use lila_agent::{Item, ItemKind, ItemState, Origin};
use lila_store::{Checkpoint, CheckpointKind};
use std::collections::HashSet;

/// How much earlier than the checkpoint a message's own time may be and still be the message of its turn
/// (omp's host stamps it by its own clock, a little before or after lilad's).
const SLACK_MS: i64 = 3000;

/// The words of a message as compared between what was sent and what the conversation shows of it.
fn normal(text: &str) -> String {
    lila_agent::without_notes(text.trim()).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The same words; a message of files only has none.
fn same_words(a: &str, b: &str) -> bool {
    a.is_empty() || b.is_empty() || a.starts_with(b) || b.starts_with(a)
}

/// What the message begins with, for a list: its first line that is words, cut.
fn first_line(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("<lila") && !l.starts_with("</lila")).unwrap_or_default();
    let cut: String = line.chars().take(100).collect();
    if cut.chars().count() < line.chars().count() { format!("{cut}…") } else { cut }
}

fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// What the agent is told after its files were put back. It goes out with the person's next message.
fn restore_note(n: i64, title: &str, d: &Diff, head_moved: bool) -> String {
    let title = title.replace('"', "'");
    let began = if title.is_empty() { "an earlier turn".to_owned() } else { format!("the turn that began with \"{title}\"") };
    let commits = if head_moved { " Commits made since then were not undone; HEAD is where it was." } else { "" };
    format!(
        "<lila note about=\"restore\">\nLila put this folder's files back to how they were before {began} (checkpoint {n}). Everything changed in that turn and after it was undone: {} file(s) put back, {} made again, {} removed.{commits} The conversation above still happened, so what you remember about these files may be wrong: read a file again before you edit it.\n</lila>",
        d.changed.len(),
        d.recreated.len(),
        d.deleted.len()
    )
}

impl<A: Agents> Daemon<A> {
    // ---------------- taking them ----------------

    /// Someone is waiting for the thread's words (not just a note that waits for them).
    pub(super) fn has_words_waiting(&self, tid: &str) -> Result<bool> {
        Ok(self.store.outbox(tid)?.iter().any(|m| !m.hold))
    }

    fn reserve_checkpoint(&mut self, tid: &str) -> i64 {
        let stored = self.store.last_checkpoint(tid).unwrap_or(0);
        let n = stored.max(self.checkpoint_numbers.get(tid).copied().unwrap_or(0)) + 1;
        self.checkpoint_numbers.insert(tid.to_owned(), n);
        n
    }

    /// The thread's latest checkpoint, to leave out a new one when nothing changed since.
    fn latest_prev(&self, tid: &str) -> Option<Prev> {
        let c = self.store.checkpoints(tid).ok()?.into_iter().next()?;
        Some(Prev { sha: c.sha, tree: c.tree, head: c.head })
    }

    /// The job that takes a checkpoint of the thread's folder for a turn that begins with `text`; none when the folder is
    /// certainly not in a git repository (nothing is started for that) or the thread was merged.
    pub(super) fn checkpoint_job(&mut self, t: &Thread, text: String, now: i64) -> Option<Job> {
        if t.merged_ms.is_some() {
            return None;
        }
        let cwd = PathBuf::from(&t.cwd);
        if !checkpoint::maybe_repo(&cwd) {
            return None;
        }
        if self.restoring.contains(&t.id) {
            let _ = self.store.event(&t.id, "lilad", "checkpoint_failed", "skipped: the files were being put back", now);
            return None;
        }
        let n = self.reserve_checkpoint(&t.id);
        let prev = self.latest_prev(&t.id);
        Some(self.job(Work::Checkpoint { thread: t.id.clone(), n, cwd, text, prev, at_ms: now }))
    }

    /// A checkpoint job is done: it is recorded (or why not), and the words that waited for it go.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn checkpoint_done(&mut self, tid: &str, n: i64, cwd: PathBuf, text: String, at_ms: i64, result: std::result::Result<Output, String>, now: i64) -> Result<Value> {
        self.snapshotting.remove(tid);
        let t = self.store.thread(tid).ok().flatten();
        let live = t.as_ref().is_some_and(|t| t.merged_ms.is_none());
        match result {
            Ok(Output::Checkpoint(Some(c))) if live => {
                let row = Checkpoint { thread: tid.to_owned(), n, kind: CheckpointKind::Turn, at_ms, sha: c.sha, tree: c.tree, head: c.head, branch: c.branch, text, item: None };
                if let Err(e) = self.store.add_checkpoint(&row).and_then(|_| self.store.remove_checkpoints_up_to(tid, n - checkpoint::KEEP)) {
                    self.say(format!("{tid}: could not record checkpoint {n}: {e:#}"));
                }
                self.mark(tid);
            }
            // The thread went (its project was removed, it was merged) while the job ran: its ref is not wanted.
            Ok(Output::Checkpoint(Some(_))) => self.jobs.push(self.job(Work::DropRefs { dir: cwd, threads: vec![tid.to_owned()] })),
            Ok(Output::Checkpoint(None)) => {}
            Err(e) => {
                self.say(format!("{tid}: no checkpoint {n}: {e}"));
                if live {
                    let _ = self.store.event(tid, "lilad", "checkpoint_failed", &e, now);
                    self.mark(tid);
                }
            }
            Ok(_) => unreachable!("a checkpoint job gives a checkpoint"),
        }
        self.deliver_with(tid, now, true);
        Ok(json!({}))
    }

    /// Forgets the checkpoints of threads that are gone (merged, their project removed) and deletes their refs in the repository `dir` is in.
    pub(super) fn drop_checkpoints_later(&mut self, threads: &[String], dir: PathBuf) -> Result<()> {
        for t in threads {
            self.store.remove_checkpoints(t)?;
            self.checkpoint_numbers.remove(t);
        }
        self.jobs.push(self.job(Work::DropRefs { dir, threads: threads.to_vec() }));
        Ok(())
    }

    // ---------------- finding the message of a turn ----------------

    /// Attaches each checkpoint to the message that began its turn, and returns them, the newest first.
    ///
    /// The conversation's ids are made by the omp connection, when the words are sent, so a checkpoint has none to begin with. It takes the first message of the person (or Lila) that is not taken yet, from
    /// its own time on, and says the same words. That is remembered in the row once found; one whose message never turned
    /// up is given up when a later checkpoint has found its own.
    pub(super) fn resolve_checkpoints(&mut self, tid: &str) -> Result<Vec<Checkpoint>> {
        let mut rows = self.store.checkpoints(tid)?;
        let open: Vec<usize> = (0..rows.len()).rev().filter(|i| rows[*i].kind == CheckpointKind::Turn && rows[*i].item.is_none()).collect();
        let Some(oldest) = open.first().map(|i| rows[*i].at_ms) else { return Ok(rows) };
        struct Said {
            id: String,
            at_ms: i64,
            text: String,
        }
        let said: Vec<Said> = self
            .store
            .items_since(tid, oldest - SLACK_MS)?
            .into_iter()
            .filter_map(|(id, at_ms, body)| {
                let v: Value = serde_json::from_str(&body).ok()?;
                matches!(v["from"].as_str(), Some("person" | "lila")).then(|| Said { id, at_ms, text: normal(v["text"].as_str().unwrap_or_default()) })
            })
            .collect();
        let mut taken: HashSet<String> = rows.iter().filter_map(|c| c.matched_item().map(String::from)).collect();
        let mut found: Vec<(usize, String)> = Vec::new();
        for &i in &open {
            let c = &rows[i];
            let words = normal(&c.text);
            let Some(m) = said.iter().find(|m| m.at_ms + SLACK_MS >= c.at_ms && !taken.contains(&m.id) && same_words(&words, &m.text)) else { continue };
            // A later checkpoint of the same words, also taken before this message, is the one that precedes it (a turn that was
            // prepared twice, as when its words could not be delivered the first time).
            if open.iter().any(|&j| rows[j].n > c.n && rows[j].at_ms <= m.at_ms && same_words(&normal(&rows[j].text), &m.text)) {
                continue;
            }
            taken.insert(m.id.clone());
            found.push((i, m.id.clone()));
        }
        let newest = found.iter().map(|(i, _)| rows[*i].n).max();
        let mut changed = false;
        for &i in &open {
            let item = match found.iter().find(|(k, _)| *k == i) {
                Some((_, id)) => id.clone(),
                None if newest.is_some_and(|n| rows[i].n < n) => String::new(),
                None => continue,
            };
            self.store.set_checkpoint_item(tid, rows[i].n, &item)?;
            rows[i].item = Some(item);
            changed = true;
        }
        if changed {
            // The window hears of it with the other news of the thread.
            self.mark(tid);
        }
        Ok(rows)
    }

    /// The checkpoints as the window gets them (`thread.checkpoints`, and `thread.read` carries the same list).
    pub(super) fn checkpoint_rows(&mut self, t: &Thread) -> Result<Vec<Value>> {
        let rows = self.resolve_checkpoints(&t.id)?;
        let restorable = self.restore_blocker(t).is_none();
        Ok(rows
            .iter()
            .map(|c| {
                json!({
                    "id": c.n, "kind": c.kind.as_str(), "at_ms": c.at_ms, "title": first_line(&c.text), "item": c.matched_item(),
                    "restorable": restorable, "head": c.head.as_deref().map(short), "branch": c.branch,
                })
            })
            .collect())
    }

    /// `thread.checkpoints {thread}`: the thread's checkpoints, the newest first. `item` is the conversation item of the message
    /// the turn began with (none while it is not in the conversation yet, or when it never will be); `can_restore` says whether a
    /// restore is accepted now, and `why_not` what stops it.
    pub(super) fn thread_checkpoints(&mut self, params: &Value) -> Result<Value> {
        let t = self.thread_of(params)?;
        let rows = self.checkpoint_rows(&t)?;
        let why = self.restore_blocker(&t);
        Ok(json!({"checkpoints": rows, "can_restore": why.is_none(), "why_not": why}))
    }

    // ---------------- going back ----------------

    /// What stops the thread's files being put back now, in words.
    pub(super) fn restore_blocker(&self, t: &Thread) -> Option<String> {
        if t.merged_ms.is_some() {
            return Some(format!("{} was merged; its folder is gone", t.id));
        }
        if let Err(e) = self.check_free(t, true) {
            return Some(format!("{e:#}"));
        }
        if self.snapshotting.contains(&t.id) {
            return Some(format!("{} has a message being sent", t.id));
        }
        let ended = self.state_of(t).0 == ThreadState::Ended;
        if !ended && self.has_words_waiting(&t.id).unwrap_or(true) {
            return Some(format!("{} has a message waiting for its agent; let it be taken first", t.id));
        }
        // The folder may be shared (threads work in place by default): a restore undoes the others' edits in it too.
        let busy = self.store.threads(&t.project, false).unwrap_or_default().into_iter().find(|o| {
            o.id != t.id && o.cwd == t.cwd && {
                let (state, waiting) = self.state_of(o);
                state == ThreadState::Running || (state == ThreadState::Waiting && waiting == Some("card"))
            }
        });
        busy.map(|o| format!("{} is working in the same folder now; putting the files back would undo its work too", o.id))
    }

    /// `thread.restore {thread, checkpoint, dry_run?}`.
    pub(super) fn restore_begin(&mut self, params: &Value) -> Result<Begin> {
        let t = self.thread_of(params)?;
        let n = params["checkpoint"].as_i64().ok_or_else(|| anyhow!("missing checkpoint"))?;
        let dry_run = params["dry_run"].as_bool().unwrap_or(false);
        if let Some(why) = self.restore_blocker(&t) {
            bail!("{why}");
        }
        let target = self.store.checkpoint(&t.id, n)?.ok_or_else(|| anyhow!("{} has no checkpoint {n}", t.id))?;
        let cwd = PathBuf::from(&t.cwd);
        if !cwd.is_dir() {
            bail!("the folder {} is gone", t.cwd);
        }
        let undo_n = if dry_run { 0 } else { self.reserve_checkpoint(&t.id) };
        let prev = self.latest_prev(&t.id);
        if !dry_run {
            self.restoring.insert(t.id.clone());
            self.mark(&t.id);
        }
        Ok(Begin::Job(self.job(Work::Restore { thread: t.id, cwd, target: Box::new(target), undo_n, prev, dry_run })))
    }

    /// A restore (or its dry run) is done.
    pub(super) fn restore_done(&mut self, tid: &str, target: Checkpoint, undo_n: i64, dry_run: bool, result: std::result::Result<Output, String>, now: i64) -> Result<Value> {
        self.restoring.remove(tid);
        self.mark(tid);
        let r = match result {
            Ok(Output::Restored(r)) => r,
            Err(e) => {
                if !dry_run && self.store.thread(tid)?.is_some() {
                    let _ = self.store.event(tid, "lilad", "restore_failed", &format!("checkpoint {}: {e}", target.n), now);
                }
                return Err(anyhow!(e));
            }
            Ok(_) => unreachable!("a restore job gives a restore"),
        };
        let head_moved = target.head != r.head_now;
        let mut reply = restore_json(tid, &target, &r, head_moved, dry_run);
        if dry_run {
            return Ok(reply);
        }
        let Some(undo) = r.undo.as_ref() else { bail!("the restore gave no checkpoint of the state before") };
        let title = first_line(&target.text);
        if self.store.thread(tid)?.is_none() {
            return Ok(reply);
        }
        // The state before is a checkpoint like the others, so this can be undone too.
        let row = Checkpoint {
            thread: tid.to_owned(),
            n: undo_n,
            kind: CheckpointKind::Restore,
            at_ms: now,
            sha: undo.sha.clone(),
            tree: undo.tree.clone(),
            head: undo.head.clone(),
            branch: undo.branch.clone(),
            text: format!("Before the files were put back to checkpoint {}", target.n),
            item: Some(String::new()),
        };
        self.store.add_checkpoint(&row)?;
        self.store.remove_checkpoints_up_to(tid, undo_n - checkpoint::KEEP)?;
        // The conversation keeps a quiet mark of it (an item of its own, so it is there after a restart too).
        let mark = Item {
            id: format!("restored-{}-{now}", target.n),
            turn: None,
            at_ms: now,
            state: ItemState::Done,
            kind: ItemKind::UserMessage { text: format!("The files were put back to how they were before: {title}"), origin: Origin::Restored { checkpoint: target.n, undo: Some(undo_n) }, attachments: Vec::new() },
        };
        self.store.put_items(tid, &[(mark.id.clone(), now, serde_json::to_string(&mark)?)])?;
        let detail = format!(
            "checkpoint {} ({title}): {} put back, {} made again, {} deleted; undo is checkpoint {undo_n}{}",
            target.n,
            r.diff.changed.len(),
            r.diff.recreated.len(),
            r.diff.deleted.len(),
            if head_moved { "; HEAD had moved and stays" } else { "" }
        );
        self.store.event(tid, "human", "restored", &detail, now)?;
        self.say(format!("{tid}: restored {detail}"));
        // The agent learns of it with the person's next message. A restore starts no turn, so no tokens go to it.
        if !r.diff.is_empty() {
            self.store.push_note(tid, &restore_note(target.n, &title, &r.diff, head_moved), now)?;
        }
        self.refresh_changes(tid);
        self.mark(tid);
        reply["undo"] = json!(undo_n);
        reply["restored"] = json!(true);
        Ok(reply)
    }
}

/// The reply of `thread.restore`: the lists are cut at `MAX_LISTED` each, the counts are all.
fn restore_json(tid: &str, target: &Checkpoint, r: &Restored, head_moved: bool, dry_run: bool) -> Value {
    let cut = |v: &[String]| v.iter().take(checkpoint::MAX_LISTED).cloned().collect::<Vec<_>>();
    let d = &r.diff;
    let truncated = [&d.changed, &d.recreated, &d.deleted].iter().any(|l| l.len() > checkpoint::MAX_LISTED);
    json!({
        "thread": tid, "checkpoint": target.n, "dry_run": dry_run, "restored": false, "undo": null,
        "head_moved": head_moved, "head": {"then": target.head.as_deref().map(short), "now": r.head_now.as_deref().map(short)},
        "changed": cut(&d.changed), "recreated": cut(&d.recreated), "deleted": cut(&d.deleted),
        "counts": {"changed": d.changed.len(), "recreated": d.recreated.len(), "deleted": d.deleted.len()},
        "truncated": truncated,
    })
}
