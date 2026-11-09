//! Commits a chain's changes when the person accepts it (`[settings] commit_on_accept`),
//! for lilad, off its main loop: a commit hook may take a while. A chain that
//! worked in a worktree slot (`worktree`) becomes one commit on top of the project
//! folder's branch, which then moves to it (`--ff-only`).

use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::check::tail;

/// A commit lilad wants made; the result comes back as `Daemon::on_commit_done`.
#[derive(Debug, Clone, PartialEq)]
pub struct CommitJob {
    pub project: String,
    /// The chain's root task.
    pub chain: String,
    pub cwd: PathBuf,
    pub message: String,
    pub path_env: String,
    /// The chain worked in a slot (`cwd`): its work goes into this project folder's branch.
    pub merge_into: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CommitResult {
    /// The new commit's short hash.
    Committed(String),
    /// Nothing had changed.
    Nothing,
    /// The folder is not in a git repository.
    NotRepo,
    /// git refused (a hook, no identity, …): the end of what it said.
    Failed(String),
    /// The chain's commit does not apply on the project's branch any more: these files conflict.
    /// The commit is on the chain's branch.
    Conflict(Vec<String>),
    /// The project folder could not move to the chain's commit (its own changes are
    /// in the way): what git said. The commit is on the chain's branch.
    NotMerged(String),
}

/// `git add -A`, then one commit with `job.message`; from a slot, then onto the
/// project's branch. Blocks.
pub fn run(job: &CommitJob) -> CommitResult {
    let git = |args: &[&str]| Command::new("git").args(args).current_dir(&job.cwd).env("PATH", &job.path_env).stdin(Stdio::null()).output();
    match git(&["rev-parse", "--is-inside-work-tree"]) {
        Ok(o) if o.status.success() => {}
        _ => return CommitResult::NotRepo,
    }
    let failed = |what: &str, o: std::io::Result<std::process::Output>| match o {
        Ok(o) => CommitResult::Failed(tail(&format!("{what}: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))),
        Err(e) => CommitResult::Failed(format!("{what}: {e}")),
    };
    match git(&["add", "-A"]) {
        Ok(o) if o.status.success() => {}
        o => return failed("git add -A", o),
    }
    let mut target = None;
    if let Some(main) = &job.merge_into {
        // Commits members made in the slot and what is left uncommitted become one commit.
        let t = match crate::worktree::target(main, &job.path_env) {
            Ok(t) => t,
            Err(e) => return CommitResult::NotMerged(e.to_string()),
        };
        let base = match crate::worktree::out(&job.cwd, &job.path_env, &["merge-base", &t, "HEAD"]) {
            Ok(b) => b,
            Err(e) => return CommitResult::Failed(e.to_string()),
        };
        if let Err(e) = crate::worktree::out(&job.cwd, &job.path_env, &["reset", "-q", "--soft", &base]) {
            return CommitResult::Failed(e.to_string());
        }
        target = Some((main, t));
    }
    if git(&["diff", "--cached", "--quiet"]).is_ok_and(|o| o.status.success()) {
        return CommitResult::Nothing;
    }
    let commit = Command::new("git")
        .args(["commit", "-q", "-F", "-"])
        .current_dir(&job.cwd)
        .env("PATH", &job.path_env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().expect("piped").write_all(job.message.as_bytes())?;
            child.wait_with_output()
        });
    match commit {
        Ok(o) if o.status.success() => {}
        o => return failed("git commit", o),
    }
    if let Some((main, t)) = target {
        if !git(&["rebase", "-q", &t]).is_ok_and(|o| o.status.success()) {
            let files = crate::worktree::out(&job.cwd, &job.path_env, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
            let _ = git(&["rebase", "--abort"]);
            return CommitResult::Conflict(files.lines().map(String::from).collect());
        }
        let branch = match crate::worktree::out(&job.cwd, &job.path_env, &["symbolic-ref", "-q", "--short", "HEAD"]) {
            Ok(b) => b,
            Err(e) => return CommitResult::Failed(e.to_string()),
        };
        if let Err(e) = crate::worktree::out(main, &job.path_env, &["merge", "-q", "--ff-only", &branch]) {
            return CommitResult::NotMerged(e.to_string());
        }
    }
    match git(&["rev-parse", "--short", "HEAD"]) {
        Ok(o) if o.status.success() => CommitResult::Committed(String::from_utf8_lossy(&o.stdout).trim().to_owned()),
        o => failed("git rev-parse", o),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(cwd: &std::path::Path) -> CommitJob {
        CommitJob {
            project: "p".into(),
            chain: "T1".into(),
            cwd: cwd.to_path_buf(),
            message: "Add login\n\nLila-Task: T1".into(),
            path_env: std::env::var("PATH").unwrap_or_default(),
            merge_into: None,
        }
    }

    fn git(dir: &std::path::Path, args: &[&str]) -> String {
        let o = Command::new("git").args(args).current_dir(dir).output().unwrap();
        assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into_owned()
    }

    #[test]
    fn commits_what_changed_once() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        git(dir, &["init", "-q"]);
        git(dir, &["config", "user.email", "lila@example.com"]);
        git(dir, &["config", "user.name", "Lila"]);
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        let CommitResult::Committed(sha) = run(&job(dir)) else { panic!("not committed") };
        assert_eq!(git(dir, &["rev-parse", "--short", "HEAD"]).trim(), sha);
        assert_eq!(git(dir, &["log", "-1", "--format=%B"]).trim(), "Add login\n\nLila-Task: T1");
        assert_eq!(run(&job(dir)), CommitResult::Nothing);
    }

    #[test]
    fn outside_a_repository() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(run(&job(tmp.path())), CommitResult::NotRepo);
    }

    #[test]
    fn a_refusing_hook_fails_it() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        git(dir, &["init", "-q"]);
        git(dir, &["config", "user.email", "lila@example.com"]);
        git(dir, &["config", "user.name", "Lila"]);
        let hook = dir.join(".git/hooks/pre-commit");
        std::fs::write(&hook, "#!/bin/sh\necho lint failed\nexit 1\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        let CommitResult::Failed(why) = run(&job(dir)) else { panic!("committed") };
        assert!(why.contains("lint failed"), "{why}");
    }

    mod slots {
        use super::*;
        use crate::worktree::{self, tests::{path_env, repo, sh}};

        fn slot_job(main: &std::path::Path, slot: &std::path::Path) -> CommitJob {
            CommitJob { merge_into: Some(main.to_path_buf()), ..job(slot) }
        }

        #[test]
        fn a_slot_becomes_one_commit_on_the_project_branch() {
            let tmp = tempfile::tempdir().unwrap();
            let (main, slot) = (tmp.path().join("main"), tmp.path().join("wt/1"));
            repo(&main);
            worktree::prepare(&main, &slot, "T1", &path_env()).unwrap();
            // A member committed once, and left more uncommitted.
            std::fs::write(slot.join("b.txt"), "b\n").unwrap();
            sh(&slot, &["add", "-A"]);
            sh(&slot, &["commit", "-q", "-m", "wip"]);
            std::fs::write(slot.join("c.txt"), "c\n").unwrap();
            // Meanwhile another chain's work reached main.
            std::fs::write(main.join("d.txt"), "d\n").unwrap();
            sh(&main, &["add", "-A"]);
            sh(&main, &["commit", "-q", "-m", "other chain"]);
            // The person has their own unrelated change in the project folder.
            std::fs::write(main.join("mine.txt"), "mine\n").unwrap();

            let CommitResult::Committed(sha) = run(&slot_job(&main, &slot)) else { panic!("not committed") };
            assert_eq!(sh(&main, &["rev-parse", "--short", "HEAD"]), sha);
            assert_eq!(sh(&main, &["log", "--format=%s"]), "Add login\nother chain\nfirst");
            assert!(main.join("b.txt").exists() && main.join("c.txt").exists());
            assert_eq!(std::fs::read_to_string(main.join("mine.txt")).unwrap(), "mine\n");
        }

        #[test]
        fn a_conflict_keeps_the_commit_on_the_chain_branch() {
            let tmp = tempfile::tempdir().unwrap();
            let (main, slot) = (tmp.path().join("main"), tmp.path().join("wt/1"));
            repo(&main);
            worktree::prepare(&main, &slot, "T1", &path_env()).unwrap();
            std::fs::write(slot.join("a.txt"), "slot\n").unwrap();
            std::fs::write(main.join("a.txt"), "main\n").unwrap();
            sh(&main, &["commit", "-q", "-am", "main changed a"]);
            assert_eq!(run(&slot_job(&main, &slot)), CommitResult::Conflict(vec!["a.txt".into()]));
            assert_eq!(sh(&slot, &["log", "-1", "--format=%s"]), "Add login");
            assert_eq!(sh(&slot, &["status", "--porcelain"]), "");
            assert_eq!(sh(&main, &["log", "-1", "--format=%s"]), "main changed a");
        }

        #[test]
        fn the_person_s_own_change_in_the_way_is_not_overwritten() {
            let tmp = tempfile::tempdir().unwrap();
            let (main, slot) = (tmp.path().join("main"), tmp.path().join("wt/1"));
            repo(&main);
            worktree::prepare(&main, &slot, "T1", &path_env()).unwrap();
            std::fs::write(slot.join("a.txt"), "slot\n").unwrap();
            std::fs::write(main.join("a.txt"), "mine, uncommitted\n").unwrap();
            let CommitResult::NotMerged(why) = run(&slot_job(&main, &slot)) else { panic!("merged") };
            assert!(why.contains("a.txt"), "{why}");
            assert_eq!(std::fs::read_to_string(main.join("a.txt")).unwrap(), "mine, uncommitted\n");
            assert_eq!(sh(&slot, &["log", "-1", "--format=%s"]), "Add login");
        }

        #[test]
        fn nothing_changed_in_the_slot() {
            let tmp = tempfile::tempdir().unwrap();
            let (main, slot) = (tmp.path().join("main"), tmp.path().join("wt/1"));
            repo(&main);
            worktree::prepare(&main, &slot, "T1", &path_env()).unwrap();
            assert_eq!(run(&slot_job(&main, &slot)), CommitResult::Nothing);
        }
    }
}
