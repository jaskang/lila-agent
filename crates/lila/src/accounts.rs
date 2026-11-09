//! Subscription accounts. Each CLI can be logged in to several accounts, each
//! in its own config folder (`CLAUDE_CONFIG_DIR` / `CODEX_HOME`); one per CLI is
//! current, and members run on it. Usage is per account, read from the CLI's own
//! records: Claude Code caches its rate-limit windows in `.claude.json`, Codex
//! writes them into its session rollouts.
//!
//! `~/.config/lila/accounts.toml`; without it each CLI has one account, its own
//! default folder (`~/.claude`, `~/.codex`).

use anyhow::{Context, Result, bail};
use lila_core::Runtime;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub runtime: Runtime,
    pub name: String,
    /// Its config folder. None: the CLI's own default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<PathBuf>,
    #[serde(default)]
    pub current: bool,
}

impl Account {
    /// The CLI's own folder, which Lila never deletes.
    pub fn is_default(&self) -> bool {
        self.dir.is_none()
    }

    /// The environment variable that points the CLI at this account, when it is not the default.
    pub fn env(&self) -> Option<(&'static str, String)> {
        let dir = self.dir.as_ref()?;
        Some((env_var(self.runtime), dir.to_string_lossy().to_string()))
    }

    /// Where its records are: the folder, or the CLI's default under `home`.
    pub fn root(&self, home: &Path) -> PathBuf {
        self.dir.clone().unwrap_or_else(|| home.join(match self.runtime {
            Runtime::Claude => ".claude",
            Runtime::Codex => ".codex",
            Runtime::Omp => ".omp",
        }))
    }

    /// The command that logs this account in, with its environment.
    pub fn login_command(&self) -> (Vec<&'static str>, Option<(&'static str, String)>) {
        let argv = match self.runtime {
            Runtime::Claude => vec!["claude", "auth", "login"],
            Runtime::Codex => vec!["codex", "login"],
            // omp signs in from inside (`/login`); it keeps its own logins in ~/.omp.
            Runtime::Omp => vec!["omp"],
        };
        (argv, self.env())
    }

    pub fn logout_command(&self) -> (Vec<&'static str>, Option<(&'static str, String)>) {
        let argv = match self.runtime {
            Runtime::Claude => vec!["claude", "auth", "logout"],
            Runtime::Codex => vec!["codex", "logout"],
            Runtime::Omp => vec!["omp"],
        };
        (argv, self.env())
    }
}

pub fn env_var(r: Runtime) -> &'static str {
    match r {
        Runtime::Claude => "CLAUDE_CONFIG_DIR",
        Runtime::Codex => "CODEX_HOME",
        // omp's own profiles (`--profile`) keep accounts apart; Lila holds none for it.
        Runtime::Omp => "LILA_OMP_UNUSED",
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    accounts: Vec<Account>,
}

fn defaults() -> Vec<Account> {
    [Runtime::Claude, Runtime::Codex]
        .into_iter()
        .map(|r| Account { id: format!("{r}-default"), runtime: r, name: "默认".into(), dir: None, current: true })
        .collect()
}

/// The accounts, with each CLI's default account first and exactly one current per CLI.
pub fn load(path: &Path) -> Result<Vec<Account>> {
    let mut list = match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str::<File>(&text).with_context(|| format!("{}", path.display()))?.accounts,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    for d in defaults() {
        if !list.iter().any(|a| a.runtime == d.runtime && a.is_default()) {
            let first_of_cli = !list.iter().any(|a| a.runtime == d.runtime);
            list.insert(0, Account { current: first_of_cli, ..d });
        }
    }
    for r in [Runtime::Claude, Runtime::Codex] {
        let current: Vec<usize> = list.iter().enumerate().filter(|(_, a)| a.runtime == r && a.current).map(|(i, _)| i).collect();
        match current.as_slice() {
            [_] => {}
            [] => {
                if let Some(a) = list.iter_mut().find(|a| a.runtime == r) {
                    a.current = true;
                }
            }
            [_, rest @ ..] => {
                for i in rest {
                    list[*i].current = false;
                }
            }
        }
    }
    list.sort_by_key(|a| (a.runtime != Runtime::Claude, !a.is_default()));
    Ok(list)
}

pub fn save(path: &Path, list: &[Account]) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let text = format!(
        "# Lila's subscription accounts: one config folder each; `current` is the one members run on.\n{}",
        toml::to_string_pretty(&File { accounts: list.to_vec() })?
    );
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Each CLI's current account.
pub fn current(list: &[Account], r: Runtime) -> Option<&Account> {
    list.iter().find(|a| a.runtime == r && a.current)
}

/// A new account for `r` named `name`, with its own folder under `root`
/// (`~/.config/lila/accounts`). Not current: it logs in first.
pub fn add(path: &Path, root: &Path, r: Runtime, name: &str) -> Result<Account> {
    let a = prepare(path, root, r, name)?;
    keep(path, &a)?;
    Ok(a)
}

/// Saves an account `prepare` made, once it signed in.
pub fn keep(path: &Path, a: &Account) -> Result<()> {
    let mut list = load(path)?;
    if !list.iter().any(|x| x.id == a.id) {
        list.push(a.clone());
    }
    save(path, &list)
}

/// An account for `r` with its folder made, not saved yet: it signs in in it
/// first (`signin`), then `keep` saves it. Forgetting it is deleting the folder.
pub fn prepare(path: &Path, root: &Path, r: Runtime, name: &str) -> Result<Account> {
    let list = load(path)?;
    forget_stale(root, &list);
    let name = name.trim();
    let name = if name.is_empty() { format!("账号 {}", list.iter().filter(|a| a.runtime == r).count() + 1) } else { name.to_owned() };
    let slug: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
    let base = if slug.is_empty() { format!("{r}") } else { format!("{r}-{slug}") };
    let id = (1..).map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") }).find(|id| !list.iter().any(|a| &a.id == id) && !root.join(id).exists()).expect("a free id");
    let dir = root.join(&id);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    Ok(Account { id, runtime: r, name, dir: Some(dir), current: false })
}

/// Folders of sign-ins that never finished (the app quit on the way): not in
/// the list, untouched for longer than a sign-in may take.
fn forget_stale(root: &Path, list: &[Account]) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    let limit = crate::signin::TIMEOUT + std::time::Duration::from_secs(5 * 60);
    for e in entries.flatten() {
        let p = e.path();
        let kept = list.iter().any(|a| a.dir.as_deref() == Some(p.as_path()));
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > limit);
        if p.is_dir() && !kept && old {
            let _ = std::fs::remove_dir_all(&p);
        }
    }
}

pub fn set_current(path: &Path, id: &str) -> Result<()> {
    let mut list = load(path)?;
    let r = list.iter().find(|a| a.id == id).map(|a| a.runtime).with_context(|| format!("no account {id}"))?;
    for a in list.iter_mut().filter(|a| a.runtime == r) {
        a.current = a.id == id;
    }
    save(path, &list)
}

/// Forgets an account and deletes its folder. Not the current one, and never
/// the CLI's own default folder. The caller logs it out first.
pub fn remove(path: &Path, id: &str) -> Result<Account> {
    let mut list = load(path)?;
    let Some(i) = list.iter().position(|a| a.id == id) else { bail!("no account {id}") };
    let a = list[i].clone();
    if a.is_default() {
        bail!("the CLI's own account stays; log out of it in the CLI");
    }
    if a.current {
        bail!("{} is in use; make another account current first", a.name);
    }
    list.remove(i);
    save(path, &list)?;
    if let Some(dir) = &a.dir
        && dir.exists()
    {
        std::fs::remove_dir_all(dir).with_context(|| format!("delete {}", dir.display()))?;
    }
    Ok(a)
}

/// One usage window as the CLI last recorded it.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    /// "5 小时", "本周", ...
    pub name: String,
    /// 0–100.
    pub used_percent: f64,
    pub resets_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AccountUsage {
    /// "Claude Pro", "ChatGPT Pro", when the records say.
    pub plan: Option<String>,
    pub windows: Vec<Window>,
    /// When the CLI recorded it.
    pub seen_ms: Option<i64>,
}

/// What an account's records say about its usage windows.
pub fn usage(a: &Account, home: &Path) -> AccountUsage {
    match a.runtime {
        // Lila reads no usage of omp's own accounts.
        Runtime::Omp => AccountUsage::default(),
        Runtime::Claude => {
            // CLAUDE_CONFIG_DIR keeps `.claude.json` inside it; the default lives in the home folder.
            let json = match &a.dir {
                Some(d) => d.join(".claude.json"),
                None => home.join(".claude.json"),
            };
            claude_usage(&std::fs::read_to_string(json).unwrap_or_default())
        }
        Runtime::Codex => codex_usage(&a.root(home).join("sessions")),
    }
}

fn plan_name(raw: &str) -> String {
    match raw {
        "claude_pro" => "Claude Pro".into(),
        "claude_max" => "Claude Max".into(),
        "claude_team" => "Claude Team".into(),
        "claude_enterprise" => "Claude Enterprise".into(),
        "pro" => "ChatGPT Pro".into(),
        "plus" => "ChatGPT Plus".into(),
        "team" => "ChatGPT Team".into(),
        "free" => "ChatGPT Free".into(),
        other => other.to_owned(),
    }
}

/// Claude Code's `.claude.json`: `cachedUsageUtilization` (the windows, in percent)
/// and `oauthAccount.organizationType` (the plan).
pub fn claude_usage(text: &str) -> AccountUsage {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return AccountUsage::default() };
    let plan = v["oauthAccount"]["organizationType"].as_str().map(plan_name);
    let cache = &v["cachedUsageUtilization"];
    let u = &cache["utilization"];
    let windows = [("five_hour", "5 小时"), ("seven_day", "本周"), ("seven_day_opus", "本周 · Opus"), ("seven_day_sonnet", "本周 · Sonnet")]
        .into_iter()
        .filter_map(|(key, name)| {
            let w = &u[key];
            let used = w["utilization"].as_f64()?;
            let resets = w["resets_at"].as_str().and_then(crate::usage::parse_time)?;
            Some(Window { name: name.into(), used_percent: used, resets_ms: resets })
        })
        .collect();
    AccountUsage { plan, windows, seen_ms: cache["fetchedAtMs"].as_i64() }
}

fn window_name(minutes: u64) -> String {
    match minutes {
        300 => "5 小时".into(),
        1440 => "今天".into(),
        10080 => "本周".into(),
        m if m % 1440 == 0 => format!("{} 天", m / 1440),
        m if m % 60 == 0 => format!("{} 小时", m / 60),
        m => format!("{m} 分钟"),
    }
}

/// The newest rate-limit record in a Codex sessions folder (`YYYY/MM/DD/rollout-*.jsonl`).
pub fn codex_usage(sessions: &Path) -> AccountUsage {
    // The newest days first, the newest files in them first; stop at the first record.
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = walkdir::WalkDir::new(sessions)
        .max_depth(4)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|x| x == "jsonl"))
        .map(|e| (e.metadata().ok().and_then(|m| m.modified().ok()).unwrap_or(std::time::UNIX_EPOCH), e.into_path()))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    for (_, f) in files.into_iter().take(30) {
        if let Some(u) = codex_record(&crate::sessions::read_tail(&f, 256 * 1024)) {
            return u;
        }
    }
    AccountUsage::default()
}

fn codex_record(text: &str) -> Option<AccountUsage> {
    for line in text.lines().rev() {
        if !line.contains("\"rate_limits\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let r = &v["payload"]["rate_limits"];
        let windows: Vec<Window> = ["primary", "secondary"]
            .iter()
            .filter_map(|k| {
                let w = &r[*k];
                Some(Window {
                    name: window_name(w["window_minutes"].as_u64()?),
                    used_percent: w["used_percent"].as_f64()?,
                    resets_ms: w["resets_at"].as_i64()? * 1000,
                })
            })
            .collect();
        if windows.is_empty() {
            continue;
        }
        return Some(AccountUsage {
            plan: r["plan_type"].as_str().map(plan_name),
            windows,
            seen_ms: v["timestamp"].as_str().and_then(crate::usage::parse_time),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn without_a_file_each_cli_has_its_own_folder() {
        let dir = tempfile::tempdir().unwrap();
        let list = load(&dir.path().join("accounts.toml")).unwrap();
        assert_eq!(list.iter().map(|a| (a.id.as_str(), a.current, a.is_default())).collect::<Vec<_>>(), [("claude-default", true, true), ("codex-default", true, true)]);
        assert!(current(&list, Runtime::Codex).unwrap().env().is_none(), "the default needs no variable");
        assert_eq!(list[0].root(Path::new("/h")), Path::new("/h/.claude"));
    }

    #[test]
    fn accounts_are_added_switched_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let (path, root) = (dir.path().join("accounts.toml"), dir.path().join("accounts"));
        let work = add(&path, &root, Runtime::Claude, "Work").unwrap();
        assert_eq!((work.id.as_str(), work.current), ("claude-work", false));
        assert!(root.join("claude-work").is_dir());
        let unnamed = add(&path, &root, Runtime::Claude, "工作").unwrap();
        assert_eq!((unnamed.id.as_str(), unnamed.name.as_str()), ("claude", "工作"), "no ASCII in the name: the CLI's name");
        assert_eq!(add(&path, &root, Runtime::Claude, "work").unwrap().id, "claude-work-2");
        let (argv, env) = work.login_command();
        assert_eq!(argv, ["claude", "auth", "login"]);
        assert_eq!(env.unwrap(), ("CLAUDE_CONFIG_DIR", root.join("claude-work").to_string_lossy().to_string()));

        set_current(&path, "claude-work").unwrap();
        let list = load(&path).unwrap();
        assert_eq!(current(&list, Runtime::Claude).unwrap().id, "claude-work");
        assert_eq!(current(&list, Runtime::Codex).unwrap().id, "codex-default", "the other CLI keeps its own");
        assert!(remove(&path, "claude-work").unwrap_err().to_string().contains("in use"));
        assert!(remove(&path, "claude-default").unwrap_err().to_string().contains("stays"));
        set_current(&path, "claude-default").unwrap();
        remove(&path, "claude-work").unwrap();
        assert!(!root.join("claude-work").exists(), "its folder goes with it");
        assert_eq!(load(&path).unwrap().iter().filter(|a| a.runtime == Runtime::Claude).count(), 3);
        // Prepared for a sign-in: its folder exists, the list doesn't have it until kept.
        let p = prepare(&path, &root, Runtime::Codex, "spare").unwrap();
        assert!(p.dir.as_ref().unwrap().is_dir() && !load(&path).unwrap().iter().any(|a| a.id == p.id));
        assert_ne!(prepare(&path, &root, Runtime::Codex, "spare").unwrap().id, p.id, "a folder in the way takes another id");
        keep(&path, &p).unwrap();
        assert!(load(&path).unwrap().iter().any(|a| a.id == "codex-spare"));
    }

    #[test]
    fn claude_windows_come_from_its_cache() {
        let text = json!({
            "oauthAccount": {"organizationType": "claude_pro"},
            "cachedUsageUtilization": {"fetchedAtMs": 1_793_954_109_619_i64, "utilization": {
                "five_hour": {"utilization": 20, "resets_at": "2026-11-06T13:00:00.390478+00:00"},
                "seven_day": {"utilization": 70.5, "resets_at": "2026-11-06T11:00:00+00:00"},
                "seven_day_opus": null,
            }},
        })
        .to_string();
        let u = claude_usage(&text);
        assert_eq!(u.plan.as_deref(), Some("Claude Pro"));
        assert_eq!(u.windows.iter().map(|w| (w.name.as_str(), w.used_percent)).collect::<Vec<_>>(), [("5 小时", 20.0), ("本周", 70.5)]);
        assert_eq!(u.seen_ms, Some(1_793_954_109_619));
        assert_eq!(claude_usage("not json"), AccountUsage::default());
    }

    #[test]
    fn codex_windows_come_from_its_newest_rollout() {
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("2026/11/06");
        std::fs::create_dir_all(&day).unwrap();
        let rec = |used: f64| {
            json!({"timestamp": "2026-11-06T07:48:47Z", "type": "event_msg", "payload": {"type": "token_count", "rate_limits": {
                "primary": {"used_percent": used, "window_minutes": 10080, "resets_at": 1_794_453_095}, "secondary": null, "plan_type": "pro"}}})
            .to_string()
        };
        std::fs::write(day.join("rollout-a.jsonl"), [rec(10.0), rec(41.0), json!({"type": "event_msg"}).to_string()].join("\n")).unwrap();
        let u = codex_usage(dir.path());
        assert_eq!(u.plan.as_deref(), Some("ChatGPT Pro"));
        assert_eq!(u.windows, [Window { name: "本周".into(), used_percent: 41.0, resets_ms: 1_794_453_095_000 }]);
        assert!(u.seen_ms.is_some());
        assert_eq!(codex_usage(&dir.path().join("none")), AccountUsage::default());
    }
}
