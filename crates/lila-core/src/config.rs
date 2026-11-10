//! Projects (`<repo>/.lila/project.toml`): a folder and the defaults a new thread
//! in it starts with. Everything else a project has (its threads) is in the database.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::thread::Tier;

/// Which agent runs a thread: the official CLIs, or an ACP agent (omp, which
/// can reach any provider's model, is the one built in).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    Claude,
    Codex,
    /// An ACP agent, run by `lila host omp`.
    Omp,
}

impl fmt::Display for Runtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Runtime {
    pub fn as_str(self) -> &'static str {
        match self {
            Runtime::Claude => "claude",
            Runtime::Codex => "codex",
            Runtime::Omp => "omp",
        }
    }

    pub fn parse(s: &str) -> Option<Runtime> {
        match s.trim() {
            "claude" => Some(Runtime::Claude),
            "codex" => Some(Runtime::Codex),
            "omp" => Some(Runtime::Omp),
            _ => None,
        }
    }

    /// The agent runs through a protocol, hosted by `lila host` (not a TUI Lila reads from the side).
    pub fn hosted(self) -> bool {
        !matches!(self, Runtime::Claude)
    }

    /// The model a thread starts on when nobody chose one.
    pub fn default_model(self) -> &'static str {
        match self {
            Runtime::Claude => "claude-sonnet-5-5",
            Runtime::Codex => "gpt-6.1-sol",
            // omp takes any `provider/model` it knows: an empty model is its own default.
            Runtime::Omp => "",
        }
    }
}

/// Whose models a thread runs on: two with the same answer are one opinion. omp
/// runs any provider's model, so there it is the model itself.
pub fn model_source(runtime: Runtime, model: &str) -> String {
    match runtime {
        Runtime::Omp => format!("omp:{}", model.trim()),
        r => r.to_string(),
    }
}

/// What a new thread in a project starts with, unless it is told otherwise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Defaults {
    pub engine: Runtime,
    /// Empty: the engine's own default (`Runtime::default_model`).
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub permission: Tier,
}

impl Default for Defaults {
    fn default() -> Self {
        Defaults { engine: Runtime::Claude, model: String::new(), effort: None, permission: Tier::Full }
    }
}

impl Defaults {
    /// The model to start with: the chosen one, else the engine's default.
    pub fn model_or_default(&self) -> String {
        if self.model.trim().is_empty() { self.engine.default_model().to_owned() } else { self.model.clone() }
    }
}

/// `.lila/project.toml`. Unknown keys (a v3 file still lists members) are ignored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectConfig {
    /// The project's id: terminal names, the database, `lila` commands. Fixed once registered.
    pub name: String,
    /// What the window calls it; any text. Without it, `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub defaults: Defaults,
}

impl ProjectConfig {
    pub fn new(name: &str) -> ProjectConfig {
        ProjectConfig { name: name.to_owned(), title: None, defaults: Defaults::default() }
    }

    pub fn parse(text: &str) -> Result<ProjectConfig, String> {
        let cfg: ProjectConfig = toml::from_str(text).map_err(|e| format!("project.toml: {e}"))?;
        validate_id(&cfg.name).map_err(|e| format!("project name {:?}: {e}", cfg.name))?;
        Ok(cfg)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }
}

/// Names that go into terminal names, labels and file names.
pub fn validate_id(id: &str) -> Result<(), String> {
    let ok = !id.is_empty()
        && id.len() <= 24
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        && id.as_bytes()[0].is_ascii_lowercase();
    if !ok {
        return Err("use 1-24 of a-z 0-9 - _, starting with a letter".into());
    }
    if id == "human" || id == "lilad" {
        return Err(format!("{id:?} is reserved"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_file_holds_defaults_and_ignores_what_v3_wrote() {
        let c = ProjectConfig::parse("name = \"shop\"\n[defaults]\nengine = \"codex\"\nmodel = \"gpt-6.1-sol\"\neffort = \"low\"\npermission = \"confirm\"\n[[members]]\nid = \"pm\"\nrole = \"pm\"\n").unwrap();
        assert_eq!((c.defaults.engine, c.defaults.permission, c.defaults.effort.as_deref()), (Runtime::Codex, Tier::Confirm, Some("low")));
        let bare = ProjectConfig::parse("name = \"shop\"").unwrap();
        assert_eq!(bare.defaults, Defaults::default());
        assert_eq!(bare.defaults.permission, Tier::Full, "full auto unless the project says otherwise");
        assert_eq!(bare.defaults.model_or_default(), "claude-sonnet-5-5");
        assert!(ProjectConfig::parse("name = \"Shop Name\"").is_err());
        assert_eq!(ProjectConfig::parse(&c.to_toml()).unwrap(), c, "what lilad writes it reads back");
        assert_eq!(ProjectConfig::parse(&bare.to_toml()).unwrap(), bare);
    }

    #[test]
    fn ids_are_short_and_plain() {
        assert!(validate_id("shop-2").is_ok());
        for bad in ["", "Shop", "2shop", "a b", "lilad", &"x".repeat(25)] {
            assert!(validate_id(bad).is_err(), "{bad:?}");
        }
        assert_eq!(Runtime::parse("omp"), Some(Runtime::Omp));
        assert!(Runtime::Codex.hosted() && !Runtime::Claude.hosted());
    }
}
