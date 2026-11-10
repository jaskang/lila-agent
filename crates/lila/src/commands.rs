//! What a member's CLI offers to call by name besides its built-in commands: skills and
//! custom commands, found on disk the way the CLI finds them (Claude Code's `.claude/skills`,
//! `.claude/commands` and plugins, Codex's `.agents/skills` and `$CODEX_HOME/skills`).
//! Read-only, for the window's `/` menu.

use serde::Serialize;
use std::path::{Path, PathBuf};
use yaml_rust2::{Yaml, YamlLoader};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Skill,
    /// A custom command (a markdown file in `commands/`).
    Command,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    /// What follows the `/` (or `$`), with a plugin's name before a colon.
    pub name: String,
    pub description: String,
    pub kind: Kind,
    /// What the arguments look like (`[word]`), when the file says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// The file that defines it (Codex takes a skill by name and path).
    pub path: PathBuf,
}

/// The front matter of a markdown file (`---` … `---`) and what follows it.
fn split(text: &str) -> (Yaml, &str) {
    let Some(rest) = text.strip_prefix("---\n").or_else(|| text.strip_prefix("---\r\n")) else { return (Yaml::Null, text) };
    let Some(end) = rest.find("\n---") else { return (Yaml::Null, text) };
    let body = rest[end + 4..].trim_start_matches(['\r', '\n']);
    let doc = YamlLoader::load_from_str(&rest[..end]).ok().and_then(|mut d| (!d.is_empty()).then(|| d.remove(0))).unwrap_or(Yaml::Null);
    (doc, body)
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn field(doc: &Yaml, key: &str) -> Option<String> {
    match &doc[key] {
        Yaml::String(s) => Some(one_line(s)),
        // `argument-hint: [issue-number]` is a YAML list to the parser; it is meant as written.
        Yaml::Array(items) => Some(format!("[{}]", items.iter().filter_map(Yaml::as_str).collect::<Vec<_>>().join(", "))),
        _ => None,
    }
    .filter(|s| !s.is_empty())
}

fn named(prefix: Option<&str>, name: &str) -> String {
    match prefix {
        Some(p) => format!("{p}:{name}"),
        None => name.to_owned(),
    }
}

/// Skills under `dir`: each folder holding a `SKILL.md`. One a person can't call (`user-invocable: false`) is left out.
pub fn skills_in(dir: &Path, prefix: Option<&str>) -> Vec<Entry> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Entry> = Vec::new();
    for e in read.flatten() {
        let file = e.path().join("SKILL.md");
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        let (doc, _) = split(&text);
        if doc["user-invocable"].as_bool() == Some(false) {
            continue;
        }
        let name = field(&doc, "name").unwrap_or_else(|| e.file_name().to_string_lossy().to_string());
        out.push(Entry { name: named(prefix, &name), description: field(&doc, "description").unwrap_or_default(), kind: Kind::Skill, hint: field(&doc, "argument-hint"), path: file });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Custom commands under `dir`: the markdown files, named by file (folders only group them).
/// The description is the front matter's, else the first line of the file.
pub fn commands_in(dir: &Path, prefix: Option<&str>) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for e in walkdir::WalkDir::new(dir).follow_links(true).into_iter().flatten() {
        let p = e.path();
        if !e.file_type().is_file() || p.extension().is_none_or(|x| x != "md") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(p) else { continue };
        let (doc, body) = split(&text);
        let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let description = field(&doc, "description").or_else(|| body.lines().map(str::trim).find(|l| !l.is_empty()).map(|l| one_line(l.trim_start_matches('#')))).unwrap_or_default();
        out.push(Entry { name: named(prefix, &stem), description, kind: Kind::Command, hint: field(&doc, "argument-hint"), path: p.to_path_buf() });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Later lists win over earlier ones of the same name and kind (a project's skill over the user's).
fn merged(lists: Vec<Vec<Entry>>) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for e in lists.into_iter().flatten() {
        match out.iter().position(|o| o.name == e.name && o.kind == e.kind) {
            Some(at) => out[at] = e,
            None => out.push(e),
        }
    }
    out
}

/// The enabled plugins of a Claude Code config folder: (name, install folder). Those installed for
/// one project only count in that project.
fn claude_plugins(config: &Path, cwd: &Path) -> Vec<(String, PathBuf)> {
    let read = |name: &str| std::fs::read_to_string(config.join(name)).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
    let (Some(installed), Some(settings)) = (read("plugins/installed_plugins.json"), read("settings.json")) else { return Vec::new() };
    let mut out = Vec::new();
    for (key, installs) in installed["plugins"].as_object().into_iter().flatten() {
        if settings["enabledPlugins"][key] != true {
            continue;
        }
        let name = key.split('@').next().unwrap_or(key);
        for i in installs.as_array().into_iter().flatten() {
            let here = i["projectPath"].as_str().is_none_or(|p| cwd.starts_with(p));
            if let (true, Some(path)) = (here, i["installPath"].as_str()) {
                out.push((name.to_owned(), PathBuf::from(path)));
            }
        }
    }
    out
}

/// Claude Code's skills and custom commands for a session in `cwd`: the user's (in the config
/// folder), the enabled plugins' (`plugin:name`), and the project's, which win.
pub fn claude(config: &Path, cwd: &Path) -> Vec<Entry> {
    let mut lists = vec![skills_in(&config.join("skills"), None), commands_in(&config.join("commands"), None)];
    for (name, dir) in claude_plugins(config, cwd) {
        lists.push(skills_in(&dir.join("skills"), Some(&name)));
        lists.push(commands_in(&dir.join("commands"), Some(&name)));
    }
    lists.push(skills_in(&cwd.join(".claude/skills"), None));
    lists.push(commands_in(&cwd.join(".claude/commands"), None));
    merged(lists)
}

/// Codex's skills for a thread in `cwd`, where `skills/list` finds them: `.agents/skills` from the
/// folder up to the repository's root, `~/.agents/skills`, and `$CODEX_HOME/skills` (its bundled
/// ones in `.system`). Custom prompts (`~/.codex/prompts`) are the TUI's: app-server does not run them.
pub fn codex(codex_home: &Path, home: &Path, cwd: &Path) -> Vec<Entry> {
    let mut lists = vec![skills_in(&codex_home.join("skills/.system"), None), skills_in(&codex_home.join("skills"), None), skills_in(&home.join(".agents/skills"), None)];
    let mut chain: Vec<&Path> = Vec::new();
    for dir in cwd.ancestors() {
        chain.push(dir);
        if dir.join(".git").exists() {
            break;
        }
    }
    // Nearest last, so it wins; with no repository, only the folder itself.
    if !chain.last().is_some_and(|d| d.join(".git").exists()) {
        chain.truncate(1);
    }
    for dir in chain.into_iter().rev() {
        lists.push(skills_in(&dir.join(".agents/skills"), None));
    }
    merged(lists)
}

#[cfg(test)]
mod tests {
    use super::{Kind, claude, codex, commands_in, skills_in, split};
    use std::path::Path;

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn front_matter_is_yaml_and_the_rest_is_the_body() {
        let (doc, body) = split("---\nname: x\ndescription: >\n  folded\n  text\n---\n# Title\nbody\n");
        assert_eq!((doc["name"].as_str(), doc["description"].as_str()), (Some("x"), Some("folded text\n")));
        assert_eq!(body, "# Title\nbody\n");
        assert!(split("no front matter").0.is_null());
        assert!(split("---\n: [broken\n---\nbody").0.is_null(), "bad yaml is no front matter, not an error");
    }

    #[test]
    fn skills_are_the_folders_with_a_skill_file_and_the_ones_a_person_can_call() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "a/SKILL.md", "---\nname: alpha\ndescription: Does a\n  and more.\nargument-hint: <thing>\n---\nbody");
        write(d.path(), "b/SKILL.md", "no front matter");
        write(d.path(), "hidden/SKILL.md", "---\nname: hidden\nuser-invocable: false\n---\n");
        write(d.path(), "c/README.md", "not a skill");
        let got = skills_in(d.path(), Some("plug"));
        let shape: Vec<(&str, &str, Option<&str>)> = got.iter().map(|e| (e.name.as_str(), e.description.as_str(), e.hint.as_deref())).collect();
        assert_eq!(shape, [("plug:alpha", "Does a and more.", Some("<thing>")), ("plug:b", "", None)]);
        assert!(got.iter().all(|e| e.kind == Kind::Skill && e.path.ends_with("SKILL.md")));
    }

    #[test]
    fn commands_are_named_by_file_and_described_by_front_matter_or_first_line() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "pong.md", "---\ndescription: Reply PONG\nargument-hint: [word]\n---\nbody $ARGUMENTS");
        write(d.path(), "team/review.md", "# Review the change\nthen more");
        write(d.path(), "notes.txt", "not a command");
        let got = commands_in(d.path(), None);
        let shape: Vec<(&str, &str, Option<&str>)> = got.iter().map(|e| (e.name.as_str(), e.description.as_str(), e.hint.as_deref())).collect();
        assert_eq!(shape, [("pong", "Reply PONG", Some("[word]")), ("review", "Review the change", None)]);
    }

    #[test]
    fn claude_finds_the_users_plugins_and_the_projects_and_the_project_wins() {
        let cfg = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        write(cfg.path(), "skills/shared/SKILL.md", "---\nname: shared\ndescription: user's\n---\n");
        write(cfg.path(), "commands/mine.md", "---\ndescription: my command\n---\n");
        write(proj.path(), ".claude/skills/shared/SKILL.md", "---\nname: shared\ndescription: project's\n---\n");
        write(proj.path(), ".claude/commands/build.md", "Build it");
        // An enabled plugin, one that is off, and one installed for another project.
        let (on, off, other) = (cfg.path().join("cache/on"), cfg.path().join("cache/off"), cfg.path().join("cache/other"));
        write(&on, "skills/use/SKILL.md", "---\nname: use\ndescription: plugin skill\n---\n");
        write(&on, "commands/go.md", "go");
        write(&off, "skills/no/SKILL.md", "---\nname: no\n---\n");
        write(&other, "skills/far/SKILL.md", "---\nname: far\n---\n");
        write(
            cfg.path(),
            "plugins/installed_plugins.json",
            &serde_json::json!({"version": 2, "plugins": {
                "on@m": [{"scope": "user", "installPath": on}],
                "off@m": [{"scope": "user", "installPath": off}],
                "other@m": [{"scope": "local", "projectPath": "/elsewhere", "installPath": other}]}})
            .to_string(),
        );
        write(cfg.path(), "settings.json", &serde_json::json!({"enabledPlugins": {"on@m": true, "off@m": false, "other@m": true}}).to_string());
        let got = claude(cfg.path(), proj.path());
        let names: Vec<(&str, &str)> = got.iter().map(|e| (e.name.as_str(), e.description.as_str())).collect();
        assert_eq!(names, [("shared", "project's"), ("mine", "my command"), ("on:use", "plugin skill"), ("on:go", "go"), ("build", "Build it")]);
    }

    #[test]
    fn codex_finds_repo_skills_up_to_the_root_the_users_and_the_bundled() {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        write(home.path(), "codex/skills/mine/SKILL.md", "---\nname: mine\ndescription: in CODEX_HOME\n---\n");
        write(home.path(), "codex/skills/.system/sys/SKILL.md", "---\nname: sys\ndescription: bundled\n---\n");
        write(home.path(), "user/.agents/skills/uskill/SKILL.md", "---\nname: uskill\ndescription: in ~/.agents\n---\n");
        write(repo.path(), ".git/HEAD", "ref: refs/heads/main");
        write(repo.path(), ".agents/skills/root/SKILL.md", "---\nname: root\ndescription: at the root\n---\n");
        write(repo.path(), "sub/.agents/skills/deep/SKILL.md", "---\nname: deep\ndescription: nearest\n---\n");
        let got = codex(&home.path().join("codex"), &home.path().join("user"), &repo.path().join("sub"));
        let mut names: Vec<&str> = got.iter().map(|e| e.name.as_str()).collect();
        names.sort();
        assert_eq!(names, ["deep", "mine", "root", "sys", "uskill"]);
        // No repository: only the folder itself.
        let bare = tempfile::tempdir().unwrap();
        write(bare.path(), ".agents/skills/here/SKILL.md", "---\nname: here\n---\n");
        let got = codex(&home.path().join("codex"), &home.path().join("user"), bare.path());
        assert!(got.iter().any(|e| e.name == "here"));
    }
}
