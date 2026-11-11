//! Checkpoints: before each turn lilad snapshots the thread's folder with plain git, so a turn that
//! broke things can be undone ("回到这里"). Only files go back; the conversation stays.
//!
//! A snapshot is a commit made without touching the person's repository state: the person's index is
//! copied (so git reuses its stat cache), `git add -A` runs on the copy, `git write-tree` and
//! `git commit-tree` (parent: HEAD, when there is one) make the commit, and a ref under
//! `refs/lila/checkpoints/<thread>/<n>` keeps it alive. HEAD, branches, the index, the stash and the
//! working tree are not changed. What is snapshotted is the thread's folder (`cwd`), not the whole
//! repository when `cwd` is a subfolder of it: that is the workspace the thread's permissions are
//! about, and a restore deletes files, so it stays inside it. Files `.gitignore` leaves out are in no
//! snapshot and no restore touches them.
//!
//! Everything here blocks (a big repository takes a while), so lilad runs it off its loop (`daemon/jobs.rs`).

use crate::changes::{Git, copy_index};
use crate::worktree::said;
use anyhow::{Context, Result, anyhow, bail};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// How many numbers back a thread's checkpoints are kept: a checkpoint older than the latest one minus this is dropped, ref and row.
pub const KEEP: i64 = 50;
/// Most files listed in each of the three lists of a restore; the counts cover all of them.
pub const MAX_LISTED: usize = 500;

/// The ref that keeps a checkpoint's commit alive.
pub fn ref_name(thread: &str, n: i64) -> String {
    format!("refs/lila/checkpoints/{thread}/{n}")
}

/// The folder or a folder above it has a `.git`: a check that starts no process, to leave out the folders that are certainly not
/// in a repository (the answer of git, `snapshot`, is the one that counts).
pub fn maybe_repo(dir: &Path) -> bool {
    dir.ancestors().any(|d| d.join(".git").exists())
}

/// The thread's latest checkpoint, to see whether nothing changed since.
#[derive(Debug, Clone)]
pub struct Prev {
    pub sha: String,
    pub tree: String,
    pub head: Option<String>,
}

/// A checkpoint to make.
pub struct Spec<'a> {
    pub thread: &'a str,
    pub n: i64,
    pub prev: Option<&'a Prev>,
    /// Checkpoints numbered this or lower are dropped (their refs).
    pub keep_below: i64,
}

/// A checkpoint that was made.
#[derive(Debug, Clone, PartialEq)]
pub struct Taken {
    pub sha: String,
    pub tree: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    /// Nothing had changed since the previous one (same tree and HEAD), whose commit this is.
    pub reused: bool,
}

/// The folder as it is: its tree, and the HEAD and branch it is on.
struct Snap {
    tree: String,
    head: Option<String>,
    head_tree: Option<String>,
    branch: Option<String>,
}

/// Git for the folder, or `None` when the folder is not (in) a git work tree.
fn repo<'a>(cwd: &'a Path, path_env: &'a str) -> Result<Option<Git<'a>>> {
    if !cwd.is_dir() {
        return Ok(None);
    }
    let g = Git { cwd, path_env, index: None };
    let o = g.command(&["rev-parse", "--is-inside-work-tree"]).output().context("git rev-parse")?;
    if !o.status.success() {
        if String::from_utf8_lossy(&o.stderr).contains("not a git repository") {
            return Ok(None);
        }
        bail!("git rev-parse: {}", said(&o));
    }
    Ok((String::from_utf8_lossy(&o.stdout).trim() == "true").then_some(g))
}

/// Writes the folder's tree, on a copy of the person's index.
fn snapshot(g: &Git) -> Result<Snap> {
    let head = g.text(&["rev-parse", "--verify", "-q", "HEAD"]).ok().filter(|h| !h.is_empty());
    let head_tree = match &head {
        Some(h) => Some(g.text(&["rev-parse", &format!("{h}^{{tree}}")])?),
        None => None,
    };
    let branch = g.text(&["symbolic-ref", "-q", "--short", "HEAD"]).ok().filter(|b| !b.is_empty());
    let index = copy_index(g)?;
    let gi = Git { index: Some(&index.0), ..*g };
    // Everything in the folder that is not ignored: changed and staged files, new files, and what was deleted.
    gi.bytes(&["add", "-A", "--", "."])?;
    let tree = gi.text(&["write-tree"])?;
    Ok(Snap { tree, head, head_tree, branch })
}

/// The commit for a snapshot: the previous checkpoint's when nothing changed since, HEAD itself when the folder is exactly its tree,
/// else a new commit on top of HEAD (no parent before the first commit).
fn commit_of(g: &Git, snap: &Snap, thread: &str, n: i64, prev: Option<&Prev>) -> Result<(String, bool)> {
    if let Some(p) = prev.filter(|p| p.tree == snap.tree && p.head == snap.head) {
        return Ok((p.sha.clone(), true));
    }
    if let (Some(head), Some(tree)) = (&snap.head, &snap.head_tree)
        && *tree == snap.tree
    {
        return Ok((head.clone(), false));
    }
    let message = format!("lila checkpoint {thread} #{n}");
    let mut args = vec!["-c", "commit.gpgsign=false", "commit-tree", snap.tree.as_str(), "-m", message.as_str()];
    if let Some(h) = &snap.head {
        args.extend(["-p", h.as_str()]);
    }
    let mut c = g.command(&args);
    for who in ["AUTHOR", "COMMITTER"] {
        c.env(format!("GIT_{who}_NAME"), "Lila").env(format!("GIT_{who}_EMAIL"), "lila@localhost");
    }
    let o = c.output().context("git commit-tree")?;
    if !o.status.success() {
        bail!("git commit-tree: {}", said(&o));
    }
    Ok((String::from_utf8_lossy(&o.stdout).trim().to_owned(), false))
}

/// Keeps the commit alive under the checkpoint's ref and drops the refs that are too old.
fn keep(g: &Git, snap: &Snap, spec: &Spec) -> Result<Taken> {
    let (sha, reused) = commit_of(g, snap, spec.thread, spec.n, spec.prev)?;
    g.bytes(&["update-ref", &ref_name(spec.thread, spec.n), &sha])?;
    if spec.keep_below > 0 {
        // A ref that stays is no great harm: not worth failing a checkpoint for.
        let _ = drop_up_to(g, spec.thread, spec.keep_below);
    }
    Ok(Taken { sha, tree: snap.tree.clone(), head: snap.head.clone(), branch: snap.branch.clone(), reused })
}

/// The numbers of a thread's checkpoint refs.
fn numbers(g: &Git, thread: &str) -> Result<Vec<i64>> {
    let out = g.text(&["for-each-ref", "--format=%(refname)", &format!("refs/lila/checkpoints/{thread}/")])?;
    Ok(out.lines().filter_map(|r| r.rsplit('/').next()?.parse().ok()).collect())
}

fn drop_up_to(g: &Git, thread: &str, up_to: i64) -> Result<()> {
    for n in numbers(g, thread)?.into_iter().filter(|n| *n <= up_to) {
        g.bytes(&["update-ref", "-d", &ref_name(thread, n)])?;
    }
    Ok(())
}

/// Takes a checkpoint of the folder. `None`: it is not in a git repository.
pub fn take(cwd: &Path, path_env: &str, spec: &Spec) -> Result<Option<Taken>> {
    let Some(g) = repo(cwd, path_env)? else { return Ok(None) };
    let snap = snapshot(&g)?;
    keep(&g, &snap, spec).map(Some)
}

/// Deletes every checkpoint ref a thread has in the repository `dir` is in (it is gone, or its project is).
/// How many there were.
pub fn drop_all(dir: &Path, path_env: &str, thread: &str) -> Result<usize> {
    let Some(g) = repo(dir, path_env)? else { return Ok(0) };
    let all = numbers(&g, thread)?;
    for n in &all {
        g.bytes(&["update-ref", "-d", &ref_name(thread, *n)])?;
    }
    Ok(all.len())
}

/// What a restore does to the folder's files, as paths relative to it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Diff {
    /// In both, and different: put back as they were.
    pub changed: Vec<String>,
    /// In the checkpoint, not in the folder now (deleted since): made again.
    pub recreated: Vec<String>,
    /// In the folder now, not in the checkpoint (made since, not ignored): deleted.
    pub deleted: Vec<String>,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.recreated.is_empty() && self.deleted.is_empty()
    }
}

/// The checkpoint to go back to.
pub struct Target<'a> {
    pub sha: &'a str,
    pub tree: &'a str,
    pub head: Option<&'a str>,
}

/// What a restore found, and did unless it was a dry run.
#[derive(Debug, Clone)]
pub struct Restored {
    pub diff: Diff,
    /// The HEAD now (a restore does not move it).
    pub head_now: Option<String>,
    /// The checkpoint of the state before: none in a dry run.
    pub undo: Option<Taken>,
}

/// The files that differ between two trees, below the folder.
fn diff_trees(g: &Git, from: &str, to: &str) -> Result<Diff> {
    let out = g.bytes(&["diff-tree", "-r", "--no-renames", "--name-status", "-z", "--relative", from, to])?;
    let mut d = Diff::default();
    let mut f = out.split(|b| *b == 0).map(|f| String::from_utf8_lossy(f).into_owned());
    while let Some(code) = f.next() {
        if code.is_empty() {
            continue;
        }
        let path = f.next().ok_or_else(|| anyhow!("git diff-tree ended in the middle of an entry"))?;
        match code.chars().next() {
            Some('A') => d.recreated.push(path),
            Some('D') => d.deleted.push(path),
            _ => d.changed.push(path),
        }
    }
    Ok(d)
}

/// Makes the folder's files the checkpoint's. Never moves HEAD or a branch, never touches the person's index or ignored files.
/// With `undo`, the folder's present state is made a checkpoint first (`None` there: a dry run, which only says what would happen).
pub fn restore(cwd: &Path, path_env: &str, target: &Target, undo: Option<&Spec>) -> Result<Restored> {
    let g = repo(cwd, path_env)?.ok_or_else(|| anyhow!("{} is not in a git repository", cwd.display()))?;
    if g.bytes(&["cat-file", "-e", &format!("{}^{{commit}}", target.sha)]).is_err() {
        bail!("the checkpoint's snapshot is no longer in the repository (git removed it, or its ref was deleted)");
    }
    let now = snapshot(&g)?;
    let diff = diff_trees(&g, &now.tree, target.tree)?;
    let Some(spec) = undo else { return Ok(Restored { diff, head_now: now.head, undo: None }) };
    // The way back first: nothing is changed unless the state it replaces is kept.
    let undo = keep(&g, &now, spec).context("could not keep the present state, so nothing was restored")?;
    apply(&g, &diff, target.tree).with_context(|| format!("the restore stopped half way; checkpoint {} has the state from before it", spec.n))?;
    Ok(Restored { diff, head_now: now.head, undo: Some(undo) })
}

/// Deletes the files made since, then writes the others from the checkpoint's tree.
fn apply(g: &Git, diff: &Diff, tree: &str) -> Result<()> {
    for path in &diff.deleted {
        let full = g.cwd.join(path);
        // A file or a link only: a folder here is a repository inside this one, which a snapshot holds as a single entry.
        if std::fs::symlink_metadata(&full).is_ok_and(|m| !m.is_dir()) {
            std::fs::remove_file(&full).with_context(|| format!("delete {path}"))?;
            remove_empty_parents(&full, g.cwd);
        }
    }
    let paths: Vec<&String> = diff.changed.iter().chain(&diff.recreated).collect();
    if paths.is_empty() {
        return Ok(());
    }
    // Written by git itself from a throwaway index holding the checkpoint's tree: modes, links and the person's
    // conversion settings are as git would check them out, and the person's index is not involved.
    let index = copy_index(g)?;
    let gi = Git { index: Some(&index.0), ..*g };
    gi.bytes(&["read-tree", tree])?;
    let mut child = gi.command(&["checkout-index", "-f", "-z", "--stdin"]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().context("git checkout-index")?;
    {
        let mut stdin = child.stdin.take().expect("piped");
        for p in &paths {
            stdin.write_all(p.as_bytes())?;
            stdin.write_all(&[0])?;
        }
    }
    let o = child.wait_with_output()?;
    if !o.status.success() {
        bail!("git checkout-index: {}", String::from_utf8_lossy(&o.stderr).trim());
    }
    Ok(())
}

/// The folders a deleted file leaves empty go too (git does not track folders), up to the thread's folder.
fn remove_empty_parents(file: &Path, root: &Path) {
    let mut dir: PathBuf = file.parent().map(Path::to_path_buf).unwrap_or_default();
    while dir != root && dir.starts_with(root) && std::fs::remove_dir(&dir).is_ok() {
        if !dir.pop() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worktree::tests::{path_env, repo as make_repo, sh};
    use std::collections::BTreeMap;

    fn write(dir: &Path, name: &str, text: &str) {
        let p = dir.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn read(dir: &Path, name: &str) -> String {
        std::fs::read_to_string(dir.join(name)).unwrap()
    }

    /// Everything the person can see of a repository's state, to compare before and after: status, HEAD, branch, stash, the index file, refs other than ours.
    fn state(dir: &Path) -> BTreeMap<&'static str, String> {
        // `git status` may refresh the index file, so it runs before the file is read.
        let status = sh(dir, &["status", "--porcelain=v2", "--branch", "--untracked-files=all"]);
        let git_dir = PathBuf::from(sh(dir, &["rev-parse", "--absolute-git-dir"]));
        let index = std::fs::read(git_dir.join("index")).map(|b| format!("{:x}", fnv(&b))).unwrap_or_default();
        BTreeMap::from([
            ("status", status),
            ("head", sh(dir, &["rev-parse", "HEAD"])),
            ("branches", sh(dir, &["for-each-ref", "refs/heads", "refs/tags", "refs/stash"])),
            ("stash", sh(dir, &["stash", "list"])),
            ("index", index),
        ])
    }

    /// The index file as it is, read without asking git anything (`git status` would refresh it).
    fn raw_index(dir: &Path) -> Vec<u8> {
        std::fs::read(PathBuf::from(sh(dir, &["rev-parse", "--absolute-git-dir"])).join("index")).unwrap_or_default()
    }

    /// A hash of bytes for comparing (not for security): FNV-1a.
    fn fnv(b: &[u8]) -> u64 {
        b.iter().fold(0xcbf29ce484222325, |h, x| (h ^ u64::from(*x)).wrapping_mul(0x100000001b3))
    }

    fn take_n(dir: &Path, n: i64, prev: Option<&Prev>) -> Taken {
        take(dir, &path_env(), &Spec { thread: "t00000001", n, prev, keep_below: 0 }).unwrap().expect("a repository")
    }

    fn prev_of(t: &Taken) -> Prev {
        Prev { sha: t.sha.clone(), tree: t.tree.clone(), head: t.head.clone() }
    }

    fn tree_of(sha: &str, dir: &Path) -> String {
        sh(dir, &["rev-parse", &format!("{sha}^{{tree}}")])
    }

    fn restore_to(dir: &Path, t: &Taken, undo_n: i64, dry: bool) -> Restored {
        let target = Target { sha: &t.sha, tree: &t.tree, head: t.head.as_deref() };
        let spec = Spec { thread: "t00000001", n: undo_n, prev: None, keep_below: 0 };
        restore(dir, &path_env(), &target, (!dry).then_some(&spec)).unwrap()
    }

    #[test]
    fn a_checkpoint_holds_modified_staged_untracked_and_deleted_files_and_leaves_the_repository_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("r");
        make_repo(&dir);
        write(&dir, "keep.txt", "keep\n");
        write(&dir, "gone.txt", "gone\n");
        sh(&dir, &["add", "-A"]);
        sh(&dir, &["commit", "-q", "-m", "more"]);
        // A stash exists, a file is staged, one modified after staging, one deleted, one untracked, one ignored.
        write(&dir, "stashed.txt", "s\n");
        sh(&dir, &["add", "stashed.txt"]);
        sh(&dir, &["stash", "-q"]);
        write(&dir, "a.txt", "a staged\n");
        sh(&dir, &["add", "a.txt"]);
        write(&dir, "a.txt", "a staged then edited\n");
        write(&dir, "keep.txt", "keep edited\n");
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        write(&dir, "new/deep/file.txt", "new\n");
        write(&dir, "target/junk", "ignored\n");
        let before = state(&dir);
        let refs_of = |dir: &Path| sh(dir, &["for-each-ref", "--format=%(refname)"]).lines().map(String::from).collect::<Vec<_>>();
        let refs_before = refs_of(&dir);

        let t = take_n(&dir, 1, None);
        assert!(!t.reused);
        assert_eq!(state(&dir), before, "status, HEAD, branches, stash and the index file are as they were");
        let added: Vec<String> = refs_of(&dir).into_iter().filter(|r| !refs_before.contains(r)).collect();
        assert_eq!(added, ["refs/lila/checkpoints/t00000001/1"], "only our ref was added");
        // The snapshot has the files as they are on disk: the edited content, the new file, no deleted one, no ignored one.
        let files = sh(&dir, &["ls-tree", "-r", "--name-only", &t.sha]);
        assert_eq!(files.lines().collect::<Vec<_>>(), [".gitignore", "a.txt", "keep.txt", "new/deep/file.txt"]);
        assert_eq!(sh(&dir, &["show", &format!("{}:a.txt", t.sha)]), "a staged then edited");
        assert_eq!(sh(&dir, &["show", &format!("{}:keep.txt", t.sha)]), "keep edited");
        // On top of HEAD.
        assert_eq!(sh(&dir, &["rev-parse", &format!("{}^", t.sha)]), before["head"]);
        assert_eq!((t.head.as_deref(), t.branch.as_deref()), (Some(before["head"].as_str()), Some("main")));
        // The person's files were not touched.
        assert_eq!(read(&dir, "a.txt"), "a staged then edited\n");
        assert!(dir.join("target/junk").exists());
    }

    #[test]
    fn nothing_changed_since_the_last_one_is_the_same_commit_and_a_clean_folder_is_head() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("r");
        make_repo(&dir);
        // A clean folder: its checkpoint is HEAD itself, no commit made.
        let before = sh(&dir, &["count-objects", "-v"]);
        let c1 = take_n(&dir, 1, None);
        assert_eq!(c1.sha, sh(&dir, &["rev-parse", "HEAD"]));
        assert_eq!(sh(&dir, &["count-objects", "-v"]), before, "no object written for a clean folder");
        write(&dir, "x.txt", "x\n");
        let c2 = take_n(&dir, 2, Some(&prev_of(&c1)));
        assert_ne!(c2.sha, c1.sha);
        // Nothing changed: the same commit, under a ref of its own.
        let c3 = take_n(&dir, 3, Some(&prev_of(&c2)));
        assert!(c3.reused);
        assert_eq!(c3.sha, c2.sha);
        assert_eq!(sh(&dir, &["rev-parse", "refs/lila/checkpoints/t00000001/3"]), c2.sha);
        // HEAD moved with the same files: not the same.
        sh(&dir, &["add", "-A"]);
        sh(&dir, &["commit", "-q", "-m", "x"]);
        let c4 = take_n(&dir, 4, Some(&prev_of(&c3)));
        assert!(!c4.reused);
        assert_eq!(c4.sha, sh(&dir, &["rev-parse", "HEAD"]), "the folder is HEAD's tree now");
    }

    #[test]
    fn a_repository_without_a_commit_and_a_folder_that_is_not_one() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("fresh");
        std::fs::create_dir_all(&dir).unwrap();
        sh(&dir, &["init", "-q", "-b", "main"]);
        write(&dir, "a.txt", "one\n");
        let t = take_n(&dir, 1, None);
        assert_eq!((t.head.clone(), t.branch.as_deref()), (None, Some("main")));
        assert_eq!(sh(&dir, &["rev-list", "--parents", "-n1", &t.sha]), t.sha, "no parent");
        assert_eq!(sh(&dir, &["ls-tree", "-r", "--name-only", &t.sha]), "a.txt");
        assert_eq!(sh(&dir, &["status", "--porcelain"]), "?? a.txt", "still not added by us");
        // And it can go back: HEAD does not exist, and still does not.
        write(&dir, "a.txt", "two\n");
        write(&dir, "b.txt", "b\n");
        let r = restore_to(&dir, &t, 2, false);
        assert_eq!((read(&dir, "a.txt"), dir.join("b.txt").exists(), r.head_now), ("one\n".to_owned(), false, None));
        // Empty folder in a repository without a commit: the empty tree.
        let empty = tmp.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        sh(&empty, &["init", "-q"]);
        assert_eq!(tree_of(&take_n(&empty, 1, None).sha, &empty), "4b825dc642cb6eb9a060e54bf8d69288fbee4904");
        // Not a repository at all: nothing, and no error.
        let plain = tmp.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert!(!maybe_repo(&plain));
        assert!(take(&plain, &path_env(), &Spec { thread: "t00000001", n: 1, prev: None, keep_below: 0 }).unwrap().is_none());
        assert!(take(&tmp.path().join("missing"), &path_env(), &Spec { thread: "t00000001", n: 1, prev: None, keep_below: 0 }).unwrap().is_none());
        assert!(maybe_repo(&dir));
    }

    #[test]
    fn a_restore_brings_back_changed_and_deleted_files_and_removes_new_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("r");
        make_repo(&dir);
        write(&dir, "src/lib.rs", "pub fn f() {}\n");
        write(&dir, "gone.txt", "gone\n");
        write(&dir, "script.sh", "#!/bin/sh\n");
        std::fs::set_permissions(dir.join("script.sh"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        write(&dir, "untracked.txt", "u\n");
        sh(&dir, &["add", "src", "gone.txt", "script.sh"]);
        sh(&dir, &["commit", "-q", "-m", "files"]);
        write(&dir, "target/cache", "ignored before\n");
        let t1 = take_n(&dir, 1, None);

        // The agent's turn: edits, deletes, creates (also in a new folder), changes a mode, touches an ignored file, even commits.
        write(&dir, "src/lib.rs", "pub fn f() { panic!() }\n");
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        std::fs::remove_file(dir.join("untracked.txt")).unwrap();
        write(&dir, "made/by/agent.txt", "x\n");
        write(&dir, "src/extra.rs", "x\n");
        std::fs::set_permissions(dir.join("script.sh"), std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
        write(&dir, "target/cache", "ignored after\n");
        write(&dir, "target/new", "ignored new\n");
        sh(&dir, &["add", "src/lib.rs"]);
        sh(&dir, &["commit", "-q", "-m", "agent commit"]);
        write(&dir, "src/lib.rs", "pub fn f() { panic!(\"again\") }\n");
        let head_now = sh(&dir, &["rev-parse", "HEAD"]);
        let before = state(&dir);
        let index_before = raw_index(&dir);

        // The dry run changes nothing, in the files, the repository or the refs, and says what would be done.
        let refs = sh(&dir, &["for-each-ref"]);
        let files_before = tree_listing(&dir);
        let dry = restore_to(&dir, &t1, 2, true);
        assert_eq!(state(&dir), before);
        assert_eq!(sh(&dir, &["for-each-ref"]), refs);
        assert_eq!(tree_listing(&dir), files_before);
        assert_eq!(dry.diff.changed, ["script.sh", "src/lib.rs"]);
        assert_eq!(dry.diff.recreated, ["gone.txt", "untracked.txt"]);
        assert_eq!(dry.diff.deleted, ["made/by/agent.txt", "src/extra.rs"]);
        assert!(dry.undo.is_none());
        assert_eq!(dry.head_now.as_deref(), Some(head_now.as_str()));

        // The real one.
        let r = restore_to(&dir, &t1, 2, false);
        assert_eq!(raw_index(&dir), index_before, "the person's index file is byte for byte as it was");
        assert_eq!(r.diff, dry.diff);
        assert_eq!(read(&dir, "src/lib.rs"), "pub fn f() {}\n");
        assert_eq!(read(&dir, "gone.txt"), "gone\n");
        assert_eq!(read(&dir, "untracked.txt"), "u\n");
        assert!(!dir.join("made").exists(), "the folders it made are gone with their files");
        assert!(!dir.join("src/extra.rs").exists() && dir.join("src").is_dir());
        assert_eq!(std::fs::metadata(dir.join("script.sh")).unwrap().permissions().mode() & 0o111, 0o111, "the mode is back");
        // Ignored files: the one that was there is as the agent left it, the new one is still there.
        assert_eq!(read(&dir, "target/cache"), "ignored after\n");
        assert_eq!(read(&dir, "target/new"), "ignored new\n");
        // HEAD, the branch and the index are where the agent left them.
        assert_eq!(sh(&dir, &["rev-parse", "HEAD"]), head_now);
        assert_eq!(sh(&dir, &["symbolic-ref", "--short", "HEAD"]), "main");
        // The folder is exactly the checkpoint's tree again (as far as git looks).
        assert_eq!(take_n(&dir, 3, None).tree, t1.tree);
        // The way back: the state before the restore.
        let undo = r.undo.expect("the state before was kept");
        assert_eq!(sh(&dir, &["rev-parse", "refs/lila/checkpoints/t00000001/2"]), undo.sha);
        let back = restore_to(&dir, &undo, 4, false);
        assert!(!back.diff.is_empty());
        assert_eq!(read(&dir, "src/lib.rs"), "pub fn f() { panic!(\"again\") }\n");
        assert!(!dir.join("gone.txt").exists() && dir.join("made/by/agent.txt").exists());
        assert_eq!(take_n(&dir, 5, None).tree, undo.tree, "exactly as it was before the first restore");
    }

    use std::os::unix::fs::PermissionsExt;

    /// Every file of the folder outside .git with its content, for comparing.
    fn tree_listing(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let mut out: Vec<(String, Vec<u8>)> = walkdir::WalkDir::new(dir)
            .into_iter()
            .filter_entry(|e| e.file_name() != ".git")
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .map(|e| (e.path().strip_prefix(dir).unwrap().to_string_lossy().to_string(), std::fs::read(e.path()).unwrap()))
            .collect();
        out.sort();
        out
    }

    #[test]
    fn a_subfolder_is_snapshotted_and_restored_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("mono");
        make_repo(&root);
        write(&root, "app/a.rs", "a\n");
        write(&root, "lib/l.rs", "l\n");
        sh(&root, &["add", "-A"]);
        sh(&root, &["commit", "-q", "-m", "two folders"]);
        let app = root.join("app");
        let t = take_n(&app, 1, None);
        // The agent edits its folder, and somebody else edits the other.
        write(&app, "a.rs", "a edited\n");
        write(&app, "new/n.rs", "n\n");
        write(&root, "lib/l.rs", "l edited by someone else\n");
        write(&root, "lib/other.rs", "o\n");
        let r = restore_to(&app, &t, 2, false);
        assert_eq!(r.diff.changed, ["a.rs"], "paths are relative to the folder");
        assert_eq!(r.diff.deleted, ["new/n.rs"]);
        assert_eq!(read(&app, "a.rs"), "a\n");
        assert!(!app.join("new").exists());
        assert_eq!(read(&root, "lib/l.rs"), "l edited by someone else\n", "outside the folder: not touched");
        assert!(root.join("lib/other.rs").exists());
        // The undo goes back too.
        let back = restore_to(&app, &r.undo.unwrap(), 3, false);
        assert_eq!((back.diff.changed, back.diff.recreated), (vec!["a.rs".to_owned()], vec!["new/n.rs".to_owned()]));
        assert_eq!(read(&app, "a.rs"), "a edited\n");
    }

    #[test]
    fn names_with_spaces_quotes_and_non_ascii_letters_survive() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("my repo 项目");
        make_repo(&dir);
        let names = ["with space.txt", "中文 文件.md", "naïve \"quoted\".txt", "tab\there.txt", "emoji-🙂/深/nested file.rs"];
        for n in names {
            write(&dir, n, &format!("{n}\n"));
        }
        sh(&dir, &["add", "-A"]);
        sh(&dir, &["commit", "-q", "-m", "names"]);
        write(&dir, "中文 文件.md", "changed\n");
        std::fs::remove_file(dir.join("with space.txt")).unwrap();
        write(&dir, "new 文件 \"x\".txt", "n\n");
        write(&dir, "emoji-🙂/深/added 文.txt", "n\n");
        let t = take_n(&dir, 1, None);
        let listed = sh(&dir, &["-c", "core.quotepath=false", "ls-tree", "-r", "--name-only", &t.sha]);
        assert!(listed.contains("emoji-🙂/深/added 文.txt"), "{listed}");
        // Undo all of it, then do it again.
        write(&dir, "中文 文件.md", "worse\n");
        write(&dir, "naïve \"quoted\".txt", "worse\n");
        write(&dir, "with space.txt", "back by the agent\n");
        std::fs::remove_file(dir.join("new 文件 \"x\".txt")).unwrap();
        let dry = restore_to(&dir, &t, 2, true);
        assert_eq!(dry.diff.changed, ["naïve \"quoted\".txt", "中文 文件.md"]);
        assert_eq!(dry.diff.recreated, ["new 文件 \"x\".txt"]);
        assert_eq!(dry.diff.deleted, ["with space.txt"]);
        restore_to(&dir, &t, 2, false);
        assert_eq!(read(&dir, "中文 文件.md"), "changed\n");
        assert_eq!(read(&dir, "naïve \"quoted\".txt"), "naïve \"quoted\".txt\n");
        assert!(!dir.join("with space.txt").exists());
        assert_eq!(read(&dir, "new 文件 \"x\".txt"), "n\n");
        assert_eq!(take_n(&dir, 3, None).tree, t.tree);
    }

    #[test]
    fn old_checkpoints_go_and_so_do_all_of_a_threads() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("r");
        make_repo(&dir);
        for n in 1..=6 {
            write(&dir, "f.txt", &format!("{n}\n"));
            take(&dir, &path_env(), &Spec { thread: "t00000001", n, prev: None, keep_below: n - 3 }).unwrap();
        }
        take(&dir, &path_env(), &Spec { thread: "t00000002", n: 1, prev: None, keep_below: 0 }).unwrap();
        let refs = |t: &str| sh(&dir, &["for-each-ref", "--format=%(refname)", &format!("refs/lila/checkpoints/{t}/")]);
        assert_eq!(refs("t00000001"), "refs/lila/checkpoints/t00000001/4\nrefs/lila/checkpoints/t00000001/5\nrefs/lila/checkpoints/t00000001/6");
        assert_eq!(drop_all(&dir, &path_env(), "t00000001").unwrap(), 3);
        assert_eq!(refs("t00000001"), "");
        assert_eq!(refs("t00000002"), "refs/lila/checkpoints/t00000002/1", "another thread's are not touched");
        assert_eq!(drop_all(tmp.path(), &path_env(), "t00000002").unwrap(), 0, "outside a repository: nothing");
    }

    #[test]
    fn a_missing_snapshot_is_an_error_and_nothing_is_touched() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("r");
        make_repo(&dir);
        write(&dir, "a.txt", "mine\n");
        let target = Target { sha: "0123456789012345678901234567890123456789", tree: "4b825dc642cb6eb9a060e54bf8d69288fbee4904", head: None };
        let err = restore(&dir, &path_env(), &target, Some(&Spec { thread: "t00000001", n: 1, prev: None, keep_below: 0 })).unwrap_err();
        assert!(format!("{err:#}").contains("no longer in the repository"), "{err:#}");
        assert_eq!(read(&dir, "a.txt"), "mine\n");
        assert!(sh(&dir, &["for-each-ref", "refs/lila"]).is_empty());
    }
}
