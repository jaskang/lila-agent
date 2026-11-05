//! Roles (global, `~/.config/lila/roles/<id>.toml` + prompt) and projects
//! (`<repo>/.lila/project.toml`), and the team they expand into.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Which official CLI runs a role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    Claude,
    Codex,
}

impl fmt::Display for Runtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Runtime::Claude => "claude",
            Runtime::Codex => "codex",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleFile {
    pub name: String,
    pub runtime: Runtime,
    pub model: String,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// Set on top of the environment; `PATH` entries are prepended to the core's PATH.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Members of which roles this role may hand work to by default.
    #[serde(default)]
    pub can_assign: Vec<String>,
    /// Prompt file next to the role file. Defaults to `<id>.md`.
    #[serde(default)]
    pub prompt_file: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Role {
    pub id: String,
    pub file: RoleFile,
    pub prompt: String,
}

impl Role {
    pub fn parse(id: &str, toml_text: &str, prompt: String) -> Result<Role, String> {
        validate_id(id).map_err(|e| format!("role {id:?}: {e}"))?;
        let file: RoleFile = toml::from_str(toml_text).map_err(|e| format!("role {id}: {e}"))?;
        Ok(Role { id: id.to_owned(), file, prompt })
    }

    pub fn prompt_file_name(id: &str, file: &RoleFile) -> String {
        file.prompt_file.clone().unwrap_or_else(|| format!("{id}.md"))
    }
}

fn default_count() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberSpec {
    pub id: String,
    pub role: String,
    /// More than one expands into `<id>-1`, `<id>-2`, ...
    #[serde(default = "default_count")]
    pub count: u32,
    /// Set over the role's, for this member only (the project settings).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<Runtime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// This member's own prompt instead of the role's, relative to `.lila/`
    /// (`prompts/<id>.md`). The caller reads it: see `Team::set_prompt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_file: Option<String>,
}

impl MemberSpec {
    /// The member ids this entry expands into.
    pub fn ids(&self) -> Vec<String> {
        if self.count == 1 { vec![self.id.clone()] } else { (1..=self.count).map(|n| format!("{}-{n}", self.id)).collect() }
    }

    /// The role with this entry's overrides applied. Another runtime drops the
    /// role's `extra_args`, which are flags for its own CLI.
    pub fn apply(&self, role: &Role) -> Role {
        let mut role = role.clone();
        if let Some(r) = self.runtime
            && r != role.file.runtime
        {
            role.file.runtime = r;
            role.file.extra_args.clear();
        }
        if let Some(m) = &self.model {
            role.file.model = m.clone();
        }
        if let Some(e) = &self.effort {
            role.file.effort = Some(e.clone()).filter(|e| !e.is_empty());
        }
        role
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Settings {
    /// How many non-owner members may be working on a task at once (subscription limits).
    pub max_working: u32,
    /// A member that went quiet with an open task this long gets one reminder, then is reported stalled.
    pub stall_minutes: u64,
    /// A member that reports no status this long after starting needs a person (trust dialog, login).
    pub startup_timeout_secs: u64,
    /// Longest parent chain (T1 → T2 → ...) before new subtasks are refused.
    pub max_chain_depth: u32,
    /// How many times two members may send work back and forth in one chain
    /// (engineer → tester → engineer is one round) before the owner decides.
    pub max_rounds: u32,
    /// Start a member's CLI afresh before it gets work from another chain
    /// than its last task (keeps long-lived contexts small). Never the PM.
    pub fresh_session: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { max_working: 1, stall_minutes: 5, startup_timeout_secs: 20, max_chain_depth: 8, max_rounds: 3, fresh_session: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    /// The project's id: terminal names, the database, `lila` commands. Fixed once registered.
    pub name: String,
    /// What the window calls it; any text. Without it, `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The member who plans and talks to the person (the PM). Without it: `pm`, else `owner`.
    #[serde(default)]
    pub owner: Option<String>,
    pub members: Vec<MemberSpec>,
    /// Who may hand work to whom, by member id (overrides the role's `can_assign`).
    #[serde(default)]
    pub assign: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub settings: Settings,
}

impl ProjectConfig {
    pub fn parse(text: &str) -> Result<ProjectConfig, String> {
        toml::from_str(text).map_err(|e| format!("project.toml: {e}"))
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

#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    pub id: String,
    /// The role with the project's overrides for this member applied.
    pub role: Role,
    pub is_owner: bool,
}

impl Member {
    /// Changes when what this member is started with changes (CLI, model,
    /// effort, flags, its prompt), so a running member can be told it is on old settings.
    pub fn launch_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let f = &self.role.file;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (f.runtime.to_string(), &f.model, &f.effort, &f.extra_args, &f.env, &self.role.prompt).hash(&mut h);
        h.finish()
    }
}

/// A project's members, expanded and checked against the roles.
#[derive(Debug, Clone, PartialEq)]
pub struct Team {
    pub project: String,
    pub owner: String,
    pub members: Vec<Member>,
    /// Resolved assignment rules: member → members it may hand work to.
    pub assign: BTreeMap<String, Vec<String>>,
    pub settings: Settings,
}

impl Team {
    pub fn build(config: &ProjectConfig, roles: &BTreeMap<String, Role>) -> Result<Team, String> {
        validate_id(&config.name).map_err(|e| format!("project name {:?}: {e}", config.name))?;
        let mut members = Vec::new();
        for spec in &config.members {
            let role = roles.get(&spec.role).ok_or_else(|| {
                let known: Vec<&str> = roles.keys().map(String::as_str).collect();
                format!("member {:?} uses unknown role {:?} (known: {})", spec.id, spec.role, known.join(", "))
            })?;
            if spec.count == 0 {
                return Err(format!("member {:?}: count must be at least 1", spec.id));
            }
            if let Some(m) = &spec.model
                && m.trim().is_empty()
            {
                return Err(format!("member {:?}: model is empty", spec.id));
            }
            let role = spec.apply(role);
            for id in spec.ids() {
                validate_id(&id).map_err(|e| format!("member {id:?}: {e}"))?;
                if members.iter().any(|m: &Member| m.id == id) {
                    return Err(format!("member {id:?} is listed twice"));
                }
                members.push(Member { is_owner: false, id, role: role.clone() });
            }
        }
        let owner = match &config.owner {
            Some(o) => o.clone(),
            None if members.iter().any(|m| m.id == "pm") => "pm".into(),
            None => "owner".into(),
        };
        for m in &mut members {
            m.is_owner = m.id == owner;
        }
        if !members.iter().any(|m| m.is_owner) {
            return Err(format!("the PM {owner:?} is not one of the members; set `owner = \"<member id>\"` in project.toml"));
        }

        // Resolve assignment: explicit per-member rules win, else the role's
        // `can_assign` (role ids) expanded to every member with that role.
        let mut assign = BTreeMap::new();
        for m in &members {
            let targets: Vec<String> = match config.assign.get(&m.id) {
                Some(list) => {
                    for t in list {
                        if !members.iter().any(|x| &x.id == t) {
                            return Err(format!("assign.{}: unknown member {t:?}", m.id));
                        }
                    }
                    list.clone()
                }
                None => members
                    .iter()
                    .filter(|x| x.id != m.id && m.role.file.can_assign.contains(&x.role.id))
                    .map(|x| x.id.clone())
                    .collect(),
            };
            assign.insert(m.id.clone(), targets);
        }
        for key in config.assign.keys() {
            if !members.iter().any(|m| &m.id == key) {
                return Err(format!("assign.{key}: unknown member"));
            }
        }
        Ok(Team { project: config.name.clone(), owner, members, assign, settings: config.settings.clone() })
    }

    /// Gives the members of one `[[members]]` entry their own prompt.
    pub fn set_prompt(&mut self, spec: &MemberSpec, prompt: &str) {
        let ids = spec.ids();
        for m in self.members.iter_mut().filter(|m| ids.contains(&m.id)) {
            m.role.prompt = prompt.to_owned();
        }
    }

    pub fn member(&self, id: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.id == id)
    }

    pub fn owner(&self) -> &Member {
        self.members.iter().find(|m| m.is_owner).expect("team has an owner")
    }

    /// Who `from` may hand work to. The owner and the person may assign to anyone.
    pub fn assignable_from(&self, from: &crate::Actor) -> Vec<String> {
        match from {
            crate::Actor::Human => self.members.iter().map(|m| m.id.clone()).collect(),
            crate::Actor::Member(id) if *id == self.owner => self.members.iter().filter(|m| &m.id != id).map(|m| m.id.clone()).collect(),
            crate::Actor::Member(id) => self.assign.get(id).cloned().unwrap_or_default(),
        }
    }

    pub fn check_assign(&self, from: &crate::Actor, to: &str) -> Result<(), String> {
        if self.member(to).is_none() {
            return Err(format!("no member {to:?} in project {}", self.project));
        }
        if let crate::Actor::Member(id) = from {
            if self.member(id).is_none() {
                return Err(format!("you ({id}) are not a member of project {}", self.project));
            }
            if id == to {
                return Err("you cannot assign a task to yourself".into());
            }
        }
        let allowed = self.assignable_from(from);
        if allowed.iter().any(|a| a == to) {
            Ok(())
        } else if allowed.is_empty() {
            Err(format!("{from} may not assign tasks to anyone; report to your assigner with `lila task done` or `lila task block`"))
        } else {
            Err(format!("{from} may not assign to {to}; allowed: {}", allowed.join(", ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Actor;

    pub fn role(id: &str, can_assign: &[&str]) -> Role {
        Role {
            id: id.into(),
            file: RoleFile {
                name: id.into(),
                runtime: Runtime::Claude,
                model: "m".into(),
                effort: None,
                extra_args: vec![],
                env: BTreeMap::new(),
                can_assign: can_assign.iter().map(|s| s.to_string()).collect(),
                prompt_file: None,
            },
            prompt: format!("You are {id}."),
        }
    }

    fn roles() -> BTreeMap<String, Role> {
        [role("owner", &[]), role("engineer", &["tester"]), role("tester", &["engineer"]), role("architect", &[])]
            .into_iter()
            .map(|r| (r.id.clone(), r))
            .collect()
    }

    const PROJECT: &str = r#"
name = "shop"
[[members]]
id = "owner"
role = "owner"
[[members]]
id = "engineer"
role = "engineer"
count = 2
[[members]]
id = "tester"
role = "tester"
[[members]]
id = "architect"
role = "architect"
"#;

    #[test]
    fn expands_counts_and_resolves_role_rules() {
        let team = Team::build(&ProjectConfig::parse(PROJECT).unwrap(), &roles()).unwrap();
        let ids: Vec<&str> = team.members.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["owner", "engineer-1", "engineer-2", "tester", "architect"]);
        assert_eq!(team.assign["engineer-1"], ["tester"]);
        assert_eq!(team.assign["tester"], ["engineer-1", "engineer-2"]);
        assert!(team.assign["architect"].is_empty());
        assert!(team.owner().is_owner);
    }

    #[test]
    fn explicit_rules_override_roles() {
        let text = format!("{PROJECT}\n[assign]\narchitect = [\"engineer-1\"]\n");
        let team = Team::build(&ProjectConfig::parse(&text).unwrap(), &roles()).unwrap();
        assert_eq!(team.assign["architect"], ["engineer-1"]);
        assert!(team.check_assign(&Actor::Member("architect".into()), "engineer-1").is_ok());
        assert!(team.check_assign(&Actor::Member("architect".into()), "tester").is_err());
    }

    #[test]
    fn owner_and_human_assign_anyone_but_members_follow_rules() {
        let team = Team::build(&ProjectConfig::parse(PROJECT).unwrap(), &roles()).unwrap();
        assert!(team.check_assign(&Actor::Human, "owner").is_ok());
        assert!(team.check_assign(&Actor::Member("owner".into()), "architect").is_ok());
        assert!(team.check_assign(&Actor::Member("owner".into()), "owner").is_err());
        assert!(team.check_assign(&Actor::Member("engineer-1".into()), "tester").is_ok());
        let err = team.check_assign(&Actor::Member("engineer-1".into()), "architect").unwrap_err();
        assert!(err.contains("allowed: tester"), "{err}");
        assert!(team.check_assign(&Actor::Member("architect".into()), "tester").unwrap_err().contains("may not assign tasks to anyone"));
        assert!(team.check_assign(&Actor::Human, "nobody").is_err());
    }

    #[test]
    fn rejects_bad_configs() {
        let no_owner = PROJECT.replace("id = \"owner\"", "id = \"boss\"");
        assert!(Team::build(&ProjectConfig::parse(&no_owner).unwrap(), &roles()).unwrap_err().contains("owner"));
        let bad_role = PROJECT.replace("role = \"tester\"", "role = \"qa\"");
        assert!(Team::build(&ProjectConfig::parse(&bad_role).unwrap(), &roles()).unwrap_err().contains("unknown role"));
        let bad_assign = format!("{PROJECT}\n[assign]\nengineer-1 = [\"ghost\"]\n");
        assert!(Team::build(&ProjectConfig::parse(&bad_assign).unwrap(), &roles()).is_err());
        let reserved = PROJECT.replace("id = \"architect\"", "id = \"human\"");
        assert!(Team::build(&ProjectConfig::parse(&reserved).unwrap(), &roles()).unwrap_err().contains("reserved"));
        assert!(ProjectConfig::parse("name = \"x\"\nmembers = []\nbogus = 1").is_err());
    }

    #[test]
    fn the_pm_is_found_without_an_owner_key() {
        let pm = PROJECT.replace("id = \"owner\"\nrole = \"owner\"", "id = \"pm\"\nrole = \"owner\"");
        let team = Team::build(&ProjectConfig::parse(&pm).unwrap(), &roles()).unwrap();
        assert_eq!(team.owner, "pm");
        let old = Team::build(&ProjectConfig::parse(PROJECT).unwrap(), &roles()).unwrap();
        assert_eq!(old.owner, "owner", "projects from before the rename keep working");
        let named = format!("owner = \"tester\"\n{PROJECT}");
        assert_eq!(Team::build(&ProjectConfig::parse(&named).unwrap(), &roles()).unwrap().owner, "tester");
    }

    #[test]
    fn members_override_their_role() {
        let text = PROJECT.replace(
            "id = \"tester\"\nrole = \"tester\"",
            "id = \"tester\"\nrole = \"tester\"\nruntime = \"codex\"\nmodel = \"gpt-x\"\neffort = \"low\"\nprompt_file = \"prompts/tester.md\"",
        );
        let cfg = ProjectConfig::parse(&format!("title = \"商店\"\n{text}")).unwrap();
        assert_eq!(cfg.title.as_deref(), Some("商店"));
        let mut roles = roles();
        roles.get_mut("tester").unwrap().file.extra_args = vec!["--claude-only".into()];
        let mut team = Team::build(&cfg, &roles).unwrap();
        let before = team.member("tester").unwrap().launch_key();
        let t = &team.member("tester").unwrap().role;
        assert_eq!((t.file.runtime, t.file.model.as_str(), t.file.effort.as_deref()), (Runtime::Codex, "gpt-x", Some("low")));
        assert!(t.file.extra_args.is_empty(), "another CLI drops the role's flags");
        assert_eq!(team.member("engineer-1").unwrap().role.file.model, "m", "others keep the role's");
        let spec = cfg.members.iter().find(|m| m.id == "tester").unwrap();
        team.set_prompt(spec, "Test everything.");
        assert_eq!(team.member("tester").unwrap().role.prompt, "Test everything.");
        assert_ne!(team.member("tester").unwrap().launch_key(), before);
        let empty = text.replace("model = \"gpt-x\"", "model = \" \"");
        assert!(Team::build(&ProjectConfig::parse(&empty).unwrap(), &roles).unwrap_err().contains("model is empty"));
    }

    #[test]
    fn parses_role_file() {
        let r = Role::parse(
            "tester",
            "name = \"测试工程师\"\nruntime = \"codex\"\nmodel = \"gpt-6.1-sol\"\neffort = \"high\"\nenv = { PATH = \"/opt/node/bin\" }\n",
            "prompt".into(),
        )
        .unwrap();
        assert_eq!(r.file.runtime, Runtime::Codex);
        assert_eq!(r.file.env["PATH"], "/opt/node/bin");
        assert!(Role::parse("Bad", "", String::new()).is_err());
    }
}
