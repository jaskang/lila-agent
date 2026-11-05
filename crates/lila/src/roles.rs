//! Role files on disk, and the defaults shipped in `defaults/roles/`.

use anyhow::{Context, Result};
use lila_core::{Role, RoleFile};
use std::collections::BTreeMap;
use std::path::Path;

pub const DEFAULTS: &[(&str, &str, &str)] = &[
    ("owner", include_str!("../../../defaults/roles/owner.toml"), include_str!("../../../defaults/roles/owner.md")),
    ("architect", include_str!("../../../defaults/roles/architect.toml"), include_str!("../../../defaults/roles/architect.md")),
    ("engineer", include_str!("../../../defaults/roles/engineer.toml"), include_str!("../../../defaults/roles/engineer.md")),
    ("tester", include_str!("../../../defaults/roles/tester.toml"), include_str!("../../../defaults/roles/tester.md")),
];

/// Writes the default roles that do not exist yet. Returns the ids written.
pub fn install_defaults(dir: &Path) -> Result<Vec<String>> {
    std::fs::create_dir_all(dir)?;
    let mut written = Vec::new();
    for (id, toml_text, prompt) in DEFAULTS {
        let toml_path = dir.join(format!("{id}.toml"));
        if toml_path.exists() {
            continue;
        }
        std::fs::write(&toml_path, toml_text)?;
        let md = dir.join(format!("{id}.md"));
        if !md.exists() {
            std::fs::write(md, prompt)?;
        }
        written.push(id.to_string());
    }
    Ok(written)
}

pub fn load(dir: &Path) -> Result<BTreeMap<String, Role>> {
    let mut roles = BTreeMap::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(roles),
        Err(e) => return Err(e).with_context(|| format!("read {}", dir.display())),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let id = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_owned();
        let text = std::fs::read_to_string(&path)?;
        let file: RoleFile = toml::from_str(&text).with_context(|| format!("{}", path.display()))?;
        let prompt_path = dir.join(Role::prompt_file_name(&id, &file));
        let prompt = std::fs::read_to_string(&prompt_path).with_context(|| format!("role {id}: read prompt {}", prompt_path.display()))?;
        let role = Role::parse(&id, &text, prompt).map_err(anyhow::Error::msg)?;
        roles.insert(id, role);
    }
    Ok(roles)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_parse_and_install_once() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(install_defaults(dir.path()).unwrap().len(), 4);
        std::fs::write(dir.path().join("owner.md"), "custom").unwrap();
        assert!(install_defaults(dir.path()).unwrap().is_empty());
        let roles = load(dir.path()).unwrap();
        assert_eq!(roles.len(), 4);
        assert_eq!(roles["owner"].prompt, "custom");
        assert_eq!(roles["tester"].file.model, "gpt-6.1-sol");
        assert_eq!(roles["engineer"].file.can_assign, ["tester"]);
    }

    #[test]
    fn missing_prompt_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x.toml"), "name = \"x\"\nruntime = \"claude\"\nmodel = \"m\"\n").unwrap();
        assert!(load(dir.path()).unwrap_err().to_string().contains("read prompt"));
    }
}
