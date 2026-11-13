//! Projects (`<repo>/.lila/project.toml`): a folder and the defaults a new thread
//! in it starts with. Everything else a project has (its threads) is in the database.

use serde::{Deserialize, Serialize};

use crate::thread::Tier;

/// What a new thread in a project starts with, unless it is told otherwise.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Defaults {
    /// A `provider/model` omp knows; empty: omp's own default.
    pub model: String,
    pub permission: Tier,
}

/// `.lila/project.toml`. Unknown keys (a v3 file still lists members, an older one an engine and an effort) are ignored.
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
        let mut table: toml::Table = toml::from_str(text).map_err(|e| format!("project.toml: {e}"))?;
        // A file from when projects chose an engine: its model was Claude's or Codex's, which omp would not know.
        if let Some(defaults) = table.get_mut("defaults").and_then(toml::Value::as_table_mut)
            && defaults.get("engine").and_then(toml::Value::as_str).is_some_and(|e| e != "omp")
        {
            defaults.remove("model");
        }
        let cfg: ProjectConfig = table.try_into().map_err(|e| format!("project.toml: {e}"))?;
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
    fn a_project_file_holds_defaults_and_ignores_what_older_versions_wrote() {
        let c = ProjectConfig::parse("name = \"shop\"\n[defaults]\nmodel = \"openai/gpt-6\"\neffort = \"low\"\npermission = \"confirm\"\n[[members]]\nid = \"pm\"\nrole = \"pm\"\n").unwrap();
        assert_eq!((c.defaults.model.as_str(), c.defaults.permission), ("openai/gpt-6", Tier::Confirm));
        let bare = ProjectConfig::parse("name = \"shop\"").unwrap();
        assert_eq!(bare.defaults, Defaults::default());
        assert_eq!(bare.defaults.permission, Tier::Full, "full auto unless the project says otherwise");
        assert_eq!(bare.defaults.model, "", "omp's own default");
        assert!(ProjectConfig::parse("name = \"Shop Name\"").is_err());
        assert_eq!(ProjectConfig::parse(&c.to_toml()).unwrap(), c, "what lilad writes it reads back");
        assert_eq!(ProjectConfig::parse(&bare.to_toml()).unwrap(), bare);
    }

    #[test]
    fn a_model_chosen_for_another_engine_is_dropped() {
        let old = ProjectConfig::parse("name = \"shop\"\n[defaults]\nengine = \"claude\"\nmodel = \"claude-opus-5-5\"\npermission = \"confirm\"\n").unwrap();
        assert_eq!((old.defaults.model.as_str(), old.defaults.permission), ("", Tier::Confirm));
        let omp = ProjectConfig::parse("name = \"shop\"\n[defaults]\nengine = \"omp\"\nmodel = \"openai/gpt-6\"\n").unwrap();
        assert_eq!(omp.defaults.model, "openai/gpt-6");
    }

    #[test]
    fn ids_are_short_and_plain() {
        assert!(validate_id("shop-2").is_ok());
        for bad in ["", "Shop", "2shop", "a b", "lilad", &"x".repeat(25)] {
            assert!(validate_id(bad).is_err(), "{bad:?}");
        }
    }
}
