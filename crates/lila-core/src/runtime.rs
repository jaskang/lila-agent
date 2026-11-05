//! How each official CLI is started with a role's model, effort and system prompt.
//! Verified in M0 (docs/spikes.md): claude `--append-system-prompt-file`,
//! codex `-c developer_instructions=<TOML string>`.

use crate::config::{Role, Runtime};

/// The program to run. `argv[0]` is a bare name; the caller resolves it on PATH.
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    pub argv: Vec<String>,
}

pub fn launch(role: &Role, prompt_path: &str, prompt_text: &str) -> Launch {
    let f = &role.file;
    let mut argv: Vec<String> = Vec::new();
    match f.runtime {
        Runtime::Claude => {
            argv.extend(["claude".into(), "--model".into(), f.model.clone()]);
            if let Some(e) = &f.effort {
                argv.extend(["--effort".into(), e.clone()]);
            }
            argv.extend(["--append-system-prompt-file".into(), prompt_path.into()]);
        }
        Runtime::Codex => {
            argv.extend(["codex".into(), "--no-daemon".into(), "-m".into(), f.model.clone()]);
            if let Some(e) = &f.effort {
                argv.extend(["-c".into(), format!("model_reasoning_effort={}", toml_string(e))]);
            }
            argv.extend(["-c".into(), format!("developer_instructions={}", toml_string(prompt_text))]);
        }
    }
    argv.extend(f.extra_args.iter().cloned());
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
            },
            prompt: String::new(),
        }
    }

    #[test]
    fn claude() {
        let l = launch(&role(Runtime::Claude, Some("high")), "/p.md", "ignored");
        assert_eq!(l.argv, ["claude", "--model", "the-model", "--effort", "high", "--append-system-prompt-file", "/p.md", "--foo"]);
    }

    #[test]
    fn codex_quotes_prompt_as_toml() {
        let l = launch(&role(Runtime::Codex, Some("high")), "/p.md", "Say \"hi\"\nline2 \\ end");
        assert_eq!(l.argv[..4], ["codex", "--no-daemon", "-m", "the-model"]);
        assert_eq!(l.argv[5], "model_reasoning_effort=\"high\"");
        let value = l.argv[7].strip_prefix("developer_instructions=").unwrap();
        let parsed: toml::Value = toml::from_str(&format!("v = {value}")).unwrap();
        assert_eq!(parsed["v"].as_str().unwrap(), "Say \"hi\"\nline2 \\ end");
        assert_eq!(l.argv.last().unwrap(), "--foo");
    }

    #[test]
    fn effort_is_optional() {
        assert!(!launch(&role(Runtime::Claude, None), "/p", "").argv.contains(&"--effort".to_string()));
    }
}
