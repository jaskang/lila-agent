//! Merging a worktree thread's work back: everything its branch has beyond the branch the project
//! folder was on becomes one commit on top of that branch's latest commit, the project folder's
//! branch moves to it (`--ff-only`), and the worktree and its branch go (`worktree::remove`).
//! Blocks (a commit hook may take a while), so lilad runs it off its loop (`daemon/jobs.rs`).
//! Whatever goes wrong, the worktree's branch is left as it was and the project folder is not touched.

use crate::worktree::{current_branch, git, out, said};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// A merge lilad wants made.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    /// The thread's title: the first line of the commit when there is more than one.
    pub title: String,
    /// The project folder, on `base_branch`.
    pub main: PathBuf,
    /// The thread's worktree, on `branch`.
    pub worktree: PathBuf,
    pub branch: String,
    pub base_branch: String,
    pub path_env: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Merged {
    /// The new commit's short hash.
    pub sha: String,
    /// How many commits of the thread it holds.
    pub commits: usize,
    /// How many files it changes.
    pub files: usize,
}

/// The commit message of several commits made one: the thread's title, then each one's title.
pub fn squash_message(title: &str, subjects: &[String]) -> String {
    let title = title.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("Lila thread");
    let list: Vec<String> = subjects.iter().map(|s| format!("- {s}")).collect();
    format!("{title}\n\n{}\n", list.join("\n"))
}

/// The files the person has changed in the project folder, untracked ones too.
fn dirty_files(main: &Path, env: &str) -> Result<Vec<String>> {
    let o = git(main, env, &["--no-optional-locks", "status", "--porcelain", "-z", "-uall"])?;
    if !o.status.success() {
        bail!("git status: {}", said(&o));
    }
    let mut files = Vec::new();
    let mut fields = o.stdout.split(|b| *b == 0).filter(|f| !f.is_empty());
    while let Some(f) = fields.next() {
        let f = String::from_utf8_lossy(f);
        if f.len() < 4 {
            continue;
        }
        files.push(f[3..].to_owned());
        // A rename lists the old name next.
        if f.starts_with(['R', 'C']) || f[1..2].starts_with(['R', 'C']) {
            files.extend(fields.next().map(|p| String::from_utf8_lossy(p).into_owned()));
        }
    }
    Ok(files)
}

fn commit(dir: &Path, env: &str, message: &str) -> Result<()> {
    use std::io::Write;
    let mut child = std::process::Command::new("git")
        .args(["commit", "-q", "-F", "-"])
        .current_dir(dir)
        .env("PATH", env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("git commit")?;
    child.stdin.take().expect("piped").write_all(message.as_bytes())?;
    let o = child.wait_with_output()?;
    if !o.status.success() {
        bail!("{}", said(&o));
    }
    Ok(())
}

/// Squashes, puts the commit on the base branch's latest, moves the project folder's branch to it.
pub fn run(job: &Job) -> Result<Merged> {
    let (main, wt, env, base) = (job.main.as_path(), job.worktree.as_path(), job.path_env.as_str(), job.base_branch.as_str());
    if !wt.join(".git").exists() {
        bail!("the worktree {} is gone", wt.display());
    }
    match current_branch(wt, env) {
        Some(b) if b == job.branch => {}
        Some(b) => bail!("the worktree is on {b}, not on its branch {}; check that out first", job.branch),
        None => bail!("the worktree is in a detached HEAD, not on its branch {}; check that out first", job.branch),
    }
    if !out(wt, env, &["status", "--porcelain"])?.is_empty() {
        bail!("the worktree has uncommitted changes; commit them first");
    }
    if out(main, env, &["rev-parse", "--verify", "-q", &format!("refs/heads/{base}")]).is_err() {
        bail!("the branch {base} the thread started from is gone");
    }
    match current_branch(main, env) {
        Some(b) if b == base => {}
        Some(b) => bail!("the project folder is on {b}, not on {base} where the thread started; check {base} out first"),
        None => bail!("the project folder is in a detached HEAD; check {base} out first"),
    }
    let range = format!("{base}..HEAD");
    let subjects: Vec<String> = out(wt, env, &["log", "--reverse", "--format=%s", &range])?.lines().map(String::from).collect();
    if subjects.is_empty() {
        bail!("nothing to merge: the thread's branch has no commit that {base} does not");
    }
    // The files this brings in must not be ones the person is changing in the project folder: git would not overwrite them, and says so late.
    let touched: Vec<String> = out(wt, env, &["diff", "--name-only", "--no-renames", &format!("{base}...HEAD")])?.lines().map(String::from).collect();
    let mine = dirty_files(main, env)?;
    let clash: Vec<&String> = touched.iter().filter(|f| mine.contains(f)).collect();
    if !clash.is_empty() {
        let names: Vec<&str> = clash.iter().take(8).map(|f| f.as_str()).collect();
        bail!("the project folder has uncommitted changes to files this merge changes: {}{}; commit or stash them first", names.join(", "), if clash.len() > 8 { ", …" } else { "" });
    }

    let original = out(wt, env, &["rev-parse", "HEAD"])?;
    // Back to how it was, whatever went wrong: the thread's commits stay as they are.
    let restore = |why: anyhow::Error| -> anyhow::Error {
        match out(wt, env, &["reset", "-q", "--hard", &original]) {
            Ok(_) => why,
            Err(e) => why.context(format!("and the worktree could not be put back to {original}: {e:#}")),
        }
    };
    if subjects.len() > 1 {
        let fork = out(wt, env, &["merge-base", base, "HEAD"])?;
        out(wt, env, &["reset", "-q", "--soft", &fork])?;
        if git(wt, env, &["diff", "--cached", "--quiet"])?.status.success() {
            return Err(restore(anyhow::anyhow!("nothing to merge: the thread's commits cancel each other out")));
        }
        if let Err(e) = commit(wt, env, &squash_message(&job.title, &subjects)) {
            return Err(restore(e.context("git commit (the thread's commits, as one)")));
        }
    }
    // The base branch may move between the rebase and the merge (the person committed): rebase again, a few times.
    let mut tries = 0;
    loop {
        if !git(wt, env, &["rebase", "-q", base])?.status.success() {
            let files = out(wt, env, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
            let _ = git(wt, env, &["rebase", "--abort"]);
            let files: Vec<&str> = files.lines().collect();
            return Err(restore(anyhow::anyhow!("the thread's changes conflict with what {base} has gained since it started{}; the worktree is as it was. Ask the agent to bring {base} into its branch and resolve it", if files.is_empty() { String::new() } else { format!(" ({})", files.join(", ")) })));
        }
        match out(main, env, &["merge", "-q", "--ff-only", &job.branch]) {
            Ok(_) => break,
            Err(e) if tries < 3 && e.to_string().contains("Not possible to fast-forward") => tries += 1,
            Err(e) => return Err(restore(e)),
        }
    }
    Ok(Merged { sha: out(main, env, &["rev-parse", "--short", "HEAD"])?, commits: subjects.len(), files: touched.len() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worktree::{self, tests::{path_env, repo, sh}};

    struct Setup {
        _tmp: tempfile::TempDir,
        main: PathBuf,
        wt: PathBuf,
    }

    impl Setup {
        fn new() -> Setup {
            let tmp = tempfile::tempdir().unwrap();
            let (main, wt) = (tmp.path().join("main"), tmp.path().join("wt/t1"));
            repo(&main);
            worktree::create(&main, &wt, "lila/t1", &path_env()).unwrap();
            Setup { _tmp: tmp, main, wt }
        }

        fn job(&self) -> Job {
            Job { title: "Add login".into(), main: self.main.clone(), worktree: self.wt.clone(), branch: "lila/t1".into(), base_branch: "main".into(), path_env: path_env() }
        }

        /// A commit in the worktree.
        fn work(&self, file: &str, text: &str, message: &str) {
            std::fs::write(self.wt.join(file), text).unwrap();
            sh(&self.wt, &["add", "-A"]);
            sh(&self.wt, &["commit", "-q", "-m", message]);
        }

        /// A commit on the project folder's branch.
        fn on_main(&self, file: &str, text: &str, message: &str) {
            std::fs::write(self.main.join(file), text).unwrap();
            sh(&self.main, &["add", "-A"]);
            sh(&self.main, &["commit", "-q", "-m", message]);
        }

        fn refused(&self, expect: &str) -> (String, String) {
            let (wt_tip, main_tip) = (sh(&self.wt, &["rev-parse", "HEAD"]), sh(&self.main, &["rev-parse", "HEAD"]));
            let err = format!("{:#}", run(&self.job()).unwrap_err());
            assert!(err.contains(expect), "{err}");
            assert_eq!(sh(&self.wt, &["rev-parse", "HEAD"]), wt_tip, "the worktree's branch is as it was");
            assert_eq!(sh(&self.main, &["rev-parse", "HEAD"]), main_tip, "the project folder did not move");
            assert!(!self.wt.join(".git").is_dir() && !sh(&self.wt, &["status", "--porcelain"]).contains("UU"));
            (wt_tip, main_tip)
        }
    }

    #[test]
    fn one_commit_goes_over_with_its_own_message() {
        let s = Setup::new();
        s.work("b.txt", "b\n", "Add b\n\nWith a body.");
        let m = run(&s.job()).unwrap();
        assert_eq!((m.commits, m.files), (1, 1));
        assert_eq!(sh(&s.main, &["rev-parse", "--short", "HEAD"]), m.sha);
        assert_eq!(sh(&s.main, &["log", "--format=%s"]), "Add b\nfirst");
        assert_eq!(sh(&s.main, &["log", "-1", "--format=%B"]), "Add b\n\nWith a body.");
        assert_eq!(std::fs::read_to_string(s.main.join("b.txt")).unwrap(), "b\n", "the project folder has the file");
        assert_eq!(sh(&s.main, &["status", "--porcelain"]), "");
    }

    #[test]
    fn several_commits_become_one_on_top_of_what_the_base_gained() {
        let s = Setup::new();
        s.work("b.txt", "b\n", "wip b");
        s.work("c.txt", "c\n", "wip c");
        s.work("b.txt", "b2\n", "fix b");
        // Meanwhile the base branch moved, and the person has a change of their own in the folder.
        s.on_main("d.txt", "d\n", "other work");
        std::fs::write(s.main.join("mine.txt"), "mine\n").unwrap();
        let m = run(&s.job()).unwrap();
        assert_eq!((m.commits, m.files), (3, 2));
        assert_eq!(sh(&s.main, &["log", "--format=%s"]), "Add login\nother work\nfirst", "one commit, on top of the base's latest");
        assert_eq!(sh(&s.main, &["log", "-1", "--format=%B"]), "Add login\n\n- wip b\n- wip c\n- fix b");
        assert_eq!(std::fs::read_to_string(s.main.join("b.txt")).unwrap(), "b2\n");
        assert!(s.main.join("c.txt").exists() && s.main.join("d.txt").exists());
        assert_eq!(std::fs::read_to_string(s.main.join("mine.txt")).unwrap(), "mine\n", "the person's own change is untouched");
        // The worktree and its branch go with `worktree::remove`.
        worktree::remove(&s.main, &s.wt, "lila/t1", &path_env()).unwrap();
        assert!(!s.wt.exists() && sh(&s.main, &["branch", "--list", "lila/*"]).is_empty());
    }

    #[test]
    fn nothing_is_merged_that_would_lose_work_or_that_is_not_there() {
        let s = Setup::new();
        s.refused("nothing to merge");
        s.work("b.txt", "b\n", "Add b");
        // Uncommitted work in the worktree.
        std::fs::write(s.wt.join("loose.txt"), "x").unwrap();
        s.refused("uncommitted changes; commit them first");
        std::fs::remove_file(s.wt.join("loose.txt")).unwrap();
        // The project folder is somewhere else.
        sh(&s.main, &["checkout", "-q", "-b", "other"]);
        s.refused("on other, not on main");
        sh(&s.main, &["checkout", "-q", "--detach"]);
        s.refused("detached HEAD");
        sh(&s.main, &["checkout", "-q", "main"]);
        // The person is changing a file the merge changes (here: a file of their own that it adds too).
        std::fs::write(s.main.join("b.txt"), "mine\n").unwrap();
        s.refused("uncommitted changes to files this merge changes: b.txt");
        std::fs::remove_file(s.main.join("b.txt")).unwrap();
        // The worktree was moved off its branch.
        sh(&s.wt, &["checkout", "-q", "--detach"]);
        s.refused("detached HEAD, not on its branch");
        sh(&s.wt, &["checkout", "-q", "lila/t1"]);
        // And now it goes.
        run(&s.job()).unwrap();
    }

    #[test]
    fn a_staged_change_in_the_project_folder_is_in_the_way_too() {
        let s = Setup::new();
        s.work("a.txt", "from the thread\n", "Change a");
        std::fs::write(s.main.join("a.txt"), "mine\n").unwrap();
        sh(&s.main, &["add", "a.txt"]);
        s.refused("a.txt");
        assert_eq!(sh(&s.main, &["diff", "--cached", "--name-only"]), "a.txt", "and it stays staged");
    }

    #[test]
    fn a_conflict_with_the_base_puts_everything_back() {
        let s = Setup::new();
        s.work("a.txt", "thread one\n", "first change");
        s.work("a.txt", "thread two\n", "second change");
        s.on_main("a.txt", "main\n", "main changed a");
        let (wt_tip, _) = s.refused("conflict");
        let log = sh(&s.wt, &["log", "--format=%s", "main..HEAD"]);
        assert_eq!(log, "second change\nfirst change", "its commits are not squashed away");
        assert_eq!(sh(&s.wt, &["rev-parse", "HEAD"]), wt_tip);
        assert_eq!(sh(&s.wt, &["status", "--porcelain"]), "");
        assert!(!s.wt.join(".git").is_dir());
        let gitdir = sh(&s.wt, &["rev-parse", "--git-dir"]);
        assert!(!Path::new(&gitdir).join("rebase-merge").exists() && !Path::new(&gitdir).join("rebase-apply").exists(), "no rebase left half done");
        assert!(format!("{:#}", run(&s.job()).unwrap_err()).contains("a.txt"));
        assert_eq!(std::fs::read_to_string(s.main.join("a.txt")).unwrap(), "main\n");
    }

    #[test]
    fn a_hook_that_refuses_the_squash_puts_everything_back() {
        use std::os::unix::fs::PermissionsExt;
        let s = Setup::new();
        s.work("b.txt", "b\n", "one");
        s.work("c.txt", "c\n", "two");
        let hook = PathBuf::from(sh(&s.main, &["rev-parse", "--git-common-dir"])).join("hooks/pre-commit");
        let hook = if hook.is_absolute() { hook } else { s.main.join(hook) };
        std::fs::write(&hook, "#!/bin/sh\necho lint failed\nexit 1\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        s.refused("lint failed");
        assert_eq!(sh(&s.wt, &["log", "--format=%s", "main..HEAD"]), "two\none");
        assert_eq!(sh(&s.wt, &["status", "--porcelain"]), "");
        // With one commit there is nothing to squash: no commit is made, no hook runs.
        sh(&s.wt, &["reset", "-q", "--hard", "HEAD~1"]);
        run(&s.job()).unwrap();
    }

    #[test]
    fn commits_that_cancel_out_are_not_merged() {
        let s = Setup::new();
        s.work("b.txt", "b\n", "add");
        sh(&s.wt, &["rm", "-q", "b.txt"]);
        sh(&s.wt, &["commit", "-q", "-m", "remove"]);
        s.refused("cancel each other out");
        assert_eq!(sh(&s.wt, &["log", "--format=%s", "main..HEAD"]), "remove\nadd");
    }

    #[test]
    fn the_message_of_several_commits() {
        assert_eq!(squash_message("Fix the login\nmore", &["a".into(), "b".into()]), "Fix the login\n\n- a\n- b\n");
        assert_eq!(squash_message("", &["a".into()]), "Lila thread\n\n- a\n");
    }
}
