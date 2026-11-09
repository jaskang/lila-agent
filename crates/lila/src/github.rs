//! GitHub through the official `gh` command line: it signs in, pages and
//! retries, and Lila never holds a token. Blocking; lilad runs these off its loop.

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::check::tail;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub author: Author,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Author {
    #[serde(default)]
    pub login: String,
}

/// Runs `gh` in `cwd` (its repository is the default `--repo`), with `stdin` piped in.
fn gh(cwd: &Path, path_env: &str, repo: Option<&str>, args: &[&str], stdin: Option<&str>) -> Result<String> {
    let mut cmd = Command::new("gh");
    cmd.args(args).current_dir(cwd).env("PATH", path_env).env("GH_PROMPT_DISABLED", "1").env("NO_COLOR", "1");
    if let Some(r) = repo {
        cmd.args(["--repo", r]);
    }
    let mut child = cmd
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("gh is not on PATH ({e}); install it and run `gh auth login`"))?;
    if let Some(text) = stdin {
        child.stdin.take().expect("piped").write_all(text.as_bytes())?;
    }
    let o = child.wait_with_output()?;
    if !o.status.success() {
        bail!("gh {}: {}", args.first().copied().unwrap_or(""), tail(&format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))));
    }
    Ok(String::from_utf8_lossy(&o.stdout).trim().to_owned())
}

/// Issues matching `query` (`gh` search syntax), newest first.
pub fn issues(cwd: &Path, path_env: &str, repo: Option<&str>, query: &str) -> Result<Vec<Issue>> {
    let out = gh(cwd, path_env, repo, &["issue", "list", "--search", query, "--limit", "30", "--json", "number,title,body,author,url"], None)?;
    serde_json::from_str(&out).context("gh issue list: unexpected output")
}

/// Comments on issue `number`; returns the comment's URL.
pub fn comment(cwd: &Path, path_env: &str, repo: Option<&str>, number: &str, body: &str) -> Result<String> {
    gh(cwd, path_env, repo, &["issue", "comment", number, "--body-file", "-"], Some(body))
}

/// Pushes `branch` from `slot` and opens a pull request into `base`; returns its URL.
pub fn open_pr(slot: &Path, path_env: &str, repo: Option<&str>, branch: &str, base: &str, title: &str, body: &str) -> Result<String> {
    crate::worktree::out(slot, path_env, &["push", "-q", "--force-with-lease", "-u", "origin", branch])?;
    gh(slot, path_env, repo, &["pr", "create", "--head", branch, "--base", base, "--title", title, "--body-file", "-"], Some(body))
        .map(|out| out.lines().rev().find(|l| l.starts_with("http")).unwrap_or(&out).to_owned())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A stand-in `gh` in `bin`: logs each call (args, then stdin) to `<bin>/gh.log`
    /// and answers `issue list` with `<bin>/issues.json`.
    pub(crate) fn fake_gh(bin: &Path) {
        std::fs::create_dir_all(bin).unwrap();
        let script = format!(
            r#"#!/bin/sh
log="{bin}/gh.log"
echo "ARGS $*" >> "$log"
case "$1 $2" in
  "issue list") cat "{bin}/issues.json" ;;
  "issue comment") cat >> "$log"; echo "https://github.com/o/r/issues/$3#issuecomment-1" ;;
  "pr create") cat >> "$log"; echo "https://github.com/o/r/pull/7" ;;
  *) echo "unknown $*" >&2; exit 1 ;;
esac
"#,
            bin = bin.display()
        );
        let p = bin.join("gh");
        std::fs::write(&p, script).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(bin.join("issues.json"), "[]").unwrap();
    }

    #[test]
    fn lists_comments_and_opens_a_pr() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        fake_gh(&bin);
        std::fs::write(bin.join("issues.json"), r#"[{"number":142,"title":"Login loops","body":"Steps…","author":{"login":"alice"},"url":"https://github.com/o/r/issues/142"}]"#).unwrap();
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
        let got = issues(tmp.path(), &path, Some("o/r"), "is:open label:bug").unwrap();
        assert_eq!((got[0].number, got[0].author.login.as_str()), (142, "alice"));
        assert_eq!(comment(tmp.path(), &path, None, "142", "Which browser?").unwrap(), "https://github.com/o/r/issues/142#issuecomment-1");
        let log = std::fs::read_to_string(bin.join("gh.log")).unwrap();
        assert!(log.contains("ARGS issue list --search is:open label:bug --limit 30 --json number,title,body,author,url --repo o/r"), "{log}");
        assert!(log.contains("ARGS issue comment 142 --body-file -\nWhich browser?"), "{log}");
    }

    #[test]
    fn a_missing_gh_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let err = issues(tmp.path(), "/nonexistent", None, "is:open").unwrap_err();
        assert!(err.to_string().contains("gh is not on PATH"), "{err:#}");
    }
}
