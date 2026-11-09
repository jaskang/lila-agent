//! The project settings the window edits: the project's title, its members
//! (added, removed, and each one's CLI, model, effort and prompt over its
//! role's), who may hand work to whom, and the `[settings]`. Saved into
//! `.lila/project.toml` (its comments and layout kept) and
//! `.lila/prompts/<member>.md`; a value equal to the role's is left out, so
//! the member follows later changes to its role.

use crate::roles;
use anyhow::{Context, Result, anyhow, bail};
use lila_core::{ProjectConfig, Role, Runtime, Settings, Team, expand_ids, validate_id};
use std::collections::BTreeMap;
use std::path::Path;
use toml_edit::{DocumentMut, Item, Table, value};

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
    /// Roles of the members this one may hand work to unless `[assign]` says otherwise.
    pub role_can_assign: Vec<String>,
}

impl MemberSettings {
    /// A new `[[members]]` entry that follows its role in everything.
    pub fn from_role(id: &str, role: &Role) -> MemberSettings {
        let f = &role.file;
        MemberSettings {
            id: id.to_owned(),
            count: 1,
            role: role.id.clone(),
            role_name: f.name.clone(),
            runtime: f.runtime,
            model: f.model.clone(),
            effort: f.effort.clone(),
            prompt: role.prompt.clone(),
            role_runtime: f.runtime,
            role_model: f.model.clone(),
            role_effort: f.effort.clone(),
            role_prompt: role.prompt.clone(),
            role_can_assign: f.can_assign.clone(),
        }
    }

    pub fn prompt_is_own(&self) -> bool {
        self.prompt.trim() != self.role_prompt.trim()
    }

    /// The member ids this entry stands for (`count` > 1: `<id>`, `<id>-2`, ...).
    pub fn ids(&self) -> Vec<String> {
        expand_ids(&self.id, self.count.max(1))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectSettings {
    /// The project's id (fixed).
    pub name: String,
    /// What the window shows; empty means the id.
    pub title: String,
    /// The PM's member id: always in the team, may assign to anyone.
    pub owner: String,
    pub members: Vec<MemberSettings>,
    /// `[assign]` rows that differ from the roles' `can_assign` (member id → member ids).
    pub assign: BTreeMap<String, Vec<String>>,
    pub settings: Settings,
    /// Every role a member can be added with, by id.
    pub roles: Vec<Role>,
}

impl ProjectSettings {
    /// Every member id, in order.
    pub fn member_ids(&self) -> Vec<String> {
        self.members.iter().flat_map(MemberSettings::ids).collect()
    }

    fn entry_of(&self, mid: &str) -> Option<&MemberSettings> {
        self.members.iter().find(|m| m.ids().iter().any(|i| i == mid))
    }

    /// Who `mid` may hand work to by its role alone.
    pub fn role_targets(&self, mid: &str) -> Vec<String> {
        if mid == self.owner {
            return self.member_ids().into_iter().filter(|i| i != mid).collect();
        }
        let Some(me) = self.entry_of(mid) else { return Vec::new() };
        self.members.iter().filter(|m| me.role_can_assign.contains(&m.role)).flat_map(MemberSettings::ids).filter(|i| i != mid).collect()
    }

    /// Who `mid` may hand work to: its `[assign]` row, else its role's.
    pub fn targets(&self, mid: &str) -> Vec<String> {
        if mid == self.owner {
            return self.role_targets(mid);
        }
        let ids = self.member_ids();
        match self.assign.get(mid) {
            Some(row) => ids.into_iter().filter(|i| row.contains(i)).collect(),
            None => self.role_targets(mid),
        }
    }

    /// Allows or forbids `from → to`. A row that ends up as the role's is dropped.
    pub fn set_target(&mut self, from: &str, to: &str, allowed: bool) {
        if from == self.owner || from == to {
            return;
        }
        let mut row = self.targets(from);
        row.retain(|t| t != to);
        if allowed {
            row.push(to.to_owned());
        }
        let ids = self.member_ids();
        row.sort_by_key(|t| ids.iter().position(|i| i == t));
        if row == self.role_targets(from) {
            self.assign.remove(from);
        } else {
            self.assign.insert(from.to_owned(), row);
        }
    }

    /// `from`'s row goes back to its role's.
    pub fn reset_targets(&mut self, from: &str) {
        self.assign.remove(from);
    }

    /// The PM's role: a project has exactly one member of it.
    pub fn owner_role(&self) -> Option<&str> {
        self.entry_of(&self.owner).map(|m| m.role.as_str())
    }

    /// Adds a member with `role`; its id is the role's, or `<role>2`, `<role>3`, ... if taken.
    pub fn add_member(&mut self, role: &str) -> Result<String> {
        if self.owner_role() == Some(role) {
            bail!("a project has one PM; it is already {}", self.owner);
        }
        let r = self.roles.iter().find(|r| r.id == role).ok_or_else(|| anyhow!("no role {role:?}"))?.clone();
        let ids = self.member_ids();
        let taken = |id: &str| ids.iter().any(|i| i == id) || self.members.iter().any(|m| m.id == id);
        let id = std::iter::once(role.to_owned()).chain((2..).map(|n| format!("{role}{n}"))).find(|id| !taken(id) && validate_id(id).is_ok()).expect("a free id");
        self.members.push(MemberSettings::from_role(&id, &r));
        Ok(id)
    }

    /// Renames a member not saved yet (or any entry; the database keeps old ids' history).
    pub fn rename_member(&mut self, ix: usize, id: &str) {
        let Some(m) = self.members.get_mut(ix) else { return };
        let old = m.ids();
        m.id = id.to_owned();
        let new = m.ids();
        let rename = |s: &str| old.iter().position(|o| o == s).map(|p| new[p].clone()).unwrap_or_else(|| s.to_owned());
        self.assign = std::mem::take(&mut self.assign).into_iter().map(|(k, v)| (rename(&k), v.iter().map(|t| rename(t)).collect())).collect();
    }

    /// How many of entry `ix` work at once (the PM's stays 1). Rules naming
    /// members that go away go with them.
    pub fn set_count(&mut self, ix: usize, count: u32) {
        let Some(m) = self.members.get(ix) else { return };
        if m.ids().contains(&self.owner) {
            return;
        }
        let before = m.ids();
        let rows: Vec<(String, Vec<String>)> = self.assign.keys().map(|k| (k.clone(), self.targets(k))).collect();
        self.members[ix].count = count.clamp(1, 9);
        let after = self.members[ix].ids();
        let gone: Vec<&String> = before.iter().filter(|i| !after.contains(i)).collect();
        self.assign.clear();
        for (k, row) in rows {
            if gone.contains(&&k) {
                continue;
            }
            let row: Vec<String> = row.into_iter().filter(|t| !gone.contains(&t)).collect();
            if row != self.role_targets(&k) {
                self.assign.insert(k, row);
            }
        }
    }

    /// Removes a `[[members]]` entry (not the PM's) and every rule naming it.
    pub fn remove_member(&mut self, id: &str) -> Result<()> {
        let ix = self.members.iter().position(|m| m.id == id).ok_or_else(|| anyhow!("no member {id:?}"))?;
        let gone = self.members[ix].ids();
        if gone.contains(&self.owner) {
            bail!("{id} is the PM; the team needs it");
        }
        // Rows of the others as they are now, minus the removed ones.
        let rows: Vec<(String, Vec<String>)> = self.assign.keys().filter(|k| !gone.contains(k)).map(|k| (k.clone(), self.targets(k).into_iter().filter(|t| !gone.contains(t)).collect())).collect();
        self.members.remove(ix);
        self.assign.clear();
        for (k, row) in rows {
            if row != self.role_targets(&k) {
                self.assign.insert(k, row);
            }
        }
        Ok(())
    }
}

fn project_file(root: &Path) -> std::path::PathBuf {
    root.join(".lila").join("project.toml")
}

pub fn load(root: &Path, roles_dir: &Path) -> Result<ProjectSettings> {
    let path = project_file(root);
    let text = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let cfg = ProjectConfig::parse(&text).map_err(anyhow::Error::msg)?;
    let roles = roles::load(roles_dir)?;
    let team = Team::build(&cfg, &roles).map_err(anyhow::Error::msg)?;
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
            role_can_assign: role.file.can_assign.clone(),
        });
    }
    let mut s = ProjectSettings {
        name: cfg.name.clone(),
        title: cfg.title.clone().unwrap_or_default(),
        owner: team.owner.clone(),
        members,
        assign: BTreeMap::new(),
        settings: cfg.settings.clone(),
        roles: roles.into_values().collect(),
    };
    // Keep only rows that say something the roles don't.
    for (k, row) in &cfg.assign {
        let row: Vec<String> = s.member_ids().into_iter().filter(|i| row.contains(i)).collect();
        if row != s.role_targets(k) {
            s.assign.insert(k.clone(), row);
        }
    }
    Ok(s)
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
        validate_id(&m.id).map_err(|e| anyhow!("member {:?}: {e}", m.id))?;
    }
    // Entries no longer listed go; their prompt files with them.
    let keep: Vec<&str> = s.members.iter().map(|m| m.id.as_str()).collect();
    for ix in (0..entries.len()).rev() {
        let t = entries.get(ix).expect("in range");
        if !t.get("id").and_then(|v| v.as_str()).is_some_and(|id| keep.contains(&id)) {
            if let Some(f) = t.get("prompt_file").and_then(|v| v.as_str()) {
                prompts.push((f.to_owned(), None));
            }
            entries.remove(ix);
        }
    }
    for m in &s.members {
        if !entries.iter().any(|t| t.get("id").and_then(|v| v.as_str()) == Some(m.id.as_str())) {
            let mut t = Table::new();
            t["id"] = value(m.id.as_str());
            t["role"] = value(m.role.as_str());
            entries.push(t);
        }
    }
    for m in &s.members {
        let entry = entries.iter_mut().find(|t| t.get("id").and_then(|v| v.as_str()) == Some(m.id.as_str())).expect("just added");
        if m.count > 1 {
            entry["count"] = value(i64::from(m.count));
        } else {
            entry.remove("count");
        }
    }
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

    // [assign]: the rows that differ from the roles, for members that exist.
    let ids = s.member_ids();
    let rows: Vec<(String, Vec<String>)> = ids.iter().filter(|k| s.assign.contains_key(*k) && **k != s.owner).map(|k| (k.clone(), s.targets(k))).filter(|(k, row)| *row != s.role_targets(k)).collect();
    if rows.is_empty() {
        doc.remove("assign");
    } else {
        if !doc.get("assign").is_some_and(Item::is_table) {
            doc["assign"] = Item::Table(Table::new());
        }
        let table = doc["assign"].as_table_mut().expect("a table");
        table.retain(|k, _| rows.iter().any(|(r, _)| r == k));
        for (k, row) in rows {
            table[k.as_str()] = value(toml_edit::Array::from_iter(row.iter().map(String::as_str)));
        }
    }

    // [settings]: keys already written are kept up to date; others only when not the default.
    let defaults = Settings::default();
    let numbers: [(&str, i64, i64); 5] = [
        ("parallel", s.settings.parallel.into(), defaults.parallel.into()),
        ("stall_minutes", s.settings.stall_minutes as i64, defaults.stall_minutes as i64),
        ("startup_timeout_secs", s.settings.startup_timeout_secs as i64, defaults.startup_timeout_secs as i64),
        ("max_chain_depth", s.settings.max_chain_depth.into(), defaults.max_chain_depth.into()),
        ("max_rounds", s.settings.max_rounds.into(), defaults.max_rounds.into()),
    ];
    // How many work at once is each member's `count` now; an old cap would hide that.
    if let Some(t) = doc.get_mut("settings").and_then(Item::as_table_mut) {
        t.remove("max_working");
    }
    let has = |doc: &DocumentMut, k: &str| doc.get("settings").and_then(|t| t.get(k)).is_some();
    let put = |doc: &mut DocumentMut, k: &str, v: Item, differs: bool| {
        if has(doc, k) || differs {
            if !doc.get("settings").is_some_and(Item::is_table) {
                doc["settings"] = Item::Table(Table::new());
            }
            let t = doc["settings"].as_table_mut().expect("a table");
            // Keep the value's comments: only its value changes.
            match t.get_mut(k).and_then(Item::as_value_mut) {
                Some(old) => {
                    let decor = old.decor().clone();
                    *old = v.into_value().expect("a value");
                    *old.decor_mut() = decor;
                }
                None => t[k] = v,
            }
        }
    };
    for (k, v, d) in numbers {
        if v < 0 {
            bail!("settings.{k} cannot be negative");
        }
        put(&mut doc, k, value(v), v != d);
    }
    put(&mut doc, "fresh_session", value(s.settings.fresh_session), s.settings.fresh_session != defaults.fresh_session);
    let check = s.settings.check.trim();
    put(&mut doc, "check", value(check), check != defaults.check);

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
        std::fs::write(project_file(&root), crate::init::default_project("shop", None)).unwrap();
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
        assert_eq!(std::fs::read_to_string(project_file(&root)).unwrap(), crate::init::default_project("shop", None));
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

#[cfg(test)]
mod team_tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let roles_dir = tmp.path().join("roles");
        roles::install_defaults(&roles_dir).unwrap();
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(root.join(".lila")).unwrap();
        std::fs::write(project_file(&root), crate::init::default_project("shop", None)).unwrap();
        (tmp, root, roles_dir)
    }

    fn text(root: &Path) -> String {
        std::fs::read_to_string(project_file(root)).unwrap()
    }

    #[test]
    fn members_come_and_go() {
        let (_tmp, root, roles_dir) = setup();
        let mut s = load(&root, &roles_dir).unwrap();
        assert_eq!(s.owner, "pm");
        assert!(s.remove_member("pm").is_err(), "the PM stays");
        assert_eq!(s.add_member("engineer").unwrap(), "engineer2");
        s.members.last_mut().unwrap().model = "claude-haiku-5-5".into();
        s.remove_member("architect").unwrap();
        save(&root, &roles_dir, &s).unwrap();

        let t = text(&root);
        assert!(t.contains("# Who may hand work to whom"), "comments stay:\n{t}");
        let cfg = ProjectConfig::parse(&t).unwrap();
        let ids: Vec<&str> = cfg.members.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["pm", "designer", "engineer", "tester", "engineer2"], "{t}");
        assert_eq!(cfg.members[4].model.as_deref(), Some("claude-haiku-5-5"));
        assert!(s.add_member("pm").is_err(), "one PM per project");
        let team = crate::daemon::load_team(&root, &roles_dir).unwrap();
        assert_eq!(team.assign["tester"], ["engineer", "engineer2"], "the new engineer follows its role's rules");
    }

    #[test]
    fn assignment_rows_only_when_they_differ() {
        let (_tmp, root, roles_dir) = setup();
        let mut s = load(&root, &roles_dir).unwrap();
        let before = text(&root);
        let engineer = s.targets("engineer");
        assert!(engineer.contains(&"tester".to_string()), "{engineer:?}");
        assert_eq!(s.targets("pm"), ["designer", "architect", "engineer", "tester"]);
        // Off and on again is the role's row: nothing written.
        s.set_target("engineer", "tester", false);
        s.set_target("engineer", "tester", true);
        assert!(s.assign.is_empty());
        save(&root, &roles_dir, &s).unwrap();
        assert_eq!(text(&root), before);

        s.set_target("tester", "architect", true);
        s.set_target("pm", "tester", false);
        assert!(!s.assign.contains_key("pm"), "the PM may always assign to anyone");
        save(&root, &roles_dir, &s).unwrap();
        let team = crate::daemon::load_team(&root, &roles_dir).unwrap();
        assert!(team.assign["tester"].contains(&"architect".to_string()));
        let mut s = load(&root, &roles_dir).unwrap();
        assert!(s.targets("tester").contains(&"architect".to_string()));

        // Removing the architect drops it from the rows; the row is the role's again.
        s.remove_member("architect").unwrap();
        assert!(s.assign.is_empty(), "{:?}", s.assign);
        save(&root, &roles_dir, &s).unwrap();
        assert!(!text(&root).contains("\n[assign]"), "{}", text(&root));
    }

    #[test]
    fn settings_keep_their_comments() {
        let (_tmp, root, roles_dir) = setup();
        let mut s = load(&root, &roles_dir).unwrap();
        s.settings.stall_minutes = 9;
        s.settings.fresh_session = false;
        s.settings.max_chain_depth = 5;
        s.settings.check = " cargo test ".into();
        save(&root, &roles_dir, &s).unwrap();
        let t = text(&root);
        assert!(t.contains("# Minutes a member may sit idle on an open task before a reminder.\nstall_minutes = 9\n"), "{t}");
        assert!(t.contains("fresh_session = false"), "{t}");
        assert!(t.contains("max_chain_depth = 5"), "{t}");
        assert!(!t.contains("startup_timeout_secs"), "defaults not written: {t}");
        let cfg = ProjectConfig::parse(&t).unwrap();
        assert_eq!((cfg.settings.stall_minutes, cfg.settings.fresh_session, cfg.settings.max_chain_depth, cfg.settings.check.as_str()), (9, false, 5, "cargo test"));
    }

    #[test]
    fn counts_and_the_old_cap() {
        let (_tmp, root, roles_dir) = setup();
        // A project from before: one non-PM member at a time.
        std::fs::write(project_file(&root), format!("{}max_working = 1\n", text(&root))).unwrap();
        let mut s = load(&root, &roles_dir).unwrap();
        let eng = s.members.iter().position(|m| m.id == "engineer").unwrap();
        s.set_count(eng, 2);
        assert!(s.targets("tester").contains(&"engineer-2".to_string()));
        s.set_target("tester", "engineer-2", false);
        let pm = s.members.iter().position(|m| m.id == "pm").unwrap();
        s.set_count(pm, 3);
        assert_eq!(s.members[pm].count, 1, "one PM");
        save(&root, &roles_dir, &s).unwrap();
        let t = text(&root);
        assert!(t.contains("id = \"engineer\"\nrole = \"engineer\"\ncount = 2\n"), "{t}");
        assert!(!t.contains("max_working"), "the cap goes: counts say it now\n{t}");
        let team = crate::daemon::load_team(&root, &roles_dir).unwrap();
        assert_eq!(team.assign["tester"], ["engineer"]);
        // Back to one: engineer-2 and the rule about it go.
        let mut s = load(&root, &roles_dir).unwrap();
        s.set_count(eng, 1);
        assert!(s.assign.is_empty(), "{:?}", s.assign);
        save(&root, &roles_dir, &s).unwrap();
        assert!(!text(&root).contains("\ncount = "), "{}", text(&root));
    }

    #[test]
    fn renaming_a_new_member_moves_its_rules() {
        let (_tmp, root, roles_dir) = setup();
        let mut s = load(&root, &roles_dir).unwrap();
        s.add_member("tester").unwrap();
        s.set_target("tester2", "architect", true);
        let ix = s.members.len() - 1;
        s.rename_member(ix, "qa");
        assert!(s.assign.contains_key("qa"));
        s.rename_member(ix, "Bad Id");
        assert!(save(&root, &roles_dir, &s).unwrap_err().to_string().contains("Bad Id"));
    }
}
