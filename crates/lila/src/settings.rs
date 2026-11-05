//! The project settings the window edits: the project's title, and each
//! member's CLI, model, effort and prompt over its role's. Saved into
//! `.lila/project.toml` (its comments and layout kept) and
//! `.lila/prompts/<member>.md`; a value equal to the role's is left out, so
//! the member follows later changes to its role.

use crate::roles;
use anyhow::{Context, Result, anyhow, bail};
use lila_core::{ProjectConfig, Runtime, Team};
use std::path::Path;
use toml_edit::{DocumentMut, value};

#[derive(Debug, Clone, PartialEq)]
pub struct MemberSettings {
    /// The `[[members]]` entry (with `count` > 1 it stands for several members).
    pub id: String,
    pub count: u32,
    pub role: String,
    pub role_name: String,
    pub runtime: Runtime,
    pub model: String,
    /// `None`: the CLI's default.
    pub effort: Option<String>,
    pub prompt: String,
    /// The role's values, to compare with and go back to.
    pub role_runtime: Runtime,
    pub role_model: String,
    pub role_effort: Option<String>,
    pub role_prompt: String,
}

impl MemberSettings {
    pub fn prompt_is_own(&self) -> bool {
        self.prompt.trim() != self.role_prompt.trim()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectSettings {
    /// The project's id (fixed).
    pub name: String,
    /// What the window shows; empty means the id.
    pub title: String,
    pub members: Vec<MemberSettings>,
}

fn project_file(root: &Path) -> std::path::PathBuf {
    root.join(".lila").join("project.toml")
}

pub fn load(root: &Path, roles_dir: &Path) -> Result<ProjectSettings> {
    let path = project_file(root);
    let text = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let cfg = ProjectConfig::parse(&text).map_err(anyhow::Error::msg)?;
    let roles = roles::load(roles_dir)?;
    let mut members = Vec::new();
    for spec in &cfg.members {
        let role = roles.get(&spec.role).ok_or_else(|| anyhow!("member {}: unknown role {}", spec.id, spec.role))?;
        let own = spec.apply(role);
        let prompt = match &spec.prompt_file {
            Some(f) => std::fs::read_to_string(root.join(".lila").join(f)).with_context(|| format!("member {}: read prompt {f}", spec.id))?,
            None => role.prompt.clone(),
        };
        members.push(MemberSettings {
            id: spec.id.clone(),
            count: spec.count,
            role: role.id.clone(),
            role_name: role.file.name.clone(),
            runtime: own.file.runtime,
            model: own.file.model.clone(),
            effort: own.file.effort.clone(),
            prompt,
            role_runtime: role.file.runtime,
            role_model: role.file.model.clone(),
            role_effort: role.file.effort.clone(),
            role_prompt: role.prompt.clone(),
        });
    }
    Ok(ProjectSettings { name: cfg.name.clone(), title: cfg.title.clone().unwrap_or_default(), members })
}

/// Writes the settings. Checks the result builds a team before touching any file.
pub fn save(root: &Path, roles_dir: &Path, s: &ProjectSettings) -> Result<()> {
    let path = project_file(root);
    let text = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let mut doc: DocumentMut = text.parse().with_context(|| format!("parse {}", path.display()))?;

    let title = s.title.trim();
    if title.is_empty() || title == s.name {
        doc.remove("title");
    } else {
        doc["title"] = value(title);
    }

    let mut prompts: Vec<(String, Option<String>)> = Vec::new();
    let entries = doc.get_mut("members").and_then(|m| m.as_array_of_tables_mut()).ok_or_else(|| anyhow!("{}: no [[members]]", path.display()))?;
    for m in &s.members {
        let entry = entries.iter_mut().find(|t| t.get("id").and_then(|v| v.as_str()) == Some(m.id.as_str())).ok_or_else(|| anyhow!("member {} is no longer in project.toml", m.id))?;
        let model = m.model.trim();
        if model.is_empty() {
            bail!("{}: pick a model", m.id);
        }
        let mut set = |key: &str, v: Option<&str>| match v {
            Some(v) => {
                entry[key] = value(v);
            }
            None => {
                entry.remove(key);
            }
        };
        let runtime = m.runtime.to_string();
        set("runtime", (m.runtime != m.role_runtime).then_some(runtime.as_str()));
        set("model", (model != m.role_model).then_some(model));
        let effort = m.effort.as_deref().unwrap_or("");
        set("effort", (m.effort != m.role_effort).then_some(effort));
        let file = entry.get("prompt_file").and_then(|v| v.as_str()).map(String::from);
        if m.prompt_is_own() {
            let file = file.unwrap_or_else(|| format!("prompts/{}.md", m.id));
            entry["prompt_file"] = value(file.as_str());
            prompts.push((file, Some(m.prompt.clone())));
        } else if let Some(file) = file {
            entry.remove("prompt_file");
            prompts.push((file, None));
        }
    }

    let new_text = doc.to_string();
    let cfg = ProjectConfig::parse(&new_text).map_err(anyhow::Error::msg)?;
    if cfg.name != s.name {
        bail!("project.toml now names the project {:?}", cfg.name);
    }
    Team::build(&cfg, &roles::load(roles_dir)?).map_err(anyhow::Error::msg)?;

    for (file, prompt) in prompts {
        let p = root.join(".lila").join(&file);
        match prompt {
            Some(text) => {
                if let Some(dir) = p.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                std::fs::write(&p, text).with_context(|| format!("write {}", p.display()))?;
            }
            // Only the files this module names; anything else the person wrote stays.
            None if file.starts_with("prompts/") => {
                let _ = std::fs::remove_file(&p);
            }
            None => {}
        }
    }
    if new_text != text {
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, &new_text)?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let roles_dir = tmp.path().join("roles");
        roles::install_defaults(&roles_dir).unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(root.join(".lila")).unwrap();
        std::fs::write(project_file(&root), crate::init::default_project("shop")).unwrap();
        (tmp, root, roles_dir)
    }

    #[test]
    fn saves_only_what_differs_from_the_role_and_keeps_comments() {
        let (_tmp, root, roles_dir) = setup();
        let mut s = load(&root, &roles_dir).unwrap();
        assert_eq!(s.title, "");
        let eng = s.members.iter_mut().find(|m| m.id == "engineer").unwrap();
        assert_eq!((eng.model.as_str(), eng.prompt_is_own()), ("claude-sonnet-5-5", false));
        eng.runtime = Runtime::Codex;
        eng.model = "gpt-6.1-sol".into();
        eng.effort = Some("medium".into());
        eng.prompt = "Write small diffs.".into();
        s.title = "网店".into();
        save(&root, &roles_dir, &s).unwrap();

        let text = std::fs::read_to_string(project_file(&root)).unwrap();
        assert!(text.contains("# Who may hand work to whom"), "comments stay:\n{text}");
        assert!(text.contains("title = \"网店\""), "{text}");
        let cfg = ProjectConfig::parse(&text).unwrap();
        let e = cfg.members.iter().find(|m| m.id == "engineer").unwrap();
        assert_eq!((e.runtime, e.model.as_deref(), e.effort.as_deref(), e.prompt_file.as_deref()), (Some(Runtime::Codex), Some("gpt-6.1-sol"), Some("medium"), Some("prompts/engineer.md")));
        let pm = cfg.members.iter().find(|m| m.id == "pm").unwrap();
        assert_eq!((pm.runtime, &pm.model, &pm.effort, &pm.prompt_file), (None, &None, &None, &None), "untouched members follow their role");
        assert_eq!(std::fs::read_to_string(root.join(".lila/prompts/engineer.md")).unwrap(), "Write small diffs.");
        let team = crate::daemon::load_team(&root, &roles_dir).unwrap();
        assert_eq!(team.member("engineer").unwrap().role.prompt, "Write small diffs.");

        // Back to the role's values: the keys and the prompt file go away.
        let mut s = load(&root, &roles_dir).unwrap();
        assert_eq!(s.title, "网店");
        let eng = s.members.iter_mut().find(|m| m.id == "engineer").unwrap();
        assert!(eng.prompt_is_own());
        (eng.runtime, eng.model, eng.effort, eng.prompt) = (eng.role_runtime, eng.role_model.clone(), eng.role_effort.clone(), eng.role_prompt.clone());
        s.title = String::new();
        save(&root, &roles_dir, &s).unwrap();
        assert_eq!(std::fs::read_to_string(project_file(&root)).unwrap(), crate::init::default_project("shop"));
        assert!(!root.join(".lila/prompts/engineer.md").exists());
    }

    #[test]
    fn refuses_settings_that_do_not_build_a_team() {
        let (_tmp, root, roles_dir) = setup();
        let before = std::fs::read_to_string(project_file(&root)).unwrap();
        let mut s = load(&root, &roles_dir).unwrap();
        s.members[1].model = "  ".into();
        s.members[0].prompt = "changed".into();
        assert!(save(&root, &roles_dir, &s).unwrap_err().to_string().contains("pick a model"));
        assert_eq!(std::fs::read_to_string(project_file(&root)).unwrap(), before);
        assert!(!root.join(".lila/prompts").exists(), "nothing written");
    }
}
