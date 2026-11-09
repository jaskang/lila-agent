//! How each official CLI is started with a role's model, effort and system prompt.
//! Verified in M0 (docs/spikes.md): claude `--append-system-prompt-file`,
//! Codex goes through `lila host codex` (`codex app-server`, `developerInstructions`).

use crate::config::{Role, Runtime};

/// The program to run. `argv[0]` is a bare name; the caller resolves it on PATH.
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    pub argv: Vec<String>,
}

/// Which conversation the CLI opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Session {
    /// A new one; Claude Code takes the id Lila picked (Codex picks its own).
    New(Option<String>),
    /// An earlier one, by id: the member picks up where it left that work.
    Resume(String),
}

pub fn launch(role: &Role, prompt_path: &str, session: &Session) -> Launch {
    let f = &role.file;
    let mut argv: Vec<String> = Vec::new();
    match f.runtime {
        Runtime::Claude => {
            argv.extend(["claude".into(), "--model".into(), f.model.clone()]);
            match session {
                Session::New(Some(id)) => argv.extend(["--session-id".into(), id.clone()]),
                Session::Resume(id) => argv.extend(["--resume".into(), id.clone()]),
                Session::New(None) => {}
            }
            if let Some(e) = &f.effort {
                argv.extend(["--effort".into(), e.clone()]);
            }
            // Lists take several values; the next flag ends each one.
            let t = &f.tools;
            if !t.only.is_empty() {
                argv.extend(["--tools".into(), t.only.join(",")]);
            }
            for (flag, rules) in [("--allowedTools", &t.allow), ("--disallowedTools", &t.deny)] {
                if !rules.is_empty() {
                    argv.push(flag.into());
                    argv.extend(rules.iter().cloned());
                }
            }
            // Allow rules mean "everything else asks": in auto mode (a person's
            // default) nothing asks, and the PM could do the work itself.
            if !t.allow.is_empty() {
                argv.extend(["--permission-mode".into(), "manual".into()]);
            }
            argv.extend(["--append-system-prompt-file".into(), prompt_path.into()]);
        }
        // Codex runs through `codex app-server`, driven by `lila host codex` in the
        // member's terminal (architecture v3 §7.2); lilad adds `--program <codex>`.
        Runtime::Codex => {
            argv.extend(["lila".into(), "host".into(), "codex".into(), "--model".into(), f.model.clone()]);
            if let Some(e) = &f.effort {
                argv.extend(["--effort".into(), e.clone()]);
            }
            if let Some(mode) = &f.tools.sandbox {
                argv.extend(["--sandbox".into(), mode.clone()]);
            }
            argv.extend(["--prompt-file".into(), prompt_path.into()]);
            if let Session::Resume(id) = session {
                argv.extend(["--thread".into(), id.clone()]);
            }
            if !f.extra_args.is_empty() {
                argv.push("--".into());
                argv.extend(f.extra_args.iter().cloned());
            }
            return Launch { argv };
        }
        // omp over ACP (`omp acp`), driven by `lila host omp`; its model is any omp
        // knows (`provider/model`), its account is omp's own (~/.omp).
        Runtime::Omp => {
            argv.extend(["lila".into(), "host".into(), "omp".into(), "--model".into(), f.model.clone(), "--prompt-file".into(), prompt_path.into()]);
            if let Session::Resume(id) = session {
                argv.extend(["--session".into(), id.clone()]);
            }
            if !f.extra_args.is_empty() {
                argv.push("--".into());
                argv.extend(f.extra_args.iter().cloned());
            }
            return Launch { argv };
        }
    }
    argv.extend(f.extra_args.iter().cloned());
    Launch { argv }
}

/// The CLI a runtime needs on PATH.
pub fn cli(runtime: crate::config::Runtime) -> &'static str {
    match runtime {
        Runtime::Claude => "claude",
        Runtime::Codex => "codex",
        Runtime::Omp => "omp",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RoleFile;
    use std::collections::BTreeMap;

    fn role(runtime: Runtime, effort: Option<&str>) -> Role {
        Role {
            id: "x".into(),
            file: RoleFile {
                name: "x".into(),
                runtime,
                model: "the-model".into(),
                effort: effort.map(Into::into),
                extra_args: vec!["--foo".into()],
                env: BTreeMap::new(),
                can_assign: vec![],
                prompt_file: None,
                tools: Default::default(),
            },
            prompt: String::new(),
        }
    }

    #[test]
    fn claude() {
        let l = launch(&role(Runtime::Claude, Some("high")), "/p.md", &Session::New(None));
        assert_eq!(l.argv, ["claude", "--model", "the-model", "--effort", "high", "--append-system-prompt-file", "/p.md", "--foo"]);
    }

    #[test]
    fn codex_runs_through_the_host() {
        let l = launch(&role(Runtime::Codex, Some("high")), "/p.md", &Session::New(None));
        assert_eq!(l.argv, ["lila", "host", "codex", "--model", "the-model", "--effort", "high", "--prompt-file", "/p.md", "--", "--foo"]);
    }

    #[test]
    fn omp_runs_through_the_host() {
        let mut r = role(Runtime::Omp, None);
        r.file.model = "deepseek/deepseek-v4".into();
        let l = launch(&r, "/p.md", &Session::Resume("s9".into()));
        assert_eq!(l.argv, ["lila", "host", "omp", "--model", "deepseek/deepseek-v4", "--prompt-file", "/p.md", "--session", "s9", "--", "--foo"]);
        assert_eq!(cli(Runtime::Omp), "omp");
    }

    #[test]
    fn tools_become_flags_the_cli_enforces() {
        let mut r = role(Runtime::Claude, None);
        r.file.tools.only = vec!["Read".into(), "Bash".into()];
        r.file.tools.allow = vec!["Bash(lila:*)".into(), "Bash(git log:*)".into()];
        r.file.tools.deny = vec!["Edit".into()];
        r.file.tools.sandbox = Some("read-only".into());
        let l = launch(&r, "/p.md", &Session::New(None));
        assert_eq!(
            l.argv,
            [
                "claude", "--model", "the-model", "--tools", "Read,Bash", "--allowedTools", "Bash(lila:*)", "Bash(git log:*)",
                "--disallowedTools", "Edit", "--permission-mode", "manual", "--append-system-prompt-file", "/p.md", "--foo"
            ],
            "the sandbox is codex's"
        );
        r.file.runtime = Runtime::Codex;
        let l = launch(&r, "/p.md", &Session::New(None));
        assert_eq!(l.argv[5..7], ["--sandbox", "read-only"]);
        assert!(!l.argv.iter().any(|a| a == "--tools"), "claude's lists are not codex flags");
    }

    #[test]
    fn sessions_are_new_with_an_id_or_resumed() {
        let r = role(Runtime::Claude, None);
        let l = launch(&r, "/p", &Session::New(Some("abc".into())));
        assert_eq!(l.argv[3..5], ["--session-id", "abc"]);
        let l = launch(&r, "/p", &Session::Resume("abc".into()));
        assert_eq!(l.argv[3..5], ["--resume", "abc"]);
        let c = role(Runtime::Codex, None);
        let l = launch(&c, "/p", &Session::New(Some("ignored".into())));
        assert!(!l.argv.contains(&"ignored".to_string()), "codex picks its own id");
        let l = launch(&c, "/p", &Session::Resume("r1".into()));
        assert!(l.argv.windows(2).any(|w| w == ["--thread", "r1"]), "{:?}", l.argv);
    }

    #[test]
    fn effort_is_optional() {
        assert!(!launch(&role(Runtime::Claude, None), "/p", &Session::New(None)).argv.contains(&"--effort".to_string()));
    }
}
