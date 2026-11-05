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

/// `follow_ups`: tasks created under this one, so the reader doesn't assign the same work again.
pub fn done_note(task: &Task, files: &[String], follow_ups: &[Task]) -> String {
    let mut text = format!("{} finished {} \"{}\": {}", task.to, task.id, task.title, task.summary.as_deref().unwrap_or("(no summary)"));
    if !files.is_empty() {
        text.push_str(&format!("\nFiles: {}", files.join(", ")));
    }
    if !follow_ups.is_empty() {
        let list: Vec<String> = follow_ups.iter().map(|t| format!("{} → {} [{}] {}", t.id, t.to, t.state, t.title)).collect();
        text.push_str(&format!("\nAlready handed on: {}", list.join("; ")));
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
        assert_eq!(done_note(&task(""), &["a.md".into()], &[]), "<lila note about=\"T3\">engineer finished T3 \"Add login\": done it\nFiles: a.md</lila>");
        let mut child = task("");
        child.id = "T4".into();
        child.to = "tester".into();
        child.title = "Verify login".into();
        assert!(done_note(&task(""), &[], &[child]).contains("Already handed on: T4 → tester [queued] Verify login"));
        assert!(answer("Q1", "Which DB?", "Postgres").contains("A: Postgres"));
    }
}
