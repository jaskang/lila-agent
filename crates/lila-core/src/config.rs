//! Roles (global, `~/.config/lila/roles/<id>.toml` + prompt) and projects
//! (`<repo>/.lila/project.toml`), and the team they expand into.

use crate::flow::Flow;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Flows `flow::builtin` may give a project; naming one the team can't run is not an error.
fn builtin_names() -> [&'static str; 2] {
    ["code", "quick"]
}

/// Which agent runs a role: the official CLIs, or omp (any model it can reach).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    Claude,
    Codex,
    Omp,
}

impl fmt::Display for Runtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Runtime::Claude => "claude",
            Runtime::Codex => "codex",
            Runtime::Omp => "omp",
        })
    }
}

/// Whose models a member runs on: two members with the same answer are one
/// opinion. omp runs any provider's model, so there it is the model itself.
pub fn model_source(runtime: Runtime, model: &str) -> String {
    match runtime {
        Runtime::Omp => format!("omp:{}", model.trim()),
        r => r.to_string(),
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
    /// What its CLI lets it do, enforced by the CLI rather than asked for in the prompt.
    #[serde(default, skip_serializing_if = "Tools::is_empty")]
    pub tools: Tools,
}

/// A role's tool limits, rendered into CLI flags (`runtime::launch`). A rule in
/// the prompt is a suggestion; these are not: the PM that can't edit files
/// delegates instead of doing the work itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Tools {
    /// claude `--tools`: the only built-in tools it has. Empty: the CLI's default set.
    pub only: Vec<String>,
    /// claude `--allowedTools`: rules that run without asking, such as `Bash(lila:*)`.
    pub allow: Vec<String>,
    /// claude `--disallowedTools`.
    pub deny: Vec<String>,
    /// codex `--sandbox`: `read-only`, `workspace-write` or `danger-full-access`.
    /// `read-only` also stops `lila` from reaching lilad (the spool is a folder in the project).
    pub sandbox: Option<String>,
}

impl Tools {
    pub fn is_empty(&self) -> bool {
        self == &Tools::default()
    }
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
    /// How many of this member work at once: `<id>`, `<id>-2`, `<id>-3`, ...
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
        expand_ids(&self.id, self.count)
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

/// The member ids a `[[members]]` entry with `count` stands for. The first
/// keeps the entry's id, so raising the count leaves a running member alone.
pub fn expand_ids(id: &str, count: u32) -> Vec<String> {
    std::iter::once(id.to_owned()).chain((2..=count).map(|n| format!("{id}-{n}"))).collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Settings {
    /// At most this many non-owner members working at once; 0: no limit
    /// (each member's `count` says how many of it run). Not in the window.
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
    /// The flow work from the PM or the person runs through when its member does
    /// the flow's first step (`code`: implement → check → verify). `none`: no flow.
    pub flow: String,
    /// The command lilad runs for a flow's `check` step, in the project folder.
    /// Empty: the step is skipped.
    pub check: String,
    /// A check running longer than this fails.
    pub check_timeout_secs: u64,
    /// With `fresh_session`: a member whose context is below this many tokens
    /// keeps its session for work from another chain (0: always a new one).
    pub fresh_after_tokens: u64,
    /// The PM gets a new session, opened with a digest of the project, once its
    /// context reaches this many tokens and it has been idle a while (0: never).
    pub pm_refresh_tokens: u64,
    /// How long the PM must have been idle first: the person may be typing to it.
    pub pm_refresh_idle_minutes: u64,
    /// A member working this long without a `lila` call gets a reminder, then
    /// the person hears (0: never).
    pub runaway_minutes: u64,
    /// Results waiting for the person's review beyond which new work the PM
    /// starts waits too: more output than anyone can check (0: no limit).
    pub max_unreviewed: u32,
    /// Chains that may work at once, each in its own worktree of the project
    /// (outside it, reused chain after chain), when the project folder is the top
    /// of a git repository with a commit. 0: no worktrees; chains share the folder (`max_chains`).
    pub parallel: u32,
    /// Without worktrees: chains that may have work in the project folder at once,
    /// from the first task a member (not the PM) gets until the person signs the
    /// chain off. Members share the folder, so 1 keeps each chain's changes apart (0: no limit).
    pub max_chains: u32,
    /// When the person accepts a chain's result, lilad commits what changed in
    /// the project folder (a git repository) as one commit. With worktrees
    /// (`parallel`) it always does: that is how the work reaches the project folder.
    pub commit_on_accept: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            max_working: 0,
            stall_minutes: 5,
            startup_timeout_secs: 20,
            max_chain_depth: 8,
            max_rounds: 3,
            fresh_session: true,
            flow: "code".into(),
            check: String::new(),
            check_timeout_secs: 600,
            fresh_after_tokens: 60_000,
            pm_refresh_tokens: 120_000,
            pm_refresh_idle_minutes: 10,
            runaway_minutes: 30,
            max_unreviewed: 3,
            parallel: 2,
            max_chains: 1,
            commit_on_accept: true,
        }
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
    /// Flows besides (or replacing) the built-in `code` and `quick` (`flow::builtin`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub flows: BTreeMap<String, Flow>,
    /// Where requests come from besides the person (`[[sources]]`): GitHub issues.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<SourceSpec>,
}

/// One `[[sources]]` entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpec {
    /// `github`.
    pub kind: String,
    /// GitHub: the issues to take, in `gh` search syntax.
    #[serde(default = "SourceSpec::default_query")]
    pub query: String,
    /// `owner/name`; without it, the project folder's git remote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Minutes between looks.
    #[serde(default = "SourceSpec::default_every")]
    pub every: u64,
    /// New requests taken a day; the rest wait for the next day.
    #[serde(default = "SourceSpec::default_max_per_day")]
    pub max_per_day: u32,
    /// The PM starts on what it finds small and worth fixing without asking the person.
    #[serde(default)]
    pub auto_fix: bool,
}

impl SourceSpec {
    fn default_query() -> String {
        "is:open".into()
    }
    fn default_every() -> u64 {
        10
    }
    fn default_max_per_day() -> u32 {
        5
    }
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
        if !f.tools.is_empty() {
            f.tools.hash(&mut h);
        }
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
    /// `[[members]]` entries with `count > 1`: entry id → its members. A task
    /// sent to the entry id goes to one of them (`dispatch::pick_slot`).
    pub pools: BTreeMap<String, Vec<String>>,
    /// The built-in flows that fit the team, and the project's own.
    pub flows: BTreeMap<String, Flow>,
    pub sources: Vec<SourceSpec>,
}

impl Team {
    pub fn build(config: &ProjectConfig, roles: &BTreeMap<String, Role>) -> Result<Team, String> {
        validate_id(&config.name).map_err(|e| format!("project name {:?}: {e}", config.name))?;
        let mut members = Vec::new();
        let mut pools = BTreeMap::new();
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
            if spec.count > 1 {
                pools.insert(spec.id.clone(), spec.ids());
            }
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
        let is_target = |id: &str| members.iter().any(|m| m.id == id) || pools.contains_key(id);
        let mut flows = crate::flow::builtin(is_target);
        for (name, f) in &config.flows {
            validate_id(name).map_err(|e| format!("flows.{name}: {e}"))?;
            crate::flow::validate(name, f, is_target)?;
            flows.insert(name.clone(), f.clone());
        }
        let wanted = &config.settings.flow;
        // A built-in the team can't run (no engineer) just means no default flow.
        if !wanted.is_empty() && wanted != "none" && !flows.contains_key(wanted) && !builtin_names().contains(&wanted.as_str()) {
            return Err(format!("settings.flow: no flow {wanted:?} (have: {})", flows.keys().cloned().collect::<Vec<_>>().join(", ")));
        }
        for s in &config.sources {
            if s.kind != "github" {
                return Err(format!("source kind {:?}: Lila knows \"github\"", s.kind));
            }
            if s.every == 0 {
                return Err("a source's `every` must be at least 1 (minutes)".into());
            }
        }
        Ok(Team { project: config.name.clone(), owner, members, assign, settings: config.settings.clone(), pools, flows, sources: config.sources.clone() })
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

    /// `member` is `by` or one of the pool `by`.
    pub fn covers(&self, by: &str, member: &str) -> bool {
        by == member || self.pool(by).is_some_and(|p| p.iter().any(|m| m == member))
    }

    /// `by` (a member or a pool) runs on these models (`model_source`).
    pub fn source_of(&self, by: &str) -> Option<String> {
        let id = self.pool(by).and_then(|p| p.first().map(String::as_str)).unwrap_or(by);
        self.member(id).map(|m| model_source(m.role.file.runtime, &m.role.file.model))
    }

    /// Verdict steps whose member runs the same models as the one it judges (`flow::self_verified`).
    pub fn self_verified(&self) -> Vec<(String, String)> {
        crate::flow::self_verified(&self.flows, |by| self.source_of(by))
    }

    /// `name`'s work ends with a member's verdict: it needs something to be judged against.
    pub fn is_verified(&self, name: &str) -> bool {
        self.flows.get(name).is_some_and(|f| f.steps.iter().any(|s| s.wants_verdict()))
    }

    /// The flow a new task to `to` starts. `asked`: `--flow` (`none`: no flow).
    /// Without it, work from the PM or the person runs the project's default
    /// flow when `to` does that flow's first step; members' hand-offs run none.
    pub fn flow_for(&self, asked: Option<&str>, to: &str, from: &crate::Actor) -> Result<Option<String>, String> {
        let starts = |name: &str| self.flows.get(name).and_then(|f| f.steps[0].by.as_deref()).is_some_and(|by| self.covers(by, to));
        match asked {
            Some("none") => Ok(None),
            Some(name) if !self.flows.contains_key(name) => {
                Err(format!("no flow {name:?}; have: {}, or none", self.flows.keys().cloned().collect::<Vec<_>>().join(", ")))
            }
            Some(name) if !starts(name) => {
                Err(format!("flow {name} starts with `{}`, not {to}", self.flows[name].steps[0].by.as_deref().unwrap_or("?")))
            }
            Some(name) => Ok(Some(name.to_owned())),
            None => {
                let lead = matches!(from, crate::Actor::Human) || from.as_str() == self.owner;
                let name = self.settings.flow.as_str();
                Ok((lead && starts(name)).then(|| name.to_owned()))
            }
        }
    }

    /// The members `to` stands for when it names a pool (an entry with `count > 1`).
    pub fn pool(&self, to: &str) -> Option<&[String]> {
        self.pools.get(to).map(Vec::as_slice)
    }

    /// `targets` with every pool whose members are all in it written once, by its id.
    pub fn collapse_pools(&self, targets: &[String]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for t in targets {
            match self.pools.iter().find(|(_, slots)| slots.contains(t)) {
                Some((id, slots)) if slots.iter().all(|s| targets.contains(s)) => {
                    if !out.contains(id) {
                        out.push(id.clone());
                    }
                }
                _ => out.push(t.clone()),
            }
        }
        out
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
                tools: Tools::default(),
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
        assert_eq!(ids, ["owner", "engineer", "engineer-2", "tester", "architect"]);
        assert_eq!(team.assign["engineer"], ["tester"]);
        assert_eq!(team.assign["tester"], ["engineer", "engineer-2"]);
        assert!(team.assign["architect"].is_empty());
        assert!(team.owner().is_owner);
        assert_eq!(team.pool("engineer").unwrap(), ["engineer", "engineer-2"]);
        assert!(team.pool("tester").is_none(), "one member is not a pool");
        assert_eq!(team.collapse_pools(&team.assign["tester"]), ["engineer"]);
        assert_eq!(team.collapse_pools(&["engineer-2".into(), "tester".into()]), ["engineer-2", "tester"]);
    }

    #[test]
    fn explicit_rules_override_roles() {
        let text = format!("{PROJECT}\n[assign]\narchitect = [\"engineer\"]\n");
        let team = Team::build(&ProjectConfig::parse(&text).unwrap(), &roles()).unwrap();
        assert_eq!(team.assign["architect"], ["engineer"]);
        assert!(team.check_assign(&Actor::Member("architect".into()), "engineer").is_ok());
        assert!(team.check_assign(&Actor::Member("architect".into()), "tester").is_err());
    }

    #[test]
    fn owner_and_human_assign_anyone_but_members_follow_rules() {
        let team = Team::build(&ProjectConfig::parse(PROJECT).unwrap(), &roles()).unwrap();
        assert!(team.check_assign(&Actor::Human, "owner").is_ok());
        assert!(team.check_assign(&Actor::Member("owner".into()), "architect").is_ok());
        assert!(team.check_assign(&Actor::Member("owner".into()), "owner").is_err());
        assert!(team.check_assign(&Actor::Member("engineer".into()), "tester").is_ok());
        let err = team.check_assign(&Actor::Member("engineer".into()), "architect").unwrap_err();
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
        let bad_assign = format!("{PROJECT}\n[assign]\nengineer = [\"ghost\"]\n");
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
        assert_eq!(team.member("engineer").unwrap().role.file.model, "m", "others keep the role's");
        let spec = cfg.members.iter().find(|m| m.id == "tester").unwrap();
        team.set_prompt(spec, "Test everything.");
        assert_eq!(team.member("tester").unwrap().role.prompt, "Test everything.");
        assert_ne!(team.member("tester").unwrap().launch_key(), before);
        let empty = text.replace("model = \"gpt-x\"", "model = \" \"");
        assert!(Team::build(&ProjectConfig::parse(&empty).unwrap(), &roles).unwrap_err().contains("model is empty"));
    }

    #[test]
    fn flows_start_from_the_pm_or_the_person() {
        let team = Team::build(&ProjectConfig::parse(PROJECT).unwrap(), &roles()).unwrap();
        assert_eq!(team.flows.keys().collect::<Vec<_>>(), ["code", "quick"]);
        let pm = Actor::Member("owner".into());
        assert_eq!(team.flow_for(None, "engineer-2", &pm).unwrap().as_deref(), Some("code"), "a pool member does the first step");
        assert_eq!(team.flow_for(None, "engineer", &Actor::Human).unwrap().as_deref(), Some("code"));
        assert_eq!(team.flow_for(None, "architect", &pm).unwrap(), None, "not the first step's member");
        assert_eq!(team.flow_for(None, "engineer", &Actor::Member("tester".into())).unwrap(), None, "members' hand-offs");
        assert_eq!(team.flow_for(Some("none"), "engineer", &pm).unwrap(), None);
        assert_eq!(team.flow_for(Some("quick"), "engineer", &pm).unwrap().as_deref(), Some("quick"));
        assert!(team.flow_for(Some("quick"), "architect", &pm).unwrap_err().contains("starts with `engineer`"));
        assert!(team.flow_for(Some("nope"), "engineer", &pm).unwrap_err().contains("no flow"));

        let own = format!(
            "{PROJECT}\n[settings]\nflow = \"design\"\n[flows.design]\nsteps = [{{ do = \"design\", by = \"architect\" }}, {{ do = \"review\", by = \"tester\", on_fail = \"design\" }}]\n"
        );
        let team = Team::build(&ProjectConfig::parse(&own).unwrap(), &roles()).unwrap();
        assert_eq!(team.flow_for(None, "architect", &pm).unwrap().as_deref(), Some("design"));
        let bad = PROJECT.to_owned() + "\n[settings]\nflow = \"nope\"\n";
        assert!(Team::build(&ProjectConfig::parse(&bad).unwrap(), &roles()).unwrap_err().contains("settings.flow"));
        let broken = PROJECT.to_owned() + "\n[flows.x]\nsteps = [{ do = \"check\" }]\n";
        assert!(Team::build(&ProjectConfig::parse(&broken).unwrap(), &roles()).unwrap_err().contains("first step"));
        let no_engineer = PROJECT.replace("role = \"engineer\"\ncount = 2", "role = \"architect\"\ncount = 2").replace("id = \"engineer\"", "id = \"writer\"");
        let team = Team::build(&ProjectConfig::parse(&no_engineer).unwrap(), &roles()).unwrap();
        assert!(team.flows.is_empty(), "the default flow needs an engineer; without one there is none");
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
        assert!(r.file.tools.is_empty());
        assert!(Role::parse("Bad", "", String::new()).is_err());
        let pm = Role::parse(
            "pm",
            "name = \"PM\"\nruntime = \"claude\"\nmodel = \"m\"\n[tools]\nonly = [\"Read\", \"Bash\"]\nallow = [\"Bash(lila:*)\"]\n",
            String::new(),
        )
        .unwrap();
        assert_eq!(pm.file.tools.only, ["Read", "Bash"]);
        assert_eq!(pm.file.tools.allow, ["Bash(lila:*)"]);
        assert!(Role::parse("pm", "name = \"PM\"\nruntime = \"claude\"\nmodel = \"m\"\n[tools]\nbogus = 1\n", String::new()).is_err());
    }
}
