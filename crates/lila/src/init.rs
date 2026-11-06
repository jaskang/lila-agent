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

/// What the new-project wizard chose: the id, the name the window shows, and
/// the team as `(role, count)`, one entry per role (the PM's role among them).
#[derive(Debug, Clone)]
pub struct NewProject {
    pub name: String,
    pub title: String,
    pub team: Vec<(String, u32)>,
}

/// Like [`init_project`], with the wizard's team and title instead of the
/// default team. Members are named after their roles. Nothing is left behind
/// when the team doesn't build.
pub fn create_project(paths: &Paths, dir: &Path, p: &NewProject) -> Result<Created> {
    if !p.team.iter().any(|(r, _)| r == roles::PM_ROLE) {
        bail!("the team needs its PM");
    }
    let had_dir = dir.join(".lila").is_dir();
    let created = init_project(paths, dir, Some(p.name.clone()))?;
    let undo = || {
        let _ = std::fs::remove_file(&created.file);
        if !had_dir {
            let _ = std::fs::remove_dir(dir.join(".lila"));
        }
    };
    let r = (|| {
        let mut s = crate::settings::load(dir, &paths.roles())?;
        let wanted = |role: &str| p.team.iter().find(|(r, _)| r == role).map(|(_, n)| *n);
        let gone: Vec<String> = s.members.iter().filter(|m| m.role != roles::PM_ROLE && wanted(&m.role).is_none()).map(|m| m.id.clone()).collect();
        for id in gone {
            s.remove_member(&id)?;
        }
        for (role, _) in &p.team {
            if !s.members.iter().any(|m| &m.role == role) {
                s.add_member(role)?;
            }
        }
        // The wizard's order, PM first.
        s.members.sort_by_key(|m| p.team.iter().position(|(r, _)| *r == m.role));
        for ix in 0..s.members.len() {
            let n = wanted(&s.members[ix].role).unwrap_or(1);
            s.set_count(ix, n);
        }
        s.title = p.title.trim().to_owned();
        crate::settings::save(dir, &paths.roles(), &s)
    })();
    match r {
        Ok(()) => Ok(created),
        Err(e) => {
            undo();
            Err(e)
        }
    }
}

pub fn slug(s: &str) -> String {
    let mut out: String = s.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        return "project".into();
    }
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
# Work the PM gives the engineer runs through this flow: `code` (implement →
# check → verify by the tester), `quick` (no tester), or `none`.
flow = "code"
# The command Lila runs for the flow's check step, in this folder. A failure goes
# back to the engineer without waking the tester. Empty: no check step.
# check = "cargo test"
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
        assert_eq!(slug("记账本"), "project");
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

    fn home() -> (tempfile::TempDir, Paths, std::path::PathBuf) {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths { config: home.path().join("c"), data: home.path().join("d"), state: home.path().join("s"), isolated: true };
        let repo = home.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        (home, paths, repo)
    }

    #[test]
    fn wizard_team_and_title() {
        let (_h, paths, repo) = home();
        let team = vec![("pm".into(), 1), ("engineer".into(), 2), ("tester".into(), 1)];
        let p = NewProject { name: "shop".into(), title: "我的小店".into(), team };
        create_project(&paths, &repo, &p).unwrap();
        let s = crate::settings::load(&repo, &paths.roles()).unwrap();
        assert_eq!(s.title, "我的小店");
        assert_eq!(s.member_ids(), ["pm", "engineer", "engineer-2", "tester"]);
        let text = std::fs::read_to_string(repo.join(".lila/project.toml")).unwrap();
        assert!(text.contains("# Who may hand work to whom"), "comments kept: {text}");
        assert!(!text.contains("designer"), "{text}");
    }

    #[test]
    fn wizard_adds_a_role_outside_the_default_team() {
        let (_h, paths, repo) = home();
        roles::install_defaults(&paths.roles()).unwrap();
        let dir = paths.roles();
        std::fs::write(dir.join("writer.toml"), "name = \"Writer\"\nruntime = \"claude\"\nmodel = \"claude-haiku-5-5\"\n").unwrap();
        std::fs::write(dir.join("writer.md"), "Write docs.").unwrap();
        let p = NewProject { name: "docs".into(), title: String::new(), team: vec![("pm".into(), 1), ("writer".into(), 1)] };
        create_project(&paths, &repo, &p).unwrap();
        let s = crate::settings::load(&repo, &paths.roles()).unwrap();
        assert_eq!(s.member_ids(), ["pm", "writer"]);
        assert_eq!(s.title, "");
    }

    #[test]
    fn wizard_leaves_nothing_when_it_fails() {
        let (_h, paths, repo) = home();
        let p = NewProject { name: "x".into(), title: String::new(), team: vec![("pm".into(), 1), ("nobody".into(), 1)] };
        assert!(create_project(&paths, &repo, &p).is_err());
        assert!(!repo.join(".lila").exists());
        let p = NewProject { name: "x".into(), title: String::new(), team: vec![("engineer".into(), 1)] };
        assert!(create_project(&paths, &repo, &p).unwrap_err().to_string().contains("PM"));
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
