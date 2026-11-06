use super::*;
use std::os::unix::fs::PermissionsExt;

#[derive(Default)]
struct Fake {
    runs: Vec<Value>,
    sends: Vec<(String, String, Option<String>)>,
    kills: Vec<String>,
    refuse_sends: bool,
}

impl Terminals for Fake {
    fn run(&mut self, req: Value) -> Result<String> {
        self.runs.push(req);
        Ok(format!("t{}", self.runs.len()))
    }
    fn send(&mut self, terminal: &str, text: &str, only_if: Option<&str>) -> Result<()> {
        if self.refuse_sends {
            bail!("unavailable");
        }
        // Like hc: `if` takes `idle` or `working` only (M1 found lilad passing `done`).
        if let Some(word) = only_if
            && !matches!(word, "idle" | "working")
        {
            bail!("unknown variant `{word}`, expected `idle` or `working` (invalid_params)");
        }
        self.sends.push((terminal.into(), text.into(), only_if.map(Into::into)));
        Ok(())
    }
    fn kill(&mut self, terminal: &str) -> Result<()> {
        self.kills.push(terminal.into());
        Ok(())
    }
}

struct World {
    d: Daemon<Fake>,
    repo: PathBuf,
    _tmp: tempfile::TempDir,
    now: i64,
}

const PROJECT: &str = r#"
name = "shop"
[[members]]
id = "owner"
role = "pm"
[[members]]
id = "engineer"
role = "engineer"
[[members]]
id = "tester"
role = "tester"
[[members]]
id = "architect"
role = "architect"
"#;

fn world_with(project_toml: &str) -> World {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let paths = Paths { config: root.join("config"), data: root.join("data"), state: root.join("state"), isolated: true };
    paths.ensure().unwrap();
    roles::install_defaults(&paths.roles()).unwrap();
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for prog in ["claude", "codex"] {
        let p = bin.join(prog);
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let repo = root.join("repo");
    std::fs::create_dir_all(repo.join(".lila")).unwrap();
    std::fs::write(repo.join(".lila/project.toml"), project_toml).unwrap();
    let store = Store::open(&paths.db()).unwrap();
    let d = Daemon::new(store, Fake::default(), paths, bin.to_string_lossy().to_string(), PathBuf::from("/opt/lila/bin/lila"));
    World { d, repo, _tmp: tmp, now: 1_000_000 }
}

fn world() -> World {
    world_with(PROJECT)
}

impl World {
    fn caller(&self, member: Option<&str>) -> Value {
        json!({"cwd": self.repo.join("src").to_string_lossy(), "project": member.map(|_| "shop"), "member": member})
    }

    fn call(&mut self, member: Option<&str>, method: &str, mut params: Value) -> Result<Value> {
        self.now += 1000;
        params["caller"] = self.caller(member);
        self.d.handle(method, &params, self.now)
    }

    fn ok(&mut self, member: Option<&str>, method: &str, params: Value) -> Value {
        self.call(member, method, params).unwrap_or_else(|e| panic!("{method}: {e:#}"))
    }

    fn up(&mut self) -> Value {
        std::fs::create_dir_all(self.repo.join("src")).unwrap();
        self.ok(None, "project.up", json!({}))
    }

    fn terminal(&self, member: &str) -> String {
        self.d.projects["shop"].live[member].terminal.clone().unwrap()
    }

    fn status(&mut self, member: &str, word: &str) {
        self.now += 1000;
        let t = self.terminal(member);
        let ev = json!({"jsonrpc":"2.0","method":"event","params":{"seq":1,"type":"status","terminal":t,"data":{"status":{"app":"x","state":word},"reported":true,"records":{}}}});
        self.d.on_event(&ev, self.now);
    }

    fn all_idle(&mut self) {
        for m in ["owner", "engineer", "tester", "architect"] {
            self.status(m, "idle");
        }
    }

    fn sends_to(&self, member: &str) -> Vec<String> {
        let t = self.terminal(member);
        self.d.term.sends.iter().filter(|(to, _, _)| *to == t).map(|(_, text, _)| text.clone()).collect()
    }

    fn state(&self, id: &str) -> TaskState {
        self.d.store.task("shop", id).unwrap().unwrap().state
    }

    fn inbox_kinds(&self) -> Vec<String> {
        self.d.store.inbox("shop", false).unwrap().into_iter().map(|i| i.kind).collect()
    }
}

#[test]
fn up_starts_every_member_with_its_command() {
    let mut w = world();
    let r = w.up();
    assert_eq!(r["started"].as_array().unwrap().len(), 4);
    let runs = &w.d.term.runs;
    let owner = &runs[0];
    let argv: Vec<&str> = owner["argv"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert!(argv[0].ends_with("/bin/claude"), "resolved to an absolute path: {argv:?}");
    assert_eq!(argv[1..5], ["--model", "claude-opus-5-5", "--effort", "high"]);
    assert_eq!(argv[5], "--append-system-prompt-file");
    let prompt = std::fs::read_to_string(argv[6]).unwrap();
    assert!(prompt.contains("## As the PM"));
    assert_eq!(owner["labels"], json!({"lila.project": "shop", "lila.member": "owner"}));
    assert_eq!(owner["env"]["LILA_MEMBER"], "owner");
    assert_eq!(owner["env"]["LILA_ROOT"], w.repo.to_string_lossy().as_ref());
    assert!(owner["env"]["PATH"].as_str().unwrap().starts_with("/opt/lila/bin:"), "members can run lila");
    assert_eq!(owner["name"], "lila-shop-owner");
    assert_eq!(owner["no_parent"], true);
    let tester = &runs[2];
    assert!(tester["argv"][0].as_str().unwrap().ends_with("/bin/codex"));
    assert!(tester["argv"][7].as_str().unwrap().starts_with("developer_instructions=\""));

    // A second `up` leaves running members alone.
    let r = w.up();
    assert!(r["started"].as_array().unwrap().is_empty());
    assert_eq!(r["running"].as_array().unwrap().len(), 4);
    assert!(w.repo.join(".lila/.gitignore").exists());
}

#[test]
fn up_reports_a_missing_program() {
    let mut w = world();
    std::fs::remove_file(w.d.path_env.clone() + "/codex").unwrap();
    std::fs::create_dir_all(w.repo.join("src")).unwrap();
    let err = format!("{:#}", w.call(None, "project.up", json!({})).unwrap_err());
    assert!(err.contains("\"codex\" is not on PATH"), "{err}");
    assert!(w.d.term.runs.is_empty(), "nobody starts when one program is missing");
}

#[test]
fn brief_goes_to_the_owner_once_it_reports_idle() {
    let mut w = world();
    w.ok(None, "project.up", json!({"brief": "Build a login page\nwith email + password"}));
    assert_eq!(w.state("T1"), TaskState::Queued, "nothing is typed before the owner reports a status");
    w.status("owner", "idle");
    assert_eq!(w.state("T1"), TaskState::Delivered);
    let sent = w.sends_to("owner");
    assert!(sent[0].starts_with("<lila task=\"T1\" from=\"human\""), "{}", sent[0]);
    assert!(sent[0].contains("Build a login page"));
    assert_eq!(w.d.term.sends[0].2.as_deref(), Some("idle"), "typed only if still idle");
    assert!(std::fs::read_to_string(w.repo.join(".lila/tasks/T1.md")).unwrap().contains("with email + password"));
}

#[test]
fn full_handoff_flow_respects_the_working_limit() {
    let mut w = world_with(&format!("{PROJECT}\n[settings]\nmax_working = 1\n"));
    w.up();
    w.all_idle();
    // Owner plans two tasks; only one non-owner member may work at a time.
    let t2 = w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Implement login", "body": "POST /login"}));
    assert_eq!(t2["id"], "T1");
    assert_eq!(t2["state"], "delivered");
    let t = w.ok(Some("owner"), "task.new", json!({"to": "architect", "title": "Design sessions"}));
    assert_eq!(t["state"], "queued", "engineer holds the only working slot");

    w.status("engineer", "working");
    w.ok(Some("engineer"), "task.start", json!({"id": "T1"}));
    // Engineer hands off to the tester (allowed by role rules), then finishes.
    let t3 = w.ok(Some("engineer"), "task.new", json!({"to": "tester", "title": "Verify login", "parent": "T1"}));
    assert_eq!(t3["state"], "queued");
    w.ok(Some("engineer"), "task.done", json!({"id": "T1", "summary": "login works", "files": ["src/login.rs"]}));
    // The tester still has to verify it: nothing the owner could act on yet, so no note (no turn).
    assert!(w.sends_to("owner").iter().all(|n| !n.contains("<lila note")), "{:?}", w.sends_to("owner"));

    // The slot freed up: the oldest queued task (architect's T2) goes first.
    assert_eq!(w.state("T2"), TaskState::Delivered);
    assert_eq!(w.state("T3"), TaskState::Queued);
    w.ok(Some("architect"), "task.done", json!({"id": "T2", "summary": "docs/sessions.md"}));
    assert_eq!(w.state("T3"), TaskState::Delivered);
    assert!(w.sends_to("tester").iter().any(|s| s.contains("<lila task=\"T3\" from=\"engineer\" parent=\"T1\"")));
    // The chain is finished: the owner hears once, with the engineer's step. The engineer's
    // part was over, so it is not woken for the PASS.
    w.ok(Some("tester"), "task.done", json!({"id": "T3", "summary": "PASS"}));
    assert!(w.sends_to("engineer").iter().all(|s| !s.contains("tester finished T3")));
    let notes: Vec<String> = w.sends_to("owner").into_iter().filter(|n| n.contains("<lila note about=\"T1\"") || n.contains("<lila note about=\"T3\"")).collect();
    assert_eq!(notes.len(), 1, "one note for the T1 → T3 chain: {notes:?}");
    assert!(notes[0].contains("tester finished T3 \"Verify login\": PASS"), "{}", notes[0]);
    assert!(notes[0].contains("Before it, engineer finished T1 \"Implement login\": login works (files: src/login.rs)"), "{}", notes[0]);
    assert!(notes[0].contains("Nothing else is open in this chain."), "{}", notes[0]);
}

#[test]
fn the_pm_hears_when_a_chain_under_its_task_ends_and_is_not_nudged_meanwhile() {
    let mut w = world();
    w.ok(None, "project.up", json!({"brief": "Ship login"}));
    w.all_idle();
    w.ok(Some("owner"), "task.start", json!({"id": "T1"}));
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Build login", "parent": "T1"}));
    w.ok(Some("owner"), "task.new", json!({"to": "architect", "title": "Review sessions", "parent": "T1"}));
    w.ok(Some("engineer"), "task.new", json!({"to": "tester", "title": "Verify login", "parent": "T2"}));
    w.ok(Some("engineer"), "task.done", json!({"id": "T2", "summary": "built"}));
    // The owner waits on its team for a long time: that is not going quiet.
    w.status("owner", "idle");
    w.now += 30 * 60_000;
    w.d.tick(w.now);
    assert!(w.sends_to("owner").iter().all(|n| !n.contains("went quiet")), "{:?}", w.sends_to("owner"));
    w.ok(Some("tester"), "task.done", json!({"id": "T4", "summary": "PASS"}));
    let note = w.sends_to("owner").into_iter().rfind(|n| n.contains("tester finished T4")).expect("owner told");
    assert!(note.contains("Before it, engineer finished T2 \"Build login\": built"), "{note}");
    assert!(note.contains("Still open under your T1: T3 → architect"), "{note}");
    w.ok(Some("architect"), "task.done", json!({"id": "T3", "summary": "fine"}));
    let note = w.sends_to("owner").into_iter().rfind(|n| n.contains("architect finished T3")).expect("owner told");
    assert!(note.contains("Nothing else is open under your T1 \"Ship login\". Finish it with `lila task done T1`"), "{note}");
}

#[test]
fn a_members_waiting_notes_go_out_as_one_message() {
    let mut w = world();
    w.up();
    w.all_idle();
    w.status("owner", "blocked"); // in a dialog: notes pile up
    w.d.store.push_outbox("shop", "owner", "<lila note about=\"T1\">one</lila>", w.now).unwrap();
    w.d.store.push_outbox("shop", "owner", "<lila note about=\"T2\">two</lila>", w.now).unwrap();
    w.d.pump("shop", w.now);
    assert!(w.sends_to("owner").is_empty());
    w.status("owner", "idle");
    assert_eq!(w.sends_to("owner"), ["<lila note about=\"T1\">one</lila>\n<lila note about=\"T2\">two</lila>"]);
    assert!(w.d.store.outbox("shop").unwrap().is_empty());
}

#[test]
fn assignment_rules_and_ownership_are_enforced() {
    let mut w = world();
    w.up();
    w.all_idle();
    let err = w.call(Some("engineer"), "task.new", json!({"to": "architect", "title": "x"})).unwrap_err().to_string();
    assert!(err.contains("allowed: tester"), "{err}");
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "x"}));
    let err = w.call(Some("tester"), "task.done", json!({"id": "T1", "summary": "s"})).unwrap_err().to_string();
    assert!(err.contains("assigned to engineer"), "{err}");
    assert!(w.call(Some("engineer"), "task.done", json!({"id": "T1"})).unwrap_err().to_string().contains("--summary"));
    assert!(w.call(Some("tester"), "task.cancel", json!({"id": "T1"})).is_err());
    w.ok(Some("owner"), "task.cancel", json!({"id": "T1", "reason": "changed plans"}));
    assert!(w.sends_to("engineer").iter().any(|s| s.contains("T1 was cancelled by owner")));
    assert!(w.call(Some("engineer"), "ask", json!({"question": "?"})).unwrap_err().to_string().contains("only the PM"));
    assert!(w.call(Some("owner"), "answer", json!({"id": "Q1", "text": "x"})).is_err());
}

#[test]
fn blocked_tasks_reach_the_assigner_and_the_owner() {
    let mut w = world();
    w.up();
    w.all_idle();
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Build"}));
    w.status("engineer", "working");
    w.ok(Some("engineer"), "task.new", json!({"to": "tester", "title": "Check"}));
    w.ok(Some("engineer"), "task.done", json!({"id": "T1", "summary": "built"}));
    w.status("engineer", "idle");
    assert_eq!(w.state("T2"), TaskState::Delivered);
    w.ok(Some("tester"), "task.block", json!({"id": "T2", "reason": "no test DB"}));
    assert!(w.sends_to("engineer").iter().any(|s| s.contains("tester is blocked on T2 \"Check\": no test DB")));
    assert!(w.sends_to("owner").iter().any(|s| s.contains("tester is blocked on T2")), "owner is copied");
    // Blocked does not hold a working slot, and unblocking resumes it.
    w.ok(Some("tester"), "task.start", json!({"id": "T2"}));
    assert_eq!(w.state("T2"), TaskState::InProgress);
}

#[test]
fn questions_go_to_the_inbox_and_answers_back_to_the_owner() {
    let mut w = world();
    w.up();
    w.all_idle();
    let q = w.ok(Some("owner"), "ask", json!({"question": "Stripe or Adyen?", "options": ["Stripe", "Adyen"]}));
    assert_eq!(q["id"], "Q1");
    let inbox = w.ok(None, "inbox", json!({}));
    assert_eq!(inbox[0]["options"], json!(["Stripe", "Adyen"]));
    assert!(w.call(None, "answer", json!({"id": "Q1"})).unwrap_err().to_string().contains("give an answer"));
    w.ok(None, "answer", json!({"id": "q1", "text": "Stripe"}));
    let last = w.sends_to("owner").pop().unwrap();
    assert_eq!(last, "<lila answer q=\"Q1\">\nQ: Stripe or Adyen?\nA: Stripe\n</lila>");
    assert!(w.inbox_kinds().is_empty());
}

#[test]
fn notes_wait_while_a_member_is_blocked_in_its_terminal() {
    let mut w = world();
    w.up();
    w.all_idle();
    w.status("owner", "blocked");
    assert_eq!(w.inbox_kinds(), ["terminal"]);
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "x"}));
    w.ok(Some("engineer"), "task.done", json!({"id": "T1", "summary": "ok"}));
    assert!(w.sends_to("owner").is_empty(), "nothing is typed into a permission dialog");
    w.status("owner", "working");
    assert!(w.inbox_kinds().is_empty(), "the terminal item closes itself");
    assert_eq!(w.sends_to("owner").len(), 1, "queued input is safe while working");
}

#[test]
fn silent_members_are_flagged_and_quiet_ones_nudged() {
    let mut w = world();
    w.up();
    w.now += 21_000;
    w.d.tick(w.now);
    assert_eq!(w.inbox_kinds().iter().filter(|k| *k == "startup").count(), 4);
    w.d.tick(w.now + 1000);
    assert_eq!(w.inbox_kinds().len(), 4, "not repeated");
    w.all_idle();
    assert!(w.inbox_kinds().is_empty(), "first status closes them");

    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Build"}));
    w.status("engineer", "working");
    w.status("engineer", "idle"); // finished a turn without `done`
    let t0 = w.now;
    w.d.tick(t0 + 4 * 60_000);
    assert_eq!(w.sends_to("engineer").len(), 1, "only the task so far");
    w.d.tick(t0 + 5 * 60_000);
    assert!(w.sends_to("engineer")[1].contains("You went quiet with T1"));
    w.d.tick(t0 + 10 * 60_000 + 1000);
    assert!(w.sends_to("owner").iter().any(|s| s.contains("engineer went quiet on T1")));
    let kinds: Vec<String> = w.d.store.events("shop", Some("T1"), 20).unwrap().into_iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&"nudged".to_string()) && kinds.contains(&"stalled".to_string()), "{kinds:?}");
}

#[test]
fn a_lost_member_returns_its_work_to_the_queue() {
    let mut w = world();
    w.up();
    w.all_idle();
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Build"}));
    let t = w.terminal("engineer");
    w.now += 1000;
    w.d.on_event(&json!({"method":"event","params":{"type":"finished","terminal":t,"data":{"reason":"exited"}}}), w.now);
    assert_eq!(w.state("T1"), TaskState::Queued);
    assert_eq!(w.inbox_kinds(), ["lost"]);
    assert!(w.sends_to("owner").iter().any(|s| s.contains("engineer ended (exited)")));
    // `lila up` restarts only the lost member, and the task goes out again once it is idle.
    let r = w.up();
    assert_eq!(r["started"].as_array().unwrap().len(), 1);
    w.status("engineer", "idle");
    assert_eq!(w.state("T1"), TaskState::Delivered);
}

#[test]
fn human_tasks_report_to_the_inbox() {
    let mut w = world();
    w.ok(None, "project.up", json!({"brief": "Ship it"}));
    w.all_idle();
    w.ok(Some("owner"), "task.done", json!({"id": "T1", "summary": "shipped v1"}));
    let inbox = w.ok(None, "inbox", json!({}));
    assert_eq!(inbox[0]["kind"], "report");
    assert_eq!(inbox[0]["text"], "T1 done: shipped v1");
    w.ok(None, "answer", json!({"id": "Q1"}));
    assert!(w.inbox_kinds().is_empty());
}

#[test]
fn pause_stops_deliveries_but_not_notes() {
    let mut w = world();
    w.up();
    w.all_idle();
    w.ok(None, "pause", json!({}));
    let t = w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "x"}));
    assert_eq!(t["state"], "queued");
    w.ok(None, "resume", json!({}));
    assert_eq!(w.state("T1"), TaskState::Delivered);
}

#[test]
fn chain_depth_is_limited() {
    let mut w = world_with(&format!("{PROJECT}\n[settings]\nmax_chain_depth = 2\n"));
    w.up();
    w.all_idle();
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "a"}));
    w.ok(Some("engineer"), "task.new", json!({"to": "tester", "title": "b", "parent": "T1"}));
    let err = w.call(Some("tester"), "task.new", json!({"to": "engineer", "title": "c", "parent": "T2"})).unwrap_err().to_string();
    assert!(err.contains("2 deep"), "{err}");
}

#[test]
fn status_shows_members_and_tasks() {
    let mut w = world();
    w.up();
    w.all_idle();
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Build"}));
    w.status("engineer", "working");
    let st = w.ok(Some("tester"), "status", json!({}));
    let eng = st["members"].as_array().unwrap().iter().find(|m| m["id"] == "engineer").unwrap().clone();
    assert_eq!(eng["status"], "working");
    assert_eq!(eng["task"]["id"], "T1");
    assert_eq!(st["open_tasks"].as_array().unwrap().len(), 1);
}

#[test]
fn down_kills_members_without_alarms() {
    let mut w = world();
    w.up();
    w.all_idle();
    let r = w.ok(None, "project.down", json!({}));
    assert_eq!(r["stopped"].as_array().unwrap().len(), 4);
    assert_eq!(w.d.term.kills.len(), 4);
    assert!(w.inbox_kinds().is_empty());
}

#[test]
fn a_daemon_restart_reloads_projects_and_reconciles() {
    let mut w = world();
    w.up();
    w.all_idle();
    let paths = w.d.paths.clone();
    let path_env = w.d.path_env.clone();
    let store = Store::open(&paths.db()).unwrap();
    let mut d2 = Daemon::new(store, Fake::default(), paths, path_env, PathBuf::from("/x/lila"));
    assert!(d2.projects.contains_key("shop"));
    let live = vec![w.terminal("owner")];
    // A snapshot taken before the members started says nothing about them.
    d2.reconcile(&live, w.now - 60_000, w.now);
    assert!(d2.store.inbox("shop", false).unwrap().is_empty());
    d2.reconcile(&live, w.now + 60_000, w.now + 60_000);
    let kinds: Vec<String> = d2.store.inbox("shop", false).unwrap().into_iter().map(|i| i.kind).collect();
    assert_eq!(kinds, ["lost", "lost", "lost"], "three terminals vanished while the daemon was away");
}

#[test]
fn delivery_retries_when_the_terminal_refuses() {
    let mut w = world();
    w.up();
    w.all_idle();
    w.d.term.refuse_sends = true;
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "x"}));
    assert_eq!(w.state("T1"), TaskState::Queued);
    w.d.term.refuse_sends = false;
    w.d.tick(w.now + 5000);
    assert_eq!(w.state("T1"), TaskState::Delivered);
}

#[test]
fn delivers_to_a_member_that_finished_a_turn_unseen() {
    let mut w = world();
    w.up();
    w.all_idle();
    w.status("engineer", "done");
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "x"}));
    assert_eq!(w.state("T1"), TaskState::Delivered, "`done` takes input like `idle`");
    assert_eq!(w.d.term.sends.last().unwrap().2, None, "and is sent without an `if` hc would reject");
}

#[test]
fn queued_tasks_are_rewritten_and_delivered_ones_get_updates() {
    let mut w = world();
    w.up();
    w.all_idle();
    w.status("engineer", "working");
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Add multiply", "body": "with asserts"}));
    assert_eq!(w.state("T1"), TaskState::Queued);
    let v = w.ok(Some("owner"), "task.edit", json!({"id": "T1", "append": "Do not commit."}));
    assert_eq!(v["update_sent"], false);
    let file = std::fs::read_to_string(w.repo.join(".lila/tasks/T1.md")).unwrap();
    assert!(file.contains("with asserts\n\n## Update from owner\n\nDo not commit."), "{file}");
    w.status("engineer", "idle");
    let typed = w.sends_to("engineer").pop().unwrap();
    assert!(typed.contains("Do not commit."), "the delivered text has the edit: {typed}");

    // Delivered: only appends, and they reach the member as a note.
    let err = w.call(Some("owner"), "task.edit", json!({"id": "T1", "title": "x"})).unwrap_err().to_string();
    assert!(err.contains("already delivered"), "{err}");
    w.ok(Some("owner"), "task.edit", json!({"id": "T1", "append": "Also handle floats."}));
    assert!(w.sends_to("engineer").pop().unwrap().contains("Update to T1 \"Add multiply\" from owner: Also handle floats."));
    assert!(w.call(Some("tester"), "task.edit", json!({"id": "T1", "append": "x"})).unwrap_err().to_string().contains("only owner"));
    w.ok(Some("engineer"), "task.done", json!({"id": "T1", "summary": "ok"}));
    assert!(w.call(Some("owner"), "task.edit", json!({"id": "T1", "append": "x"})).unwrap_err().to_string().contains("is done"));
}

#[test]
fn back_and_forth_stops_after_max_rounds() {
    let mut w = world_with(&format!("{PROJECT}\n[settings]\nmax_rounds = 1\nmax_working = 9\n"));
    w.up();
    w.all_idle();
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Build"}));
    w.ok(Some("engineer"), "task.new", json!({"to": "tester", "title": "Check", "parent": "T1"}));
    w.ok(Some("tester"), "task.new", json!({"to": "engineer", "title": "Fix 1", "parent": "T2"}));
    w.ok(Some("engineer"), "task.new", json!({"to": "tester", "title": "Check again", "parent": "T3"}));
    let err = w.call(Some("tester"), "task.new", json!({"to": "engineer", "title": "Fix 2", "parent": "T4"})).unwrap_err().to_string();
    assert!(err.contains("back and forth 1 times") && err.contains("lila task block T4"), "{err}");
    // A different pair is not a bounce: the owner can always re-plan.
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Re-plan", "parent": "T4"}));
}

#[test]
fn sandboxed_members_talk_through_the_spool() {
    let mut w = world();
    w.up();
    w.all_idle();
    let dir = spool_dir(&w.repo);
    assert!(dir.is_dir(), "`up` creates it");
    let gi = std::fs::read_to_string(w.repo.join(".lila/.gitignore")).unwrap();
    assert!(gi.contains("tasks/") && gi.contains("spool/"));
    let caller = json!({"cwd": w.repo.to_string_lossy(), "project": "shop", "member": "owner"});
    let id = crate::client::write_spool(&dir, "task.new", &json!({"caller": caller, "to": "engineer", "title": "via spool"})).unwrap();
    let bad = crate::client::write_spool(&dir, "task.done", &json!({"caller": caller, "id": "T9", "summary": "s"})).unwrap();
    w.d.process_spool(w.now);
    let v = crate::client::await_spool(&dir, &id, std::time::Duration::from_secs(1)).unwrap();
    assert_eq!((v["id"].as_str(), v["state"].as_str()), (Some("T1"), Some("delivered")));
    let err = crate::client::await_spool(&dir, &bad, std::time::Duration::from_secs(1)).unwrap_err().to_string();
    assert!(err.contains("no task T9"), "{err}");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "requests and replies are cleaned up");
    let stop = crate::client::write_spool(&dir, "stop", &json!({})).unwrap();
    w.d.process_spool(w.now);
    assert!(crate::client::await_spool(&dir, &stop, std::time::Duration::from_secs(1)).is_err());
}

#[test]
fn spool_requests_time_out_without_a_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let id = crate::client::write_spool(dir.path(), "status", &json!({})).unwrap();
    let err = crate::client::await_spool(dir.path(), &id, std::time::Duration::from_millis(300)).unwrap_err().to_string();
    assert!(err.contains("did not answer"), "{err}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "the unanswered request is withdrawn");
}

#[test]
fn the_fingerprint_moves_on_writes_and_live_changes_only() {
    let mut w = world();
    w.up();
    w.all_idle();
    let a = w.d.fingerprint();
    w.ok(None, "status", json!({}));
    w.ok(None, "task.ls", json!({"all": true}));
    w.ok(None, "inbox", json!({}));
    assert_eq!(w.d.fingerprint(), a, "reads change nothing");
    w.status("engineer", "idle");
    assert_eq!(w.d.fingerprint(), a, "the same status again changes nothing");
    w.status("engineer", "working");
    let b = w.d.fingerprint();
    assert_ne!(b, a, "a status change is news");
    w.ok(Some("owner"), "task.new", json!({"to": "tester", "title": "x"}));
    assert_ne!(w.d.fingerprint(), b, "a write is news");
}

#[test]
fn answering_twice_is_harmless() {
    let mut w = world();
    w.up();
    w.all_idle();
    w.ok(Some("owner"), "ask", json!({"question": "Which DB?"}));
    w.ok(None, "answer", json!({"id": "Q1", "text": "Postgres"}));
    let again = w.ok(None, "answer", json!({"id": "Q1", "text": "MySQL"}));
    assert_eq!(again["already"], true);
    let answers: Vec<String> = w.sends_to("owner").into_iter().filter(|t| t.contains("<lila answer")).collect();
    assert_eq!(answers.len(), 1, "the owner hears one answer: {answers:?}");
    assert!(answers[0].contains("Postgres"));
    assert!(w.call(None, "answer", json!({"id": "Q9"})).is_err(), "an unknown item is still an error");
}

#[test]
fn new_settings_mark_running_members_until_restarted() {
    let mut w = world();
    w.up();
    w.all_idle();
    let t = w.ok(None, "task.new", json!({"to": "engineer", "title": "Fix it"}))["id"].as_str().unwrap().to_owned();
    w.ok(Some("engineer"), "task.start", json!({"id": t}));

    // The person gives the engineer another model and its own prompt.
    let toml = PROJECT.replace("id = \"engineer\"\nrole = \"engineer\"", "id = \"engineer\"\nrole = \"engineer\"\nmodel = \"claude-haiku-5-5\"\nprompt_file = \"prompts/engineer.md\"");
    std::fs::create_dir_all(w.repo.join(".lila/prompts")).unwrap();
    std::fs::write(w.repo.join(".lila/prompts/engineer.md"), "Be brief.").unwrap();
    std::fs::write(w.repo.join(".lila/project.toml"), toml).unwrap();
    let r = w.ok(None, "project.reload", json!({}));
    assert_eq!(r["stale"], json!(["engineer"]));
    let status = w.ok(None, "status", json!({}));
    let stale: Vec<&str> = status["members"].as_array().unwrap().iter().filter(|m| m["stale"] == true).map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(stale, ["engineer"]);
    assert!(w.call(Some("owner"), "member.restart", json!({"member": "engineer"})).is_err(), "only the person restarts");

    let old = w.terminal("engineer");
    w.ok(None, "member.restart", json!({"member": "engineer"}));
    assert!(w.d.term.kills.contains(&old));
    let run = w.d.term.runs.last().unwrap();
    let argv: Vec<&str> = run["argv"].as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
    assert!(argv.windows(2).any(|p| p == ["--model", "claude-haiku-5-5"]), "{argv:?}");
    let prompt = std::fs::read_to_string(argv[argv.iter().position(|a| *a == "--append-system-prompt-file").unwrap() + 1]).unwrap();
    assert!(prompt.starts_with("Be brief."), "{prompt}");
    assert_eq!(w.state(&t), TaskState::Queued, "its task goes back to the queue");
    assert!(w.ok(None, "project.reload", json!({}))["stale"].as_array().unwrap().is_empty());
    w.status("engineer", "idle");
    assert!(w.sends_to("engineer").iter().any(|s| s.contains(&t)), "and is delivered again");
}

#[test]
fn a_missing_member_prompt_file_is_an_error() {
    let toml = PROJECT.replace("id = \"tester\"\nrole = \"tester\"", "id = \"tester\"\nrole = \"tester\"\nprompt_file = \"prompts/tester.md\"");
    let mut w = world_with(&toml);
    std::fs::create_dir_all(w.repo.join("src")).unwrap();
    let e = w.call(None, "project.up", json!({})).unwrap_err();
    assert!(format!("{e:#}").contains("read prompt"), "{e:#}");
}

#[test]
fn work_from_another_chain_starts_a_new_session() {
    let mut w = world();
    w.up();
    w.all_idle();
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Build login"}));
    w.ok(Some("engineer"), "task.done", json!({"id": "T1", "summary": "ok"}));
    // Rework in the same chain keeps the session (it remembers what it did).
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Fix login typo", "parent": "T1"}));
    assert_eq!(w.state("T2"), TaskState::Delivered);
    assert!(w.d.term.kills.is_empty());
    w.ok(Some("engineer"), "task.done", json!({"id": "T2", "summary": "ok"}));

    // Unrelated work: the engineer's CLI starts again, and T3 waits for the new session.
    let old = w.terminal("engineer");
    let t3 = w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Build search"}));
    assert_eq!(t3["state"], "queued");
    assert_eq!(w.d.term.kills, std::slice::from_ref(&old));
    assert_ne!(w.terminal("engineer"), old);
    assert!(w.inbox_kinds().is_empty(), "a planned restart raises no alarm");
    assert!(!w.sends_to("owner").iter().any(|s| s.contains("engineer ended")));
    w.status("engineer", "idle");
    assert_eq!(w.state("T3"), TaskState::Delivered);
    assert!(w.sends_to("engineer").iter().any(|s| s.contains("<lila task=\"T3\"")));
    let events = w.d.store.events("shop", None, 50).unwrap();
    assert!(events.iter().any(|e| e.kind == "fresh_session"), "{events:?}");

    // The PM keeps its conversation with the person.
    w.ok(None, "task.new", json!({"to": "owner", "title": "Plan A"}));
    w.ok(Some("owner"), "task.done", json!({"id": "T4", "summary": "ok"}));
    w.ok(None, "task.new", json!({"to": "owner", "title": "Plan B"}));
    assert_eq!(w.state("T5"), TaskState::Delivered);
    assert_eq!(w.d.term.kills.len(), 1);
}

#[test]
fn fresh_sessions_can_be_turned_off() {
    let mut w = world_with(&format!("{PROJECT}\n[settings]\nfresh_session = false\n"));
    w.up();
    w.all_idle();
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "A"}));
    w.ok(Some("engineer"), "task.done", json!({"id": "T1", "summary": "ok"}));
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "B"}));
    assert_eq!(w.state("T2"), TaskState::Delivered);
    assert!(w.d.term.kills.is_empty());
}

#[test]
fn a_waiting_note_is_read_before_a_new_session() {
    let mut w = world();
    w.up();
    w.all_idle();
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "A"}));
    w.ok(Some("engineer"), "task.done", json!({"id": "T1", "summary": "ok"}));
    w.status("engineer", "blocked");
    // A note and unrelated work arrive while it sits in a dialog.
    w.d.store.push_outbox("shop", "engineer", "<lila note about=\"T1\">thanks</lila>", w.now).unwrap();
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "B"}));
    assert!(w.d.term.kills.is_empty());
    w.status("engineer", "idle");
    assert!(w.d.term.kills.is_empty(), "the note went in; the restart waits until it was read");
    assert!(w.sends_to("engineer").iter().any(|s| s.contains("thanks")));
    w.status("engineer", "working");
    w.status("engineer", "idle");
    assert_eq!(w.d.term.kills.len(), 1);
}

#[test]
fn a_removed_member_stops_and_its_tasks_are_cancelled() {
    let mut w = world();
    w.up();
    w.all_idle();
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Build"}));
    w.ok(Some("engineer"), "task.new", json!({"to": "tester", "title": "Check build", "parent": "T1"}));
    let tester = w.terminal("tester");
    std::fs::write(w.repo.join(".lila/project.toml"), PROJECT.replace("[[members]]\nid = \"tester\"\nrole = \"tester\"\n", "")).unwrap();
    let r = w.ok(None, "project.reload", json!({}));
    assert_eq!(r["removed"], json!(["tester"]));
    assert!(w.d.term.kills.contains(&tester));
    assert_eq!(w.state("T2"), TaskState::Cancelled);
    assert!(w.sends_to("engineer").iter().any(|s| s.contains("T2 \"Check build\" was cancelled: tester was removed")));
    assert!(w.sends_to("owner").iter().any(|s| s.contains("tester was removed from the team. Its open tasks were cancelled: T2")));
    assert!(!w.d.projects["shop"].live.contains_key("tester"));
    let status = w.ok(None, "status", json!({}));
    assert!(!status["members"].as_array().unwrap().iter().any(|m| m["id"] == "tester"));
}

#[test]
fn a_member_removed_while_lilad_was_down_is_cleaned_up() {
    let mut w = world();
    w.up();
    w.all_idle();
    let tester = w.terminal("tester");
    w.ok(None, "task.new", json!({"to": "tester", "title": "Check"}));
    std::fs::write(w.repo.join(".lila/project.toml"), PROJECT.replace("[[members]]\nid = \"tester\"\nrole = \"tester\"\n", "")).unwrap();
    // A new lilad on the same database.
    let paths = w.d.paths.clone();
    let mut d = Daemon::new(Store::open(&paths.db()).unwrap(), Fake::default(), paths, String::new(), PathBuf::from("/opt/lila/bin/lila"));
    assert!(d.term.kills.contains(&tester));
    assert_eq!(d.store.task("shop", "T1").unwrap().unwrap().state, TaskState::Cancelled);
    assert!(!d.store.members("shop").unwrap().iter().any(|m| m.id == "tester"));
    let again = Daemon::new(Store::open(&d.paths.db()).unwrap(), Fake::default(), d.paths.clone(), String::new(), PathBuf::from("/x"));
    assert!(again.term.kills.is_empty(), "only once");
    d.tick(w.now);
}

#[test]
fn reload_loads_a_project_whose_config_came_back() {
    let mut w = world();
    w.up();
    // Its config disappears and lilad starts again: the project is not loaded.
    let toml = w.repo.join(".lila/project.toml");
    std::fs::remove_file(&toml).unwrap();
    let paths = w.d.paths.clone();
    w.d = Daemon::new(Store::open(&paths.db()).unwrap(), Fake::default(), paths, String::new(), PathBuf::from("/opt/lila/bin/lila"));
    assert!(!w.d.projects.contains_key("shop"));
    // The window writes it again and asks lilad to read it; no member starts.
    std::fs::write(&toml, PROJECT).unwrap();
    let r = w.ok(None, "project.reload", json!({}));
    assert_eq!(r["loaded"], true);
    assert!(w.d.projects.contains_key("shop"));
    assert!(w.d.term.runs.is_empty());
    assert!(w.call(Some("owner"), "project.reload", json!({})).is_err(), "only the person");
}
