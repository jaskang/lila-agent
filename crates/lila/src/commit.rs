//! Commits a chain's changes when the person accepts it (`[settings] commit_on_accept`),
//! for lilad, off its main loop: a commit hook may take a while.

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
}

/// `git add -A`, then one commit with `job.message`. Blocks.
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
}
