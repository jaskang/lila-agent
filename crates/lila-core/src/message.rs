//! Text typed into members' terminals. One shape, `<lila …>…</lila>`, like hc's own `<hc …>` tags.

use crate::task::Task;

/// Bodies longer than this are not inlined: the member reads the task file.
const INLINE_BODY: usize = 1200;

pub fn task_delivery(task: &Task, file: &str) -> String {
    let parent = task.parent.as_deref().map(|p| format!(" parent=\"{p}\"")).unwrap_or_default();
    let mut s = format!("<lila task=\"{}\" from=\"{}\"{parent} file=\"{file}\">\n{}\n", task.id, task.from, task.title);
    let body = task.body.trim();
    if !body.is_empty() {
        if body.chars().count() <= INLINE_BODY {
            s.push('\n');
            s.push_str(body);
            s.push('\n');
        } else {
            s.push_str(&format!("\nDetails are in {file}. Read it first.\n"));
        }
    }
    s.push_str(&format!(
        "\nStart: lila task start {id} · Finish: lila task done {id} --summary \"…\" · Stuck: lila task block {id} --reason \"…\"\n</lila>",
        id = task.id
    ));
    s
}

pub fn note(about: &str, text: &str) -> String {
    format!("<lila note about=\"{about}\">{}</lila>", text.trim())
}

/// A finished task, for the member whose work waited on it.
/// `path`: tasks finished on the way to `waiting` that nobody was told about yet, with their files.
/// `still_open`: what is open under `waiting` (or the chain, when nobody waits).
pub fn done_note(task: &Task, files: &[String], path: &[(&Task, Vec<String>)], waiting: Option<&Task>, still_open: &[&Task]) -> String {
    let mut text = format!("{} finished {} \"{}\": {}", task.to, task.id, task.title, task.summary.as_deref().unwrap_or("(no summary)"));
    if !files.is_empty() {
        text.push_str(&format!("\nFiles: {}", files.join(", ")));
    }
    for (t, files) in path {
        text.push_str(&format!("\nBefore it, {} finished {} \"{}\": {}", t.to, t.id, t.title, t.summary.as_deref().unwrap_or("(no summary)")));
        if !files.is_empty() {
            text.push_str(&format!(" (files: {})", files.join(", ")));
        }
    }
    let open = || still_open.iter().map(|t| format!("{} → {} [{}] {}", t.id, t.to, t.state, t.title)).collect::<Vec<_>>().join("; ");
    match (waiting, still_open.is_empty()) {
        (Some(w), true) => text.push_str(&format!(
            "\nNothing else is open under your {} \"{}\". Finish it with `lila task done {}` once your part is done.",
            w.id, w.title, w.id
        )),
        (Some(w), false) => text.push_str(&format!("\nStill open under your {}: {}", w.id, open())),
        (None, true) => text.push_str("\nNothing else is open in this chain."),
        (None, false) => text.push_str(&format!("\nStill open in this chain: {}", open())),
    }
    note(&task.id, &text)
}

pub fn blocked_note(task: &Task) -> String {
    note(&task.id, &format!("{} is blocked on {} \"{}\": {}", task.to, task.id, task.title, task.block_reason.as_deref().unwrap_or("(no reason)")))
}

pub fn answer(question_id: &str, question: &str, answer: &str) -> String {
    format!("<lila answer q=\"{question_id}\">\nQ: {}\nA: {}\n</lila>", question.trim(), answer.trim())
}

pub fn nudge(task: &Task) -> String {
    note(
        &task.id,
        &format!(
            "You went quiet with {} \"{}\" still open. If it is finished, run `lila task done {}`; if you are stuck, run `lila task block {} --reason \"…\"`; otherwise continue.",
            task.id, task.title, task.id, task.id
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{Actor, TaskState};

    fn task(body: &str) -> Task {
        Task {
            id: "T3".into(),
            project: "p".into(),
            title: "Add login".into(),
            body: body.into(),
            state: TaskState::Queued,
            from: Actor::Member("owner".into()),
            to: "engineer".into(),
            parent: Some("T1".into()),
            priority: 0,
            summary: Some("done it".into()),
            block_reason: None,
            created_ms: 0,
            updated_ms: 0,
        }
    }

    #[test]
    fn delivery_inlines_short_bodies() {
        let s = task_delivery(&task("Use bcrypt."), ".lila/tasks/T3.md");
        assert!(s.starts_with("<lila task=\"T3\" from=\"owner\" parent=\"T1\" file=\".lila/tasks/T3.md\">\nAdd login\n\nUse bcrypt.\n"));
        assert!(s.contains("lila task start T3"));
        assert!(s.ends_with("</lila>"));
    }

    #[test]
    fn delivery_points_to_file_for_long_bodies() {
        let s = task_delivery(&task(&"x".repeat(2000)), "f.md");
        assert!(s.contains("Details are in f.md"));
        assert!(!s.contains("xxxx"));
    }

    #[test]
    fn notes() {
        assert_eq!(
            done_note(&task(""), &["a.md".into()], &[], None, &[]),
            "<lila note about=\"T3\">engineer finished T3 \"Add login\": done it\nFiles: a.md\nNothing else is open in this chain.</lila>"
        );
        let mut parent = task("");
        parent.id = "T1".into();
        parent.to = "owner".into();
        parent.title = "Ship login".into();
        let mut step = task("");
        step.id = "T2".into();
        step.summary = Some("built it".into());
        let s = done_note(&task(""), &[], &[(&step, vec!["src/a.rs".into()])], Some(&parent), &[]);
        assert!(s.contains("\nBefore it, engineer finished T2 \"Add login\": built it (files: src/a.rs)"), "{s}");
        assert!(s.contains("Nothing else is open under your T1 \"Ship login\". Finish it with `lila task done T1`"), "{s}");
        let mut other = task("");
        other.id = "T4".into();
        other.to = "designer".into();
        other.title = "Polish".into();
        assert!(done_note(&task(""), &[], &[], Some(&parent), &[&other]).contains("Still open under your T1: T4 → designer [queued] Polish"));
        assert!(answer("Q1", "Which DB?", "Postgres").contains("A: Postgres"));
    }
}
