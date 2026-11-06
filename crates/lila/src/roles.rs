//! Role files on disk, and the defaults shipped in `defaults/roles/`.

use anyhow::{Context, Result, anyhow, bail};
use lila_core::{ProjectConfig, Role, RoleFile, Runtime, validate_id};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, value};

pub const DEFAULTS: &[(&str, &str, &str)] = &[
    ("pm", include_str!("../../../defaults/roles/pm.toml"), include_str!("../../../defaults/roles/pm.md")),
    ("designer", include_str!("../../../defaults/roles/designer.toml"), include_str!("../../../defaults/roles/designer.md")),
    ("architect", include_str!("../../../defaults/roles/architect.toml"), include_str!("../../../defaults/roles/architect.md")),
    ("engineer", include_str!("../../../defaults/roles/engineer.toml"), include_str!("../../../defaults/roles/engineer.md")),
    ("tester", include_str!("../../../defaults/roles/tester.toml"), include_str!("../../../defaults/roles/tester.md")),
];

/// Files earlier Lilas shipped, by file name (oldest first). One still exactly
/// like one of these was never edited, so it is brought up to the current default.
const PREVIOUS: &[(&str, &str)] = &[
    ("pm.toml", include_str!("../../../defaults/roles/previous/pm.1.toml")),
    ("pm.md", include_str!("../../../defaults/roles/previous/pm.1.md")),
    ("engineer.toml", include_str!("../../../defaults/roles/previous/engineer.1.toml")),
    ("pm.toml", include_str!("../../../defaults/roles/previous/pm.2.toml")),
    ("pm.md", include_str!("../../../defaults/roles/previous/pm.2.md")),
    ("pm.toml", include_str!("../../../defaults/roles/previous/pm.3.toml")),
    ("tester.md", include_str!("../../../defaults/roles/previous/tester.1.md")),
    ("engineer.md", include_str!("../../../defaults/roles/previous/engineer.1.md")),
    ("tester.md", include_str!("../../../defaults/roles/previous/tester.2.md")),
];

/// The role every project's PM has: it can't be deleted.
pub const PM_ROLE: &str = "pm";

/// Writes the default roles that do not exist yet, and updates default files
/// nobody edited. Returns the ids of roles written.
pub fn install_defaults(dir: &Path) -> Result<Vec<String>> {
    std::fs::create_dir_all(dir)?;
    let mut written = Vec::new();
    for (id, toml_text, prompt) in DEFAULTS {
        let toml_path = dir.join(format!("{id}.toml"));
        if toml_path.exists() {
            for (file, now) in [(format!("{id}.toml"), *toml_text), (format!("{id}.md"), *prompt)] {
                let path = dir.join(&file);
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                if text != now && PREVIOUS.iter().any(|(f, old)| *f == file && *old == text) {
                    std::fs::write(&path, now)?;
                }
            }
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

/// What the role library edits; `extra_args` and `env` stay as written in the file.
#[derive(Debug, Clone, PartialEq)]
pub struct RoleEdit {
    pub id: String,
    pub name: String,
    pub runtime: Runtime,
    pub model: String,
    pub effort: Option<String>,
    pub can_assign: Vec<String>,
    pub prompt: String,
}

impl RoleEdit {
    pub fn of(role: &Role) -> RoleEdit {
        let f = &role.file;
        RoleEdit { id: role.id.clone(), name: f.name.clone(), runtime: f.runtime, model: f.model.clone(), effort: f.effort.clone(), can_assign: f.can_assign.clone(), prompt: role.prompt.clone() }
    }
}

fn toml_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.toml"))
}

/// Writes one role (a new one, or over its file keeping comments and other keys).
pub fn save(dir: &Path, r: &RoleEdit) -> Result<()> {
    validate_id(&r.id).map_err(|e| anyhow!("role {:?}: {e}", r.id))?;
    if r.name.trim().is_empty() {
        bail!("{}: give the role a name", r.id);
    }
    if r.model.trim().is_empty() {
        bail!("{}: pick a model", r.id);
    }
    std::fs::create_dir_all(dir)?;
    let path = toml_path(dir, &r.id);
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: DocumentMut = old.parse().with_context(|| format!("parse {}", path.display()))?;
    doc["name"] = value(r.name.trim());
    doc["runtime"] = value(r.runtime.to_string());
    doc["model"] = value(r.model.trim());
    match r.effort.as_deref().filter(|e| !e.is_empty()) {
        Some(e) => doc["effort"] = value(e),
        None => {
            doc.remove("effort");
        }
    }
    if r.can_assign.is_empty() {
        doc.remove("can_assign");
    } else {
        doc["can_assign"] = value(toml_edit::Array::from_iter(r.can_assign.iter().map(String::as_str)));
    }
    let text = doc.to_string();
    let file: RoleFile = toml::from_str(&text).with_context(|| format!("{}", path.display()))?;
    let prompt_path = dir.join(Role::prompt_file_name(&r.id, &file));
    std::fs::write(&prompt_path, &r.prompt).with_context(|| format!("write {}", prompt_path.display()))?;
    if text != old {
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, &text)?;
        std::fs::rename(&tmp, &path)?;
    }
    Ok(())
}

/// Removes a role's files. `users` (from [`users`]) must not name it.
pub fn delete(dir: &Path, id: &str, users: &BTreeMap<String, Vec<String>>) -> Result<()> {
    if id == PM_ROLE {
        bail!("every project has a PM; the {PM_ROLE} role stays");
    }
    if let Some(p) = users.get(id).filter(|p| !p.is_empty()) {
        bail!("{id} is used by {}; change those members' role first", p.join(", "));
    }
    let path = toml_path(dir, id);
    let text = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let file: RoleFile = toml::from_str(&text).with_context(|| format!("{}", path.display()))?;
    let _ = std::fs::remove_file(dir.join(Role::prompt_file_name(id, &file)));
    std::fs::remove_file(&path)?;
    Ok(())
}

/// Role id → the projects (by id) with a member of that role. Projects whose
/// config can't be read are skipped.
pub fn users(projects: &[(String, PathBuf)]) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (pid, cwd) in projects {
        let Ok(text) = std::fs::read_to_string(cwd.join(".lila").join("project.toml")) else { continue };
        let Ok(cfg) = ProjectConfig::parse(&text) else { continue };
        for m in cfg.members {
            let list = out.entry(m.role).or_default();
            if !list.contains(pid) {
                list.push(pid.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_parse_and_install_once() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(install_defaults(dir.path()).unwrap().len(), 5);
        std::fs::write(dir.path().join("pm.md"), "custom").unwrap();
        assert!(install_defaults(dir.path()).unwrap().is_empty());
        let roles = load(dir.path()).unwrap();
        assert_eq!(roles.len(), 5);
        assert_eq!(roles["pm"].prompt, "custom");
        assert_eq!(roles["tester"].file.model, "gpt-6.1-sol");
        assert_eq!(roles["engineer"].file.can_assign, ["tester"]);
    }

    #[test]
    fn untouched_old_defaults_are_updated_and_edits_kept() {
        let dir = tempfile::tempdir().unwrap();
        // The first defaults, as an early install has them.
        for (file, text) in &PREVIOUS[..3] {
            std::fs::write(dir.path().join(file), text).unwrap();
        }
        std::fs::write(dir.path().join("engineer.md"), "mine").unwrap();
        std::fs::write(dir.path().join("engineer.toml"), format!("{}# edited\n", PREVIOUS[2].1)).unwrap();
        install_defaults(dir.path()).unwrap();
        let roles = load(dir.path()).unwrap();
        assert_eq!(roles["pm"].file.name, "PM（项目经理）");
        assert!(roles["pm"].prompt.starts_with("You are the PM (project manager)"));
        // And the second.
        std::fs::write(dir.path().join("pm.toml"), PREVIOUS[3].1).unwrap();
        install_defaults(dir.path()).unwrap();
        assert_eq!(load(dir.path()).unwrap()["pm"].file.name, "PM（项目经理）");
        assert_eq!(roles["engineer"].file.name, "工程师", "an edited file stays");
        assert_eq!(roles["engineer"].prompt, "mine");
        assert!(roles.contains_key("designer"));
        assert!(delete(dir.path(), "pm", &BTreeMap::new()).is_err());
        // The Opus PM with no tool limits, and the tester before it checked the person's words.
        let last = |f: &str| PREVIOUS.iter().rev().find(|(n, _)| *n == f).unwrap().1;
        std::fs::write(dir.path().join("pm.toml"), last("pm.toml")).unwrap();
        std::fs::write(dir.path().join("tester.md"), last("tester.md")).unwrap();
        install_defaults(dir.path()).unwrap();
        let roles = load(dir.path()).unwrap();
        assert_eq!(roles["pm"].file.model, "claude-sonnet-5-5");
        assert!(roles["pm"].file.tools.allow.contains(&"Bash(lila:*)".to_string()));
        assert!(!roles["pm"].file.tools.only.contains(&"Edit".to_string()));
        assert!(roles["tester"].prompt.contains("Done when") && roles["tester"].prompt.contains("--verdict"));
        // Every earlier engineer and tester prompt is brought up to the flow-aware one.
        for f in ["engineer.md", "tester.md"] {
            for (_, old) in PREVIOUS.iter().filter(|(n, _)| *n == f) {
                std::fs::write(dir.path().join(f), old).unwrap();
                install_defaults(dir.path()).unwrap();
                assert!(std::fs::read_to_string(dir.path().join(f)).unwrap().contains("Lila"), "{f}");
            }
        }
        assert!(load(dir.path()).unwrap()["engineer"].prompt.contains("If your task says Lila sends it on"));
    }

    #[test]
    fn missing_prompt_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x.toml"), "name = \"x\"\nruntime = \"claude\"\nmodel = \"m\"\n").unwrap();
        assert!(load(dir.path()).unwrap_err().to_string().contains("read prompt"));
    }

    #[test]
    fn edits_keep_other_keys_and_comments() {
        let dir = tempfile::tempdir().unwrap();
        install_defaults(dir.path()).unwrap();
        let path = dir.path().join("engineer.toml");
        std::fs::write(&path, format!("# mine\n{}extra_args = [\"--verbose\"]\n", std::fs::read_to_string(&path).unwrap())).unwrap();
        let mut r = RoleEdit::of(&load(dir.path()).unwrap()["engineer"]);
        r.model = "claude-haiku-5-5".into();
        r.effort = None;
        r.can_assign = vec!["tester".into(), "architect".into()];
        r.prompt = "Small diffs.".into();
        save(dir.path(), &r).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# mine\n"), "{text}");
        assert!(text.contains("extra_args = [\"--verbose\"]") && !text.contains("effort"), "{text}");
        let e = &load(dir.path()).unwrap()["engineer"];
        assert_eq!((e.file.model.as_str(), e.prompt.as_str()), ("claude-haiku-5-5", "Small diffs."));
        assert_eq!(e.file.can_assign, ["tester", "architect"]);

        // A new role, then deleting it; a role in use stays.
        let mut qa = r.clone();
        qa.id = "qa".into();
        qa.name = "QA".into();
        save(dir.path(), &qa).unwrap();
        assert!(load(dir.path()).unwrap().contains_key("qa"));
        let users: BTreeMap<String, Vec<String>> = [("engineer".to_string(), vec!["shop".to_string()])].into();
        assert!(delete(dir.path(), "engineer", &users).unwrap_err().to_string().contains("used by shop"));
        delete(dir.path(), "qa", &users).unwrap();
        assert!(!dir.path().join("qa.md").exists() && !load(dir.path()).unwrap().contains_key("qa"));
        qa.id = "Bad".into();
        assert!(save(dir.path(), &qa).is_err());
    }

    #[test]
    fn users_lists_projects_per_role() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("shop");
        std::fs::create_dir_all(repo.join(".lila")).unwrap();
        std::fs::write(repo.join(".lila/project.toml"), crate::init::default_project("shop")).unwrap();
        let u = users(&[("shop".into(), repo), ("gone".into(), dir.path().join("nope"))]);
        assert_eq!(u["tester"], ["shop"]);
        assert!(!u.contains_key("qa"));
    }
}
