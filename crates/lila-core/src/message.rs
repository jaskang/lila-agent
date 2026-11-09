//! Text typed into members' terminals. One shape, `<lila …>…</lila>`, like hc's own `<hc …>` tags.

use crate::task::{Brief, Outcome, Task};

/// Bodies longer than this are not inlined: the member reads the task file.
const INLINE_BODY: usize = 1200;
/// The person's words are cut here in a delivery; the task file holds all of them.
const INLINE_QUOTE: usize = 1200;

/// The brief of the task that started a chain: its id, its file, the brief.
pub struct ChainBrief<'a> {
    pub id: &'a str,
    pub file: &'a str,
    pub brief: &'a Brief,
}

/// What a member doing a flow step is told about the rest of the flow.
#[derive(Debug, Clone, PartialEq)]
pub enum StepHint {
    /// What lilad does once it is done (`flow::later`): "runs `cargo test`", ...
    HandOn(Vec<String>),
    /// The step gives a verdict.
    Verdict,
}

/// `chain`: the brief of the task this one's chain started from, when that is another task.
/// `hint`: the task is a step of a flow.
pub fn task_delivery(task: &Task, file: &str, chain: Option<ChainBrief>, hint: Option<&StepHint>) -> String {
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
    if !task.brief.is_empty() {
        s.push('\n');
        push_brief(&mut s, &task.brief, file);
    }
    if let Some(c) = chain.filter(|c| !c.brief.is_empty()) {
        s.push_str(&format!("\nThe request this work serves ({}), the bar to meet:\n", c.id));
        push_brief(&mut s, c.brief, c.file);
    }
    let id = &task.id;
    match hint {
        Some(StepHint::HandOn(next)) if !next.is_empty() => s.push_str(&format!(
            "\nWhen you finish, Lila {} with your summary. Don't hand it on yourself.\n",
            and_list(next)
        )),
        Some(StepHint::Verdict) => s.push_str(
            "\nThis is a verification: test it against the bar above and give a verdict. For a fail, say what fails: expected vs actual and how to reproduce. Lila sends it back.\n",
        ),
        _ => {}
    }
    let verdict = if hint == Some(&StepHint::Verdict) { " --verdict pass|fail" } else { "" };
    s.push_str(&format!(
        "\nStart: lila task start {id} · Finish: lila task done {id}{verdict} --summary \"…\" [--decision \"…\"] [--unverified \"…\"] · Stuck: lila task block {id} --reason \"…\"\n</lila>"
    ));
    s
}

/// "a, b and c".
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The text of the next flow step lilad creates after `done` (and its files).
/// `check`: the check command that passed before it. `verdict`: the step verifies.
pub fn step_body(done: &Task, files: &[String], check: Option<&str>, verdict: bool) -> String {
    let mut s = format!("{} finished {} \"{}\": {}", done.to, done.id, done.title, done.summary.as_deref().unwrap_or("(no summary)"));
    s.push_str(&outcome_text(&done.outcome));
    if !files.is_empty() {
        s.push_str(&format!("\nFiles: {}", files.join(", ")));
    }
    if let Some(c) = check {
        s.push_str(&format!("\nLila ran `{c}`: it passed."));
    }
    if verdict {
        s.push_str("\n\nThat is their account. Verify the work against the person's words and what counts as done, below.");
    }
    s
}

/// A fix after a failed verdict: what `failed` (the verification) found.
pub fn fix_from_verdict(failed: &Task, files: &[String]) -> String {
    let mut s = format!("{} found a failure in {} \"{}\": {}", failed.to, failed.id, failed.title, failed.summary.as_deref().unwrap_or("(no summary)"));
    s.push_str(&outcome_text(&failed.outcome));
    if !files.is_empty() {
        s.push_str(&format!("\nFiles: {}", files.join(", ")));
    }
    s.push_str("\n\nFix it and finish this task; Lila checks and verifies it again.");
    s
}

/// A fix after the check command failed. `exit`: None when it did not finish (timeout, could not start).
pub fn fix_from_check(cmd: &str, exit: Option<i32>, tail: &str) -> String {
    let how = match exit {
        Some(code) => format!("exited with {code}"),
        None => "did not finish".into(),
    };
    format!("Lila ran `{cmd}` after your work and it {how}. The end of its output:\n\n```\n{}\n```\n\nFix it and finish this task; Lila runs the check again.", tail.trim_end())
}

/// A flow run that failed more than `max_rounds` times. `last`: the last failure.
pub fn gave_up(anchor: &Task, flow: &str, rounds: u32, last: &str) -> String {
    note(
        &anchor.id,
        &format!(
            "{} \"{}\" ({flow} flow) still fails after {rounds} rounds of fixes. Last failure: {}\nNothing more is sent back. Decide: clarify the task, re-plan it, or ask the person.",
            anchor.id,
            anchor.title,
            last.trim()
        ),
    )
}

fn push_brief(s: &mut String, b: &Brief, file: &str) {
    if let Some(q) = b.quote.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        s.push_str("The person's words:\n");
        let cut: String = q.chars().take(INLINE_QUOTE).collect();
        for line in cut.lines() {
            s.push_str(&format!("> {line}\n"));
        }
        if cut.len() < q.len() {
            s.push_str(&format!("> … (the rest is in {file})\n"));
        }
    }
    for (head, items) in [("Done when:", &b.accept), ("Out of scope:", &b.out_of_scope)] {
        if !items.is_empty() {
            s.push_str(head);
            s.push('\n');
            for i in items {
                s.push_str(&format!("- {}\n", i.trim()));
            }
        }
    }
}

/// A member's decisions and unchecked points after its summary, one line each kind.
pub fn outcome_text(o: &Outcome) -> String {
    let mut s = String::new();
    if !o.decisions.is_empty() {
        s.push_str(&format!("\n  Decided: {}", o.decisions.join("; ")));
    }
    if !o.unverified.is_empty() {
        s.push_str(&format!("\n  Not checked: {}", o.unverified.join("; ")));
    }
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
    text.push_str(&outcome_text(&task.outcome));
    if !files.is_empty() {
        text.push_str(&format!("\nFiles: {}", files.join(", ")));
    }
    for (t, files) in path {
        text.push_str(&format!("\nBefore it, {} finished {} \"{}\": {}", t.to, t.id, t.title, t.summary.as_deref().unwrap_or("(no summary)")));
        if !files.is_empty() {
            text.push_str(&format!(" (files: {})", files.join(", ")));
        }
        text.push_str(&outcome_text(&t.outcome));
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

/// What the PM gets first in a new session Lila started: its earlier conversation
/// is gone, so this says what stands. `decided`: the person's answers; `open`:
/// open work, a line each (sub-lines indented); `closed`: recently finished work;
/// `waiting`: inbox items open.
pub fn digest(decided: &[String], open: &[String], closed: &[String], waiting: usize) -> String {
    let mut s = String::from(
        "<lila digest>\nLila started you in a new session to keep your context small: your earlier conversation is not in it. This is where the project stands; `lila task show <id>` and `lila log` have the details.\n",
    );
    let section = |s: &mut String, head: &str, items: &[String]| {
        if !items.is_empty() {
            s.push_str(&format!("\n{head}\n"));
            for i in items {
                s.push_str(&format!("- {i}\n"));
            }
        }
    };
    section(&mut s, "The person decided:", decided);
    section(&mut s, "Open work:", open);
    section(&mut s, "Recently finished:", closed);
    if open.is_empty() {
        s.push_str("\nNothing is open.\n");
    }
    if waiting > 0 {
        s.push_str(&format!("\n{waiting} item(s) wait for the person in the inbox.\n"));
    }
    s.push_str("Carry on from here; if the person asks about something earlier, look it up instead of guessing.\n</lila>");
    s
}

/// The commit lilad makes when the person accepts `task`'s chain: its title,
/// what came of it and the person's words, and the task it was.
pub fn commit_message(task: &Task) -> String {
    let subject: String = task.title.lines().next().unwrap_or_default().trim().chars().take(72).collect();
    let mut s = if subject.is_empty() { task.id.clone() } else { subject };
    let mut body = Vec::new();
    if let Some(sum) = task.summary.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        body.push(sum.to_owned());
    }
    if let Some(q) = task.brief.quote.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        body.push(format!("Asked: {q}"));
    }
    if !task.outcome.decisions.is_empty() {
        body.push(format!("Decided: {}", task.outcome.decisions.join("; ")));
    }
    if !body.is_empty() {
        s.push_str("\n\n");
        s.push_str(&body.join("\n\n"));
    }
    s.push_str(&format!("\n\nLila-Task: {}", task.id));
    s
}

/// A member working a long time without telling Lila anything.
/// The person accepted work whose commit conflicts with the project's branch: the PM has it resolved.
pub fn resolve_conflicts(task: &Task, chain: &str, target: &str, files: &[String], comment: Option<&str>) -> String {
    let files = if files.is_empty() { "files git did not name".to_owned() } else { files.join(", ") };
    let mut s = format!(
        "The person accepted {} \"{}\", but its commit (branch lila/{chain}) conflicts with {target} in: {files}. \
         Assign the member who did the work to rebase it onto {target} in the chain's worktree (`git rebase {target}`, \
         resolve, keep both sides' intent) under this task, then finish this task. Lila asks the person to accept again.",
        task.id, task.title
    );
    if let Some(c) = comment.filter(|c| !c.trim().is_empty()) {
        s.push_str(&format!("\n\nThe person added: {}", c.trim()));
    }
    s
}

pub fn runaway(task: &Task, minutes: u64) -> String {
    note(
        &task.id,
        &format!(
            "You have been working on {} \"{}\" for {minutes} minutes without a `lila` call. If you are going round in circles, stop and run `lila task block {} --reason \"<what keeps failing>\"`. If it is going well, carry on.",
            task.id, task.title, task.id
        ),
    )
}

/// A flow run that used its budget, held before its next step.
pub fn over_budget(anchor: &Task, flow: &str, spent: u64, budget: u64) -> String {
    format!(
        "{} \"{}\" ({flow} flow) has used {} tokens of its {} budget; it stops before its next step until the person decides.",
        anchor.id,
        anchor.title,
        short_tokens(spent),
        short_tokens(budget)
    )
}

/// 1234 → 1.2k, 2_500_000 → 2.5M.
pub fn short_tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{}k", trim_point(n as f64 / 1e3)),
        _ => format!("{}M", trim_point(n as f64 / 1e6)),
    }
}

fn trim_point(x: f64) -> String {
    let s = if x >= 100.0 { format!("{x:.0}") } else { format!("{x:.1}") };
    s.strip_suffix(".0").map(String::from).unwrap_or(s)
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
            brief: Default::default(),
            outcome: Default::default(),
            run: None,
        }
    }

    #[test]
    fn delivery_inlines_short_bodies() {
        let s = task_delivery(&task("Use bcrypt."), ".lila/tasks/T3.md", None, None);
        assert!(s.starts_with("<lila task=\"T3\" from=\"owner\" parent=\"T1\" file=\".lila/tasks/T3.md\">\nAdd login\n\nUse bcrypt.\n"));
        assert!(s.contains("lila task start T3"));
        assert!(s.contains("--decision"));
        assert!(!s.contains("The request"), "no brief, no section");
        assert!(s.ends_with("</lila>"));
    }

    #[test]
    fn delivery_carries_its_own_brief_and_the_chains() {
        let mut t = task("Use bcrypt.");
        t.brief.accept = vec!["login returns a token".into()];
        let root = Brief { quote: Some("加一个登录\n用邮箱".into()), accept: vec!["wrong password: 401".into()], out_of_scope: vec!["sign-up".into()] };
        let s = task_delivery(&t, "f.md", Some(ChainBrief { id: "T1", file: ".lila/tasks/T1.md", brief: &root }), None);
        assert!(s.contains("Use bcrypt.\n\nDone when:\n- login returns a token\n"), "{s}");
        assert!(
            s.contains("The request this work serves (T1), the bar to meet:\nThe person's words:\n> 加一个登录\n> 用邮箱\nDone when:\n- wrong password: 401\nOut of scope:\n- sign-up\n"),
            "{s}"
        );
        let long = Brief { quote: Some("字".repeat(1500)), ..Default::default() };
        let s = task_delivery(&task(""), "f.md", Some(ChainBrief { id: "T1", file: ".lila/tasks/T1.md", brief: &long }), None);
        assert!(s.contains("> … (the rest is in .lila/tasks/T1.md)"), "{s}");
        assert!(!s.contains(&"字".repeat(1201)));
    }

    #[test]
    fn delivery_points_to_file_for_long_bodies() {
        let s = task_delivery(&task(&"x".repeat(2000)), "f.md", None, None);
        assert!(s.contains("Details are in f.md"));
        assert!(!s.contains("xxxx"));
    }

    #[test]
    fn flow_steps_say_what_lila_does_next() {
        let hand_on = StepHint::HandOn(vec!["runs `cargo test`".into(), "sends it to `tester` to verify".into()]);
        let s = task_delivery(&task(""), "f.md", None, Some(&hand_on));
        assert!(s.contains("When you finish, Lila runs `cargo test` and sends it to `tester` to verify with your summary. Don't hand it on yourself."), "{s}");
        let s = task_delivery(&task(""), "f.md", None, Some(&StepHint::Verdict));
        assert!(s.contains("This is a verification"), "{s}");
        assert!(s.contains("lila task done T3 --verdict pass|fail --summary"), "{s}");
        let mut done = task("");
        done.outcome.decisions = vec!["bcrypt cost 12".into()];
        let v = step_body(&done, &["src/login.rs".into()], Some("cargo test"), true);
        assert!(v.contains("That is their account"));
        assert!(!step_body(&done, &[], None, false).contains("That is their account"));
        assert!(v.starts_with("engineer finished T3 \"Add login\": done it\n  Decided: bcrypt cost 12\nFiles: src/login.rs\nLila ran `cargo test`: it passed."), "{v}");
        assert!(fix_from_check("cargo test", Some(101), "boom\n").contains("it exited with 101. The end of its output:\n\n```\nboom\n```"));
        assert!(fix_from_check("x", None, "").contains("did not finish"));
    }

    #[test]
    fn a_digest_lists_what_stands() {
        let d = digest(&["Q1 Which DB? → Postgres".into()], &["T1 from human → pm [in_progress] \"Ship\"\n    T2 → engineer [queued] Build".into()], &[], 2);
        assert!(d.starts_with("<lila digest>\n") && d.ends_with("</lila>"));
        assert!(d.contains("\nThe person decided:\n- Q1 Which DB? → Postgres\n"), "{d}");
        assert!(d.contains("\nOpen work:\n- T1 from human → pm [in_progress] \"Ship\"\n    T2 → engineer [queued] Build\n"), "{d}");
        assert!(d.contains("2 item(s) wait for the person"));
        assert!(digest(&[], &[], &[], 0).contains("Nothing is open."));
    }

    #[test]
    fn watchdog_text() {
        let mut t = task("");
        t.outcome.unverified = vec!["load".into()];
        assert!(runaway(&t, 30).contains("for 30 minutes without a `lila` call"));
        assert!(over_budget(&t, "code", 312_000, 300_000).contains("used 312k tokens of its 300k budget"));
        assert_eq!((short_tokens(999), short_tokens(1_250), short_tokens(2_500_000)), ("999".into(), "1.2k".into(), "2.5M".into()));
    }

    #[test]
    fn commit_messages() {
        let mut t = task("");
        t.brief.quote = Some("能登录就行".into());
        t.outcome.decisions = vec!["24h tokens".into()];
        assert_eq!(commit_message(&t), "Add login\n\ndone it\n\nAsked: 能登录就行\n\nDecided: 24h tokens\n\nLila-Task: T3");
        t.title = "x".repeat(100);
        t.summary = None;
        t.brief.quote = None;
        t.outcome.decisions.clear();
        assert_eq!(commit_message(&t), format!("{}\n\nLila-Task: T3", "x".repeat(72)));
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
        let mut decided = task("");
        decided.outcome = Outcome { decisions: vec!["tokens last 24h".into()], unverified: vec!["concurrent logins".into()], passed: None };
        let s = done_note(&decided, &[], &[], None, &[]);
        assert!(s.contains(": done it\n  Decided: tokens last 24h\n  Not checked: concurrent logins\n"), "{s}");
    }
}
