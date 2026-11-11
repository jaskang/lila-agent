//! What a thread's folder has changed since its HEAD, for the changes panel: the files with their
//! counts, one file's diff, and a file's text. The git command line, as `worktree.rs`: the person's
//! git configuration applies. Everything here blocks (a big repository takes a while), so lilad runs
//! it off its loop (`daemon/jobs.rs`).
//!
//! Renames are found without touching the person's index: the files git does not know yet are
//! entered as intent-to-add (no content is written) in a throwaway copy of the index, and
//! `git diff HEAD -M` then pairs a deleted file with its new name and counts the new files as added.

use crate::worktree::said;
use anyhow::{Context, Result, anyhow, bail};
use lila_agent::FileDiff;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

/// Most files listed; the counts in the summary still cover all of them.
pub const MAX_FILES: usize = 2000;
/// Most of a patch read from git (all files together).
const MAX_PATCH: usize = 8 * 1024 * 1024;
/// Most lines and bytes of one file's diff kept.
const MAX_DIFF_LINES: usize = 3000;
const MAX_DIFF_BYTES: usize = 300_000;
/// Largest file `thread.file` returns the text of.
pub const MAX_FILE: u64 = 1024 * 1024;
/// How many leading bytes tell a binary file (what git looks at).
const SNIFF: usize = 8000;
/// Commits listed for a worktree thread.
const MAX_COMMITS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Added,
    Modified,
    Deleted,
    Renamed,
    /// Git does not know the file yet: all of it is new.
    Untracked,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FileChange {
    pub path: String,
    pub status: Status,
    pub add: u64,
    pub del: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    /// Git can't count a binary file's lines.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub binary: bool,
}

/// What a folder has changed relative to its HEAD.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Changes {
    /// The folder is in a git repository (and exists).
    pub repo: bool,
    /// The branch checked out; none in a detached HEAD.
    pub branch: Option<String>,
    pub files: Vec<FileChange>,
    pub add: u64,
    pub del: u64,
    /// There were more files than `MAX_FILES`.
    pub truncated: bool,
    /// Worktree threads: commits on the branch that its base branch does not have, newest first.
    pub ahead: Option<usize>,
    pub commits: Vec<(String, String)>,
    /// How many files, all of them.
    pub count: usize,
}

/// The few numbers the thread list carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub files: usize,
    pub add: u64,
    pub del: u64,
}

impl Changes {
    pub fn summary(&self) -> Option<Summary> {
        self.repo.then_some(Summary { files: self.count, add: self.add, del: self.del })
    }

    pub fn json(&self, worktree: bool, base_branch: Option<&str>) -> Value {
        let mut v = json!({
            "repo": self.repo, "branch": self.branch, "worktree": worktree,
            "files": self.files, "add": self.add, "del": self.del, "truncated": self.truncated,
        });
        if worktree {
            v["base_branch"] = json!(base_branch);
            v["ahead"] = json!(self.ahead);
            v["commits"] = json!(self.commits.iter().map(|(sha, subject)| json!({"sha": sha, "subject": subject})).collect::<Vec<_>>());
        }
        v
    }
}

impl Summary {
    pub fn json(&self) -> Value {
        json!({"files": self.files, "add": self.add, "del": self.del})
    }
}

/// A file's diff as the window shows it: the conversation's `FileDiff` (the path and the unified-diff
/// lines from the first `@@`), with what the window needs to know about the file.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChangeDiff {
    #[serde(flatten)]
    pub diff: FileDiff,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    /// A binary file: no lines.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub binary: bool,
    /// Too long: the lines stop where it was cut.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

/// A throwaway copy of the folder's index, removed when dropped.
pub(crate) struct TempIndex(pub(crate) PathBuf);

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The git command line for one folder: never takes the person's index lock (`status` would, to refresh it).
pub(crate) struct Git<'a> {
    pub(crate) cwd: &'a Path,
    pub(crate) path_env: &'a str,
    /// Work on this index instead of the person's (`GIT_INDEX_FILE`).
    pub(crate) index: Option<&'a Path>,
}

impl Git<'_> {
    pub(crate) fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new("git");
        c.args(["-c", "core.quotepath=false"]).args(args).current_dir(self.cwd).env("PATH", self.path_env).env("GIT_OPTIONAL_LOCKS", "0").stdin(Stdio::null());
        if let Some(i) = self.index {
            c.env("GIT_INDEX_FILE", i);
        }
        c
    }

    pub(crate) fn bytes(&self, args: &[&str]) -> Result<Vec<u8>> {
        let o = self.command(args).output().with_context(|| format!("git {}", args.join(" ")))?;
        if !o.status.success() {
            bail!("git {}: {}", args.join(" "), said(&o));
        }
        Ok(o.stdout)
    }

    pub(crate) fn text(&self, args: &[&str]) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.bytes(args)?).trim().to_owned())
    }

    /// Standard output, up to `cap` bytes; whether there was more.
    fn capped(&self, args: &[&str], cap: usize) -> Result<(Vec<u8>, bool)> {
        let mut child = self.command(args).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().with_context(|| format!("git {}", args.join(" ")))?;
        let mut buf = Vec::new();
        child.stdout.take().expect("piped").take(cap as u64 + 1).read_to_end(&mut buf)?;
        let more = buf.len() > cap;
        buf.truncate(cap);
        if more {
            let _ = child.kill();
        }
        let status = child.wait()?;
        if !more && !status.success() {
            bail!("git {} failed", args.join(" "));
        }
        Ok((buf, more))
    }
}

/// What the folder's HEAD is to compare with: HEAD, or the empty tree before the first commit.
fn head(g: &Git) -> String {
    match g.text(&["rev-parse", "--verify", "-q", "HEAD"]) {
        Ok(h) if !h.is_empty() => "HEAD".to_owned(),
        _ => g.text(&["hash-object", "-t", "tree", "/dev/null"]).unwrap_or_else(|_| "4b825dc642cb6eb9a060e54bf8d69288fbee4904".to_owned()),
    }
}

/// A copy of the person's index, to work on instead of it: the stat information in it is reused, so git does not read every file again.
pub(crate) fn copy_index(g: &Git) -> Result<TempIndex> {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let real = g.text(&["rev-parse", "--git-path", "index"])?;
    let real = if Path::new(&real).is_absolute() { PathBuf::from(real) } else { g.cwd.join(real) };
    let copy = std::env::temp_dir().join(format!("lila-index-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    // No index yet (a repository nothing was added to): an empty one.
    if real.is_file() {
        std::fs::copy(&real, &copy).with_context(|| format!("copy {}", real.display()))?;
    }
    Ok(TempIndex(copy))
}

/// A copy of the index with the files git does not know yet entered as intent-to-add.
fn temp_index(g: &Git) -> Result<TempIndex> {
    let index = copy_index(g)?;
    Git { index: Some(&index.0), ..*g }.bytes(&["add", "-N", "-A"])?;
    Ok(index)
}

struct Raw {
    status: Status,
    path: String,
    old_path: Option<String>,
}

/// `git status` for the branch and the untracked files, paths relative to the folder.
fn status(g: &Git) -> Result<(Option<String>, BTreeSet<String>)> {
    let prefix = g.text(&["rev-parse", "--show-prefix"]).unwrap_or_default();
    let out = g.bytes(&["status", "--porcelain=v2", "-z", "-uall", "--branch", "--", "."])?;
    let (mut branch, mut untracked) = (None, BTreeSet::new());
    let mut fields = out.split(|b| *b == 0).filter(|f| !f.is_empty());
    while let Some(f) = fields.next() {
        let f = String::from_utf8_lossy(f);
        if let Some(h) = f.strip_prefix("# branch.head ") {
            branch = Some(h.to_owned()).filter(|h| h != "(detached)");
        } else if let Some(p) = f.strip_prefix("? ") {
            untracked.insert(p.strip_prefix(prefix.as_str()).unwrap_or(p).to_owned());
        } else if f.starts_with("2 ") {
            // A rename entry is followed by the path it came from.
            fields.next();
        }
    }
    Ok((branch, untracked))
}

/// The changed files, by git's own pairing of renames, in git's order.
fn raw_list(g: &Git, base: &str, untracked: &BTreeSet<String>) -> Result<Vec<Raw>> {
    let out = g.bytes(&["diff", base, "--name-status", "-z", "-M", "--relative"])?;
    let mut list = Vec::new();
    let mut f = out.split(|b| *b == 0).map(|f| String::from_utf8_lossy(f).into_owned());
    while let Some(code) = f.next() {
        if code.is_empty() {
            continue;
        }
        let path = f.next().ok_or_else(|| anyhow!("git diff --name-status ended in the middle of an entry"))?;
        list.push(match code.chars().next() {
            Some('R' | 'C') => {
                let new = f.next().ok_or_else(|| anyhow!("git diff --name-status: a rename without its new name"))?;
                Raw { status: Status::Renamed, path: new, old_path: Some(path) }
            }
            Some('A') if untracked.contains(&path) => Raw { status: Status::Untracked, path, old_path: None },
            Some('A') => Raw { status: Status::Added, path, old_path: None },
            Some('D') => Raw { status: Status::Deleted, path, old_path: None },
            _ => Raw { status: Status::Modified, path, old_path: None },
        });
    }
    Ok(list)
}

/// `(added, deleted)` per file by its path (the new one of a rename); `None` for a binary file.
fn numstat(g: &Git, base: &str) -> Result<HashMap<String, Option<(u64, u64)>>> {
    let out = g.bytes(&["diff", base, "--numstat", "-z", "-M", "--relative"])?;
    let mut counts = HashMap::new();
    let mut f = out.split(|b| *b == 0).map(|f| String::from_utf8_lossy(f).into_owned());
    while let Some(head) = f.next() {
        if head.is_empty() {
            continue;
        }
        let mut parts = head.splitn(3, '\t');
        let (add, del, path) = (parts.next().unwrap_or("-"), parts.next().unwrap_or("-"), parts.next().unwrap_or(""));
        let n = match (add.parse::<u64>(), del.parse::<u64>()) {
            (Ok(a), Ok(d)) => Some((a, d)),
            _ => None,
        };
        // A rename: the path is empty and the old and new names follow.
        let path = if path.is_empty() {
            f.next();
            f.next().unwrap_or_default()
        } else {
            path.to_owned()
        };
        counts.insert(path, n);
    }
    Ok(counts)
}

/// The folder's changes against its HEAD. `base_branch`: for a worktree thread, the branch its work
/// goes into, to count and list the commits it has that it does not.
pub fn changes(cwd: &Path, path_env: &str, base_branch: Option<&str>) -> Result<Changes> {
    if !cwd.is_dir() {
        return Ok(Changes::default());
    }
    let g = Git { cwd, path_env, index: None };
    let Ok((branch, untracked)) = status(&g) else { return Ok(Changes::default()) };
    let base = head(&g);
    let index = temp_index(&g)?;
    let g = Git { index: Some(&index.0), ..g };
    let raw = raw_list(&g, &base, &untracked)?;
    let counts = numstat(&g, &base)?;
    let mut c = Changes { repo: true, branch, count: raw.len(), truncated: raw.len() > MAX_FILES, ..Default::default() };
    for r in raw {
        let n = counts.get(&r.path).copied().flatten();
        let (add, del) = n.unwrap_or((0, 0));
        c.add += add;
        c.del += del;
        if c.files.len() < MAX_FILES {
            c.files.push(FileChange { path: r.path, status: r.status, add, del, old_path: r.old_path, binary: n.is_none() });
        }
    }
    if let Some(b) = base_branch {
        let g = Git { cwd, path_env, index: None };
        let range = format!("{b}..HEAD");
        c.ahead = g.text(&["rev-list", "--count", &range]).ok().and_then(|n| n.parse().ok());
        let limit = format!("-n{MAX_COMMITS}");
        c.commits = g
            .text(&["log", "--format=%h%x1f%s", &limit, &range])
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.split_once('\u{1f}').map(|(a, b)| (a.to_owned(), b.to_owned())))
            .collect();
    }
    Ok(c)
}

/// A relative path inside the folder: no root, no `..`, nothing in `.git`.
pub fn clean_relative(path: &str) -> Result<PathBuf> {
    let p = Path::new(path);
    if path.is_empty() || p.is_absolute() {
        bail!("give a path relative to the thread's folder");
    }
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(n) if n == ".git" && out.as_os_str().is_empty() => bail!("{path} is inside .git"),
            Component::Normal(n) => out.push(n),
            Component::CurDir => {}
            _ => bail!("{path} leaves the thread's folder"),
        }
    }
    if out.as_os_str().is_empty() {
        bail!("give a path relative to the thread's folder");
    }
    Ok(out)
}

/// The diffs of the changed files (one file's, when `path` names it), in the order of `changes`.
pub fn diffs(cwd: &Path, path_env: &str, path: Option<&str>) -> Result<(Vec<ChangeDiff>, bool)> {
    let path = path.map(clean_relative).transpose()?.map(|p| p.to_string_lossy().to_string());
    if !cwd.is_dir() {
        return Ok((Vec::new(), false));
    }
    let g = Git { cwd, path_env, index: None };
    let Ok((_, untracked)) = status(&g) else { return Ok((Vec::new(), false)) };
    let base = head(&g);
    let index = temp_index(&g)?;
    let g = Git { index: Some(&index.0), ..g };
    let mut raw = raw_list(&g, &base, &untracked)?;
    // One file: git is asked for it (and for the name it had), not for everything.
    let mut specs: Vec<String> = Vec::new();
    if let Some(p) = &path {
        raw.retain(|r| &r.path == p || r.old_path.as_ref() == Some(p));
        specs.extend(raw.iter().flat_map(|r| std::iter::once(r.path.clone()).chain(r.old_path.clone())));
        if raw.is_empty() {
            return Ok((Vec::new(), false));
        }
    }
    let mut args = vec!["diff", &base, "-M", "--relative", "--no-color", "--no-ext-diff", "--no-textconv", "--patch"];
    if !specs.is_empty() {
        args.push("--");
        args.extend(specs.iter().map(String::as_str));
    }
    let (patch, cut) = g.capped(&args, MAX_PATCH)?;
    let patch = String::from_utf8_lossy(&patch);
    // A block of the patch starts at `diff --git`: no line of a diff does (they start with a space, `+`, `-`, `\` or `@`).
    let blocks: Vec<&str> = patch.split("\ndiff --git ").enumerate().map(|(i, b)| if i == 0 { b.strip_prefix("diff --git ").unwrap_or(b) } else { b }).filter(|b| !b.trim().is_empty()).collect();
    let mut out = Vec::new();
    for (i, r) in raw.into_iter().enumerate() {
        let Some(block) = blocks.get(i) else {
            // The patch was cut before this file.
            out.push(ChangeDiff { diff: FileDiff { path: r.path, lines: Vec::new() }, status: r.status, old_path: r.old_path, binary: false, truncated: true });
            continue;
        };
        let binary = block.lines().any(|l| l.starts_with("Binary files ") || l.starts_with("GIT binary patch"));
        let mut lines: Vec<String> = block.lines().skip_while(|l| !l.starts_with("@@")).map(String::from).collect();
        let (mut truncated, mut bytes) = (cut && i + 1 == blocks.len(), 0);
        let keep = lines.iter().take_while(|l| {
            bytes += l.len() + 1;
            bytes <= MAX_DIFF_BYTES
        });
        let keep = keep.count().min(MAX_DIFF_LINES);
        if keep < lines.len() {
            lines.truncate(keep);
            truncated = true;
        }
        out.push(ChangeDiff { diff: FileDiff { path: r.path, lines }, status: r.status, old_path: r.old_path, binary, truncated });
    }
    Ok((out, cut))
}

/// A file of the thread's folder: its text, or that it is binary or too large. Nothing outside the
/// folder: not by `..`, not by an absolute path, not through a symbolic link.
pub fn read_file(cwd: &Path, path: &str) -> Result<Value> {
    let rel = clean_relative(path)?;
    let root = std::fs::canonicalize(cwd).with_context(|| format!("{} is gone", cwd.display()))?;
    let full = std::fs::canonicalize(root.join(&rel)).map_err(|_| anyhow!("{path} does not exist"))?;
    if !full.starts_with(&root) {
        bail!("{path} is outside the thread's folder");
    }
    let meta = std::fs::metadata(&full)?;
    if !meta.is_file() {
        bail!("{path} is not a file");
    }
    let rel = rel.to_string_lossy().to_string();
    if meta.len() > MAX_FILE {
        return Ok(json!({"path": rel, "too_large": true, "size": meta.len()}));
    }
    let bytes = std::fs::read(&full)?;
    if bytes[..bytes.len().min(SNIFF)].contains(&0) {
        return Ok(json!({"path": rel, "binary": true, "size": bytes.len()}));
    }
    Ok(match String::from_utf8(bytes) {
        Ok(text) => json!({"path": rel, "text": text, "size": text.len()}),
        Err(e) => json!({"path": rel, "binary": true, "size": e.as_bytes().len()}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worktree::tests::{path_env, repo, sh};

    fn write(dir: &Path, name: &str, text: &str) {
        let p = dir.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn change<'a>(c: &'a Changes, path: &str) -> &'a FileChange {
        c.files.iter().find(|f| f.path == path).unwrap_or_else(|| panic!("{path} not in {:?}", c.files))
    }

    #[test]
    fn a_folder_outside_git_has_no_changes() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "a.txt", "a");
        let c = changes(tmp.path(), &path_env(), None).unwrap();
        assert!(!c.repo && c.files.is_empty() && c.summary().is_none());
        assert!(!changes(&tmp.path().join("gone"), &path_env(), None).unwrap().repo);
    }

    #[test]
    fn every_kind_of_change_is_listed_with_its_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        repo(dir);
        let lines: String = (1..=30).map(|n| format!("line {n}\n")).collect();
        write(dir, "big.txt", &lines);
        write(dir, "gone.txt", "bye\n");
        write(dir, "blob.bin", "a\0b");
        sh(dir, &["add", "-A"]);
        sh(dir, &["commit", "-q", "-m", "more"]);
        let env = path_env();
        assert_eq!(changes(dir, &env, None).unwrap().files, vec![], "a clean folder");

        write(dir, "a.txt", "a\nmore\nlines\n");
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        std::fs::rename(dir.join("big.txt"), dir.join("moved.txt")).unwrap();
        write(dir, "src/new.rs", "fn main() {}\nfn two() {}\n");
        write(dir, "blob2.bin", "x\0y");
        write(dir, "target/ignored", "never listed");
        sh(dir, &["add", "src/new.rs"]);
        write(dir, "untracked.txt", "u1\nu2\nu3\n");
        let index_before = std::fs::read(dir.join(".git/index")).unwrap();

        let c = changes(dir, &env, None).unwrap();
        assert!(c.repo && c.branch.as_deref() == Some("main"));
        let a = change(&c, "a.txt");
        assert_eq!((a.status, a.add, a.del), (Status::Modified, 2, 0));
        assert_eq!((change(&c, "gone.txt").status, change(&c, "gone.txt").del), (Status::Deleted, 1));
        let m = change(&c, "moved.txt");
        assert_eq!((m.status, m.old_path.as_deref(), m.add, m.del), (Status::Renamed, Some("big.txt"), 0, 0));
        let s = change(&c, "src/new.rs");
        assert_eq!((s.status, s.add), (Status::Added, 2), "added to the index: added, not untracked");
        let u = change(&c, "untracked.txt");
        assert_eq!((u.status, u.add, u.del), (Status::Untracked, 3, 0), "an untracked file is all new");
        let b = change(&c, "blob2.bin");
        assert_eq!((b.status, b.binary, b.add), (Status::Untracked, true, 0));
        assert!(!c.files.iter().any(|f| f.path.starts_with("target")), "ignored files are not changes");
        assert_eq!(c.count, 6);
        assert_eq!((c.add, c.del), (2 + 2 + 3, 1), "{:?}", c.files);
        assert_eq!(c.summary(), Some(Summary { files: 6, add: 7, del: 1 }));
        assert_eq!(std::fs::read(dir.join(".git/index")).unwrap(), index_before, "the person's index is not touched");
        assert_eq!(sh(dir, &["status", "--short"]).lines().count(), 7, "git still sees the rename as a deletion and a new file: {}", sh(dir, &["status", "--short"]));
        // The copy of the index is removed when done with.
        let g = Git { cwd: dir, path_env: &env, index: None };
        let copy = temp_index(&g).unwrap();
        let path = copy.0.clone();
        assert!(path.is_file());
        drop(copy);
        assert!(!path.exists());
    }

    #[test]
    fn a_repository_before_its_first_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        sh(dir, &["init", "-q", "-b", "main"]);
        write(dir, "a.txt", "1\n2\n");
        let c = changes(dir, &path_env(), None).unwrap();
        assert!(c.repo);
        assert_eq!((c.files.len(), c.files[0].status, c.files[0].add), (1, Status::Untracked, 2));
        let (d, _) = diffs(dir, &path_env(), None).unwrap();
        assert_eq!(d[0].diff.lines, ["@@ -0,0 +1,2 @@", "+1", "+2"]);
    }

    #[test]
    fn a_folder_inside_a_repository_sees_only_its_own() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        repo(dir);
        write(dir, "pkg/in.txt", "in\n");
        write(dir, "out.txt", "out\n");
        let c = changes(&dir.join("pkg"), &path_env(), None).unwrap();
        assert_eq!(c.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["in.txt"]);
        let (d, _) = diffs(&dir.join("pkg"), &path_env(), None).unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].diff.path, "in.txt");
    }

    #[test]
    fn diffs_use_the_conversations_file_diff() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        repo(dir);
        write(dir, "gone.txt", "x\ny\n");
        write(dir, "big.txt", &(1..=20).map(|n| format!("l{n}\n")).collect::<String>());
        sh(dir, &["add", "-A"]);
        sh(dir, &["commit", "-q", "-m", "more"]);
        write(dir, "a.txt", "a\nb\n");
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        std::fs::rename(dir.join("big.txt"), dir.join("big2.txt")).unwrap();
        write(dir, "fresh.txt", "one\ntwo\n");
        write(dir, "bin.dat", "q\0r");
        let env = path_env();
        let (all, cut) = diffs(dir, &env, None).unwrap();
        assert!(!cut);
        let of = |p: &str| all.iter().find(|d| d.diff.path == p).unwrap_or_else(|| panic!("{p}")).clone();
        assert_eq!(of("a.txt").diff.lines, ["@@ -1 +1,2 @@", " a", "+b"]);
        assert_eq!(of("gone.txt").diff.lines, ["@@ -1,2 +0,0 @@", "-x", "-y"]);
        assert_eq!(of("gone.txt").status, Status::Deleted);
        assert_eq!(of("fresh.txt").diff.lines, ["@@ -0,0 +1,2 @@", "+one", "+two"], "an untracked file is all added");
        assert_eq!(of("fresh.txt").status, Status::Untracked);
        let bin = of("bin.dat");
        assert!(bin.binary && bin.diff.lines.is_empty());
        let moved = of("big2.txt");
        assert_eq!((moved.status, moved.old_path.as_deref(), moved.diff.lines.len()), (Status::Renamed, Some("big.txt"), 0));

        // One file, by either of its names.
        let (one, _) = diffs(dir, &env, Some("fresh.txt")).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0], of("fresh.txt"));
        assert_eq!(diffs(dir, &env, Some("big.txt")).unwrap().0, vec![moved.clone()]);
        assert_eq!(diffs(dir, &env, Some("big2.txt")).unwrap().0, vec![moved]);
        assert!(diffs(dir, &env, Some("a.txt")).unwrap().0[0].diff.lines.len() == 3);
        assert!(diffs(dir, &env, Some("nothing.txt")).unwrap().0.is_empty());
        assert!(diffs(dir, &env, Some("../x")).is_err());

        // The window's JSON: the same shape as a tool call's diffs, with the extras.
        let j = serde_json::to_value(&one[0]).unwrap();
        assert_eq!(j["path"], "fresh.txt");
        assert_eq!(j["lines"][0], "@@ -0,0 +1,2 @@");
        assert_eq!(j["status"], "untracked");
        assert!(j.get("binary").is_none() && j.get("truncated").is_none());
    }

    #[test]
    fn a_long_diff_is_cut() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        repo(dir);
        write(dir, "long.txt", &(1..=(MAX_DIFF_LINES + 500)).map(|n| format!("line {n}\n")).collect::<String>());
        let (d, _) = diffs(dir, &path_env(), None).unwrap();
        assert!(d[0].truncated && d[0].diff.lines.len() == MAX_DIFF_LINES, "{}", d[0].diff.lines.len());
        // The summary still counts all of it.
        assert_eq!(changes(dir, &path_env(), None).unwrap().files[0].add, (MAX_DIFF_LINES + 500) as u64);
    }

    #[test]
    fn a_worktree_lists_the_commits_it_has_beyond_its_base() {
        let tmp = tempfile::tempdir().unwrap();
        let (main, wt) = (tmp.path().join("main"), tmp.path().join("wt"));
        repo(&main);
        crate::worktree::create(&main, &wt, "lila/t1", &path_env()).unwrap();
        write(&wt, "b.txt", "b\n");
        sh(&wt, &["add", "-A"]);
        sh(&wt, &["commit", "-q", "-m", "add b"]);
        write(&wt, "c.txt", "c\n");
        let c = changes(&wt, &path_env(), Some("main")).unwrap();
        assert_eq!(c.branch.as_deref(), Some("lila/t1"));
        assert_eq!((c.ahead, c.commits.iter().map(|(_, s)| s.as_str()).collect::<Vec<_>>()), (Some(1), vec!["add b"]));
        assert_eq!(c.files.len(), 1, "only what is not committed yet");
        let j = c.json(true, Some("main"));
        assert_eq!((j["ahead"].clone(), j["base_branch"].clone(), j["commits"][0]["subject"].clone()), (json!(1), json!("main"), json!("add b")));
        assert!(changes(&main, &path_env(), None).unwrap().json(false, None).get("ahead").is_none());
    }

    #[test]
    fn a_file_is_read_inside_the_folder_only() {
        let tmp = tempfile::tempdir().unwrap();
        let (dir, outside) = (tmp.path().join("work"), tmp.path().join("secret.txt"));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        write(&dir, "src/a.rs", "fn a() {}\n");
        write(&dir, "bin.dat", "a\0b");
        write(&dir, "latin.txt", "caf\u{e9}");
        std::fs::write(dir.join("latin1.txt"), [b'c', b'a', b'f', 0xe9]).unwrap();
        std::fs::write(&outside, "secret").unwrap();
        std::fs::write(dir.join("big.txt"), vec![b'x'; MAX_FILE as usize + 1]).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("link.txt")).unwrap();
        std::os::unix::fs::symlink(tmp.path(), dir.join("up")).unwrap();
        std::os::unix::fs::symlink(dir.join("src/a.rs"), dir.join("inside.rs")).unwrap();
        write(&dir, ".git/config", "[core]");

        assert_eq!(read_file(&dir, "src/a.rs").unwrap(), json!({"path": "src/a.rs", "text": "fn a() {}\n", "size": 10}));
        assert_eq!(read_file(&dir, "./src/a.rs").unwrap()["path"], "src/a.rs");
        assert_eq!(read_file(&dir, "bin.dat").unwrap(), json!({"path": "bin.dat", "binary": true, "size": 3}));
        assert_eq!(read_file(&dir, "latin1.txt").unwrap()["binary"], true);
        assert_eq!(read_file(&dir, "latin.txt").unwrap()["text"], "caf\u{e9}");
        assert_eq!(read_file(&dir, "big.txt").unwrap(), json!({"path": "big.txt", "too_large": true, "size": MAX_FILE + 1}));
        assert_eq!(read_file(&dir, "inside.rs").unwrap()["text"], "fn a() {}\n", "a link that stays inside is fine");
        for bad in ["../secret.txt", "src/../../secret.txt", outside.to_str().unwrap(), "/etc/passwd", "link.txt", "up/secret.txt", ".git/config", "", "."] {
            assert!(read_file(&dir, bad).is_err(), "{bad} was read");
        }
        assert!(read_file(&dir, "src").unwrap_err().to_string().contains("not a file"));
        assert!(read_file(&dir, "nope.txt").unwrap_err().to_string().contains("does not exist"));
    }
}
