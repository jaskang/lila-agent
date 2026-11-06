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
        if member.is_owner { ", the PM" } else { "" }
    ));
    for m in &team.members {
        let you = if m.id == member.id { "  ← you" } else { "" };
        out.push_str(&format!("- `{}`: {} ({}){you}\n", m.id, m.role.file.name, m.role.file.model));
    }
    for (id, slots) in &team.pools {
        out.push_str(&format!(
            "\n`{id}` stands for {}: assign to `{id}` and Lila gives the task to whichever is free; more work in the same chain goes back to the one that did it.\n",
            slots.iter().map(|s| format!("`{s}`")).collect::<Vec<_>>().join(", ")
        ));
    }
    let targets = team.collapse_pools(&team.assignable_from(&crate::Actor::Member(member.id.clone())));
    if targets.is_empty() {
        out.push_str("\nYou may not assign tasks. Report results to whoever assigned you.\n");
    } else {
        out.push_str(&format!("\nYou may assign tasks to: {}\n", targets.iter().map(|t| format!("`{t}`")).collect::<Vec<_>>().join(", ")));
    }
    out.push_str(PROTOCOL);
    if member.is_owner {
        out.push_str(OWNER);
        out.push_str(&flows_section(team));
    }
    out
}

/// The PM's view of the project's flows: what Lila does with work it hands out.
fn flows_section(team: &Team) -> String {
    if team.flows.is_empty() {
        return String::new();
    }
    let check = team.settings.check.trim();
    let describe = |f: &crate::flow::Flow| {
        f.steps
            .iter()
            .filter(|s| !s.is_check() || !check.is_empty())
            .map(|s| match &s.by {
                Some(by) => format!("{} by `{by}`", s.action),
                None => format!("check (`{check}`, run by Lila)"),
            })
            .collect::<Vec<_>>()
            .join(" → ")
    };
    let mut s = String::from("\n## Flows\n\n");
    let default = team.settings.flow.as_str();
    if let (Some(f), Some(by)) = (team.flows.get(default), team.flows.get(default).and_then(|f| f.steps[0].by.as_deref())) {
        s.push_str(&format!(
            "Work you give `{by}` runs through the `{default}` flow: {}. Lila hands each step on, sends failures back to whoever did the work (up to {} times) and tells you once: when it passed, or when it keeps failing. Don't create those hand-offs yourself.\n\n",
            describe(f),
            team.settings.max_rounds
        ));
    }
    s.push_str("Pick another with `lila task new … --flow <name>`, or `--flow none` for work that needs no checking (a question, a write-up):\n");
    for (name, f) in &team.flows {
        s.push_str(&format!("- `{name}`: {}\n", describe(f)));
    }
    s
}

const PROTOCOL: &str = r#"
## Protocol (always follow it)

Work arrives in your input as:

    <lila task="T12" from="pm" file=".lila/tasks/T12.md">Title, details…</lila>

1. When you begin, run `lila task start T12`.
2. When finished, run `lila task done T12 --summary "<one line>"`; if the task asks for a verdict, add `--verdict pass|fail`. Add `--decision "<…>"` for each choice you made that the task did not settle (a default, a name, a trade-off) and `--unverified "<…>"` for each thing you did not check. Add `--file <path>` for each deliverable or long report; put long text in a file and pass its path instead of pasting it.
3. If you cannot continue, run `lila task block T12 --reason "<exactly what you need>"`. Do not guess and do not wait silently.
4. To hand work to a teammate: `lila task new --to <member> --title "<title>" --body "<details>" --parent T12`. Lila adds the person's words and what counts as done to its task, so don't copy them. When your task says Lila sends it on, don't hand it on yourself. For long details write a file and use `--body-file <path>`. To correct or add to a task you already assigned, use `lila task edit T13 --append "<what changed>"`, never a separate correction task. To withdraw it: `lila task cancel T13 --reason "<why>"`.
5. Lila tells you about other tasks with `<lila note …>` and delivers answers with `<lila answer …>`. They are information for you, not new tasks, unless they say so. When work you handed on finishes, you hear about it only while a task of yours still waits on it, and only once the whole hand-off chain below it is finished or stuck.

A task may show the person's words, what counts as done (`Done when`) and what is out of scope: its own, or those of the request it serves. They are the bar. Meet them; if the task text contradicts them, block instead of choosing.

Every task you receive must end with `lila task done` or `lila task block`. A task's text in your input is complete; its file holds the same text unless the task says to read it. `lila status` shows the team and open tasks; `lila task show T12` shows one task. Never edit files under `.lila/` by hand.
"#;

const OWNER: &str = r#"
## As the PM

The person talks only to you. Reply in the language they use.

- Turn their requests into tasks for your team with `lila task new`. Plan, split, assign and follow up. You have no tools to edit files or run builds, by design: the team does the work.
- Start each piece of work the person asks for here with their exact words and what counts as done: `lila task new --to <member> --title "<title>" --quote "<their words, unchanged>" --accept "<a checkable outcome>" [--accept "…"] [--out-of-scope "…"]`. Everyone down the chain, the tester too, sees them as written. Work under a task from the person (`--parent T1`) carries their words already. If you can't say what done looks like, ask them one question first.
- Keep tasks small and concrete: what to change, where, and how to verify it.
- When a member reports done or blocked, decide the next step: assign follow-up work, clarify, or re-plan. Resolve blockers yourself when you can.
- Members hand work to each other (an engineer sends its change to the tester). You hear about a chain when all of it has finished or something is stuck, not at every hand-off. The note gives each step's summary and what is still open under your task, so you rarely need `lila status`. Never create a second task for work that is already queued or running.
- Trust your team's reports. A tester's PASS is the verification: do not re-run tests, read diffs or re-check files yourself unless reports contradict each other or the person asks.
- Ask the person only for decisions, credentials, approvals or information nobody on the team can get: `lila ask "<question>" [--option "<A>" --option "<B>"] [--task T12]`. Their answer arrives as `<lila answer q="Q3">`. Keep working on anything that does not depend on it.
- A task from the person (`from="human"`) is yours: close it with `lila task done` once the team's work for it is complete, with a summary the person can read. Pass on the team's decisions (`--decision`) and what nobody checked (`--unverified`) from the notes: the person signs off on them.
- Use `lila status` when you need the whole picture, for example after the person asks where things stand.
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
                tools: Default::default(),
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
        assert!(owner.contains("the PM"));
        assert!(owner.contains("You may assign tasks to: `engineer`, `tester`"));
        assert!(owner.contains("## As the PM"));
        let tester = render(&t, t.member("tester").unwrap(), "/repo");
        assert!(tester.contains("You may not assign tasks."));
        assert!(owner.contains("--quote"));
        assert!(!tester.contains("## Flows"), "only the PM hands out work");
    }

    #[test]
    fn the_pm_learns_the_flows() {
        let mut t = team();
        t.flows = crate::flow::builtin(|_| true);
        t.settings.check = "cargo test".into();
        let owner = render(&t, t.owner(), "/repo");
        assert!(
            owner.contains("Work you give `engineer` runs through the `code` flow: implement by `engineer` → check (`cargo test`, run by Lila) → verify by `tester`."),
            "{owner}"
        );
        assert!(owner.contains("- `quick`: implement by `engineer` → check"), "{owner}");
        t.settings.check.clear();
        assert!(render(&t, t.owner(), "/repo").contains("- `quick`: implement by `engineer`\n"), "no check command, no check step");
        assert!(!render(&t, t.member("engineer").unwrap(), "/repo").contains("## Flows"));
    }

    #[test]
    fn pools_are_named_once() {
        let mut t = team();
        let mut e2 = t.member("engineer").unwrap().clone();
        e2.id = "engineer-2".into();
        t.members.insert(2, e2);
        t.pools.insert("engineer".into(), vec!["engineer".into(), "engineer-2".into()]);
        let owner = render(&t, t.owner(), "/repo");
        assert!(owner.contains("`engineer` stands for `engineer`, `engineer-2`: assign to `engineer`"), "{owner}");
        assert!(owner.contains("You may assign tasks to: `engineer`, `tester`\n"), "{owner}");
    }
}
