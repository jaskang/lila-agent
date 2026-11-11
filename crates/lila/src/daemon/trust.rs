//! Claude Code's "trust this folder" dialog. A folder Claude Code has not been trusted in (every new project, and every
//! thread with a worktree of its own) shows it in the terminal before anything else, and reports no status while it does.
//! lilad recognises it from the terminal's screen (herdr-core's `terminal.read`), so the window can offer to answer it
//! in place (`thread.trust`). The decision is the person's: lilad never answers it by itself, and it presses keys only
//! when the screen it has just read is this dialog.
//!
//! The text is Claude Code 2.1.322's (`fixtures/claude-trust-2.1.322.txt`). A screen that is not exactly this dialog is
//! not recognised, and then the thread waits in its terminal as before.

use super::*;

/// What a recognised dialog shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dialog {
    /// The folder it asks about.
    pub folder: String,
    /// The cursor is on "Yes, I trust this folder" (it starts on "No, exit").
    pub on_yes: bool,
}

const HEADING: &str = "Accessing workspace:";
const QUESTION: &str = "Quick safety check";
const YES: &str = "Yes, I trust this folder";
const NO: &str = "No, exit";
const CONFIRM: &str = "Enter to confirm";

/// The dialog on `screen`, if it is the one.
pub fn recognise(screen: &str) -> Option<Dialog> {
    let lines: Vec<&str> = screen.lines().map(str::trim).collect();
    let at = |text: &str| lines.iter().position(|l| l.trim_start_matches('❯').trim() == text);
    let (heading, yes, no) = (lines.iter().position(|l| *l == HEADING)?, at(YES)?, at(NO)?);
    // The options, one after the other, and the line that says how to answer under them.
    if no + 1 != yes || !lines.get(yes + 1..)?.iter().any(|l| l.starts_with(CONFIRM)) || heading > no {
        return None;
    }
    // The folder: the lines between the heading and the question (a long path wraps).
    let question = lines.iter().position(|l| l.starts_with(QUESTION))?;
    if question < heading || question > no {
        return None;
    }
    let folder: String = lines[heading + 1..question].iter().filter(|l| !l.is_empty()).copied().collect();
    if folder.is_empty() {
        return None;
    }
    let cursor = |i: usize| lines[i].starts_with('❯');
    // The cursor is on one of the two, and only one.
    (cursor(yes) != cursor(no)).then_some(Dialog { folder, on_yes: cursor(yes) })
}

impl<T: Terminals> Daemon<T> {
    /// The trust dialog on a thread's terminal right now, read from its screen.
    fn trust_dialog(&mut self, terminal: &str) -> Result<Option<Dialog>> {
        Ok(recognise(&self.term.read(terminal)?))
    }

    /// Every few seconds: the screens of the Claude threads that wait in their terminal or have not started yet are read, and a thread
    /// found at the trust dialog is marked (`waiting_for: "trust"`, with the folder), so clients need not read terminals.
    pub fn look(&mut self) {
        self.look_at(now_ms());
    }

    pub(super) fn look_at(&mut self, now: i64) {
        let mut seen: Vec<(String, Option<Dialog>)> = Vec::new();
        for t in self.store.live_threads().unwrap_or_default() {
            if t.engine != Runtime::Claude || t.archived {
                continue;
            }
            let (state, waiting) = self.state_of(&t, now);
            let Some(terminal) = t.terminal.clone().filter(|_| state == ThreadState::Starting || waiting == Some("terminal")) else { continue };
            // Nothing is on the screen yet in the first moments.
            if self.live.get(&t.id).is_some_and(|l| now - l.started_ms < 1000) {
                continue;
            }
            seen.push((t.id.clone(), self.trust_dialog(&terminal).unwrap_or(None)));
        }
        let ids: Vec<String> = self.trust_seen.keys().cloned().collect();
        for tid in ids {
            if !seen.iter().any(|(t, d)| *t == tid && d.is_some()) {
                self.trust_seen.remove(&tid);
                self.mark(&tid);
            }
        }
        for (tid, dialog) in seen {
            if let Some(d) = dialog
                && self.trust_seen.get(&tid) != Some(&d.folder)
            {
                self.trust_seen.insert(tid.clone(), d.folder);
                self.mark(&tid);
            }
        }
    }

    /// `thread.trust {thread}`: the person trusts the folder the thread's Claude Code asks about. The keys that accept go to its terminal only
    /// when the screen, read just before, is this dialog. Then the thread goes on, and what waited for it is sent.
    pub(super) fn thread_trust(&mut self, params: &Value, now: i64) -> Result<Value> {
        let t = self.thread_of(params)?;
        if t.engine != Runtime::Claude {
            bail!("{} is not a Claude Code thread: it has no trust dialog", t.id);
        }
        let terminal = t.terminal.clone().filter(|_| self.live.contains_key(&t.id)).ok_or_else(|| anyhow!("{} is not running", t.id))?;
        let Some(mut dialog) = self.trust_dialog(&terminal)? else {
            if self.trust_seen.remove(&t.id).is_some() {
                self.mark(&t.id);
            }
            bail!("{} is not at Claude Code's trust dialog; nothing was sent to its terminal", t.id);
        };
        let folder = dialog.folder.clone();
        // The cursor starts on "No, exit": Enter there would end Claude Code. It is moved to "Yes" and seen there first.
        if !dialog.on_yes {
            self.term.keys(&terminal, &["down"])?;
            for _ in 0..20 {
                std::thread::sleep(std::time::Duration::from_millis(75));
                match self.trust_dialog(&terminal)? {
                    Some(d) if d.on_yes => {
                        dialog = d;
                        break;
                    }
                    Some(_) => {}
                    None => bail!("the screen of {} changed while its cursor was being moved; nothing more was sent", t.id),
                }
            }
            if !dialog.on_yes {
                bail!("could not move the cursor of {}'s dialog to \"{YES}\"; nothing more was sent", t.id);
            }
        }
        // Right before Enter, once more.
        if !self.trust_dialog(&terminal)?.is_some_and(|d| d.on_yes) {
            bail!("the screen of {} is not the trust dialog with \"{YES}\" chosen any more; nothing more was sent", t.id);
        }
        self.term.keys(&terminal, &["enter"])?;
        self.trust_seen.remove(&t.id);
        self.store.event(&t.id, "human", "trusted", &folder, now)?;
        self.mark(&t.id);
        Ok(json!({"thread": t.id, "trusted": true, "folder": folder}))
    }
}

#[cfg(test)]
mod tests {
    use super::{Dialog, recognise};

    const TRUST: &str = include_str!("fixtures/claude-trust-2.1.322.txt");
    const LOGIN: &str = include_str!("fixtures/claude-login-2.1.322.txt");

    #[test]
    fn the_trust_dialog_of_claude_code_2_1_322_is_recognised_with_its_folder() {
        assert_eq!(recognise(TRUST), Some(Dialog { folder: "/tmp/shop".into(), on_yes: false }));
        // Once the cursor has moved down.
        let moved = TRUST.replace("❯ No, exit", "  No, exit").replace("  Yes, I trust this folder", "❯ Yes, I trust this folder");
        assert_eq!(recognise(&moved), Some(Dialog { folder: "/tmp/shop".into(), on_yes: true }));
        // A long path wraps over lines; the folder is put back together.
        let wrapped = TRUST.replace("/tmp/shop", "/Users/someone/projects/a-rather-long-folder-name/\n sub-folder-with-a-longer-name");
        assert_eq!(recognise(&wrapped).map(|d| d.folder), Some("/Users/someone/projects/a-rather-long-folder-name/sub-folder-with-a-longer-name".into()));
        // Other things on the screen above or below it do not matter.
        assert!(recognise(&format!("some earlier output\n\n{TRUST}\n\n")).is_some());
    }

    #[test]
    fn anything_else_is_not_recognised() {
        assert_eq!(recognise(LOGIN), None, "the sign-in screen");
        assert_eq!(recognise(""), None);
        assert_eq!(recognise("❯ \n"), None);
        // A screen that only mentions the words, or has lost a part of the dialog.
        for broken in [
            TRUST.replace("Yes, I trust this folder", "Yes, I trust this folder (always)"),
            TRUST.replace("No, exit", "No, quit"),
            TRUST.replace("Enter to confirm", "Press a key"),
            TRUST.replace("Accessing workspace:", "Workspace:"),
            TRUST.replace("/tmp/shop", ""),
            TRUST.replace("❯ No, exit", "  No, exit"),
            TRUST.replace("  Yes, I trust this folder", "❯ Yes, I trust this folder"),
            "I said: Yes, I trust this folder, No, exit\nEnter to confirm".to_owned(),
            // The options in another order, or apart.
            TRUST.replace("❯ No, exit\n   Yes, I trust this folder", "   Yes, I trust this folder\n❯ No, exit"),
            TRUST.replace("❯ No, exit\n   Yes, I trust this folder", "❯ No, exit\n\n   Yes, I trust this folder"),
        ] {
            assert_eq!(recognise(&broken), None, "{broken}");
        }
    }
}
