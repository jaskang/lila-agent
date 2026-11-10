use super::*;
use lila_agent::{AgentEvent, Command, Item, ItemKind, ItemState, PermKind, PermOption, PermissionRequest, ToolKind};
use std::os::unix::fs::PermissionsExt;

#[derive(Default)]
struct Fake {
    runs: Vec<Value>,
    sends: Vec<(String, String, Option<String>)>,
    kills: Vec<String>,
    keys: Vec<(String, Vec<String>)>,
    refuse_sends: bool,
}

impl Terminals for Fake {
    fn list(&mut self, _project: &str) -> Result<Vec<Value>> {
        Ok(Vec::new())
    }

    fn run(&mut self, req: Value) -> Result<String> {
        self.runs.push(req);
        Ok(format!("t{}", self.runs.len()))
    }
    fn send(&mut self, terminal: &str, text: &str, only_if: Option<&str>) -> Result<()> {
        if self.refuse_sends {
            bail!("unavailable");
        }
        // Like hc: `if` takes `idle` or `working` only.
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
    fn keys(&mut self, terminal: &str, keys: &[&str]) -> Result<()> {
        self.keys.push((terminal.into(), keys.iter().map(|k| k.to_string()).collect()));
        Ok(())
    }
}

/// A stand-in host: what lilad sends it, and what was read so far.
type StandIn = (std::sync::mpsc::Receiver<String>, Vec<Command>);

struct World {
    d: Daemon<Fake>,
    repo: PathBuf,
    _tmp: tempfile::TempDir,
    now: i64,
    /// Stand-in hosts of Codex and ACP threads (`lila host …`): the commands lilad sent them.
    hosts: std::cell::RefCell<BTreeMap<String, StandIn>>,
}

fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let paths = Paths { config: root.join("config"), data: root.join("data"), state: root.join("state"), isolated: true };
    paths.ensure().unwrap();
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for prog in ["claude", "codex", "omp"] {
        let p = bin.join(prog);
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let repo = std::fs::canonicalize(root).unwrap().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    let store = Store::open(&paths.db()).unwrap();
    let mut d = Daemon::new(store, Fake::default(), paths, bin.to_string_lossy().to_string(), PathBuf::from("/opt/lila/bin/lila"));
    // The CLIs' session records, here instead of the real ~/.claude and ~/.codex.
    d.sources = Sources { claude: root.join("claude-home"), codex: root.join("codex-home") };
    World { d, repo, _tmp: tmp, now: 1_000_000, hosts: Default::default() }
}

impl World {
    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.now += 1000;
        self.d.handle(method, &params, self.now)
    }

    fn ok(&mut self, method: &str, params: Value) -> Value {
        self.call(method, params).unwrap_or_else(|e| panic!("{method}: {e:#}"))
    }

    /// The project `shop` in the repo folder, added.
    fn project(&mut self) -> Value {
        let repo = self.repo.to_string_lossy().to_string();
        self.ok("project.add", json!({"cwd": repo, "name": "shop"}))
    }

    fn new_thread(&mut self, params: Value) -> String {
        let mut p = json!({"project": "shop"});
        for (k, v) in params.as_object().unwrap() {
            p[k] = v.clone();
        }
        self.ok("thread.new", p)["id"].as_str().unwrap().to_owned()
    }

    fn terminal(&self, tid: &str) -> String {
        self.d.store.thread(tid).unwrap().unwrap().terminal.unwrap()
    }

    fn hosted(&self, tid: &str) -> bool {
        self.d.store.thread(tid).unwrap().unwrap().engine.hosted()
    }

    /// A hosted thread's host attaches when it first reports (as `lila host` does once running).
    fn attach(&mut self, tid: &str) {
        if self.hosted(tid) && !self.hosts.borrow().contains_key(tid) {
            let (tx, rx) = std::sync::mpsc::channel();
            self.hosts.borrow_mut().insert(tid.to_owned(), (rx, Vec::new()));
            self.d.host_attach(tid, tx, self.now);
        }
    }

    /// The commands a hosted thread's host got so far.
    fn commands(&self, tid: &str) -> Vec<Command> {
        let mut hosts = self.hosts.borrow_mut();
        let Some((rx, got)) = hosts.get_mut(tid) else { return Vec::new() };
        got.extend(rx.try_iter().filter_map(|l| serde_json::from_str(&l).ok()));
        got.clone()
    }

    fn status(&mut self, tid: &str, word: &str) {
        self.now += 1000;
        self.attach(tid);
        let t = self.terminal(tid);
        let ev = json!({"jsonrpc":"2.0","method":"event","params":{"seq":1,"type":"status","terminal":t,"data":{"status":{"app":"x","state":word},"reported":true,"records":{}}}});
        self.d.on_event(&ev, self.now);
    }

    /// What a thread was given: typed into its terminal, or its host's prompts.
    fn sends_to(&self, tid: &str) -> Vec<String> {
        if self.hosted(tid) {
            return self.commands(tid).into_iter().filter_map(|c| match c {
                Command::Prompt { text, .. } => Some(text),
                _ => None,
            }).collect();
        }
        let t = self.terminal(tid);
        self.d.term.sends.iter().filter(|(to, _, _)| *to == t).map(|(_, text, _)| text.clone()).collect()
    }

    fn state(&mut self, tid: &str) -> String {
        self.ok("thread.show", json!({"thread": tid}))["state"].as_str().unwrap().to_owned()
    }

    fn events(&mut self, tid: &str) -> Vec<String> {
        self.ok("thread.log", json!({"thread": tid}))
            .as_array()
            .unwrap()
            .iter()
            .map(|e| format!("{} {}", e["kind"].as_str().unwrap(), e["detail"].as_str().unwrap()))
            .collect()
    }

    fn host_event(&mut self, tid: &str, ev: AgentEvent) {
        self.now += 1000;
        self.d.host_event(tid, ev, self.now);
    }

    /// A Claude Code hook event for a thread.
    fn hook(&mut self, tid: &str, event: &str, payload: Value) -> Value {
        self.ok("hook", json!({"thread": tid, "event": event, "payload": payload}))
    }
}

fn argv(w: &World, run: usize) -> Vec<String> {
    w.d.term.runs[run]["argv"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect()
}

fn ask(id: &str, name: &str, command: &str) -> PermissionRequest {
    PermissionRequest {
        name: name.into(),
        command: Some(command.into()),
        ..PermissionRequest::new(
            id,
            ToolKind::Execute,
            command,
            "",
            vec![
                PermOption { id: "accept".into(), label: "允许".into(), kind: PermKind::Allow },
                PermOption { id: "acceptForSession".into(), label: "本会话都允许".into(), kind: PermKind::AllowAlways },
                PermOption { id: "decline".into(), label: "拒绝".into(), kind: PermKind::Reject },
            ],
        )
    }
}

fn edit_ask(id: &str, path: &str) -> PermissionRequest {
    PermissionRequest { name: "apply_patch".into(), paths: vec![path.into()], ..PermissionRequest { tool: ToolKind::Edit, ..ask(id, "apply_patch", "") } }
}

// ---------------- projects ----------------

#[test]
fn a_project_is_a_folder_with_defaults_that_new_threads_start_with() {
    let mut w = world();
    let p = w.project();
    assert_eq!(p["id"], "shop");
    assert_eq!(p["created_config"], true);
    assert_eq!(p["defaults"], json!({"engine": "claude", "model": "claude-sonnet-5-5", "effort": null, "permission": "full"}), "full auto unless the project says otherwise");
    assert!(w.repo.join(".lila/project.toml").is_file());
    // Added twice: the same project.
    assert_eq!(w.project()["created_config"], false);
    assert_eq!(w.ok("project.list", json!({})).as_array().unwrap().len(), 1);
    // By a folder inside it.
    let sub = w.repo.join("src").to_string_lossy().to_string();
    assert_eq!(w.ok("project.show", json!({"cwd": sub}))["id"], "shop");
    assert!(w.call("project.show", json!({"cwd": "/nowhere"})).is_err());
    assert!(w.call("thread.list", json!({"project": "nope"})).is_err());

    // The defaults are written to the file and used.
    let p = w.ok("project.set", json!({"project": "shop", "title": "The Shop", "defaults": {"engine": "codex", "model": "gpt-6.1-sol", "effort": "low", "permission": "confirm"}}));
    assert_eq!((p["label"].as_str(), p["defaults"]["engine"].as_str(), p["defaults"]["permission"].as_str()), (Some("The Shop"), Some("codex"), Some("confirm")));
    let on_disk = crate::project::load(&w.repo).unwrap();
    assert_eq!((on_disk.defaults.engine, on_disk.defaults.effort.as_deref(), on_disk.defaults.permission), (Runtime::Codex, Some("low"), Tier::Confirm));
    let tid = w.new_thread(json!({}));
    let t = w.ok("thread.show", json!({"thread": tid}));
    assert_eq!((t["engine"].as_str(), t["model"].as_str(), t["effort"].as_str(), t["permission"].as_str()), (Some("codex"), Some("gpt-6.1-sol"), Some("low"), Some("confirm")));
    // Told otherwise, a thread differs; another engine does not inherit the project's model.
    let tid = w.new_thread(json!({"engine": "claude", "permission": "read_only"}));
    let t = w.ok("thread.show", json!({"thread": tid}));
    assert_eq!((t["engine"].as_str(), t["model"].as_str(), t["permission"].as_str()), (Some("claude"), Some("claude-sonnet-5-5"), Some("read_only")));
    assert!(w.call("project.set", json!({"project": "shop", "defaults": {"permission": "yolo"}})).is_err());
    assert!(w.call("project.set", json!({"project": "shop", "defaults": {"engine": "gemini"}})).is_err());
}

#[test]
fn removing_a_project_stops_its_threads_and_forgets_them() {
    let mut w = world();
    w.project();
    let a = w.new_thread(json!({"text": "hi"}));
    let b = w.new_thread(json!({}));
    w.ok("thread.stop", json!({"thread": b}));
    let r = w.ok("project.remove", json!({"project": "shop"}));
    assert_eq!(r["stopped"], json!([a]), "only the running one had a terminal to stop");
    assert_eq!(w.d.term.kills.len(), 2, "the thread stopped before and the one running now");
    assert!(w.d.store.thread(&a).unwrap().is_none() && w.d.store.projects().unwrap().is_empty());
    assert!(w.repo.join(".lila/project.toml").is_file(), "the folder is not touched");
    assert!(w.call("thread.list", json!({"project": "shop"})).is_err());
}

// ---------------- a thread's life ----------------

#[test]
fn a_claude_thread_starts_hooked_and_asks_in_manual_mode() {
    let mut w = world();
    w.project();
    let tid = w.new_thread(json!({"text": "make a file", "model": "claude-haiku-5-5", "effort": "low"}));
    let t = w.d.store.thread(&tid).unwrap().unwrap();
    let session = t.session.clone().expect("Claude Code takes the id Lila picks");
    let run = &w.d.term.runs[0];
    assert_eq!(run["labels"], json!({"lila.project": "shop", "lila.thread": tid}));
    assert_eq!(run["name"], format!("lila-{tid}"));
    assert_eq!(run["cwd"], w.repo.to_string_lossy().as_ref());
    assert_eq!((run["env"]["LILA_THREAD"].as_str(), run["env"]["LILA_PROJECT"].as_str()), (Some(tid.as_str()), Some("shop")));
    let args = argv(&w, 0);
    assert!(args[0].ends_with("/bin/claude"), "{args:?}");
    assert_eq!(args[1..9], ["--model", "claude-haiku-5-5", "--session-id", session.as_str(), "--effort", "low", "--permission-mode", "manual"]);
    let at = args.iter().position(|a| a == "--settings").expect("--settings");
    let settings: Value = serde_json::from_str(&std::fs::read_to_string(&args[at + 1]).unwrap()).unwrap();
    assert_eq!(settings["hooks"]["PermissionRequest"][0]["hooks"][0]["command"], "'/opt/lila/bin/lila' hook PermissionRequest");
    assert_eq!(settings["hooks"]["PermissionRequest"][0]["hooks"][0]["timeout"], 600);

    // Starting: the first message waits for the prompt, and goes where it is idle.
    assert_eq!(w.state(&tid), "starting");
    assert!(w.d.term.sends.is_empty());
    w.status(&tid, "idle");
    assert_eq!(w.d.term.sends, [(w.terminal(&tid), "make a file".into(), Some("idle".into()))]);
    assert_eq!(w.state(&tid), "idle");
    assert_eq!(t.title, "make a file");
    // A message while it works goes in too (Claude queues it); one at a time as a unit.
    w.status(&tid, "working");
    assert_eq!(w.state(&tid), "running");
    w.ok("thread.send", json!({"thread": tid, "text": "and a second one"}));
    assert_eq!(w.sends_to(&tid), ["make a file", "and a second one"]);
    assert_eq!(w.d.term.sends[1].2, None, "not guarded while it works");
    assert!(w.call("thread.send", json!({"thread": tid, "text": " "})).is_err(), "nothing to send");
}

#[test]
fn a_codex_thread_runs_through_its_host_under_the_tiers_sandbox() {
    let mut w = world();
    w.project();
    let tid = w.new_thread(json!({"engine": "codex", "model": "gpt-6.1-sol", "effort": "low", "text": "hello", "permission": "confirm"}));
    let args = argv(&w, 0);
    assert_eq!(args[0], "/opt/lila/bin/lila");
    assert_eq!(args[1..6], ["host", "codex", "--program", args[4].as_str(), "--model"]);
    assert!(args[4].ends_with("/bin/codex"), "{args:?}");
    assert_eq!(args[6..], ["gpt-6.1-sol", "--effort", "low", "--sandbox", "workspace-write", "--approval", "untrusted"]);
    assert_eq!(w.d.store.thread(&tid).unwrap().unwrap().session, None, "Codex picks its own");
    // Nothing goes before the host is attached; then the first message does.
    assert!(w.sends_to(&tid).is_empty() && w.d.term.sends.is_empty());
    w.status(&tid, "working");
    assert_eq!(w.sends_to(&tid), ["hello"]);
    assert!(w.d.term.sends.is_empty(), "never typed into the terminal");
    w.host_event(&tid, AgentEvent::Session { id: "th1".into() });
    assert_eq!(w.ok("thread.show", json!({"thread": tid}))["session"], "th1");
    // Items are stored and read back by seq.
    let said = Item { id: "m1".into(), turn: None, at_ms: 5, state: ItemState::Done, kind: ItemKind::AgentMessage { text: "hi there".into() } };
    w.host_event(&tid, AgentEvent::Item { item: said });
    let r = w.ok("thread.read", json!({"thread": tid}));
    assert_eq!((r["items"][0]["text"].as_str(), r["seq"].as_i64()), (Some("hi there"), Some(1)));
    assert!(w.ok("thread.read", json!({"thread": tid, "after": 1}))["items"].as_array().unwrap().is_empty());
    assert_eq!(r["thread"]["caps"]["tui"], false);
    // Interrupt goes to the host.
    w.ok("thread.interrupt", json!({"thread": tid}));
    assert_eq!(w.commands(&tid).last(), Some(&Command::Interrupt));
    // omp is an ACP agent behind the same host.
    let omp = w.new_thread(json!({"engine": "omp", "model": "deepseek/deepseek-v4"}));
    let args = argv(&w, 1);
    assert_eq!(args[1..3], ["host", "omp"]);
    assert_eq!(args[args.len() - 2..], ["--model", "deepseek/deepseek-v4"]);
    assert_eq!(w.state(&omp), "starting");
}

#[test]
fn an_ended_thread_is_resumed_on_its_session() {
    let mut w = world();
    w.project();
    let tid = w.new_thread(json!({"text": "first"}));
    let session = w.d.store.thread(&tid).unwrap().unwrap().session.unwrap();
    w.status(&tid, "idle");
    assert!(w.call("thread.resume", json!({"thread": tid})).is_err(), "it is running");
    // The engine ends by itself (hc says its terminal finished).
    let t = w.terminal(&tid);
    w.now += 1000;
    w.d.on_event(&json!({"method": "event", "params": {"type": "finished", "terminal": t, "data": {"reason": "exited"}}}), w.now);
    assert_eq!(w.state(&tid), "ended");
    let shown = w.ok("thread.show", json!({"thread": tid}));
    assert_eq!((shown["terminal"].is_null(), shown["ended_ms"].is_i64()), (true, true));
    assert!(w.call("thread.send", json!({"thread": tid, "text": "hello?"})).unwrap_err().to_string().contains("resume"));
    assert!(w.call("thread.stop", json!({"thread": tid})).is_err(), "nothing runs");

    // Its transcript is there: resumed on it, with a message.
    let file = sessions::claude_file(&w.d.live_sources(), &w.repo.to_string_lossy(), &session);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "").unwrap();
    let r = w.ok("thread.resume", json!({"thread": tid, "text": "go on"}));
    assert_eq!(r["state"], "starting");
    let args = argv(&w, 1);
    assert_eq!(args[args.iter().position(|a| a == "--resume").unwrap() + 1], session);
    assert!(!args.contains(&"--session-id".to_string()));
    w.status(&tid, "idle");
    assert_eq!(w.sends_to(&tid).last().map(String::as_str), Some("go on"));
    // Stopped by the person: ended too, and the terminal is killed.
    w.ok("thread.stop", json!({"thread": tid}));
    assert_eq!(w.d.term.kills.last(), Some(&"t2".to_string()));
    assert_eq!(w.state(&tid), "ended");
    // Ended before its engine wrote anything: started under the same id, not resumed (there is nothing to resume).
    std::fs::remove_file(&file).unwrap();
    w.ok("thread.resume", json!({"thread": tid}));
    let args = argv(&w, 2);
    assert_eq!(args[args.iter().position(|a| a == "--session-id").unwrap() + 1], session);

    // Codex goes on with its thread id.
    let cx = w.new_thread(json!({"engine": "codex", "model": "gpt-6.1-sol"}));
    w.status(&cx, "idle");
    w.host_event(&cx, AgentEvent::Session { id: "th9".into() });
    w.ok("thread.stop", json!({"thread": cx}));
    w.ok("thread.resume", json!({"thread": cx, "effort": "high", "permission": "read_only"}));
    let args = argv(&w, 4);
    assert!(args.windows(2).any(|p| p == ["--thread", "th9"]), "{args:?}");
    assert!(args.windows(2).any(|p| p == ["--effort", "high"]) && args.windows(2).any(|p| p == ["--sandbox", "read-only"]), "{args:?}");
}

#[test]
fn threads_list_by_activity_and_archived_ones_leave_the_list() {
    let mut w = world();
    w.project();
    let a = w.new_thread(json!({"title": "old"}));
    let b = w.new_thread(json!({"title": "new"}));
    w.status(&b, "idle");
    w.ok("thread.send", json!({"thread": b, "text": "hi"}));
    let ids = |w: &mut World, archived: bool| w.ok("thread.list", json!({"project": "shop", "archived": archived})).as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
    assert_eq!(ids(&mut w, false), [b.clone(), a.clone()], "the one just talked to first");
    w.ok("thread.archive", json!({"thread": a}));
    assert_eq!(ids(&mut w, false), std::slice::from_ref(&b));
    assert_eq!(ids(&mut w, true).len(), 2);
    assert_eq!(w.state(&a), "ended", "archiving ends its engine");
    w.ok("thread.unarchive", json!({"thread": a}));
    assert_eq!(ids(&mut w, false).len(), 2);
    w.ok("thread.rename", json!({"thread": a, "title": "renamed"}));
    assert_eq!(w.ok("thread.show", json!({"thread": a}))["title"], "renamed");
    // Everything, when no project is named.
    assert_eq!(w.ok("thread.list", json!({})).as_array().unwrap().len(), 2);
}

#[test]
fn lilad_finds_its_threads_again_after_a_restart() {
    let mut w = world();
    w.project();
    let alive = w.new_thread(json!({}));
    let lost = w.new_thread(json!({}));
    w.status(&alive, "idle");
    // A new lilad over the same database: they are presumed alive until herdr-core's snapshot says.
    let paths = w.d.paths.clone();
    let store = Store::open(&paths.db()).unwrap();
    let mut d = Daemon::new(store, Fake::default(), paths, w.d.path_env.clone(), PathBuf::from("/opt/lila/bin/lila"));
    let t_alive = w.d.store.thread(&alive).unwrap().unwrap().terminal.unwrap();
    let state = |d: &Daemon<Fake>, id: &str| d.thread_json(&d.store.thread(id).unwrap().unwrap(), 5_000_000)["state"].as_str().unwrap().to_owned();
    assert_eq!(state(&d, &alive), "starting", "alive, nothing known yet");
    d.reconcile(std::slice::from_ref(&t_alive), 5_000_000, 5_000_000);
    assert_eq!(state(&d, &alive), "starting");
    assert_eq!(state(&d, &lost), "ended", "its terminal is not in the snapshot");
    // Its status comes with the snapshot's record, labeled with the thread.
    d.on_terminal(&json!({"state": "live", "address": t_alive, "labels": {"lila.project": "shop", "lila.thread": alive}, "reported": true, "status": {"state": "working"}}), 5_000_001);
    assert_eq!(state(&d, &alive), "running");
}

// ---------------- tiers and permission requests ----------------

fn claude_ask(w: &mut World, tid: &str, tool: &str, input: Value) -> Value {
    w.hook(tid, "PermissionRequest", json!({"tool_name": tool, "tool_input": input, "cwd": w.repo.to_string_lossy()}))
}

#[test]
fn claude_requests_are_answered_by_the_threads_tier() {
    let mut w = world();
    w.project();
    let repo = w.repo.to_string_lossy().to_string();
    let tid = w.new_thread(json!({}));
    w.status(&tid, "idle");
    assert_eq!(w.ok("thread.show", json!({"thread": tid}))["permission"], "full");

    // Full auto: ordinary work goes without anyone being asked.
    assert_eq!(claude_ask(&mut w, &tid, "Bash", json!({"command": "cargo test"}))["decision"], "allow");
    assert_eq!(claude_ask(&mut w, &tid, "Write", json!({"file_path": format!("{repo}/src/a.rs")}))["decision"], "allow");
    assert!(w.events(&tid).iter().any(|e| e == "auto_allowed full · Bash: cargo test"), "{:?}", w.events(&tid));
    assert_eq!(w.state(&tid), "idle");
    // … but not a push, a write outside the folder, a pull request or computer use.
    for (tool, input) in [
        ("Bash", json!({"command": "git push origin main"})),
        ("Write", json!({"file_path": "/etc/hosts"})),
        ("Bash", json!({"command": "gh pr create --title x --body y"})),
        ("mcp__computer-use__left_click", json!({})),
    ] {
        let r = claude_ask(&mut w, &tid, tool, input.clone());
        let id = r["wait"].as_str().unwrap_or_else(|| panic!("{tool} {input} should wait: {r}")).to_owned();
        assert_eq!(w.state(&tid), "waiting");
        let read = w.ok("thread.read", json!({"thread": tid}));
        let card = &read["permissions"][0];
        assert!(card["why"].as_str().unwrap().starts_with("全自动档仍需确认"), "{card}");
        assert_eq!(w.ok("hook.answer", json!({"thread": tid, "id": id})), json!({"pending": true}));
        // A message into a waiting thread would answer its card: refused.
        assert!(w.call("thread.send", json!({"thread": tid, "text": "hey"})).is_err());
        // The person says yes, or no.
        let allow = tool == "Write";
        w.ok("thread.answer", json!({"thread": tid, "card": card["id"], "allow": allow}));
        let a = w.ok("hook.answer", json!({"thread": tid, "id": id}));
        assert_eq!(a["decision"], if allow { "allow" } else { "deny" });
        assert_eq!(w.state(&tid), "idle");
        assert!(w.call("thread.answer", json!({"thread": tid, "card": card["id"], "allow": true})).is_err(), "answered once");
    }

    // Read-only: reads go, changes are refused with the reason.
    w.ok("thread.permission", json!({"thread": tid, "permission": "read_only"}));
    assert_eq!(claude_ask(&mut w, &tid, "Read", json!({"file_path": "/etc/hosts"}))["decision"], "allow");
    let r = claude_ask(&mut w, &tid, "Edit", json!({"file_path": format!("{repo}/src/a.rs")}));
    assert_eq!(r["decision"], "deny");
    assert!(r["message"].as_str().unwrap().contains("read-only"), "{r}");
    assert_eq!(claude_ask(&mut w, &tid, "Bash", json!({"command": "touch x"}))["decision"], "deny");
    // Confirm: everything that changes asks.
    w.ok("thread.permission", json!({"thread": tid, "permission": "confirm"}));
    let r = claude_ask(&mut w, &tid, "Bash", json!({"command": "ls -la"}));
    let id = r["wait"].as_str().expect("asks for a command").to_owned();
    let card = w.ok("thread.read", json!({"thread": tid}))["permissions"][0].clone();
    assert_eq!((card["why"].as_str(), card["danger"].is_null()), (Some("改动前确认"), true));
    // Left to the terminal: the card is closed without an answer.
    w.ok("thread.answer", json!({"thread": tid, "card": card["id"], "dismiss": true}));
    assert_eq!(w.ok("hook.answer", json!({"thread": tid, "id": id})), json!({"decision": null}));
    // And back to full: what was asked stays, the next goes by itself.
    w.ok("thread.permission", json!({"thread": tid, "permission": "full"}));
    assert_eq!(claude_ask(&mut w, &tid, "Bash", json!({"command": "npm install"}))["decision"], "allow");
    assert!(w.call("thread.permission", json!({"thread": tid, "permission": "yolo"})).is_err());
    let log = w.events(&tid);
    assert!(log.iter().any(|e| e.starts_with("auto_denied read_only")) && log.iter().any(|e| e.starts_with("permission full → read_only")) && log.iter().any(|e| e.starts_with("asked ")), "{log:?}");
}

#[test]
fn codex_requests_are_answered_by_the_tier_and_a_switch_reaches_the_host() {
    let mut w = world();
    w.project();
    let tid = w.new_thread(json!({"engine": "codex", "model": "gpt-6.1-sol"}));
    w.status(&tid, "idle");
    let answer = |w: &World, id: &str| w.commands(&tid).into_iter().rev().find_map(|c| match c {
        Command::Answer { permission, option } if permission == id => Some(option),
        _ => None,
    });
    // Full: a command that needs the network or a folder goes; a push does not.
    w.host_event_ask(&tid, AgentEvent::Permission { request: ask("c1", "shell", "npm install") });
    assert_eq!(answer(&w, "c1").as_deref(), Some("accept"), "the first allow option: once, not for the session");
    w.host_event_ask(&tid, AgentEvent::Permission { request: ask("c2", "shell", "git push -u origin HEAD") });
    assert_eq!(answer(&w, "c2"), None);
    assert_eq!(w.state(&tid), "waiting");
    // Asked again (lilad came back, the host repeated it): still one card.
    w.host_event_ask(&tid, AgentEvent::Permission { request: ask("c2", "shell", "git push -u origin HEAD") });
    let read = w.ok("thread.read", json!({"thread": tid}));
    assert_eq!(read["permissions"].as_array().unwrap().len(), 1);
    assert_eq!(read["permissions"][0]["danger"], "git_push");
    w.ok("thread.answer", json!({"thread": tid, "permission": "c2", "option": "拒绝"}));
    assert_eq!(answer(&w, "c2").as_deref(), Some("decline"));
    // A patch outside the workspace asks; the request the turn took back closes its card.
    w.host_event_ask(&tid, AgentEvent::Permission { request: edit_ask("c3", "/Users/someone/.zshrc") });
    assert_eq!(w.ok("thread.read", json!({"thread": tid}))["permissions"][0]["danger"], "outside_workspace");
    w.host_event(&tid, AgentEvent::PermissionResolved { id: "c3".into() });
    assert!(w.ok("thread.read", json!({"thread": tid}))["permissions"].as_array().unwrap().is_empty());
    assert_eq!(w.state(&tid), "idle");

    // Read-only is a sandbox too: the host is told, and whatever it still asks to escape is declined.
    w.ok("thread.permission", json!({"thread": tid, "permission": "read_only"}));
    assert_eq!(w.commands(&tid).last(), Some(&Command::SetPermission { approval: "on-request".into(), sandbox: "read-only".into() }));
    w.host_event_ask(&tid, AgentEvent::Permission { request: ask("c4", "shell", "touch x") });
    assert_eq!(answer(&w, "c4").as_deref(), Some("decline"));
    w.ok("thread.permission", json!({"thread": tid, "permission": "confirm"}));
    assert_eq!(w.commands(&tid).last(), Some(&Command::SetPermission { approval: "untrusted".into(), sandbox: "workspace-write".into() }));
    w.host_event_ask(&tid, AgentEvent::Permission { request: ask("c5", "shell", "ls") });
    assert_eq!(answer(&w, "c5"), None, "confirm asks");
    w.ok("thread.permission", json!({"thread": tid, "permission": "full"}));
    assert_eq!(w.commands(&tid).last(), Some(&Command::SetPermission { approval: "on-request".into(), sandbox: "workspace-write".into() }));
}

impl World {
    /// A request from a host: lilad answers it, or opens a card.
    fn host_event_ask(&mut self, tid: &str, ev: AgentEvent) {
        self.host_event(tid, ev);
    }
}

// ---------------- conversation ----------------

#[test]
fn a_claude_threads_transcript_becomes_its_conversation() {
    let mut w = world();
    w.project();
    let tid = w.new_thread(json!({}));
    w.status(&tid, "idle");
    let session = w.d.store.thread(&tid).unwrap().unwrap().session.unwrap();
    let file = sessions::claude_file(&w.d.live_sources(), &w.repo.to_string_lossy(), &session);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let line = |v: Value| v.to_string() + "\n";
    let mut text = line(json!({"type": "user", "uuid": "u1", "timestamp": "2026-11-10T01:00:00.000Z", "message": {"role": "user", "content": "make a.txt"}}));
    // The reply being written shows before the transcript has it.
    w.hook(&tid, "MessageDisplay", json!({"message_id": "m1", "delta": "Creating "}));
    assert_eq!(w.ok("thread.read", json!({"thread": tid}))["writing"], "Creating ");
    text.push_str(&line(json!({"type": "assistant", "uuid": "a1", "timestamp": "2026-11-10T01:00:05.000Z", "message": {"id": "m1", "role": "assistant", "model": "claude-haiku-5-5", "content": [{"type": "text", "text": "Created a.txt"}]}})));
    std::fs::write(&file, text).unwrap();
    w.d.take_dirty();
    w.d.follow_transcripts();
    let r = w.ok("thread.read", json!({"thread": tid}));
    let kinds: Vec<&str> = r["items"].as_array().unwrap().iter().map(|i| i["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["user_message", "agent_message"]);
    assert!(r["writing"].is_null(), "the reply is in the transcript now");
    assert_eq!(r["thread"]["model"], "claude-haiku-5-5", "what it really runs on");
    // The watchers hear which thread changed, and how far its conversation goes.
    let (dirty, _) = w.d.take_dirty();
    assert_eq!(dirty, std::slice::from_ref(&tid));
    let ev = w.d.thread_event(&tid, 7).unwrap();
    assert_eq!((ev["event"].as_str(), ev["items_seq"].as_i64(), ev["state"].as_str()), (Some("thread"), Some(2), Some("idle")));
    // Nothing new: nothing changed.
    w.d.follow_transcripts();
    assert!(w.d.take_dirty().0.is_empty());
    // Activity moves the thread up the list.
    assert!(w.d.store.thread(&tid).unwrap().unwrap().active_ms > w.d.store.thread(&tid).unwrap().unwrap().created_ms);
}

#[test]
fn a_thread_that_waits_in_its_terminal_says_so() {
    let mut w = world();
    w.project();
    let tid = w.new_thread(json!({"text": "hi"}));
    // Claude Code asks "trust this folder?" and reports nothing.
    assert_eq!(w.state(&tid), "starting");
    w.now += 25_000;
    let t = w.d.store.thread(&tid).unwrap().unwrap();
    let j = w.d.thread_json(&t, w.now);
    assert_eq!((j["state"].as_str(), j["waiting"].as_str()), (Some("waiting"), Some("terminal")));
    // Blocked on a dialog it reports: the same.
    w.status(&tid, "blocked");
    let j = w.d.thread_json(&t, w.now);
    assert_eq!((j["state"].as_str(), j["waiting"].as_str()), (Some("waiting"), Some("terminal")));
    assert!(w.call("thread.send", json!({"thread": tid, "text": "x"})).is_err());
    w.status(&tid, "idle");
    assert_eq!(w.d.thread_json(&t, w.now)["waiting"], Value::Null);
}

#[test]
fn a_message_that_cannot_be_typed_yet_is_not_lost() {
    let mut w = world();
    w.project();
    let tid = w.new_thread(json!({"text": "first"}));
    w.d.term.refuse_sends = true;
    w.status(&tid, "idle");
    assert!(w.d.term.sends.is_empty());
    assert_eq!(w.d.store.outbox(&tid).unwrap().len(), 1, "kept for the next try");
    w.d.term.refuse_sends = false;
    w.now += 6000;
    w.d.tick(w.now);
    assert_eq!(w.sends_to(&tid), ["first"]);
    assert!(w.d.store.outbox(&tid).unwrap().is_empty());
}

#[test]
fn interrupting_presses_escape_only_while_a_claude_thread_works() {
    let mut w = world();
    w.project();
    let tid = w.new_thread(json!({}));
    w.status(&tid, "idle");
    let err = w.call("thread.interrupt", json!({"thread": tid})).unwrap_err();
    assert!(err.to_string().contains("not working"), "{err:#}");
    assert!(w.d.term.keys.is_empty(), "Esc at an idle prompt would clear what is typed there");
    w.status(&tid, "working");
    w.ok("thread.interrupt", json!({"thread": tid}));
    assert_eq!(w.d.term.keys, [(w.terminal(&tid), vec!["esc".to_string()])]);
}

#[test]
fn the_model_changes_where_the_engine_allows_it() {
    let mut w = world();
    w.project();
    let claude = w.new_thread(json!({}));
    w.status(&claude, "idle");
    assert!(w.call("thread.model", json!({"thread": claude, "model": "claude-haiku-5-5"})).is_err(), "its /model would save the person's defaults");
    let codex = w.new_thread(json!({"engine": "codex", "model": "gpt-6.1-sol"}));
    w.status(&codex, "idle");
    w.ok("thread.model", json!({"thread": codex, "model": "gpt-6.1-luna", "effort": "low"}));
    assert_eq!(w.commands(&codex)[1..], [Command::SetModel { model: "gpt-6.1-luna".into() }, Command::SetEffort { effort: "low".into() }]);
    assert_eq!(w.d.store.thread(&codex).unwrap().unwrap().model, "gpt-6.1-luna", "and on resume too");
    // An ended Claude Code thread can: it starts on it next time.
    w.ok("thread.stop", json!({"thread": claude}));
    w.ok("thread.model", json!({"thread": claude, "model": "claude-haiku-5-5"}));
    w.ok("thread.resume", json!({"thread": claude}));
    assert!(argv(&w, 2).windows(2).any(|p| p == ["--model", "claude-haiku-5-5"]));
}

#[test]
fn unknown_requests_and_threads_are_errors() {
    let mut w = world();
    w.project();
    assert!(w.call("task.new", json!({})).is_err());
    assert!(w.call("thread.show", json!({"thread": "t00000000"})).is_err());
    assert!(w.call("thread.new", json!({"project": "shop", "engine": "gemini"})).is_err());
    assert!(w.d.term.runs.is_empty(), "nothing was started");
    let status = w.ok("status", json!({}));
    assert_eq!(status["projects"], 1);
}
