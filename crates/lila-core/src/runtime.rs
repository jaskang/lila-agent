//! How each official CLI is started for a thread: its model, effort, permission
//! tier and the conversation to open. Claude Code runs as its TUI (Lila reads its
//! transcript and hooks); Codex and ACP agents run through `lila host`.

use crate::config::Runtime;
use crate::thread::Tier;

/// The program to run. `argv[0]` is a bare name; the caller resolves it on PATH.
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    pub argv: Vec<String>,
}

/// Which conversation the CLI opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Session {
    /// A new one; Claude Code takes the id Lila picked (Codex and ACP agents pick their own).
    New(Option<String>),
    /// An earlier one, by id.
    Resume(String),
}

/// What a thread is started with.
#[derive(Debug, Clone)]
pub struct Spec<'a> {
    pub engine: Runtime,
    pub model: &'a str,
    pub effort: Option<&'a str>,
    pub tier: Tier,
    pub session: &'a Session,
    /// A system prompt to append, when there is one.
    pub prompt_file: Option<&'a str>,
}

/// The `(approval policy, sandbox)` Codex runs a tier under (`codex app-server`: `thread/start`, then each `turn/start`).
///
/// - read-only: the sandbox allows no writes; whatever it escalates is refused by lilad.
/// - confirm: `untrusted` asks before anything but known-safe reads.
/// - full: the sandbox allows writes in the workspace but no network and nothing outside it,
///   and `on-request` lets Codex ask to go beyond: lilad answers those, and asks the person for the dangerous ones.
pub fn codex_settings(tier: Tier) -> (&'static str, &'static str) {
    match tier {
        Tier::ReadOnly => ("on-request", "read-only"),
        Tier::Confirm => ("untrusted", "workspace-write"),
        Tier::Full => ("on-request", "workspace-write"),
    }
}

/// Claude Code is always started in `manual` mode: everything it does not do on its own (edits,
/// commands) asks, and the asking reaches lilad's `PermissionRequest` hook, which answers by the
/// thread's tier. So a tier switch needs no restart, and the CLI's own default mode (the person's
/// `auto`) does not matter.
pub const CLAUDE_MODE: &str = "manual";

pub fn launch(spec: &Spec) -> Launch {
    let mut argv: Vec<String> = Vec::new();
    match spec.engine {
        Runtime::Claude => {
            argv.extend(["claude".into()]);
            if !spec.model.is_empty() {
                argv.extend(["--model".into(), spec.model.into()]);
            }
            match spec.session {
                Session::New(Some(id)) => argv.extend(["--session-id".into(), id.clone()]),
                Session::Resume(id) => argv.extend(["--resume".into(), id.clone()]),
                Session::New(None) => {}
            }
            if let Some(e) = spec.effort {
                argv.extend(["--effort".into(), e.into()]);
            }
            argv.extend(["--permission-mode".into(), CLAUDE_MODE.into()]);
            if let Some(f) = spec.prompt_file {
                argv.extend(["--append-system-prompt-file".into(), f.into()]);
            }
        }
        // Driven by `lila host codex` in the thread's terminal; lilad adds `--program <codex>`.
        Runtime::Codex => {
            let (approval, sandbox) = codex_settings(spec.tier);
            argv.extend(["lila".into(), "host".into(), "codex".into(), "--model".into(), spec.model.into()]);
            if let Some(e) = spec.effort {
                argv.extend(["--effort".into(), e.into()]);
            }
            argv.extend(["--sandbox".into(), sandbox.into(), "--approval".into(), approval.into()]);
            if let Some(f) = spec.prompt_file {
                argv.extend(["--prompt-file".into(), f.into()]);
            }
            if let Session::Resume(id) = spec.session {
                argv.extend(["--thread".into(), id.clone()]);
            }
        }
        Runtime::Omp => {
            argv.extend(["lila".into(), "host".into(), "omp".into()]);
            if !spec.model.is_empty() {
                argv.extend(["--model".into(), spec.model.into()]);
            }
            if let Some(f) = spec.prompt_file {
                argv.extend(["--prompt-file".into(), f.into()]);
            }
            if let Session::Resume(id) = spec.session {
                argv.extend(["--session".into(), id.clone()]);
            }
        }
    }
    Launch { argv }
}

/// The CLI a runtime needs on PATH.
pub fn cli(runtime: Runtime) -> &'static str {
    match runtime {
        Runtime::Claude => "claude",
        Runtime::Codex => "codex",
        Runtime::Omp => "omp",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(engine: Runtime, tier: Tier, session: &Session) -> Spec<'_> {
        Spec { engine, model: "the-model", effort: Some("low"), tier, session, prompt_file: None }
    }

    #[test]
    fn claude_always_asks_and_lilad_answers() {
        let new = Session::New(Some("abc".into()));
        for tier in [Tier::ReadOnly, Tier::Confirm, Tier::Full] {
            let l = launch(&spec(Runtime::Claude, tier, &new));
            assert_eq!(l.argv, ["claude", "--model", "the-model", "--session-id", "abc", "--effort", "low", "--permission-mode", "manual"]);
        }
        let l = launch(&spec(Runtime::Claude, Tier::Full, &Session::Resume("abc".into())));
        assert_eq!(l.argv[3..5], ["--resume", "abc"]);
    }

    #[test]
    fn codex_gets_a_sandbox_and_an_approval_policy_per_tier() {
        let new = Session::New(None);
        let argv = |t| launch(&spec(Runtime::Codex, t, &new)).argv;
        assert_eq!(argv(Tier::Full), ["lila", "host", "codex", "--model", "the-model", "--effort", "low", "--sandbox", "workspace-write", "--approval", "on-request"]);
        assert_eq!(argv(Tier::Confirm)[7..], ["--sandbox", "workspace-write", "--approval", "untrusted"]);
        assert_eq!(argv(Tier::ReadOnly)[7..], ["--sandbox", "read-only", "--approval", "on-request"]);
        let l = launch(&spec(Runtime::Codex, Tier::Full, &Session::Resume("th1".into())));
        assert!(l.argv.windows(2).any(|w| w == ["--thread", "th1"]), "{:?}", l.argv);
        assert!(!launch(&spec(Runtime::Codex, Tier::Full, &Session::New(Some("ignored".into())))).argv.contains(&"ignored".to_string()), "codex picks its own id");
    }

    #[test]
    fn acp_agents_run_through_the_host() {
        let l = launch(&Spec { prompt_file: Some("/p.md"), ..spec(Runtime::Omp, Tier::Full, &Session::Resume("s9".into())) });
        assert_eq!(l.argv, ["lila", "host", "omp", "--model", "the-model", "--prompt-file", "/p.md", "--session", "s9"]);
        assert_eq!(cli(Runtime::Omp), "omp");
    }
}
