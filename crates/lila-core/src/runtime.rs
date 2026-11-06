//! How each official CLI is started with a role's model, effort and system prompt.
//! Verified in M0 (docs/spikes.md): claude `--append-system-prompt-file`,
//! codex `-c developer_instructions=<TOML string>`.

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

pub fn launch(role: &Role, prompt_path: &str, prompt_text: &str, session: &Session) -> Launch {
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
            argv.extend(["--append-system-prompt-file".into(), prompt_path.into()]);
        }
        Runtime::Codex => {
            argv.push("codex".into());
            if let Session::Resume(_) = session {
                argv.push("resume".into());
            }
            argv.extend(["--no-daemon".into(), "-m".into(), f.model.clone()]);
            if let Some(e) = &f.effort {
                argv.extend(["-c".into(), format!("model_reasoning_effort={}", toml_string(e))]);
            }
            if let Some(mode) = &f.tools.sandbox {
                argv.extend(["--sandbox".into(), mode.clone()]);
            }
            argv.extend(["-c".into(), format!("developer_instructions={}", toml_string(prompt_text))]);
        }
    }
    argv.extend(f.extra_args.iter().cloned());
    if let (Runtime::Codex, Session::Resume(id)) = (f.runtime, session) {
        argv.push(id.clone());
    }
    Launch { argv }
}

/// A TOML basic string literal: codex parses `-c key=value` values as TOML.
pub fn toml_string(s: &str) -> String {
    toml::Value::String(s.to_owned()).to_string()
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
        let l = launch(&role(Runtime::Claude, Some("high")), "/p.md", "ignored", &Session::New(None));
        assert_eq!(l.argv, ["claude", "--model", "the-model", "--effort", "high", "--append-system-prompt-file", "/p.md", "--foo"]);
    }

    #[test]
    fn codex_quotes_prompt_as_toml() {
        let l = launch(&role(Runtime::Codex, Some("high")), "/p.md", "Say \"hi\"\nline2 \\ end", &Session::New(None));
        assert_eq!(l.argv[..4], ["codex", "--no-daemon", "-m", "the-model"]);
        assert_eq!(l.argv[5], "model_reasoning_effort=\"high\"");
        let value = l.argv[7].strip_prefix("developer_instructions=").unwrap();
        let parsed: toml::Value = toml::from_str(&format!("v = {value}")).unwrap();
        assert_eq!(parsed["v"].as_str().unwrap(), "Say \"hi\"\nline2 \\ end");
        assert_eq!(l.argv.last().unwrap(), "--foo");
    }

    #[test]
    fn tools_become_flags_the_cli_enforces() {
        let mut r = role(Runtime::Claude, None);
        r.file.tools.only = vec!["Read".into(), "Bash".into()];
        r.file.tools.allow = vec!["Bash(lila:*)".into(), "Bash(git log:*)".into()];
        r.file.tools.deny = vec!["Edit".into()];
        r.file.tools.sandbox = Some("read-only".into());
        let l = launch(&r, "/p.md", "", &Session::New(None));
        assert_eq!(
            l.argv,
            [
                "claude", "--model", "the-model", "--tools", "Read,Bash", "--allowedTools", "Bash(lila:*)", "Bash(git log:*)",
                "--disallowedTools", "Edit", "--append-system-prompt-file", "/p.md", "--foo"
            ],
            "the sandbox is codex's"
        );
        r.file.runtime = Runtime::Codex;
        let l = launch(&r, "/p.md", "", &Session::New(None));
        assert_eq!(l.argv[4..6], ["--sandbox", "read-only"]);
        assert!(!l.argv.iter().any(|a| a == "--tools"), "claude's lists are not codex flags");
    }

    #[test]
    fn sessions_are_new_with_an_id_or_resumed() {
        let r = role(Runtime::Claude, None);
        let l = launch(&r, "/p", "", &Session::New(Some("abc".into())));
        assert_eq!(l.argv[3..5], ["--session-id", "abc"]);
        let l = launch(&r, "/p", "", &Session::Resume("abc".into()));
        assert_eq!(l.argv[3..5], ["--resume", "abc"]);
        let c = role(Runtime::Codex, None);
        let l = launch(&c, "/p", "x", &Session::New(Some("ignored".into())));
        assert!(!l.argv.contains(&"ignored".to_string()), "codex picks its own id");
        let l = launch(&c, "/p", "x", &Session::Resume("r1".into()));
        assert_eq!(l.argv[..3], ["codex", "resume", "--no-daemon"]);
        assert_eq!(l.argv.last().unwrap(), "r1", "the session id comes last");
    }

    #[test]
    fn effort_is_optional() {
        assert!(!launch(&role(Runtime::Claude, None), "/p", "", &Session::New(None)).argv.contains(&"--effort".to_string()));
    }
}
