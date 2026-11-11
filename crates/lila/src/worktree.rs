//! A thread's own worktree: a project that is a git repository of its own can give a thread a
//! worktree outside the repository (`<data>/worktrees/<project>/<thread>`, so search tools and
//! editors do not see it twice) on a branch of its own, `lila/<thread>`, made from the commit the
//! project folder is at. The thread's work stays there until it is merged back (`merge.rs`).
//! The git command line, as `merge.rs` and `changes.rs`: the person's git configuration and hooks apply.

use anyhow::{Context, Result, anyhow, bail};
use lila_store::Base;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use crate::check::tail;

/// The branch a thread works on.
pub fn branch(thread: &str) -> String {
    format!("lila/{thread}")
}

pub(crate) fn git(dir: &Path, path_env: &str, args: &[&str]) -> std::io::Result<Output> {
    Command::new("git").args(args).current_dir(dir).env("PATH", path_env).stdin(Stdio::null()).output()
}

/// What git said, for a message: the end of its output, both streams.
pub(crate) fn said(o: &Output) -> String {
    tail(&format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
}

/// Trimmed standard output, or what git said when it refused.
pub(crate) fn out(dir: &Path, path_env: &str, args: &[&str]) -> Result<String> {
    let o = git(dir, path_env, args).with_context(|| format!("git {}", args.join(" ")))?;
    if !o.status.success() {
        bail!("git {}: {}", args.join(" "), said(&o));
    }
    Ok(String::from_utf8_lossy(&o.stdout).trim().to_owned())
}

/// The folder is the top of a git repository (not a folder inside a bigger one,
/// such as a home folder kept in git): only then are worktrees of it made.
pub fn is_repo_root(dir: &Path, path_env: &str) -> bool {
    let Ok(top) = out(dir, path_env, &["rev-parse", "--show-toplevel"]) else { return false };
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(Path::new(&top)) == canon(dir)
}

/// The branch `dir` has checked out; `None` in a detached HEAD (or outside a repository).
pub fn current_branch(dir: &Path, path_env: &str) -> Option<String> {
    out(dir, path_env, &["symbolic-ref", "-q", "--short", "HEAD"]).ok().filter(|b| !b.is_empty())
}

/// Makes a thread's worktree at `path` on a new branch `branch`, from the commit the project folder
/// `main` is at, and says where that was. Blocks: a checkout of a big repository takes a while.
pub fn create(main: &Path, path: &Path, branch: &str, path_env: &str) -> Result<Base> {
    if !is_repo_root(main, path_env) {
        bail!("the project folder is not the top of a git repository, so a thread can't have a worktree of its own");
    }
    let base_branch = current_branch(main, path_env).ok_or_else(|| anyhow!("the project folder is not on a branch (detached HEAD); check one out to start a thread in a worktree"))?;
    let commit = out(main, path_env, &["rev-parse", "--verify", "-q", "HEAD"]).map_err(|_| anyhow!("the project has no commit yet; a worktree starts from one"))?;
    if path.exists() && std::fs::read_dir(path).map(|mut d| d.next().is_some()).unwrap_or(true) {
        bail!("{} is in the way of a worktree", path.display());
    }
    std::fs::create_dir_all(path.parent().unwrap_or(path))?;
    // A worktree whose folder was deleted by hand is still registered, and may hold the name.
    let _ = git(main, path_env, &["worktree", "prune"]);
    out(main, path_env, &["worktree", "add", "-q", "-b", branch, &path.to_string_lossy(), &commit])?;
    Ok(Base { branch: base_branch, commit })
}

/// Takes the worktree at `path` away, and its branch with it. What is left in it (ignored build
/// caches, nothing else once its work was merged) goes too.
pub fn remove(main: &Path, path: &Path, branch: &str, path_env: &str) -> Result<()> {
    if path.exists() {
        out(main, path_env, &["worktree", "remove", "--force", &path.to_string_lossy()])?;
    }
    let _ = std::fs::remove_dir_all(path);
    let _ = git(main, path_env, &["worktree", "prune"]);
    if out(main, path_env, &["rev-parse", "--verify", "-q", &format!("refs/heads/{branch}")]).is_ok() {
        out(main, path_env, &["branch", "-q", "-D", branch])?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn path_env() -> String {
        std::env::var("PATH").unwrap_or_default()
    }

    pub(crate) fn sh(dir: &Path, args: &[&str]) -> String {
        out(dir, &path_env(), args).unwrap_or_else(|e| panic!("{e:#}"))
    }

    /// A repository with one commit on `main`.
    pub(crate) fn repo(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        sh(dir, &["init", "-q", "-b", "main"]);
        sh(dir, &["config", "user.email", "lila@example.com"]);
        sh(dir, &["config", "user.name", "Lila"]);
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();
        std::fs::write(dir.join(".gitignore"), "target/\n").unwrap();
        sh(dir, &["add", "-A"]);
        sh(dir, &["commit", "-q", "-m", "first"]);
    }

    #[test]
    fn a_worktree_starts_from_the_project_s_commit_on_a_branch_of_its_own() {
        let tmp = tempfile::tempdir().unwrap();
        let (main, wt) = (tmp.path().join("main"), tmp.path().join("wt/t00000001"));
        repo(&main);
        let env = path_env();
        assert!(is_repo_root(&main, &env));
        let start = create(&main, &wt, &branch("t00000001"), &env).unwrap();
        assert_eq!((start.branch.as_str(), start.commit.as_str()), ("main", sh(&main, &["rev-parse", "HEAD"]).as_str()));
        assert_eq!(current_branch(&wt, &env).as_deref(), Some("lila/t00000001"));
        assert_eq!(std::fs::read_to_string(wt.join("a.txt")).unwrap(), "a\n");
        assert_eq!(current_branch(&main, &env).as_deref(), Some("main"), "the project folder is where it was");

        // Nothing is in the way of it going again.
        std::fs::create_dir_all(wt.join("target")).unwrap();
        std::fs::write(wt.join("target/cache"), "ignored").unwrap();
        remove(&main, &wt, "lila/t00000001", &env).unwrap();
        assert!(!wt.exists());
        assert!(sh(&main, &["branch", "--list", "lila/*"]).is_empty());
        assert!(!sh(&main, &["worktree", "list"]).contains("wt/"));
        remove(&main, &wt, "lila/t00000001", &env).unwrap();
        create(&main, &wt, &branch("t00000001"), &env).unwrap();
    }

    #[test]
    fn no_worktrees_without_a_commit_a_branch_or_inside_a_bigger_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let (main, env) = (tmp.path().join("main"), path_env());
        std::fs::create_dir_all(&main).unwrap();
        sh(&main, &["init", "-q"]);
        let err = create(&main, &tmp.path().join("wt/1"), "lila/t1", &env).unwrap_err();
        assert!(err.to_string().contains("no commit"), "{err:#}");
        let inner = main.join("app");
        std::fs::create_dir_all(&inner).unwrap();
        assert!(!is_repo_root(&inner, &env));
        assert!(!is_repo_root(tmp.path(), &env));
        let err = create(&inner, &tmp.path().join("wt/2"), "lila/t2", &env).unwrap_err();
        assert!(err.to_string().contains("not the top of a git repository"), "{err:#}");

        let (other, wt) = (tmp.path().join("other"), tmp.path().join("wt/3"));
        repo(&other);
        sh(&other, &["checkout", "-q", "--detach"]);
        assert_eq!(current_branch(&other, &env), None);
        let err = create(&other, &wt, "lila/t3", &env).unwrap_err();
        assert!(err.to_string().contains("detached HEAD"), "{err:#}");
        assert!(!wt.exists(), "nothing is left behind");
    }

    #[test]
    fn a_folder_in_the_way_is_not_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let (main, wt) = (tmp.path().join("main"), tmp.path().join("wt/t1"));
        repo(&main);
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join("mine"), "x").unwrap();
        let err = create(&main, &wt, "lila/t1", &path_env()).unwrap_err();
        assert!(err.to_string().contains("in the way"), "{err:#}");
        assert!(wt.join("mine").exists());
    }
}
