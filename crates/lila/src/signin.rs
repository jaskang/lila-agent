//! Signing in a new subscription account in the vendor's web page, the way
//! magpie does it: the CLI's own login runs in the background, pointed at the
//! account's config folder, and its sign-in page opens in the browser. The
//! tokens and who signed in stay the CLI's; Lila asks the vendor nothing.
//!
//! - Claude Code (`claude auth login --claudeai`) opens the page through
//!   `$BROWSER`: Lila gives it a stand-in that writes the link down, and opens
//!   the page itself. If the page can't send the browser back to this machine,
//!   it shows a code, which goes to the login's input (`paste`).
//! - Codex (`codex login`) opens the browser itself and prints the link; Lila
//!   keeps it to open the page again.
//!
//! The login exits 0 once the browser came back: the account is signed in.

use crate::accounts::Account;
use anyhow::{Context, Result};
use lila_core::Runtime;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The environment variable the stand-in browser writes the link to.
const OPENED_URL_ENV: &str = "LILA_OPENED_URL";

/// How long a login waits for the browser before it is stopped (magpie's too).
pub const TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The sign-in page. `opened`: the CLI opened it already; else whoever
    /// started the sign-in opens it.
    Link { url: String, opened: bool },
    /// Signed in.
    Done,
    /// The login ended without signing in: why, in its own last words.
    Failed(String),
}

/// A login running in the background.
pub struct SignIn {
    pub events: Receiver<Event>,
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
}

impl SignIn {
    /// A code the sign-in page showed (when it couldn't send the browser back).
    pub fn paste(&self, code: &str) -> Result<()> {
        let mut g = self.stdin.lock().unwrap();
        let w = g.as_mut().context("the login no longer takes input")?;
        writeln!(w, "{}", code.trim())?;
        w.flush()?;
        Ok(())
    }

    /// Stops the login.
    pub fn cancel(&self) {
        let _ = self.child.lock().unwrap().kill();
    }
}

impl Drop for SignIn {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// The command for `a`'s login, its program found on `path`.
pub fn command(a: &Account, program: &Path, browser: &Path, opened: &Path, path_env: &str) -> Command {
    let mut cmd = Command::new(program);
    match a.runtime {
        Runtime::Claude => {
            cmd.args(["auth", "login", "--claudeai"]);
        }
        Runtime::Codex => {
            cmd.arg("login");
        }
    }
    // None of the wiring that would point it at another folder or session.
    for k in ["CLAUDE_CONFIG_DIR", "CLAUDE_SECURESTORAGE_CONFIG_DIR", "CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CODEX_HOME", "BROWSER"] {
        cmd.env_remove(k);
    }
    if let Some((k, v)) = a.env() {
        cmd.env(k, v);
    }
    cmd.env("PATH", path_env).env("BROWSER", browser).env(OPENED_URL_ENV, opened);
    if let Some(d) = &a.dir {
        cmd.current_dir(d);
    }
    cmd
}

/// The stand-in browser: writes the page it is asked to open to `$LILA_OPENED_URL`.
pub fn write_browser(dir: &Path) -> Result<PathBuf> {
    let p = dir.join(".lila-browser");
    std::fs::write(&p, format!("#!/bin/sh\nprintf '%s' \"$1\" > \"${OPENED_URL_ENV}\"\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(p)
}

/// The first https link in a line the login printed. Its terminal escapes stay
/// in: Claude Code's OSC 8 hyperlink holds the whole link, its text a shortened one.
pub fn link_in(line: &str) -> Option<String> {
    let mut finder = linkify::LinkFinder::new();
    finder.kinds(&[linkify::LinkKind::Url]);
    finder.links(line).map(|l| l.as_str()).find(|u| u.starts_with("https://")).map(String::from)
}

/// Starts `a`'s login. `program`: the CLI (absolute); `path_env`: its PATH.
pub fn start(a: &Account, program: &Path, path_env: &str) -> Result<SignIn> {
    let dir = a.dir.clone().context("the CLI's own account signs in in the CLI itself")?;
    std::fs::create_dir_all(&dir)?;
    let browser = write_browser(&dir)?;
    let opened = dir.join(".lila-opened-url");
    let _ = std::fs::remove_file(&opened);
    let mut cmd = command(a, program, &browser, &opened, path_env);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().with_context(|| format!("start {}", program.display()))?;
    let stdin = Arc::new(Mutex::new(child.stdin.take()));
    let (out, err) = (child.stdout.take().expect("piped"), child.stderr.take().expect("piped"));
    let child = Arc::new(Mutex::new(child));
    let (tx, rx) = channel();

    // What it prints: the link it shows, and its last words for a failure.
    let printed: Arc<Mutex<(Option<String>, String)>> = Arc::new(Mutex::new((None, String::new())));
    for stream in [Box::new(out) as Box<dyn std::io::Read + Send>, Box::new(err)] {
        let printed = printed.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(|l| l.ok()) {
                let mut p = printed.lock().unwrap();
                if let Some(u) = link_in(&line) {
                    p.0.get_or_insert(u);
                }
                let plain = strip_escapes(&line);
                let plain = plain.trim().trim_start_matches("Paste code here if prompted >").trim();
                if !plain.is_empty() && !plain.contains("https://") {
                    p.1 = plain.to_owned();
                }
            }
        });
    }
    let watched = child.clone();
    let stdin_kept = stdin.clone();
    std::thread::spawn(move || watch(watched, stdin_kept, printed, opened, tx, TIMEOUT));
    Ok(SignIn { events: rx, child, stdin })
}

/// Reports the link (the stand-in browser's first; a printed one once it is
/// clear none comes that way), then how the login ended.
fn watch(child: Arc<Mutex<Child>>, stdin: Arc<Mutex<Option<ChildStdin>>>, printed: Arc<Mutex<(Option<String>, String)>>, opened: PathBuf, tx: Sender<Event>, timeout: Duration) {
    let started = Instant::now();
    let mut timed_out = false;
    let mut told = false;
    let mut printed_at: Option<Instant> = None;
    loop {
        if !told {
            if let Ok(u) = std::fs::read_to_string(&opened)
                && !u.trim().is_empty()
            {
                let _ = tx.send(Event::Link { url: u.trim().to_owned(), opened: false });
                told = true;
            } else if let Some(u) = printed.lock().unwrap().0.clone() {
                // Printed but not handed to the stand-in: the CLI opens its browser itself (Codex).
                let since = *printed_at.get_or_insert_with(Instant::now);
                if since.elapsed() >= Duration::from_millis(1500) {
                    let _ = tx.send(Event::Link { url: u, opened: true });
                    told = true;
                }
            }
        }
        let status = child.lock().unwrap().try_wait();
        match status {
            Ok(Some(s)) => {
                stdin.lock().unwrap().take();
                // Let the readers finish its last words.
                std::thread::sleep(Duration::from_millis(100));
                let last = printed.lock().unwrap().1.clone();
                let why = if timed_out {
                    format!("{} 分钟内没有在网页上完成登录", timeout.as_secs() / 60)
                } else if last.is_empty() {
                    format!("the login ended ({s})")
                } else {
                    last
                };
                let _ = tx.send(if s.success() { Event::Done } else { Event::Failed(why) });
                let _ = std::fs::remove_file(&opened);
                return;
            }
            Ok(None) => {
                if !timed_out && started.elapsed() >= timeout {
                    timed_out = true;
                    let _ = child.lock().unwrap().kill();
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                let _ = tx.send(Event::Failed(e.to_string()));
                return;
            }
        }
    }
}

fn strip_escapes(s: &str) -> String {
    String::from_utf8_lossy(&strip_ansi_escapes::strip(s)).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn account(dir: &Path, r: Runtime) -> Account {
        Account { id: "x".into(), runtime: r, name: "x".into(), dir: Some(dir.to_path_buf()), current: false }
    }

    #[test]
    fn links_come_out_of_what_the_cli_prints() {
        let osc = "If the browser didn't open, visit: \u{1b}]8;;https://claude.com/cai/oauth/authorize?code=true&x=1\u{7}https://claude.com/cai/…\u{1b}]8;;\u{7}";
        assert_eq!(link_in(osc).as_deref(), Some("https://claude.com/cai/oauth/authorize?code=true&x=1"));
        assert_eq!(link_in("https://auth.openai.com/oauth/authorize?a=b").as_deref(), Some("https://auth.openai.com/oauth/authorize?a=b"));
        assert_eq!(link_in("Starting local login server on http://localhost:1455."), None);
        assert_eq!(strip_escapes("\u{1b}[1mPaste\u{1b}[0m code"), "Paste code");
    }

    #[test]
    fn the_command_points_the_cli_at_the_account() {
        let dir = tempfile::tempdir().unwrap();
        let cmd = command(&account(dir.path(), Runtime::Claude), Path::new("/bin/claude"), Path::new("/b"), Path::new("/o"), "/usr/bin");
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();
        assert_eq!(args, ["auth", "login", "--claudeai"]);
        let envs: std::collections::BTreeMap<String, Option<String>> =
            cmd.get_envs().map(|(k, v)| (k.to_string_lossy().to_string(), v.map(|v| v.to_string_lossy().to_string()))).collect();
        assert_eq!(envs["CLAUDE_CONFIG_DIR"].as_deref(), Some(dir.path().to_str().unwrap()));
        assert_eq!(envs["BROWSER"].as_deref(), Some("/b"));
        assert_eq!(envs["CLAUDECODE"], None, "removed");
        let codex = command(&account(dir.path(), Runtime::Codex), Path::new("/bin/codex"), Path::new("/b"), Path::new("/o"), "/usr/bin");
        assert_eq!(codex.get_args().collect::<Vec<_>>(), ["login"]);
    }

    /// A fake CLI: opens its page through `$BROWSER` (like Claude Code) or only
    /// prints it (like Codex), then waits for a code on its input.
    fn fake_cli(dir: &Path, through_browser: bool, exit: i32) -> PathBuf {
        let p = dir.join("fake-cli");
        let open = if through_browser { "\"$BROWSER\" \"https://example.com/sign-in?x=1\"" } else { "true" };
        std::fs::write(&p, format!("#!/bin/sh\n{open}\necho 'visit: https://example.com/sign-in?x=1'\necho 'Paste code here if prompted >'\nread code\necho \"got $code\"\nexit {exit}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn next(s: &SignIn) -> Event {
        s.events.recv_timeout(Duration::from_secs(10)).expect("an event")
    }

    #[test]
    fn a_sign_in_gives_its_page_then_ends() {
        let dir = tempfile::tempdir().unwrap();
        let path = std::env::var("PATH").unwrap_or_default();
        let s = start(&account(dir.path(), Runtime::Claude), &fake_cli(dir.path(), true, 0), &path).unwrap();
        assert_eq!(next(&s), Event::Link { url: "https://example.com/sign-in?x=1".into(), opened: false }, "through the stand-in: Lila opens it");
        s.paste("CODE-123").unwrap();
        assert_eq!(next(&s), Event::Done);

        let s = start(&account(dir.path(), Runtime::Codex), &fake_cli(dir.path(), false, 3), &path).unwrap();
        assert_eq!(next(&s), Event::Link { url: "https://example.com/sign-in?x=1".into(), opened: true }, "printed only: the CLI opened it");
        s.paste("nope").unwrap();
        assert_eq!(next(&s), Event::Failed("got nope".into()));
    }

    #[test]
    fn a_login_nobody_finishes_is_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let a = account(dir.path(), Runtime::Claude);
        let mut cmd = command(&a, &fake_cli(dir.path(), true, 0), &write_browser(dir.path()).unwrap(), &dir.path().join("o"), &std::env::var("PATH").unwrap_or_default());
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let stdin = Arc::new(Mutex::new(child.stdin.take()));
        let (tx, rx) = channel();
        let printed = Arc::new(Mutex::new((None, String::new())));
        std::thread::spawn(move || watch(Arc::new(Mutex::new(child)), stdin, printed, dir.path().join("o"), tx, Duration::from_millis(500)));
        let last = std::iter::from_fn(|| rx.recv_timeout(Duration::from_secs(10)).ok()).last();
        assert_eq!(last, Some(Event::Failed("0 分钟内没有在网页上完成登录".into())));
    }

    #[test]
    fn a_cancelled_sign_in_stops_its_login() {
        let dir = tempfile::tempdir().unwrap();
        let s = start(&account(dir.path(), Runtime::Claude), &fake_cli(dir.path(), true, 0), &std::env::var("PATH").unwrap_or_default()).unwrap();
        assert!(matches!(next(&s), Event::Link { .. }));
        s.cancel();
        assert!(matches!(next(&s), Event::Failed(_)));
    }
}
