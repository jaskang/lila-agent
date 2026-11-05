//! `lila init`: a project file with the default team, and the default roles.

use crate::paths::Paths;
use crate::roles;
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Created {
    pub file: PathBuf,
    pub name: String,
    pub roles_installed: Vec<String>,
}

/// Writes `<dir>/.lila/project.toml` (name: `name`, else the folder's) and installs missing default roles.
pub fn init_project(paths: &Paths, dir: &Path, name: Option<String>) -> Result<Created> {
    let file = dir.join(".lila").join("project.toml");
    if file.exists() {
        bail!("{} already exists", file.display());
    }
    let name = match name {
        Some(n) => n,
        None => slug(&dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "project".into())),
    };
    lila_core::config::validate_id(&name).map_err(|e| anyhow::anyhow!("project name {name:?}: {e}; pass --name"))?;
    std::fs::create_dir_all(file.parent().unwrap())?;
    std::fs::write(&file, default_project(&name))?;
    let roles_installed = roles::install_defaults(&paths.roles())?;
    Ok(Created { file, name, roles_installed })
}

pub fn slug(s: &str) -> String {
    let mut out: String = s.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let out = out.trim_matches('-').to_string();
    let out = if out.starts_with(|c: char| c.is_ascii_lowercase()) { out } else { format!("p-{out}") };
    out.chars().take(24).collect::<String>().trim_end_matches('-').to_string()
}

pub fn default_project(name: &str) -> String {
    format!(
        r#"name = "{name}"
owner = "pm"

[[members]]
id = "pm"
role = "pm"

[[members]]
id = "designer"
role = "designer"

[[members]]
id = "architect"
role = "architect"

[[members]]
id = "engineer"
role = "engineer"

[[members]]
id = "tester"
role = "tester"

# Who may hand work to whom (the PM may assign to anyone).
# Without an entry, a member follows its role's `can_assign`.
# [assign]
# engineer = ["tester"]

# `count = 2` on a member runs two of it (engineer, engineer-2): more work at
# once, and more of your subscription used.

[settings]
# Minutes a member may sit idle on an open task before a reminder.
stall_minutes = 5
# Fix cycles two members may bounce one chain between them before the PM decides.
max_rounds = 3
# Start a member's CLI again before work from an unrelated chain, so its
# context stays small. Rework in the same chain keeps the session. Never the PM.
fresh_session = true
"#
    )
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("My Shop!"), "my-shop");
        assert_eq!(slug("lila-agent"), "lila-agent");
        assert_eq!(slug("2048"), "p-2048");
    }

    #[test]
    fn default_project_builds_with_default_roles() {
        let dir = tempfile::tempdir().unwrap();
        roles::install_defaults(dir.path()).unwrap();
        let roles = roles::load(dir.path()).unwrap();
        let cfg = lila_core::ProjectConfig::parse(&default_project("shop")).unwrap();
        let team = lila_core::Team::build(&cfg, &roles).unwrap();
        assert_eq!(team.members.len(), 5);
        assert_eq!(team.assign["designer"], ["engineer"]);
        assert_eq!(team.settings.max_working, 0, "no cap: each member's count decides");
        assert_eq!(team.assign["engineer"], ["tester"]);
        assert_eq!(team.assign["tester"], ["engineer"]);
    }

    #[test]
    fn init_writes_once() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths { config: home.path().join("c"), data: home.path().join("d"), state: home.path().join("s"), isolated: true };
        let repo = home.path().join("My Repo");
        std::fs::create_dir_all(&repo).unwrap();
        let c = init_project(&paths, &repo, None).unwrap();
        assert_eq!(c.name, "my-repo");
        assert_eq!(c.roles_installed.len(), 5);
        assert!(init_project(&paths, &repo, None).unwrap_err().to_string().contains("already exists"));
    }
}
