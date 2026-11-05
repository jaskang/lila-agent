//! A member's system prompt: its role prompt plus the generated team and protocol section.

use crate::config::{Member, Team};

pub fn render(team: &Team, member: &Member, cwd: &str) -> String {
    let mut out = String::new();
    out.push_str(member.role.prompt.trim_end());
    out.push_str("\n\n---\n\n## Your team (Lila)\n\n");
    out.push_str(&format!("Project: {}, working directory: {cwd}\n", team.project));
    out.push_str(&format!(
        "You are `{}` ({}){}.\n\nTeam:\n",
        member.id,
        member.role.file.name,
        if member.is_owner { ", the owner" } else { "" }
    ));
    for m in &team.members {
        let you = if m.id == member.id { "  ← you" } else { "" };
        out.push_str(&format!("- `{}`: {} ({}){you}\n", m.id, m.role.file.name, m.role.file.model));
    }
    let targets = team.assignable_from(&crate::Actor::Member(member.id.clone()));
    if targets.is_empty() {
        out.push_str("\nYou may not assign tasks. Report results to whoever assigned you.\n");
    } else {
        out.push_str(&format!("\nYou may assign tasks to: {}\n", targets.iter().map(|t| format!("`{t}`")).collect::<Vec<_>>().join(", ")));
    }
    out.push_str(PROTOCOL);
    if member.is_owner {
        out.push_str(OWNER);
    }
    out
}

const PROTOCOL: &str = r#"
## Protocol (always follow it)

Work arrives in your input as:

    <lila task="T12" from="owner" file=".lila/tasks/T12.md">Title, details…</lila>

1. When you begin, run `lila task start T12`.
2. When finished, run `lila task done T12 --summary "<one line>"`. Add `--file <path>` for each deliverable or long report; put long text in a file and pass its path instead of pasting it.
3. If you cannot continue, run `lila task block T12 --reason "<exactly what you need>"`. Do not guess and do not wait silently.
4. To hand work to a teammate: `lila task new --to <member> --title "<title>" --body "<details>" --parent T12`. For long details write a file and use `--body-file <path>`.
5. Lila tells you about other tasks with `<lila note …>` and delivers answers with `<lila answer …>`. They are information for you, not new tasks, unless they say so.

Every task you receive must end with `lila task done` or `lila task block`. `lila status` shows the team and open tasks; `lila task show T12` shows one task. Never edit files under `.lila/` by hand.
"#;

const OWNER: &str = r#"
## As the owner

The person talks only to you. Reply in the language they use.

- Turn their requests into tasks for your team with `lila task new`. Plan, split, assign and follow up; do not implement code yourself.
- Keep tasks small and concrete: what to change, where, and how to verify it.
- When a member reports done or blocked, decide the next step: assign follow-up work, clarify, or re-plan. Resolve blockers yourself when you can.
- Members hand work to each other (an engineer sends its change to the tester). A done note lists what was "Already handed on"; check `lila status` before assigning, and never create a second task for work that is already queued or running. Wait for that chain to finish instead.
- Ask the person only for decisions, credentials, approvals or information nobody on the team can get: `lila ask "<question>" [--option "<A>" --option "<B>"] [--task T12]`. Their answer arrives as `<lila answer q="Q3">`. Keep working on anything that does not depend on it.
- A task from the person (`from="human"`) is yours: close it with `lila task done` once the team's work for it is complete, with a summary the person can read.
- Use `lila status` to see who is working on what before assigning more.
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ProjectConfig, Role, RoleFile, Runtime, Team};
    use std::collections::BTreeMap;

    fn team() -> Team {
        let mk = |id: &str, can: &[&str]| Role {
            id: id.into(),
            file: RoleFile {
                name: format!("{id}-name"),
                runtime: Runtime::Claude,
                model: format!("{id}-model"),
                effort: None,
                extra_args: vec![],
                env: BTreeMap::new(),
                can_assign: can.iter().map(|s| s.to_string()).collect(),
                prompt_file: None,
            },
            prompt: format!("ROLE PROMPT {id}\n"),
        };
        let roles = [mk("owner", &[]), mk("engineer", &["tester"]), mk("tester", &[])].into_iter().map(|r| (r.id.clone(), r)).collect();
        let cfg = ProjectConfig::parse(
            "name = \"shop\"\n[[members]]\nid = \"owner\"\nrole = \"owner\"\n[[members]]\nid = \"engineer\"\nrole = \"engineer\"\n[[members]]\nid = \"tester\"\nrole = \"tester\"\n",
        )
        .unwrap();
        Team::build(&cfg, &roles).unwrap()
    }

    #[test]
    fn engineer_prompt() {
        let t = team();
        let p = render(&t, t.member("engineer").unwrap(), "/repo");
        assert!(p.starts_with("ROLE PROMPT engineer"));
        assert!(p.contains("You are `engineer` (engineer-name)."));
        assert!(p.contains("- `tester`: tester-name (tester-model)\n"));
        assert!(p.contains("- `engineer`: engineer-name (engineer-model)  ← you"));
        assert!(p.contains("You may assign tasks to: `tester`"));
        assert!(p.contains("lila task done T12"));
        assert!(!p.contains("As the owner"));
    }

    #[test]
    fn owner_and_leaf_prompts() {
        let t = team();
        let owner = render(&t, t.owner(), "/repo");
        assert!(owner.contains("the owner"));
        assert!(owner.contains("You may assign tasks to: `engineer`, `tester`"));
        assert!(owner.contains("## As the owner"));
        let tester = render(&t, t.member("tester").unwrap(), "/repo");
        assert!(tester.contains("You may not assign tasks."));
    }
}
