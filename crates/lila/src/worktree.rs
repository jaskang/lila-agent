//! Worktree slots: a project that is a git repository of its own works on each
//! chain in one of a few worktrees (`[settings] parallel`), outside the repository
//! so search tools and editors do not see them twice. A slot is reused chain
//! after chain, so ignored build caches (`target/`, `node_modules/`) stay warm and
//! the CLIs ask to trust each folder once. Which chain a slot holds is its branch,
//! `lila/<chain>`; a free slot has a detached HEAD. The git command line, as
//! `commit.rs`: the person's git configuration and hooks apply.

use anyhow::{Context, Result, anyhow, bail};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use crate::check::tail;

/// The branch a chain works on.
pub fn branch(chain: &str) -> String {
    format!("lila/{chain}")
}

pub(crate) fn git(dir: &Path, path_env: &str, args: &[&str]) -> std::io::Result<Output> {
    Command::new("git").args(args).current_dir(dir).env("PATH", path_env).stdin(Stdio::null()).output()
}

/// Trimmed standard output, or what git said when it refused.
pub(crate) fn out(dir: &Path, path_env: &str, args: &[&str]) -> Result<String> {
    let o = git(dir, path_env, args).with_context(|| format!("git {}", args.join(" ")))?;
    if !o.status.success() {
        bail!("git {}: {}", args.join(" "), tail(&format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))));
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

/// The branch the project folder has checked out: where accepted work goes.
pub fn target(main: &Path, path_env: &str) -> Result<String> {
    out(main, path_env, &["symbolic-ref", "-q", "--short", "HEAD"]).map_err(|_| anyhow!("the project folder is not on a branch (detached HEAD)"))
}

/// The chain a slot holds, if any.
pub fn occupant(slot: &Path, path_env: &str) -> Option<String> {
    if !slot.join(".git").exists() {
        return None;
    }
    let b = out(slot, path_env, &["symbolic-ref", "-q", "--short", "HEAD"]).ok()?;
    b.strip_prefix("lila/").map(String::from)
}

/// Makes `slot` hold `chain`: the slot's worktree is made the first time, then
/// reset to the project folder's current commit on `lila/<chain>`. Untracked
/// files go; ignored ones (build caches) stay.
pub fn prepare(main: &Path, slot: &Path, chain: &str, path_env: &str) -> Result<()> {
    let base = out(main, path_env, &["rev-parse", "--verify", "-q", "HEAD"]).map_err(|_| anyhow!("the project has no commit yet; worktrees start from one"))?;
    if !slot.join(".git").exists() {
        if slot.exists() && std::fs::read_dir(slot).map(|mut d| d.next().is_some()).unwrap_or(true) {
            bail!("{} is in the way of a worktree", slot.display());
        }
        std::fs::create_dir_all(slot.parent().unwrap_or(slot))?;
        // A slot whose folder was deleted by hand is still registered: forget it first.
        let _ = git(main, path_env, &["worktree", "prune"]);
        out(main, path_env, &["worktree", "add", "-q", "--detach", &slot.to_string_lossy(), &base])?;
    }
    exclude_run_state(main, path_env)?;
    out(slot, path_env, &["reset", "-q", "--hard"])?;
    out(slot, path_env, &["clean", "-fdq"])?;
    out(slot, path_env, &["checkout", "-q", "-B", &branch(chain), &base])?;
    Ok(())
}

/// Task files and the spool are Lila's run state in every worktree: kept out of
/// git in the repository's shared `info/exclude`, so a slot never commits them
/// (a `.lila/.gitignore` the person has not committed is not in a slot).
fn exclude_run_state(main: &Path, path_env: &str) -> Result<()> {
    let common = PathBuf::from(out(main, path_env, &["rev-parse", "--git-common-dir"])?);
    let common = if common.is_absolute() { common } else { main.join(common) };
    let file = common.join("info").join("exclude");
    let mut text = std::fs::read_to_string(&file).unwrap_or_default();
    let before = text.clone();
    for line in ["/.lila/tasks/", "/.lila/spool/"] {
        if !text.lines().any(|l| l.trim() == line) {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(line);
            text.push('\n');
        }
    }
    if text != before {
        std::fs::create_dir_all(file.parent().unwrap_or(&common))?;
        std::fs::write(file, text)?;
    }
    Ok(())
}

/// The slot has work that is not in the project folder's branch yet: changed
/// files, or commits a member made.
pub fn has_changes(main: &Path, slot: &Path, path_env: &str) -> Result<bool> {
    if !out(slot, path_env, &["status", "--porcelain"])?.is_empty() {
        return Ok(true);
    }
    let target = target(main, path_env)?;
    Ok(out(slot, path_env, &["rev-list", "--count", &format!("{target}..HEAD")])? != "0")
}

/// A cancelled chain's work stays on its branch (one commit, no hooks: the branch
/// is Lila's, not the person's history), and the slot is free again.
pub fn park(slot: &Path, chain: &str, path_env: &str) -> Result<()> {
    out(slot, path_env, &["add", "-A"])?;
    let staged = !git(slot, path_env, &["diff", "--cached", "--quiet"])?.status.success();
    if staged {
        out(slot, path_env, &["commit", "-q", "--no-verify", "-m", &format!("Lila: {chain} as it was when cancelled")])?;
    }
    out(slot, path_env, &["checkout", "-q", "--detach"])?;
    Ok(())
}

/// The slot is free again; its branch goes when its work is in the project's branch.
pub fn release(main: &Path, slot: &Path, chain: &str, delete_branch: bool, path_env: &str) -> Result<()> {
    out(slot, path_env, &["checkout", "-q", "--detach"])?;
    if delete_branch {
        out(main, path_env, &["branch", "-q", "-D", &branch(chain)])?;
    }
    Ok(())
}

/// Slot folders that exist under `dir`, by number.
pub fn existing(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<(u32, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok().map(|n| (n, e.path())))
        .collect();
    v.sort();
    v.into_iter().map(|(_, p)| p).collect()
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
    fn a_slot_holds_a_chain_and_keeps_ignored_files() {
        let tmp = tempfile::tempdir().unwrap();
        let (main, slot) = (tmp.path().join("main"), tmp.path().join("wt/1"));
        repo(&main);
        let env = path_env();
        assert!(is_repo_root(&main, &env));
        assert_eq!(occupant(&slot, &env), None);

        prepare(&main, &slot, "T1", &env).unwrap();
        assert_eq!(occupant(&slot, &env).as_deref(), Some("T1"));
        assert!(!has_changes(&main, &slot, &env).unwrap());
        std::fs::create_dir_all(slot.join(".lila/tasks")).unwrap();
        std::fs::write(slot.join(".lila/tasks/T1.md"), "# T1").unwrap();
        assert!(!has_changes(&main, &slot, &env).unwrap(), "run state is not work");
        std::fs::create_dir_all(slot.join("target")).unwrap();
        std::fs::write(slot.join("target/cache"), "warm").unwrap();
        std::fs::write(slot.join("a.txt"), "changed\n").unwrap();
        std::fs::write(slot.join("new.txt"), "new\n").unwrap();
        assert!(has_changes(&main, &slot, &env).unwrap());

        // Cancelled: the work is kept on its branch, the slot is free.
        park(&slot, "T1", &env).unwrap();
        assert_eq!(occupant(&slot, &env), None);
        assert!(sh(&main, &["branch", "--list", "lila/T1"]).contains("lila/T1"));

        // The next chain starts from the project's commit, without T1's files, with the cache.
        prepare(&main, &slot, "T2", &env).unwrap();
        assert_eq!(occupant(&slot, &env).as_deref(), Some("T2"));
        assert_eq!(std::fs::read_to_string(slot.join("a.txt")).unwrap(), "a\n");
        assert!(!slot.join("new.txt").exists());
        assert_eq!(std::fs::read_to_string(slot.join("target/cache")).unwrap(), "warm");

        release(&main, &slot, "T2", true, &env).unwrap();
        assert_eq!(occupant(&slot, &env), None);
        assert!(!sh(&main, &["branch", "--list", "lila/T2"]).contains("lila/T2"));
        assert_eq!(existing(&tmp.path().join("wt")), vec![slot]);
    }

    #[test]
    fn a_member_commit_counts_as_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let (main, slot) = (tmp.path().join("main"), tmp.path().join("wt/1"));
        repo(&main);
        let env = path_env();
        prepare(&main, &slot, "T1", &env).unwrap();
        std::fs::write(slot.join("b.txt"), "b\n").unwrap();
        sh(&slot, &["add", "-A"]);
        sh(&slot, &["commit", "-q", "-m", "by a member"]);
        assert!(has_changes(&main, &slot, &env).unwrap());
    }

    #[test]
    fn no_worktrees_without_a_commit_or_inside_a_bigger_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        std::fs::create_dir_all(&main).unwrap();
        sh(&main, &["init", "-q"]);
        let err = prepare(&main, &tmp.path().join("wt/1"), "T1", &path_env()).unwrap_err();
        assert!(err.to_string().contains("no commit"), "{err:#}");
        let inner = main.join("app");
        std::fs::create_dir_all(&inner).unwrap();
        assert!(!is_repo_root(&inner, &path_env()));
        assert!(!is_repo_root(tmp.path(), &path_env()));
    }
}
