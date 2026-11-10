//! A project's `.lila/project.toml`: its id, title and the defaults a new thread starts with.
//! lilad reads and writes it; the person may edit it by hand and `project.reload`.

use anyhow::{Context, Result, bail};
use lila_core::{ProjectConfig, validate_id};
use std::path::{Path, PathBuf};

pub fn file(root: &Path) -> PathBuf {
    root.join(".lila").join("project.toml")
}

pub fn load(root: &Path) -> Result<ProjectConfig> {
    let path = file(root);
    let text = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    ProjectConfig::parse(&text).map_err(anyhow::Error::msg).with_context(|| path.display().to_string())
}

pub fn save(root: &Path, cfg: &ProjectConfig) -> Result<()> {
    let path = file(root);
    std::fs::create_dir_all(path.parent().expect("has a parent"))?;
    std::fs::write(&path, cfg.to_toml()).with_context(|| format!("write {}", path.display()))
}

/// A project id from a folder's name: lower case, `-` for anything else, a letter first, at most 24.
pub fn default_name(root: &Path) -> String {
    let raw = root.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut id: String = raw.chars().map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' { c } else { '-' }).collect();
    id = id.trim_matches('-').to_owned();
    if !id.chars().next().is_some_and(|c| c.is_ascii_lowercase()) {
        id.insert(0, 'p');
    }
    id.truncate(24);
    id.trim_end_matches('-').to_owned()
}

/// The project at `root`: its file as it is, or a new one with the default settings (and `name` or the
/// folder's name). Returns whether it was made.
pub fn ensure(root: &Path, name: Option<&str>, title: Option<&str>) -> Result<(ProjectConfig, bool)> {
    if !root.is_dir() {
        bail!("{} is not a folder", root.display());
    }
    if file(root).is_file() {
        let mut cfg = load(root)?;
        if let Some(n) = name.filter(|n| *n != cfg.name) {
            bail!("{} is already the project {:?}, not {n:?}", root.display(), cfg.name);
        }
        if let Some(t) = title.filter(|t| cfg.title.as_deref() != Some(*t)) {
            cfg.title = Some(t.to_owned());
            save(root, &cfg)?;
        }
        return Ok((cfg, false));
    }
    let name = name.map(String::from).unwrap_or_else(|| default_name(root));
    validate_id(&name).map_err(|e| anyhow::anyhow!("project name {name:?}: {e}"))?;
    let mut cfg = ProjectConfig::new(&name);
    cfg.title = title.map(String::from);
    save(root, &cfg)?;
    Ok((cfg, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_come_from_folders_and_files_are_made_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("My Shop 2024");
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(default_name(&root), "my-shop-2024");
        assert_eq!(default_name(Path::new("/x/2fast")), "p2fast");
        let (cfg, made) = ensure(&root, None, Some("My Shop")).unwrap();
        assert!(made && cfg.name == "my-shop-2024" && cfg.title.as_deref() == Some("My Shop"));
        let (again, made) = ensure(&root, None, None).unwrap();
        assert!(!made && again == cfg);
        assert!(ensure(&root, Some("other"), None).is_err(), "one name per folder");
        assert!(ensure(&dir.path().join("missing"), None, None).is_err());
        assert_eq!(load(&root).unwrap(), cfg);
    }
}
