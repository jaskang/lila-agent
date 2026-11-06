use super::*;
use std::os::unix::fs::PermissionsExt;

#[derive(Default)]
struct Fake {
    runs: Vec<Value>,
    sends: Vec<(String, String, Option<String>)>,
    kills: Vec<String>,
    refuse_sends: bool,
    /// Live terminals left over from before this daemon loaded a project.
    existing: Vec<Value>,
}

impl Terminals for Fake {
    fn list(&mut self, project: &str) -> Result<Vec<Value>> {
        Ok(self.existing.iter().filter(|t| t["labels"]["lila.project"] == project).cloned().collect())
    }

    fn run(&mut self, req: Value) -> Result<String> {
        if self.existing.iter().any(|t| t["state"] == "live" && t["name"] == req["name"]) {
            bail!("a live terminal is already named {} (name_taken)", req["name"]);
        }
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
    let mut d = Daemon::new(store, Fake::default(), paths, bin.to_string_lossy().to_string(), PathBuf::from("/opt/lila/bin/lila"));
    // The CLIs' session records, here instead of the real ~/.claude and ~/.codex.
    d.sources = Sources { claude: root.join("claude-home"), codex: root.join("codex-home") };
    World { d, repo, _tmp: tmp, now: 1_000_000 }
}

fn world() -> World {
    world_with(PROJECT)
}

/// Members hand work on themselves (no flow), with these `[settings]` lines.
fn manual(settings: &str) -> String {
    format!("{PROJECT}\n[settings]\nflow = \"none\"\n{settings}")
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
    assert_eq!(argv[1..3], ["--model", "claude-sonnet-5-5"]);
    let flag = |f: &str| argv.iter().position(|a| *a == f).map(|i| argv[i + 1]).unwrap_or_else(|| panic!("{f} in {argv:?}"));
    assert_eq!(flag("--effort"), "high");
    // Lila picks the session id, so it knows where Claude Code records it.
    let sid = flag("--session-id");
    assert_eq!(w.d.store.session("shop", sid).unwrap().unwrap().member, "owner");
    // The PM's limits are the CLI's, not a line in its prompt.
    assert_eq!(flag("--tools"), "Read,Grep,Glob,Bash");
    let allow = argv.iter().position(|a| *a == "--allowedTools").expect("allowed rules");
    assert_eq!(argv[allow + 1], "Bash(lila:*)");
    let at = argv.iter().position(|a| *a == "--append-system-prompt-file").unwrap();
    let prompt = std::fs::read_to_string(argv[at + 1]).unwrap();
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
fn up_recovers_a_terminal_seen_before_the_project_was_loaded() {
    let mut w = world_with(&PROJECT.replace("id = \"engineer\"\nrole = \"engineer\"", "id = \"designer\"\nrole = \"designer\""));
    let terminal = json!({
        "address": "t-existing", "name": "lila-shop-designer", "state": "live",
        "cwd": w.repo, "labels": {"lila.project": "shop", "lila.member": "designer"},
        "reported": true, "status": {"state": "idle"}
    });
    w.d.term.existing.push(terminal.clone());
    w.d.on_terminal(&terminal, w.now);
    let result = w.ok(None, "project.up", json!({}));
    assert_eq!(w.terminal("designer"), "t-existing");
    assert!(result["running"].as_array().unwrap().iter().any(|m| m["member"] == "designer"));
    assert!(!w.d.term.runs.iter().any(|r| r["name"] == "lila-shop-designer"));
    assert!(w.d.term.kills.is_empty());
    assert_eq!(w.d.store.members("shop").unwrap().into_iter().find(|m| m.id == "designer").unwrap().terminal.as_deref(), Some("t-existing"));
    assert_eq!(w.d.projects["shop"].live["designer"].status, AgentStatus::Idle);
    let again = w.ok(None, "project.up", json!({}));
    assert!(again["started"].as_array().unwrap().is_empty());
    assert_eq!(again["running"].as_array().unwrap().len(), 4);
    assert_eq!(w.d.term.runs.len(), 3);
}

#[test]
fn up_does_not_adopt_a_name_collision_from_another_folder_or_member() {
    for mismatch in ["folder", "member", "project", "labels"] {
        let mut w = world();
        let mut terminal = json!({
            "address": "t-unrelated", "name": "lila-shop-owner", "state": "live",
            "cwd": w.repo, "labels": {"lila.project": "shop", "lila.member": "owner"}
        });
        match mismatch {
            "folder" => terminal["cwd"] = json!(w.repo.join("other")),
            "member" => terminal["labels"]["lila.member"] = json!("someone-else"),
            "project" => terminal["labels"]["lila.project"] = json!("another-project"),
            _ => terminal["labels"] = json!({}),
        }
        w.d.term.existing.push(terminal);
        let error = w.call(None, "project.up", json!({})).unwrap_err();
        assert!(format!("{error:#}").contains("name_taken"), "{mismatch}: {error:#}");
        assert!(w.d.term.kills.is_empty(), "{mismatch}");
        assert!(w.d.projects["shop"].live["owner"].terminal.is_none(), "{mismatch}");
    }
}

#[test]
fn up_starts_a_new_member_when_the_old_terminal_finished() {
    let mut w = world();
    w.d.term.existing.push(json!({
        "address": "t-finished", "name": "lila-shop-owner", "state": "finished",
        "cwd": w.repo, "labels": {"lila.project": "shop", "lila.member": "owner"}
    }));
    let result = w.up();
    assert_eq!(result["started"].as_array().unwrap().len(), 4);
    assert_ne!(w.terminal("owner"), "t-finished");
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
    let mut w = world_with(&manual("max_working = 1\n"));
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
fn the_persons_words_and_the_bar_reach_every_step() {
    let mut w = world_with(&manual(""));
    w.ok(None, "project.up", json!({"brief": "加一个登录页\n用邮箱和密码"}));
    w.all_idle();
    // Work under the person's task carries their words without the PM copying them.
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Build login", "parent": "T1"}));
    w.ok(Some("engineer"), "task.new", json!({"to": "tester", "title": "Verify login", "parent": "T2", "body": "I changed src/login.rs"}));
    w.ok(Some("engineer"), "task.done", json!({"id": "T2", "summary": "built"}));
    let to_tester = w.sends_to("tester").into_iter().find(|s| s.contains("<lila task=\"T3\"")).unwrap();
    assert!(to_tester.contains("The request this work serves (T1), the bar to meet:\nThe person's words:\n> 加一个登录页\n> 用邮箱和密码\n"), "{to_tester}");

    // A chain the PM starts from what the person said in its terminal.
    let t = w.ok(
        Some("owner"),
        "task.new",
        json!({"to": "engineer", "title": "Search", "quote": "能按标题搜索", "accept": ["empty query lists all"], "out_of_scope": ["fuzzy matching"]}),
    );
    assert_eq!(t["accept"], json!(["empty query lists all"]));
    let file = std::fs::read_to_string(w.repo.join(".lila/tasks/T4.md")).unwrap();
    assert!(file.contains("## The person's words\n\n> 能按标题搜索\n") && file.contains("## Out of scope\n\n- fuzzy matching\n"), "{file}");
    w.status("engineer", "idle");
    w.ok(Some("engineer"), "task.new", json!({"to": "tester", "title": "Verify search", "parent": "T4"}));
    w.ok(Some("tester"), "task.done", json!({"id": "T3", "summary": "PASS"}));
    w.status("tester", "idle"); // T5 is from another chain: it waits for the tester's new session
    let to_tester = w.sends_to("tester").into_iter().find(|s| s.contains("<lila task=\"T5\"")).unwrap();
    assert!(to_tester.contains("(T4), the bar to meet:\nThe person's words:\n> 能按标题搜索\nDone when:\n- empty query lists all\nOut of scope:\n- fuzzy matching\n"), "{to_tester}");
}

#[test]
fn decisions_and_unchecked_points_travel_with_done() {
    let mut w = world_with(&manual(""));
    w.ok(None, "project.up", json!({"brief": "Ship login"}));
    w.all_idle();
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Build login", "parent": "T1"}));
    w.ok(Some("engineer"), "task.done", json!({"id": "T2", "summary": "built", "decisions": ["tokens last 24h"], "unverified": ["load"]}));
    let note = w.sends_to("owner").into_iter().rfind(|n| n.contains("engineer finished T2")).expect("owner told");
    assert!(note.contains("built\n  Decided: tokens last 24h\n  Not checked: load"), "{note}");
    let shown = w.ok(Some("owner"), "task.show", json!({"id": "T2"}));
    assert_eq!(shown["decisions"], json!(["tokens last 24h"]));
    // The PM passes them on to the person, who signs off on them.
    w.ok(Some("owner"), "task.done", json!({"id": "T1", "summary": "login is in", "decisions": ["tokens last 24h"], "unverified": ["load"]}));
    let inbox = w.ok(None, "inbox", json!({}));
    assert_eq!(inbox[0]["text"], "T1 done: login is in\n  Decided: tokens last 24h\n  Not checked: load");
}

#[test]
fn a_pool_shares_work_and_rework_returns_to_its_author() {
    let mut w = world_with(&manual("").replace("id = \"engineer\"\nrole = \"engineer\"", "id = \"engineer\"\nrole = \"engineer\"\ncount = 2"));
    w.up();
    w.all_idle();
    w.status("engineer-2", "idle");
    let a = w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Login"}));
    let b = w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Search"}));
    assert_eq!((a["to"].as_str(), b["to"].as_str()), (Some("engineer"), Some("engineer-2")), "the second goes to the free one");
    // engineer-2 hands Search to the tester, who sends a fix back to "engineer":
    // it reaches engineer-2, which wrote the code, though engineer has less open.
    w.ok(Some("engineer"), "task.done", json!({"id": "T1", "summary": "ok"}));
    w.ok(Some("engineer-2"), "task.new", json!({"to": "tester", "title": "Verify search", "parent": "T2"}));
    w.ok(Some("engineer-2"), "task.done", json!({"id": "T2", "summary": "ok"}));
    let fix = w.ok(Some("tester"), "task.new", json!({"to": "engineer", "title": "Fix: empty query", "parent": "T3"}));
    assert_eq!(fix["to"], "engineer-2");
    // One named member is still one member.
    let direct = w.ok(Some("owner"), "task.new", json!({"to": "engineer-2", "title": "Docs"}));
    assert_eq!(direct["to"], "engineer-2");
}

#[test]
fn the_pm_hears_when_a_chain_under_its_task_ends_and_is_not_nudged_meanwhile() {
    let mut w = world_with(&manual(""));
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
    let mut w = world_with(&manual(""));
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
    let mut w = world_with(&manual(""));
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
    let mut w = world_with(&manual("max_chain_depth = 2\n"));
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
    let mut w = world_with(&manual("max_rounds = 1\nmax_working = 9\n"));
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
    let mut w = world_with(&manual(""));
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
    let mut w = world_with(&manual(""));
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

// ---------------- flows ----------------

fn with_check(cmd: &str, more: &str) -> String {
    format!("{PROJECT}\n[settings]\ncheck = \"{cmd}\"\n{more}")
}

impl World {
    /// The check lilad asked for finishes with `exit`.
    fn finish_check(&mut self, anchor: &str, exit: Option<i32>, tail: &str) {
        let jobs = self.d.take_checks();
        assert_eq!(jobs.iter().map(|j| j.anchor.as_str()).collect::<Vec<_>>(), [anchor], "one check for {anchor}");
        self.now += 1000;
        self.d.on_check_done("shop", anchor, &crate::check::CheckResult { exit, tail: tail.into() }, self.now);
    }

    fn run(&self, anchor: &str) -> Run {
        self.d.store.run("shop", anchor).unwrap().unwrap()
    }

    fn task(&self, id: &str) -> Task {
        self.d.store.task("shop", id).unwrap().unwrap()
    }
}

#[test]
fn the_code_flow_checks_verifies_and_reports_once() {
    let mut w = world_with(&with_check("cargo test", ""));
    w.ok(None, "project.up", json!({"brief": "加一个登录页"}));
    w.all_idle();
    w.ok(Some("owner"), "task.start", json!({"id": "T1"}));
    let t2 = w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Build login", "parent": "T1", "accept": ["401 on a bad password"]}));
    assert_eq!(t2["run"], "T2", "work from the PM to the engineer runs the default flow");
    let to_engineer = w.sends_to("engineer").into_iter().find(|s| s.contains("<lila task=\"T2\"")).unwrap();
    assert!(to_engineer.contains("When you finish, Lila runs `cargo test` and sends it to `tester` to verify"), "{to_engineer}");

    // Done: lilad runs the check itself. Nobody is woken meanwhile, nor nudged.
    w.ok(Some("engineer"), "task.done", json!({"id": "T2", "summary": "login added", "files": ["src/login.rs"], "decisions": ["bcrypt"]}));
    assert_eq!(w.run("T2").state, RunState::Checking);
    let job = w.d.take_checks().pop().unwrap();
    assert_eq!((job.cmd.as_str(), job.cwd.as_path()), ("cargo test", w.repo.as_path()));
    w.d.checks.push(job);
    assert!(w.sends_to("owner").iter().all(|n| !n.contains("<lila note")), "{:?}", w.sends_to("owner"));
    w.status("owner", "idle");
    w.now += 30 * 60_000;
    w.d.tick(w.now);
    assert!(w.sends_to("owner").iter().all(|n| !n.contains("went quiet")), "the PM waits on the flow");

    // Green: the tester gets a verification with the engineer's account and the bar.
    w.finish_check("T2", Some(0), "ok");
    let t3 = w.task("T3");
    assert_eq!((t3.from.as_str(), t3.to.as_str(), t3.parent.as_deref(), t3.title.as_str()), ("lilad", "tester", Some("T2"), "Verify: Build login"));
    assert!(t3.body.contains("engineer finished T2 \"Build login\": login added\n  Decided: bcrypt\nFiles: src/login.rs\nLila ran `cargo test`: it passed."), "{}", t3.body);
    let to_tester = w.sends_to("tester").into_iter().find(|s| s.contains("<lila task=\"T3\"")).unwrap();
    assert!(to_tester.contains("> 加一个登录页") && to_tester.contains("- 401 on a bad password"), "T1's words and T2's bar: {to_tester}");
    assert!(to_tester.contains("--verdict pass|fail"), "{to_tester}");

    let e = w.call(Some("tester"), "task.done", json!({"id": "T3", "summary": "PASS"})).unwrap_err();
    assert!(e.to_string().contains("--verdict pass|fail"), "{e}");
    w.ok(Some("tester"), "task.done", json!({"id": "T3", "summary": "PASS: 4 cases", "verdict": "pass", "unverified": ["rate limits"]}));
    assert_eq!(w.run("T2").state, RunState::Done);
    assert_eq!(w.task("T3").outcome.passed, Some(true));
    let notes: Vec<String> = w.sends_to("owner").into_iter().filter(|n| n.contains("<lila note")).collect();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("tester finished T3 \"Verify: Build login\": PASS: 4 cases\n  Not checked: rate limits"), "{}", notes[0]);
    assert!(notes[0].contains("Before it, engineer finished T2"), "{}", notes[0]);
    assert!(notes[0].contains("Nothing else is open under your T1"), "{}", notes[0]);
    assert!(w.ok(None, "status", json!({}))["runs"].as_array().unwrap().is_empty());
}

#[test]
fn failures_go_back_to_their_author_until_the_rounds_run_out() {
    let mut w = world_with(&with_check("make test", "max_rounds = 2\n"));
    w.up();
    w.all_idle();
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Search"}));
    w.ok(Some("engineer"), "task.done", json!({"id": "T1", "summary": "done"}));
    // A red check goes straight back to the engineer: no tester turn spent.
    w.finish_check("T1", Some(2), "error[E0308]: mismatched types");
    let fix = w.task("T2");
    assert_eq!((fix.to.as_str(), fix.parent.as_deref(), fix.title.as_str()), ("engineer", Some("T1"), "Fix: Search"));
    assert!(fix.body.contains("Lila ran `make test` after your work and it exited with 2") && fix.body.contains("mismatched types"), "{}", fix.body);
    assert_eq!(w.run("T1").round, 1);
    assert!(w.sends_to("tester").iter().all(|s| !s.contains("<lila task")));
    assert_eq!(w.ok(None, "status", json!({}))["runs"][0]["step"], "implement");

    w.ok(Some("engineer"), "task.done", json!({"id": "T2", "summary": "fixed the types"}));
    w.finish_check("T1", Some(0), "");
    assert_eq!(w.task("T3").to, "tester");
    w.ok(Some("tester"), "task.done", json!({"id": "T3", "summary": "FAIL: empty query crashes", "verdict": "fail"}));
    let fix = w.task("T4");
    assert_eq!((fix.to.as_str(), fix.parent.as_deref()), ("engineer", Some("T3")));
    assert!(fix.body.contains("tester found a failure in T3") && fix.body.contains("empty query crashes"), "{}", fix.body);
    assert_eq!(w.run("T1").round, 2);

    // A third failure is one round too many: the PM decides, nothing more goes back.
    w.ok(Some("engineer"), "task.done", json!({"id": "T4", "summary": "handled empty"}));
    w.finish_check("T1", Some(1), "still broken");
    assert_eq!(w.run("T1").state, RunState::Failed);
    assert!(w.d.store.task("shop", "T6").unwrap().is_none());
    let note = w.sends_to("owner").into_iter().rfind(|n| n.contains("still fails after 2 rounds")).expect("the PM hears");
    assert!(note.contains("T1 \"Search\" (code flow)") && note.contains("still broken"), "{note}");
}

#[test]
fn members_do_not_hand_on_inside_a_flow() {
    let mut w = world();
    w.up();
    w.all_idle();
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Search"}));
    let e = w.call(Some("engineer"), "task.new", json!({"to": "tester", "title": "Verify", "parent": "T1"})).unwrap_err();
    assert!(e.to_string().contains("finish T1 with `lila task done T1`: Lila sends it to `tester` to verify"), "{e}");
    // Help from outside the flow is still fine.
    w.ok(Some("owner"), "task.new", json!({"to": "architect", "title": "Review the index", "parent": "T1", "flow": "none"}));
    // Without a check command the verification comes next.
    w.ok(Some("engineer"), "task.done", json!({"id": "T1", "summary": "done"}));
    assert!(w.d.take_checks().is_empty());
    assert_eq!(w.task("T3").title, "Verify: Search");
    let e = w.call(Some("tester"), "task.new", json!({"to": "engineer", "title": "Fix", "parent": "T3"})).unwrap_err();
    assert!(e.to_string().contains("--verdict fail"), "{e}");
    let e = w.call(Some("architect"), "task.done", json!({"id": "T2", "summary": "ok", "verdict": "pass"})).unwrap_err();
    assert!(e.to_string().contains("asks for no verdict"), "{e}");
}

#[test]
fn which_work_runs_a_flow() {
    let mut w = world();
    w.up();
    w.all_idle();
    assert!(w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Docs", "flow": "none"}))["run"].is_null());
    assert!(w.ok(Some("owner"), "task.new", json!({"to": "architect", "title": "Design"}))["run"].is_null(), "not the flow's first step");
    let e = w.call(Some("owner"), "task.new", json!({"to": "architect", "title": "x", "flow": "code"})).unwrap_err();
    assert!(e.to_string().contains("starts with `engineer`"), "{e}");
    // quick: no tester; with no check command the work is simply done.
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Rename", "flow": "quick"}));
    w.ok(Some("engineer"), "task.done", json!({"id": "T1", "summary": "docs"}));
    w.status("engineer", "idle");
    w.ok(Some("engineer"), "task.done", json!({"id": "T3", "summary": "renamed"}));
    assert_eq!(w.run("T3").state, RunState::Done);
    assert!(w.sends_to("owner").iter().any(|n| n.contains("engineer finished T3 \"Rename\": renamed")));
    // Work the person gives the engineer: the inbox hears once the flow passed.
    w.status("engineer", "idle");
    w.ok(None, "task.new", json!({"to": "engineer", "title": "Fix the footer"}));
    w.status("engineer", "idle");
    w.ok(Some("engineer"), "task.done", json!({"id": "T4", "summary": "fixed"}));
    assert!(w.inbox_kinds().is_empty(), "verification first");
    w.ok(Some("tester"), "task.done", json!({"id": "T5", "summary": "PASS", "verdict": "pass"}));
    let inbox = w.ok(None, "inbox", json!({}));
    assert_eq!(inbox[0]["text"], "T4 done, code flow passed: fixed\ntester T5: PASS");
}

#[test]
fn a_check_under_way_runs_again_after_a_restart() {
    let mut w = world_with(&with_check("true", ""));
    w.up();
    w.all_idle();
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Search"}));
    w.ok(Some("engineer"), "task.done", json!({"id": "T1", "summary": "done"}));
    let paths = w.d.paths.clone();
    let mut d2 = Daemon::new(Store::open(&paths.db()).unwrap(), Fake::default(), paths, w.d.path_env.clone(), PathBuf::from("/x/lila"));
    assert_eq!(d2.take_checks().len(), 1);
    // And a real one, end to end through the runner.
    let job = w.d.take_checks().pop().unwrap();
    let r = crate::check::run(&job);
    w.d.on_check_done("shop", "T1", &r, w.now + 1000);
    assert_eq!(w.task("T2").title, "Verify: Search");
    assert!(w.task("T2").body.contains("Lila ran `true`: it passed."));
}

// ---------------- sessions ----------------

impl World {
    /// The member's current session, as the store has it.
    fn session_of(&self, member: &str) -> SessionRow {
        let id = self.d.projects["shop"].live[member].session.clone().unwrap_or_else(|| panic!("{member} has no known session"));
        self.d.store.session("shop", &id).unwrap().unwrap()
    }

    /// Its CLI's record says the latest turn read `tokens`.
    fn set_context(&self, member: &str, tokens: u64) {
        let file = PathBuf::from(self.session_of(member).file.unwrap());
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let line = json!({"type": "assistant", "message": {"usage": {"input_tokens": 1, "cache_read_input_tokens": tokens - 1, "output_tokens": 5}}});
        std::fs::write(file, format!("{line}\n")).unwrap();
    }

    fn last_argv(&self) -> Vec<String> {
        self.d.term.runs.last().unwrap()["argv"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect()
    }
}

#[test]
fn another_chain_keeps_a_small_session_and_returns_to_the_one_that_did_the_work() {
    let mut w = world_with(&manual(""));
    w.up();
    w.all_idle();
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Login"}));
    let first = w.session_of("engineer").id;
    w.set_context("engineer", 10_000);
    w.ok(Some("engineer"), "task.done", json!({"id": "T1", "summary": "ok"}));

    // Small context: the same session takes work from another chain.
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Search"}));
    assert_eq!(w.state("T2"), TaskState::Delivered);
    assert!(w.d.term.kills.is_empty());
    w.ok(Some("engineer"), "task.done", json!({"id": "T2", "summary": "ok"}));

    // Big context: a new session.
    w.set_context("engineer", 90_000);
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Export"}));
    assert_eq!(w.d.term.kills.len(), 1);
    let second = w.session_of("engineer").id;
    assert_ne!(second, first);
    assert!(w.last_argv().contains(&second), "started with --session-id {second}");
    w.status("engineer", "idle");
    w.ok(Some("engineer"), "task.done", json!({"id": "T3", "summary": "ok"}));

    // Rework on Search: back to the session that wrote it.
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Search: fix paging", "parent": "T2"}));
    assert_eq!(w.d.term.kills.len(), 2);
    let argv = w.last_argv();
    let at = argv.iter().position(|a| a == "--resume").expect("resumed");
    assert_eq!(argv[at + 1], first);
    assert!(!argv.contains(&"--session-id".to_string()));
    assert_eq!(w.session_of("engineer").id, first);
    w.status("engineer", "idle");
    assert_eq!(w.state("T4"), TaskState::Delivered);
    let kinds: Vec<String> = w.d.store.events("shop", None, 100).unwrap().into_iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&"fresh_session".to_string()) && kinds.contains(&"resumed_session".to_string()), "{kinds:?}");

    // What a session did survives a lilad restart.
    let paths = w.d.paths.clone();
    let d2 = Daemon::new(Store::open(&paths.db()).unwrap(), Fake::default(), paths, w.d.path_env.clone(), PathBuf::from("/x/lila"));
    assert_eq!(d2.projects["shop"].live["engineer"].chain.as_deref(), Some("T2"));
}

#[test]
fn codex_sessions_are_found_once_their_rollout_appears() {
    let mut w = world_with(&manual(""));
    w.up();
    w.all_idle();
    assert!(w.d.projects["shop"].live["tester"].session.is_none(), "codex picks its own id");
    let cwd = w.repo.to_string_lossy().to_string();
    let day = w.d.sources.codex.join("sessions/2026/11/06");
    std::fs::create_dir_all(&day).unwrap();
    let prompt = format!("## Your team (Lila)\n\nProject: shop, working directory: {cwd}\nYou are `tester` (测试工程师).\n");
    let lines = [
        json!({"type": "session_meta", "payload": {"id": "cx-1", "cwd": cwd}}),
        json!({"type": "response_item", "payload": {"role": "developer", "content": [{"type": "input_text", "text": prompt}]}}),
        json!({"type": "event_msg", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 31000}}}}),
    ];
    std::fs::write(day.join("rollout-x-cx-1.jsonl"), lines.map(|l| l.to_string()).join("\n")).unwrap();
    w.d.tick(w.now);
    assert_eq!(w.session_of("tester").id, "cx-1");
    assert_eq!(w.d.store.members("shop").unwrap().iter().find(|m| m.id == "tester").unwrap().session.as_deref(), Some("cx-1"));
    assert_eq!(w.d.context_of("shop", Some("cx-1")), Some(31_000));
}

#[test]
fn a_pm_with_a_big_context_starts_again_from_a_digest() {
    let mut w = world_with(&manual(""));
    w.ok(None, "project.up", json!({"brief": "Ship login"}));
    w.all_idle();
    w.ok(Some("owner"), "task.start", json!({"id": "T1"}));
    w.ok(Some("owner"), "ask", json!({"question": "Which DB?", "options": ["Postgres", "SQLite"]}));
    w.ok(None, "answer", json!({"id": "Q1", "text": "Postgres"}));
    w.ok(Some("owner"), "task.new", json!({"to": "engineer", "title": "Build login", "parent": "T1"}));
    w.status("owner", "idle");
    w.set_context("owner", 150_000);
    let before = w.terminal("owner");

    // Idle only a moment: the person may be typing to it.
    w.d.tick(w.now);
    assert_eq!(w.terminal("owner"), before);
    // Ten quiet minutes later it starts again.
    w.now += 11 * 60_000;
    w.d.tick(w.now);
    assert_eq!(w.d.term.kills, std::slice::from_ref(&before));
    assert_ne!(w.terminal("owner"), before);
    assert!(w.inbox_kinds().is_empty(), "a planned restart raises no alarm");
    w.status("owner", "idle");
    let digest = w.sends_to("owner").into_iter().find(|s| s.starts_with("<lila digest>")).expect("digest typed in");
    assert!(digest.contains("- Q1 Which DB? → Postgres"), "{digest}");
    assert!(digest.contains("- T1 from human → owner [in_progress] \"Ship login\"\n    T2 → engineer [delivered] Build login"), "{digest}");
    let events = w.d.store.events("shop", None, 50).unwrap();
    assert!(events.iter().any(|e| e.kind == "pm_refreshed" && e.detail.contains("150k")), "{events:?}");

    // The new session is small: no second restart.
    w.now += 30 * 60_000;
    w.d.tick(w.now);
    assert_eq!(w.d.term.kills.len(), 1);
}
