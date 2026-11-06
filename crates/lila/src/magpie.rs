//! magpie (github.com/yetone/magpie), when this computer has it: the Claude
//! Code and Codex accounts it is signed in to, with their allowance, and
//! switching the CLI's own sign-in to one of them.
//!
//! Lila never copies magpie's tokens: both vendors rotate refresh tokens, so
//! a second holder would sign one of the two out. magpie keeps them and moves
//! the chosen one into the CLI's own store (`magpie accounts switch`), which is
//! what members on Lila's default account (the CLI's own folder) run on.

use crate::accounts::{AccountUsage, Window};
use anyhow::{Context, Result, bail};
use lila_core::Runtime;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// One subscription account magpie knows.
#[derive(Debug, Clone, PartialEq)]
pub struct MagpieAccount {
    pub runtime: Runtime,
    /// The email it signed in as: what `switch` takes.
    pub user: String,
    /// Signed in to the CLI's own store right now.
    pub active: bool,
    pub usage: AccountUsage,
}

/// The magpie command line: the app's, or one on `path`.
pub fn find(path: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    [PathBuf::from("/Applications/magpie.app/Contents/MacOS/magpie"), home.join("Applications/magpie.app/Contents/MacOS/magpie")]
        .into_iter()
        .find(|p| p.is_file())
        .or_else(|| crate::paths::which("magpie", path))
}

/// Runs magpie with `args`, giving up after `timeout`.
fn run(bin: &Path, args: &[&str], timeout: Duration) -> Result<String> {
    let mut child = Command::new(bin).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().with_context(|| format!("run {}", bin.display()))?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            let out = child.wait_with_output()?;
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            if !status.success() {
                let err = String::from_utf8_lossy(&out.stderr);
                let why = err.lines().chain(text.lines()).map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("").to_owned();
                bail!("magpie {} failed: {why}", args.join(" "));
            }
            return Ok(text);
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            bail!("magpie {} took longer than {} s", args.join(" "), timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn window_name(magpie: &str) -> String {
    match magpie {
        "5 hours" => "5 小时".into(),
        "7 days" => "本周".into(),
        "1 day" | "24 hours" => "今天".into(),
        other => other.to_owned(),
    }
}

fn plan_name(r: Runtime, raw: &str) -> String {
    match (r, raw) {
        (Runtime::Claude, "pro") => "Claude Pro".into(),
        (Runtime::Claude, "max") => "Claude Max".into(),
        (Runtime::Codex, "pro") => "ChatGPT Pro".into(),
        (Runtime::Codex, "plus") => "ChatGPT Plus".into(),
        (Runtime::Codex, "team") => "ChatGPT Team".into(),
        (_, other) => other.to_owned(),
    }
}

/// `magpie accounts --json`: the Claude Code and Codex accounts (others are left out).
pub fn parse(json: &str) -> Result<Vec<MagpieAccount>> {
    let v: Value = serde_json::from_str(json).context("magpie accounts --json")?;
    let list = v.as_array().context("magpie accounts --json: not a list")?;
    Ok(list
        .iter()
        .filter_map(|a| {
            let runtime = match a["agent"].as_str()? {
                "claude" => Runtime::Claude,
                "codex" => Runtime::Codex,
                _ => return None,
            };
            let windows = a["windows"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|w| {
                    Some(Window {
                        name: window_name(w["name"].as_str()?),
                        used_percent: w["used"].as_f64()?,
                        resets_ms: w["resetsAt"].as_str().and_then(crate::usage::parse_time)?,
                    })
                })
                .collect();
            Some(MagpieAccount {
                runtime,
                user: a["user"].as_str()?.to_owned(),
                active: a["active"].as_bool().unwrap_or(false),
                usage: AccountUsage {
                    plan: a["plan"].as_str().map(|p| plan_name(runtime, p)),
                    windows,
                    seen_ms: a["readAt"].as_str().and_then(crate::usage::parse_time),
                },
            })
        })
        .collect())
}

/// The accounts magpie knows, as it reads them now.
pub fn accounts(bin: &Path) -> Result<Vec<MagpieAccount>> {
    parse(&run(bin, &["accounts", "--json"], Duration::from_secs(30))?)
}

/// Signs `r`'s own store in to `user`'s account (magpie moves the tokens).
pub fn switch(bin: &Path, r: Runtime, user: &str) -> Result<()> {
    let agent = if r == Runtime::Claude { "claude" } else { "codex" };
    run(bin, &["accounts", "switch", agent, user], Duration::from_secs(60)).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const JSON: &str = r#"[
      {"agent":"claude","user":"a@x.com","plan":"pro","active":true,"on":true,"readAt":"2026-11-06T17:13:21.33+08:00",
       "windows":[{"name":"5 hours","used":66,"remaining":34,"resetsAt":"2026-11-06T21:00:00+08:00"},{"name":"7 days","used":76,"remaining":24,"resetsAt":"2026-11-06T19:00:00+08:00"}]},
      {"agent":"codex","user":"b@x.com","plan":"pro","active":false,"on":true,"windows":[{"name":"7 days","used":100,"remaining":0,"resetsAt":"2026-11-12T11:11:35+08:00"}]},
      {"agent":"plugin:cursor","user":"c@x.com","plan":"Pro","windows":[{"name":"month","used":3}]},
      {"agent":"codex","user":"d@x.com","plan":"plus","windows":[{"name":"7 days","used":5}]}
    ]"#;

    #[test]
    fn its_claude_and_codex_accounts_with_their_windows() {
        let list = parse(JSON).unwrap();
        assert_eq!(list.iter().map(|a| (a.runtime, a.user.as_str(), a.active)).collect::<Vec<_>>(), [
            (Runtime::Claude, "a@x.com", true),
            (Runtime::Codex, "b@x.com", false),
            (Runtime::Codex, "d@x.com", false)
        ]);
        let c = &list[0].usage;
        assert_eq!(c.plan.as_deref(), Some("Claude Pro"));
        assert_eq!(c.windows.iter().map(|w| (w.name.as_str(), w.used_percent)).collect::<Vec<_>>(), [("5 小时", 66.0), ("本周", 76.0)]);
        assert!(c.seen_ms.is_some());
        assert!(list[2].usage.windows.is_empty(), "a window with no reset time is left out");
        assert_eq!(list[2].usage.plan.as_deref(), Some("ChatGPT Plus"));
        assert!(parse("{}").is_err());
    }

    fn fake_magpie(dir: &Path, body: &str) -> PathBuf {
        let p = dir.join("magpie");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn it_runs_magpie_and_says_why_it_failed() {
        let dir = tempfile::tempdir().unwrap();
        let ok = fake_magpie(dir.path(), "echo \"$@\" > \"$(dirname \"$0\")/args\"; echo '[]'");
        assert!(accounts(&ok).unwrap().is_empty());
        switch(&ok, Runtime::Codex, "b@x.com").unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("args")).unwrap().trim(), "accounts switch codex b@x.com");
        let bad = fake_magpie(dir.path(), "echo 'no account b@x.com for codex' >&2; exit 1");
        assert!(switch(&bad, Runtime::Codex, "b@x.com").unwrap_err().to_string().contains("no account b@x.com"));
    }
}
