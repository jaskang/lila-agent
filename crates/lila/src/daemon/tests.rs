use super::*;
use lila_agent::{AgentEvent, Command, HostStatus, Item, ItemKind, ItemState, PermKind, PermOption, PermissionRequest, Question, QuestionKind, Reply, SlashCommand, ToolKind};
use std::os::unix::fs::PermissionsExt;

/// A stand-in for the omp of every thread: what it was started with, what lilad sent it, and whether it was let go.
#[derive(Default)]
struct Fake {
    runs: Vec<Spawn>,
    rx: std::cell::RefCell<Vec<std::sync::mpsc::Receiver<Command>>>,
    got: std::cell::RefCell<Vec<Vec<Command>>>,
    refuse: bool,
}

impl Agents for Fake {
    fn start(&mut self, spawn: Spawn) -> Result<Started> {
        if self.refuse {
            bail!("no omp");
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.runs.push(spawn);
        self.rx.borrow_mut().push(rx);
        self.got.borrow_mut().push(Vec::new());
        Ok(Started { run: self.runs.len() as u64, commands: tx })
    }

    fn wait(&mut self) {}
}

impl Fake {
    /// Reads what each run was sent so far; for each, whether lilad let go of it (its omp is to end).
    fn drain(&self) -> Vec<bool> {
        use std::sync::mpsc::TryRecvError;
        self.rx
            .borrow()
            .iter()
            .zip(self.got.borrow_mut().iter_mut())
            .map(|(rx, got)| loop {
                match rx.try_recv() {
                    Ok(c) => got.push(c),
                    Err(TryRecvError::Empty) => break false,
                    Err(TryRecvError::Disconnected) => break true,
                }
            })
            .collect()
    }

    /// How many of the omp started were let go.
    fn ended(&self) -> usize {
        self.drain().into_iter().filter(|e| *e).count()
    }

    /// What a thread's omp (every run of it) was sent so far.
    fn commands(&self, tid: &str) -> Vec<Command> {
        self.drain();
        self.runs.iter().zip(self.got.borrow().iter()).filter(|(r, _)| r.thread == tid).flat_map(|(_, got)| got.clone()).collect()
    }
}

struct World {
    d: Daemon<Fake>,
    repo: PathBuf,
    _tmp: tempfile::TempDir,
    now: i64,
}

fn world() -> World {
    world_over(|_, _| {})
}

/// A world whose database `prepare` has written first, given the paths and the repo folder (an existing database from an earlier version).
fn world_over(prepare: impl FnOnce(&Paths, &Path)) -> World {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let paths = Paths { data: root.join("data"), state: root.join("state"), isolated: true };
    paths.ensure().unwrap();
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let p = bin.join("omp");
    std::fs::write(&p, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    let repo = std::fs::canonicalize(root).unwrap().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    prepare(&paths, &repo);
    let store = Store::open(&paths.db()).unwrap();
    let d = Daemon::new(store, Fake::default(), paths, bin.to_string_lossy().to_string());
    World { d, repo, _tmp: tmp, now: 1_000_000 }
}

impl World {
    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.now += 1000;
        let r = self.d.handle(method, &params, self.now);
        self.settle_checkpoints();
        r
    }

    /// The checkpoint jobs lilad queued run now, and the words that waited for one go (lilad's server runs them at once);
    /// the other jobs stay queued for `settle`.
    fn settle_checkpoints(&mut self) {
        loop {
            let (mine, other): (Vec<_>, Vec<_>) = self.d.take_jobs().into_iter().partition(|j| j.is_checkpoint());
            self.d.jobs.extend(other);
            if mine.is_empty() {
                return;
            }
            for job in mine {
                self.now += 10;
                let result = job.run();
                let _ = self.d.finish(job, result, self.now);
            }
        }
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

    /// The commands a thread's omp got so far.
    fn commands(&self, tid: &str) -> Vec<Command> {
        self.d.agents.commands(tid)
    }

    fn status(&mut self, tid: &str, word: &str) {
        self.status_raw(tid, word);
        self.settle_checkpoints();
    }

    /// The same, without running the checkpoint jobs it may queue.
    fn status_raw(&mut self, tid: &str, word: &str) {
        self.now += 1000;
        let status = match word {
            "idle" => HostStatus::Idle,
            "working" => HostStatus::Working,
            "done" => HostStatus::Done,
            "blocked" => HostStatus::Blocked { why: String::new() },
            "error" => HostStatus::Error { why: String::new() },
            other => panic!("no status {other}"),
        };
        let run = self.d.live.get(tid).map_or(0, |l| l.run);
        self.d.agent_fact(tid, run, Fact::Status(status), self.now);
    }

    /// omp of a thread is gone by itself.
    fn exited(&mut self, tid: &str, why: &str) {
        self.now += 1000;
        let run = self.d.live.get(tid).map_or(0, |l| l.run);
        self.d.agent_fact(tid, run, Fact::Exited(why.to_owned()), self.now);
    }

    /// What a thread was given: its omp's prompts.
    fn sends_to(&self, tid: &str) -> Vec<String> {
        self.commands(tid)
            .into_iter()
            .filter_map(|c| match c {
                Command::Prompt { text, .. } => Some(text),
                _ => None,
            })
            .collect()
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

    /// The jobs lilad queued itself (the summaries of changes, worktrees to remove), run now, with what follows from them.
    fn settle(&mut self) {
        loop {
            let jobs = self.d.take_jobs();
            if jobs.is_empty() {
                return;
            }
            for job in jobs {
                self.now += 10;
                let result = job.run();
                let _ = self.d.finish(job, result, self.now);
            }
        }
    }

    fn host_event(&mut self, tid: &str, ev: AgentEvent) {
        self.now += 1000;
        self.d.agent_event(tid, ev, self.now);
    }

    /// omp asks for `request`: the option lilad answered with at once, or none when a card is open.
    fn raise(&mut self, tid: &str, request: PermissionRequest) -> Option<String> {
        let id = request.id.clone();
        self.host_event(tid, AgentEvent::Permission { request });
        self.answer_to(tid, &id)
    }

    /// The option omp was told for a request, if it was.
    fn answer_to(&self, tid: &str, id: &str) -> Option<String> {
        self.commands(tid).into_iter().rev().find_map(|c| match c {
            Command::Answer { permission, option } if permission == id => Some(option),
            _ => None,
        })
    }

    /// What lilad decides for a command, as `allow` or `ask`; an `ask` is taken back (omp moved on), so no card stays.
    fn verdict(&mut self, tid: &str, command: &str) -> &'static str {
        let id = format!("v{}", self.now);
        match self.raise(tid, ask(&id, "shell", command)) {
            Some(_) => "allow",
            None => {
                self.host_event(tid, AgentEvent::PermissionResolved { id });
                "ask"
            }
        }
    }

    /// lilad is stopped and started again: a new one over the same database, in which the threads that were running are ended (their omp ended with it).
    fn restart(&mut self) {
        let paths = self.d.paths.clone();
        let store = Store::open(&paths.db()).unwrap();
        self.d = Daemon::new(store, Fake::default(), paths, self.d.path_env.clone());
        self.now += 5000;
    }
}

fn argv(w: &World, run: usize) -> Vec<String> {
    w.d.agents.runs[run].args.clone()
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
    PermissionRequest { name: "edit".into(), paths: vec![path.into()], ..PermissionRequest { tool: ToolKind::Edit, ..ask(id, "edit", "") } }
}

fn read_ask(id: &str, path: &str) -> PermissionRequest {
    PermissionRequest { name: "read".into(), command: None, paths: vec![path.into()], ..PermissionRequest { tool: ToolKind::Read, ..ask(id, "read", path) } }
}

// ---------------- projects ----------------

#[test]
fn a_project_is_a_folder_with_defaults_that_new_threads_start_with() {
    let mut w = world();
    let p = w.project();
    assert_eq!(p["id"], "shop");
    assert_eq!(p["created_config"], true);
    assert_eq!(p["defaults"], json!({"model": "", "permission": "full"}), "omp's own model, and full auto unless the project says otherwise");
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
    let p = w.ok("project.set", json!({"project": "shop", "title": "The Shop", "defaults": {"model": "deepseek/deepseek-v4", "permission": "confirm"}}));
    assert_eq!((p["label"].as_str(), p["defaults"]["model"].as_str(), p["defaults"]["permission"].as_str()), (Some("The Shop"), Some("deepseek/deepseek-v4"), Some("confirm")));
    let on_disk = crate::project::load(&w.repo).unwrap();
    assert_eq!((on_disk.defaults.model.as_str(), on_disk.defaults.permission), ("deepseek/deepseek-v4", Tier::Confirm));
    let tid = w.new_thread(json!({}));
    let t = w.ok("thread.show", json!({"thread": tid}));
    assert_eq!((t["model"].as_str(), t["permission"].as_str(), t["legacy"].is_null()), (Some("deepseek/deepseek-v4"), Some("confirm"), true));
    // Told otherwise, a thread differs.
    let tid = w.new_thread(json!({"model": "openai/gpt-6", "permission": "read_only"}));
    let t = w.ok("thread.show", json!({"thread": tid}));
    assert_eq!((t["model"].as_str(), t["permission"].as_str()), (Some("openai/gpt-6"), Some("read_only")));
    assert!(w.call("project.set", json!({"project": "shop", "defaults": {"permission": "yolo"}})).is_err());
    assert!(w.call("thread.new", json!({"project": "shop", "permission": "yolo"})).is_err());
    // Cleared, it is omp's again.
    let p = w.ok("project.set", json!({"project": "shop", "defaults": {"model": ""}}));
    assert_eq!(p["defaults"]["model"], "");
}

#[test]
fn removing_a_project_stops_its_threads_and_forgets_them() {
    let mut w = world();
    w.project();
    let a = w.new_thread(json!({"text": "hi"}));
    let b = w.new_thread(json!({}));
    w.ok("thread.stop", json!({"thread": b}));
    let r = w.ok("project.remove", json!({"project": "shop"}));
    assert_eq!(r["stopped"], json!([a]), "only the running one had an omp to stop");
    assert_eq!(w.d.agents.ended(), 2, "the thread stopped before and the one running now");
    assert!(w.d.store.thread(&a).unwrap().is_none() && w.d.store.projects().unwrap().is_empty());
    assert!(w.repo.join(".lila/project.toml").is_file(), "the folder is not touched");
    assert!(w.call("thread.list", json!({"project": "shop"})).is_err());
}

// ---------------- a thread's life ----------------

#[test]
fn a_thread_runs_omp_and_its_first_message_waits_until_omp_is_free() {
    let mut w = world();
    w.project();
    let tid = w.new_thread(json!({"model": "deepseek/deepseek-v4", "text": "hello", "permission": "confirm"}));
    let run = &w.d.agents.runs[0];
    assert_eq!(run.thread, tid);
    assert_eq!(run.cwd, w.repo);
    assert!(run.program.ends_with("bin/omp"), "{:?}", run.program);
    let args = argv(&w, 0);
    assert_eq!(args[..4], ["--mode", "rpc-ui", "--approval-mode", "always-ask"]);
    assert_eq!(args[4..], ["--model", "deepseek/deepseek-v4", "--session-dir", w.d.paths.omp_sessions(&tid).to_str().unwrap()], "the thread's own folder of omp sessions");
    assert!(w.d.paths.omp_sessions(&tid).is_dir());
    assert_eq!(w.d.store.thread(&tid).unwrap().unwrap().session, None, "omp's session id comes from omp");
    // Starting: nothing goes before omp is free; then the first message does.
    assert_eq!(w.state(&tid), "starting");
    assert!(w.sends_to(&tid).is_empty());
    assert_eq!(w.d.store.outbox(&tid).unwrap().len(), 1, "kept until omp takes it");
    w.status(&tid, "idle");
    assert_eq!(w.sends_to(&tid), ["hello"]);
    assert!(w.d.store.outbox(&tid).unwrap().is_empty());
    assert_eq!(w.state(&tid), "idle");
    w.host_event(&tid, AgentEvent::Session { id: "s1".into() });
    assert_eq!(w.ok("thread.show", json!({"thread": tid}))["session"], "s1");
    // Items are stored and read back by seq.
    let said = Item { id: "m1".into(), turn: None, at_ms: 5, state: ItemState::Done, kind: ItemKind::AgentMessage { text: "hi there".into() } };
    w.host_event(&tid, AgentEvent::Item { item: said });
    let r = w.ok("thread.read", json!({"thread": tid}));
    assert_eq!((r["items"][0]["text"].as_str(), r["seq"].as_i64()), (Some("hi there"), Some(1)));
    assert!(w.ok("thread.read", json!({"thread": tid, "after": 1}))["items"].as_array().unwrap().is_empty());
    assert!(r["thread"].get("caps").is_none() && r["thread"].get("engine").is_none());
    // The watchers hear which thread changed, and how far its conversation goes.
    let ev = w.d.thread_event(&tid, 7).unwrap();
    assert_eq!((ev["event"].as_str(), ev["items_seq"].as_i64(), ev["state"].as_str()), (Some("thread"), Some(1), Some("idle")));
    // omp takes words one turn at a time: a message while it works waits for the turn to end.
    w.status(&tid, "working");
    assert_eq!(w.state(&tid), "running");
    w.ok("thread.send", json!({"thread": tid, "text": "and a second one"}));
    assert_eq!(w.sends_to(&tid), ["hello"]);
    w.status(&tid, "idle");
    assert_eq!(w.sends_to(&tid), ["hello", "and a second one"]);
    assert!(w.call("thread.send", json!({"thread": tid, "text": " "})).is_err(), "nothing to send");
}

#[test]
fn interrupting_goes_to_the_host_only_while_the_thread_works() {
    let mut w = world();
    w.project();
    let tid = w.new_thread(json!({}));
    w.status(&tid, "idle");
    let err = w.call("thread.interrupt", json!({"thread": tid})).unwrap_err();
    assert!(err.to_string().contains("not working"), "{err:#}");
    w.status(&tid, "working");
    w.ok("thread.interrupt", json!({"thread": tid}));
    assert_eq!(w.commands(&tid).last(), Some(&Command::Interrupt));
}

#[test]
fn files_named_with_at_go_to_omp_as_files() {
    let mut w = world();
    w.project();
    std::fs::write(w.repo.join("README.md"), "# shop").unwrap();
    let path = w.repo.join("README.md").to_string_lossy().to_string();
    let attached = |c: &Command| match c {
        Command::Prompt { attachments, .. } => attachments.iter().map(|a| (a.path.clone(), a.name.clone())).collect::<Vec<_>>(),
        other => panic!("{other:?}"),
    };
    // The first message and a later one: the mentioned file that exists goes along, the one that does not stays words.
    let tid = w.new_thread(json!({"text": "read @README.md and @nothing.md"}));
    w.status(&tid, "idle");
    w.ok("thread.send", json!({"thread": tid, "text": "again @README.md"}));
    let got: Vec<_> = w.commands(&tid).iter().filter(|c| matches!(c, Command::Prompt { .. })).map(attached).collect();
    let file = vec![(path, "README.md".to_owned())];
    assert_eq!(got, [file.clone(), file]);
}

#[test]
fn an_ended_thread_is_resumed_on_its_omp_session() {
    let mut w = world();
    w.project();
    let tid = w.new_thread(json!({"text": "first"}));
    w.status(&tid, "idle");
    w.host_event(&tid, AgentEvent::Session { id: "s9".into() });
    assert!(w.call("thread.resume", json!({"thread": tid})).is_err(), "it is running");
    // omp ends by itself.
    w.exited(&tid, "omp stopped (code 0)");
    assert_eq!(w.state(&tid), "ended");
    let shown = w.ok("thread.show", json!({"thread": tid}));
    assert!(shown["ended_ms"].is_i64());
    assert!(w.events(&tid).contains(&"ended omp stopped (code 0)".to_owned()), "{:?}", w.events(&tid));
    assert!(w.call("thread.send", json!({"thread": tid, "text": "hello?"})).unwrap_err().to_string().contains("resume"));
    assert!(w.call("thread.stop", json!({"thread": tid})).is_err(), "nothing runs");

    // Resumed on its session, with a message and another tier and model: in the same folder of omp sessions, which omp goes on with.
    let dir = w.d.paths.omp_sessions(&tid);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("2026-11-13_s9.jsonl"), "{}\n").unwrap();
    let r = w.ok("thread.resume", json!({"thread": tid, "text": "go on", "model": "openai/gpt-6", "permission": "read_only"}));
    assert_eq!((r["state"].as_str(), r["permission"].as_str(), r["model"].as_str()), (Some("starting"), Some("read_only"), Some("openai/gpt-6")));
    let args = argv(&w, 1);
    assert_eq!(args[args.iter().position(|a| a == "--session-dir").unwrap() + 1], dir.to_str().unwrap());
    assert_eq!(args[args.iter().position(|a| a == "--model").unwrap() + 1], "openai/gpt-6");
    let resumed = |w: &World, tid: &str| w.d.store.events(tid, 50).unwrap().into_iter().find(|e| e.kind == "resumed").map(|e| e.detail).unwrap_or_default();
    assert!(resumed(&w, &tid).starts_with("goes on with the omp session in"), "{}", resumed(&w, &tid));
    w.status(&tid, "idle");
    assert_eq!(w.sends_to(&tid).last().map(String::as_str), Some("go on"));
    assert!(argv(&w, 1).contains(&"--continue".to_owned()), "it goes on with its session");
    // Stopped by the person: ended too, and omp is let go.
    assert_eq!(w.d.agents.ended(), 1, "the first run ended by itself");
    w.ok("thread.stop", json!({"thread": tid}));
    assert_eq!(w.d.agents.ended(), 2);
    assert_eq!(w.state(&tid), "ended");

    // One that ended before omp made a session starts a new one, in a folder of its own.
    let early = w.new_thread(json!({}));
    w.ok("thread.stop", json!({"thread": early}));
    w.ok("thread.resume", json!({"thread": early}));
    assert!(resumed(&w, &early).starts_with("no omp session in"), "{}", resumed(&w, &early));
    assert!(!argv(&w, 3).contains(&"--continue".to_owned()), "a new omp session");
    assert!(argv(&w, 3).contains(&w.d.paths.omp_sessions(&early).to_string_lossy().to_string()));
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
    assert_eq!(w.state(&a), "ended", "archiving ends omp");
    w.ok("thread.unarchive", json!({"thread": a}));
    assert_eq!(ids(&mut w, false).len(), 2);
    w.ok("thread.rename", json!({"thread": a, "title": "renamed"}));
    assert_eq!(w.ok("thread.show", json!({"thread": a}))["title"], "renamed");
    // Everything, when no project is named.
    assert_eq!(w.ok("thread.list", json!({})).as_array().unwrap().len(), 2);
}

#[test]
fn threads_that_were_running_when_lilad_stopped_are_ended_when_it_is_back() {
    let mut w = world();
    w.project();
    let a = w.new_thread(json!({}));
    let b = w.new_thread(json!({}));
    w.status(&a, "idle");
    w.ok("thread.stop", json!({"thread": b}));
    // A new lilad over the same database: their omp ended with the old one.
    w.restart();
    for t in [&a, &b] {
        assert_eq!(w.state(t), "ended", "{t}");
    }
    assert!(w.events(&a).contains(&"ended lilad was not running".to_owned()), "{:?}", w.events(&a));
    assert!(w.d.store.live_threads().unwrap().is_empty());
    // Resumed, one goes on.
    w.ok("thread.resume", json!({"thread": a}));
    assert_eq!(w.state(&a), "starting");
}

#[test]
fn a_blocked_thread_waits_and_takes_no_words_until_it_is_free() {
    let mut w = world();
    w.project();
    let tid = w.new_thread(json!({"text": "hi"}));
    w.status(&tid, "blocked");
    let j = w.d.thread_json(&w.d.store.thread(&tid).unwrap().unwrap());
    assert_eq!((j["state"].as_str(), j["waiting"].as_str()), (Some("waiting"), Some("card")));
    assert!(w.call("thread.send", json!({"thread": tid, "text": "x"})).is_err());
    assert!(w.sends_to(&tid).is_empty(), "nothing goes into a thread that waits");
    w.status(&tid, "idle");
    assert_eq!(w.d.thread_json(&w.d.store.thread(&tid).unwrap().unwrap())["waiting"], Value::Null);
    assert_eq!(w.sends_to(&tid), ["hi"]);
}

#[test]
fn the_model_changes_at_once_in_a_running_thread_and_on_resume_in_an_ended_one() {
    let mut w = world();
    w.project();
    let tid = w.new_thread(json!({"model": "deepseek/deepseek-v4"}));
    w.status(&tid, "idle");
    w.ok("thread.model", json!({"thread": tid, "model": "openai/gpt-6", "effort": "low"}));
    assert_eq!(w.commands(&tid), [Command::SetModel { model: "openai/gpt-6".into() }, Command::SetEffort { effort: "low".into() }]);
    let t = w.d.store.thread(&tid).unwrap().unwrap();
    assert_eq!((t.model.as_str(), t.effort.as_deref()), ("openai/gpt-6", Some("low")), "and on resume too");
    assert!(w.call("thread.model", json!({"thread": tid})).is_err(), "nothing to change");
    // What omp reports it runs on, and what it lists, is what the box under the conversation shows.
    w.host_event(&tid, AgentEvent::Config { model: Some("openai/gpt-6-luna".into()), effort: None, models: vec![lila_agent::Choice { id: "openai/gpt-6-luna".into(), name: "Luna".into() }], efforts: vec![] });
    let config = w.ok("thread.read", json!({"thread": tid}))["config"].clone();
    assert_eq!((config["model"].as_str(), config["switch"].as_bool(), config["default_model"].as_str()), (Some("openai/gpt-6-luna"), Some(true), Some("openai/gpt-6")));
    // An ended thread starts on it next time.
    w.ok("thread.stop", json!({"thread": tid}));
    w.ok("thread.model", json!({"thread": tid, "model": "deepseek/deepseek-v4"}));
    w.ok("thread.resume", json!({"thread": tid}));
    assert!(argv(&w, 1).windows(2).any(|p| p == ["--model", "deepseek/deepseek-v4"]));
}

#[test]
fn the_slash_menu_offers_what_omp_declared() {
    let mut w = world();
    w.project();
    let tid = w.new_thread(json!({}));
    w.status(&tid, "idle");
    assert!(w.ok("thread.commands", json!({"thread": tid}))["commands"].as_array().unwrap().is_empty());
    let command = |name: &str, hint: Option<&str>| SlashCommand { name: name.into(), description: "d".into(), hint: hint.map(Into::into) };
    w.host_event(&tid, AgentEvent::Commands { commands: vec![command("compact", Some("arguments")), command("skill:tdd", None)] });
    let menu = w.ok("thread.commands", json!({"thread": tid}))["commands"].clone();
    assert_eq!((menu[0]["name"].as_str(), menu[0]["kind"].as_str(), menu[0]["insert"].as_str(), menu[0]["hint"].as_str()), (Some("compact"), Some("builtin"), Some("/compact "), Some("[参数]")));
    assert_eq!((menu[1]["name"].as_str(), menu[1]["kind"].as_str(), menu[1]["insert"].as_str()), (Some("tdd"), Some("skill"), Some("/skill:tdd ")));
}

#[test]
fn unknown_requests_and_threads_are_errors() {
    let mut w = world();
    w.project();
    assert!(w.call("task.new", json!({})).is_err());
    assert!(w.call("hook", json!({"thread": "t00000000", "event": "Stop"})).is_err(), "Claude Code's hooks are gone");
    assert!(w.call("thread.show", json!({"thread": "t00000000"})).is_err());
    assert!(w.call("thread.new", json!({"project": "shop", "permission": "yolo"})).is_err());
    assert!(w.d.agents.runs.is_empty(), "nothing was started");
    let status = w.ok("status", json!({}));
    assert_eq!(status["projects"], 1);
}

#[test]
fn ping_and_status_report_the_version_and_protocol() {
    let mut w = world();
    for method in ["ping", "status"] {
        let r = w.ok(method, json!({}));
        assert_eq!((r["version"].as_str(), r["protocol"].as_u64()), (Some(env!("CARGO_PKG_VERSION")), Some(u64::from(crate::PROTOCOL))), "{method}: {r}");
    }
    assert_eq!(w.ok("ping", json!({}))["pong"], true);
}

// ---------------- threads from before omp was the only engine ----------------

#[test]
fn threads_of_claude_code_and_codex_in_an_existing_database_are_kept_ended_and_cannot_be_resumed() {
    let mut w = world_over(|paths, repo| {
        // A database as the last version with engines left it: a running Claude Code thread, an ended Codex one, one of omp's.
        let db = rusqlite::Connection::open(paths.db()).unwrap();
        db.execute_batch(&format!(
            "CREATE TABLE projects (id TEXT PRIMARY KEY, cwd TEXT NOT NULL UNIQUE, created_ms INTEGER NOT NULL);
             CREATE TABLE threads (id TEXT PRIMARY KEY, project TEXT NOT NULL, title TEXT NOT NULL, engine TEXT NOT NULL, model TEXT NOT NULL, effort TEXT, tier TEXT NOT NULL,
                cwd TEXT NOT NULL, session TEXT, terminal TEXT, created_ms INTEGER NOT NULL, active_ms INTEGER NOT NULL, started_ms INTEGER, ended_ms INTEGER, archived INTEGER NOT NULL DEFAULT 0,
                worktree INTEGER NOT NULL DEFAULT 0, base_branch TEXT, base_commit TEXT, merged_ms INTEGER);
             INSERT INTO projects VALUES ('shop', '{cwd}', 1);
             INSERT INTO threads (id, project, title, engine, model, tier, cwd, session, terminal, created_ms, active_ms, started_ms) VALUES ('t00000001', 'shop', 'claude', 'claude', 'claude-sonnet-5-5', 'full', '{cwd}', 'sid-1', 'old-1', 1, 5, 2);
             INSERT INTO threads (id, project, title, engine, model, tier, cwd, session, terminal, created_ms, active_ms, ended_ms) VALUES ('t00000002', 'shop', 'codex', 'codex', 'gpt-6.1-sol', 'full', '{cwd}', 'th-1', NULL, 1, 4, 4);
             INSERT INTO threads (id, project, title, engine, model, tier, cwd, session, terminal, created_ms, active_ms, ended_ms) VALUES ('t00000003', 'shop', 'omp', 'omp', 'deepseek/deepseek-v4', 'full', '{cwd}', 's-1', NULL, 1, 3, 3);
             PRAGMA user_version = 7;",
            cwd = repo.display()
        ))
        .unwrap();
    });
    let list = w.ok("thread.list", json!({}));
    let by_id = |id: &str| list.as_array().unwrap().iter().find(|t| t["id"] == id).unwrap().clone();
    // Both are ended, whatever they were doing, and say which engine they ran on; omp's is as it was.
    for id in ["t00000001", "t00000002"] {
        assert_eq!((by_id(id)["state"].as_str(), by_id(id)["legacy"].as_str()), (Some("ended"), Some(if id == "t00000001" { "claude" } else { "codex" })), "{id}");
    }
    assert_eq!((by_id("t00000003")["state"].as_str(), by_id("t00000003")["legacy"].is_null()), (Some("ended"), true));
    // Nothing is running here.
    assert!(w.d.agents.runs.is_empty());
    assert_eq!(w.state("t00000001"), "ended");
    // Their conversations can be read, but they take no words and cannot be resumed; omp's can.
    w.ok("thread.read", json!({"thread": "t00000001"}));
    let err = w.call("thread.resume", json!({"thread": "t00000001"})).unwrap_err().to_string();
    assert!(err.contains("claude") && err.contains("no longer"), "{err}");
    assert!(w.call("thread.resume", json!({"thread": "t00000002"})).is_err());
    assert!(w.call("thread.send", json!({"thread": "t00000002", "text": "x"})).is_err());
    assert!(w.d.agents.runs.is_empty());
    w.ok("thread.resume", json!({"thread": "t00000003"}));
    // It had an ACP session id and no folder of omp sessions: it starts a new omp session, its conversation kept.
    assert_eq!(w.d.store.thread("t00000003").unwrap().unwrap().session.as_deref(), Some("s-1"));
    assert!(argv(&w, 0).contains(&w.d.paths.omp_sessions("t00000003").to_string_lossy().to_string()));
    assert!(w.d.store.events("t00000003", 50).unwrap().iter().any(|e| e.kind == "resumed" && e.detail.starts_with("no omp session in")));
    // They can be archived and renamed like any.
    w.ok("thread.archive", json!({"thread": "t00000001"}));
    // The line `lila thread list` prints says why they will not start.
    assert_eq!(w.ok("thread.show", json!({"thread": "t00000002"}))["legacy"], "codex");
}

// ---------------- questions ----------------

fn question(id: &str, kind: QuestionKind, options: &[&str]) -> Question {
    Question { id: id.into(), title: "Which database?\nFor the new service.".into(), detail: String::new(), kind, options: options.iter().map(|o| o.to_string()).collect(), prefill: String::new(), secret: false }
}

impl World {
    /// The reply the host was told for a question, if it was.
    fn reply_to(&self, tid: &str, id: &str) -> Option<Reply> {
        self.commands(tid).into_iter().rev().find_map(|c| match c {
            Command::Reply { question, reply } if question == id => Some(reply),
            _ => None,
        })
    }
}

#[test]
fn a_question_is_a_card_the_thread_waits_on_until_the_person_answers_it() {
    let mut w = world();
    w.project();
    let tid = w.new_thread(json!({}));
    w.status(&tid, "idle");
    w.host_event(&tid, AgentEvent::Question { question: question("u1", QuestionKind::Choice, &["Postgres", "SQLite"]) });
    assert_eq!(w.state(&tid), "waiting", "a question holds the thread like a permission");
    let read = w.ok("thread.read", json!({"thread": tid}));
    let card = read["questions"][0].clone();
    assert_eq!((card["kind"].as_str(), card["title"].as_str(), card["options"].clone()), (Some("choice"), Some("Which database?\nFor the new service."), json!(["Postgres", "SQLite"])));
    assert!(read["permissions"].as_array().unwrap().is_empty());
    assert_eq!(w.ok("thread.show", json!({"thread": tid}))["cards"], 1);
    assert_eq!(w.ok("status", json!({}))["cards"], 1);
    assert!(w.call("thread.send", json!({"thread": tid, "text": "hey"})).is_err(), "a message would not answer it");
    // Asked again (the host came back): still one card.
    w.host_event(&tid, AgentEvent::Question { question: question("u1", QuestionKind::Choice, &["Postgres", "SQLite"]) });
    assert_eq!(w.ok("thread.read", json!({"thread": tid}))["questions"].as_array().unwrap().len(), 1);
    // A choice is not one that was not offered; it is given by its words or its number.
    assert!(w.call("thread.answer", json!({"thread": tid, "card": card["id"], "value": "Mongo"})).is_err());
    assert!(w.call("thread.answer", json!({"thread": tid, "card": card["id"]})).is_err());
    assert!(w.call("thread.answer", json!({"thread": tid, "card": card["id"], "value": "3"})).is_err());
    w.ok("thread.answer", json!({"thread": tid, "card": card["id"], "value": "2"}));
    assert_eq!(w.reply_to(&tid, "u1"), Some(Reply::Choice { value: "SQLite".into() }));
    assert_eq!(w.state(&tid), "idle");
    assert!(w.call("thread.answer", json!({"thread": tid, "card": card["id"], "value": "1"})).is_err(), "answered once");
    assert!(w.events(&tid).iter().any(|e| e == "answered Which database? · SQLite"), "{:?}", w.events(&tid));

    // Yes or no; words of the person's own; no answer at all.
    w.host_event(&tid, AgentEvent::Question { question: question("u2", QuestionKind::Confirm, &[]) });
    let id = w.ok("thread.read", json!({"thread": tid}))["questions"][0]["id"].clone();
    assert!(w.call("thread.answer", json!({"thread": tid, "card": id, "value": "maybe"})).is_err());
    w.ok("thread.answer", json!({"thread": tid, "card": id, "value": "no"}));
    assert_eq!(w.reply_to(&tid, "u2"), Some(Reply::Confirm { yes: false }));
    w.host_event(&tid, AgentEvent::Question { question: Question { secret: true, ..question("u3", QuestionKind::Text, &[]) } });
    let id = w.ok("thread.read", json!({"thread": tid}))["questions"][0]["id"].clone();
    assert_eq!(w.ok("thread.read", json!({"thread": tid}))["questions"][0]["secret"], true);
    w.ok("thread.answer", json!({"thread": tid, "card": id, "value": "hunter2"}));
    assert_eq!(w.reply_to(&tid, "u3"), Some(Reply::Text { value: "hunter2".into() }));
    assert!(w.events(&tid).iter().all(|e| !e.contains("hunter2")), "a secret is not logged");
    w.host_event(&tid, AgentEvent::Question { question: question("u4", QuestionKind::Text, &[]) });
    let id = w.ok("thread.read", json!({"thread": tid}))["questions"][0]["id"].clone();
    w.ok("thread.answer", json!({"thread": tid, "card": id, "cancel": true}));
    assert_eq!(w.reply_to(&tid, "u4"), Some(Reply::Cancel));

    // omp takes a question back, or the thread ends with one open: no card is left.
    w.host_event(&tid, AgentEvent::Question { question: question("u5", QuestionKind::Choice, &["a"]) });
    w.host_event(&tid, AgentEvent::QuestionResolved { id: "u5".into() });
    assert!(w.ok("thread.read", json!({"thread": tid}))["questions"].as_array().unwrap().is_empty());
    w.host_event(&tid, AgentEvent::Question { question: question("u6", QuestionKind::Choice, &["a"]) });
    w.ok("thread.stop", json!({"thread": tid}));
    assert!(w.ok("thread.read", json!({"thread": tid}))["questions"].as_array().unwrap().is_empty());
    assert_eq!(w.ok("thread.show", json!({"thread": tid}))["cards"], 0);
}

#[test]
fn the_models_omp_offers_are_asked_of_an_omp_of_its_own_and_remembered() {
    let mut w = world();
    w.project();
    // A stand-in omp: says it is ready and, as the request for the models goes out, names two.
    let omp = w._tmp.path().join("bin/omp");
    std::fs::write(&omp, "#!/bin/sh\necho '{\"type\":\"ready\",\"supportedProtocolVersions\":[1,2]}'\necho '{\"type\":\"response\",\"id\":\"l3\",\"success\":true,\"data\":{\"models\":[{\"provider\":\"openai\",\"id\":\"gpt-6\",\"name\":\"GPT-6\"},{\"provider\":\"magpie\",\"id\":\"claude/sonnet\",\"name\":\"Sonnet\"}]}}'\nsleep 30\n").unwrap();
    let r = w.ok("models.list", json!({}));
    assert_eq!(r["models"], json!([{"id": "openai/gpt-6", "name": "GPT-6"}, {"id": "magpie/claude/sonnet", "name": "Sonnet"}]));
    // Remembered: omp is not started again (it is gone now, and the answer is the same).
    std::fs::remove_file(&omp).unwrap();
    assert_eq!(w.ok("models.list", json!({})), r);
    assert!(w.call("models.list", json!({"refresh": true})).is_err());
    std::fs::write(&omp, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&omp, std::fs::Permissions::from_mode(0o755)).unwrap();
    // A thread whose omp has not named its models yet is offered these.
    let tid = w.new_thread(json!({}));
    w.status(&tid, "idle");
    let cfg = w.ok("thread.read", json!({"thread": tid}))["config"].clone();
    assert_eq!((cfg["switch"].clone(), cfg["models"].as_array().unwrap().len()), (json!(true), 2));
}

// ---------------- tiers and permission requests ----------------

#[test]
fn requests_are_answered_by_the_threads_tier() {
    let mut w = world();
    w.project();
    let repo = w.repo.to_string_lossy().to_string();
    let tid = w.new_thread(json!({}));
    w.status(&tid, "idle");
    assert_eq!(w.ok("thread.show", json!({"thread": tid}))["permission"], "full");

    // Full auto: ordinary work goes without anyone being asked, the first allow option (once, not for the session).
    assert_eq!(w.raise(&tid, ask("r1", "shell", "cargo test")).as_deref(), Some("accept"));
    assert_eq!(w.raise(&tid, edit_ask("r2", &format!("{repo}/src/a.rs"))).as_deref(), Some("accept"));
    assert!(w.events(&tid).iter().any(|e| e == "auto_allowed full · shell: cargo test"), "{:?}", w.events(&tid));
    assert_eq!(w.state(&tid), "idle");
    // … but not a push, a write outside the folder, a pull request or computer use.
    for (i, request) in [
        ask("d1", "shell", "git push origin main"),
        edit_ask("d2", "/etc/hosts"),
        ask("d3", "shell", "gh pr create --title x --body y"),
        PermissionRequest { tool: ToolKind::Other, command: None, ..ask("d4", "mcp__computer-use__left_click", "") },
    ]
    .into_iter()
    .enumerate()
    {
        let id = request.id.clone();
        assert_eq!(w.raise(&tid, request.clone()), None, "{request:?} should wait");
        assert_eq!(w.state(&tid), "waiting");
        let read = w.ok("thread.read", json!({"thread": tid}));
        let card = &read["permissions"][0];
        assert!(card["why"].as_str().unwrap().starts_with("全自动档仍需确认"), "{card}");
        // A message into a waiting thread would answer its card: refused.
        assert!(w.call("thread.send", json!({"thread": tid, "text": "hey"})).is_err());
        // Asked again (lilad came back, the host repeated it): still one card.
        w.host_event(&tid, AgentEvent::Permission { request });
        assert_eq!(w.ok("thread.read", json!({"thread": tid}))["permissions"].as_array().unwrap().len(), 1);
        // The person says yes, or no.
        let allow = i == 1;
        w.ok("thread.answer", json!({"thread": tid, "card": card["id"], "allow": allow}));
        assert_eq!(w.answer_to(&tid, &id).as_deref(), Some(if allow { "accept" } else { "decline" }));
        assert_eq!(w.state(&tid), "idle");
        assert!(w.call("thread.answer", json!({"thread": tid, "card": card["id"], "allow": true})).is_err(), "answered once");
    }
    // The request omp took back closes its card.
    assert_eq!(w.raise(&tid, edit_ask("d5", "/Users/someone/.zshrc")), None);
    assert_eq!(w.ok("thread.read", json!({"thread": tid}))["permissions"][0]["danger"], "outside_workspace");
    w.host_event(&tid, AgentEvent::PermissionResolved { id: "d5".into() });
    assert!(w.ok("thread.read", json!({"thread": tid}))["permissions"].as_array().unwrap().is_empty());
    assert_eq!(w.state(&tid), "idle");

    // Read-only: reads go, changes are refused.
    w.ok("thread.permission", json!({"thread": tid, "permission": "read_only"}));
    assert_eq!(w.raise(&tid, read_ask("o1", "/etc/hosts")).as_deref(), Some("accept"));
    assert_eq!(w.raise(&tid, edit_ask("o2", &format!("{repo}/src/a.rs"))).as_deref(), Some("decline"));
    assert_eq!(w.raise(&tid, ask("o3", "shell", "touch x")).as_deref(), Some("decline"));
    // Confirm: everything that changes asks, and says why.
    w.ok("thread.permission", json!({"thread": tid, "permission": "confirm"}));
    assert_eq!(w.raise(&tid, ask("c1", "shell", "ls -la")), None);
    let card = w.ok("thread.read", json!({"thread": tid}))["permissions"][0].clone();
    assert_eq!((card["why"].as_str(), card["danger"].is_null()), (Some("改动前确认"), true));
    w.ok("thread.answer", json!({"thread": tid, "permission": "c1", "option": "拒绝"}));
    assert_eq!(w.answer_to(&tid, "c1").as_deref(), Some("decline"));
    assert!(w.call("thread.answer", json!({"thread": tid, "card": card["id"]})).is_err(), "no card is open any more");
    // And back to full: the next goes by itself.
    w.ok("thread.permission", json!({"thread": tid, "permission": "full"}));
    assert_eq!(w.raise(&tid, ask("f1", "shell", "npm install")).as_deref(), Some("accept"));
    assert!(w.call("thread.permission", json!({"thread": tid, "permission": "yolo"})).is_err());
    let log = w.events(&tid);
    assert!(log.iter().any(|e| e.starts_with("auto_denied read_only")) && log.iter().any(|e| e.starts_with("permission full → read_only")) && log.iter().any(|e| e.starts_with("asked ")), "{log:?}");
}

// ---------------- "always allow for this thread" ----------------

/// The card the thread has open, with its option labels.
fn the_card(w: &mut World, tid: &str) -> Value {
    let read = w.ok("thread.read", json!({"thread": tid}));
    assert_eq!(read["permissions"].as_array().unwrap().len(), 1, "{read}");
    read["permissions"][0].clone()
}

fn labels(card: &Value) -> Vec<String> {
    card["options"].as_array().unwrap().iter().map(|o| o["label"].as_str().unwrap().to_owned()).collect()
}

fn rule_keys(w: &mut World, tid: &str) -> Vec<String> {
    w.ok("thread.show", json!({"thread": tid}))["rules"].as_array().unwrap().iter().map(|r| r["key"].as_str().unwrap().to_owned()).collect()
}

#[test]
fn always_allow_is_a_rule_of_the_thread_that_answers_the_next_one_and_replaces_omps_own() {
    let mut w = world();
    w.project();
    let tid = w.new_thread(json!({"permission": "confirm"}));
    w.status(&tid, "idle");

    // The first `cargo test` asks, and the card offers the rule it would make (and says what that allows); omp's own "for the session" is not shown.
    assert_eq!(w.raise(&tid, ask("c1", "shell", "cargo test -p lila")), None);
    let card = the_card(&mut w, &tid);
    assert_eq!(labels(&card), ["允许", "本会话总是允许 cargo test", "拒绝"]);
    assert_eq!((card["options"][1]["id"].as_str(), card["options"][1]["kind"].as_str(), card["danger"].is_null()), (Some("always"), Some("allow_always"), true));
    assert!(rule_keys(&mut w, &tid).is_empty());
    w.ok("thread.answer", json!({"thread": tid, "card": card["id"], "option": "always"}));
    assert_eq!(w.answer_to(&tid, "c1").as_deref(), Some("accept"), "omp is told a plain allow, so it asks again next time and Lila answers");
    assert_eq!(rule_keys(&mut w, &tid), ["cargo test"]);
    let shown = w.ok("thread.show", json!({"thread": tid}))["rules"][0].clone();
    assert_eq!((shown["kind"].as_str(), shown["label"].as_str()), (Some("command"), Some("cargo test")));
    assert_eq!(w.ok("thread.read", json!({"thread": tid}))["rules"].as_array().unwrap().len(), 1);

    // The same program and sub-command now goes without a card, and the log names the rule.
    assert_eq!(w.verdict(&tid, "cargo test --lib"), "allow");
    assert!(w.events(&tid).iter().any(|e| e == "auto_allowed confirm · shell: cargo test --lib · rule: cargo test"), "{:?}", w.events(&tid));
    // Another sub-command, another program, a chain with a part that is not covered, a wrapper: asked.
    for c in ["cargo build", "ls", "cargo test && ls", "sudo cargo test", "bash -c 'cargo test'", "cargo test $(ls)", "FOO=1 cargo test"] {
        assert_eq!(w.verdict(&tid, c), "ask", "{c}");
    }
    assert!(w.ok("thread.read", json!({"thread": tid}))["permissions"].as_array().unwrap().is_empty());

    // A chain: `cargo test` is covered, `tail` is offered; and `git commit`, once ruled, still does not let a push through.
    w.raise(&tid, ask("c2", "shell", "cargo test 2>&1 | tail -5"));
    let card = the_card(&mut w, &tid);
    assert_eq!(labels(&card)[1], "本会话总是允许 tail");
    w.ok("thread.answer", json!({"thread": tid, "card": card["id"], "option": "本会话总是允许 tail"}));
    assert_eq!(rule_keys(&mut w, &tid), ["cargo test", "tail"]);
    assert_eq!(w.verdict(&tid, "cargo test | tail"), "allow");
    w.raise(&tid, ask("c3", "shell", "git commit -am x"));
    let card = the_card(&mut w, &tid);
    w.ok("thread.answer", json!({"thread": tid, "card": card["id"], "option": "always"}));
    assert_eq!(w.verdict(&tid, "git commit -m y"), "allow");
    for (i, c) in ["git commit -m y && git push", "git push", "cargo test; git push origin main", "git branch -D x"].into_iter().enumerate() {
        assert_eq!(w.verdict(&tid, c), "ask", "{c}");
        w.raise(&tid, ask(&format!("p{i}"), "shell", c));
        let card = the_card(&mut w, &tid);
        assert_eq!(labels(&card), ["允许", "拒绝"], "a push never gets an always: {c}");
        assert!(w.call("thread.answer", json!({"thread": tid, "card": card["id"], "option": "always"})).is_err());
        w.ok("thread.answer", json!({"thread": tid, "card": card["id"], "allow": false}));
    }
    // Not even in the full tier: the danger is asked, with no "always" on its card.
    w.ok("thread.permission", json!({"thread": tid, "permission": "full"}));
    assert_eq!(w.verdict(&tid, "git push"), "ask");
    w.raise(&tid, ask("p9", "shell", "git push"));
    let card = the_card(&mut w, &tid);
    assert_eq!((labels(&card), card["danger"].as_str()), (vec!["允许".to_owned(), "拒绝".to_owned()], Some("git_push")));
    w.ok("thread.answer", json!({"thread": tid, "card": card["id"], "allow": false}));
    // And what was ruled stays for when the tier asks again.
    assert_eq!(rule_keys(&mut w, &tid), ["cargo test", "tail", "git commit"]);
}

#[test]
fn rules_survive_a_restart_and_a_resume_go_when_the_tier_is_read_only_and_can_be_taken_back() {
    let mut w = world();
    w.project();
    let tid = w.new_thread(json!({"permission": "confirm"}));
    w.status(&tid, "idle");
    w.raise(&tid, ask("a1", "shell", "touch a"));
    let card = the_card(&mut w, &tid);
    w.ok("thread.answer", json!({"thread": tid, "card": card["id"], "option": "always"}));
    assert_eq!(w.verdict(&tid, "touch b"), "allow");

    // lilad restarted (the thread's omp ended with it, and it is resumed): the rule is there and answers.
    w.restart();
    w.ok("thread.resume", json!({"thread": tid}));
    w.status(&tid, "idle");
    assert_eq!(rule_keys(&mut w, &tid), ["touch"]);
    assert_eq!(w.verdict(&tid, "touch c"), "allow");
    // The thread ended and resumed: still there.
    w.ok("thread.stop", json!({"thread": tid}));
    w.ok("thread.resume", json!({"thread": tid}));
    w.status(&tid, "idle");
    assert_eq!(rule_keys(&mut w, &tid), ["touch"]);
    assert_eq!(w.verdict(&tid, "touch d"), "allow");

    // Full and back: kept, and answering again. Read-only: forgotten, and logged.
    w.ok("thread.permission", json!({"thread": tid, "permission": "full"}));
    assert_eq!(rule_keys(&mut w, &tid), ["touch"]);
    w.ok("thread.permission", json!({"thread": tid, "permission": "read_only"}));
    assert!(rule_keys(&mut w, &tid).is_empty());
    assert!(w.events(&tid).iter().any(|e| e == "rules_dropped touch"), "{:?}", w.events(&tid));
    w.ok("thread.permission", json!({"thread": tid, "permission": "confirm"}));
    assert_eq!(w.verdict(&tid, "touch e"), "ask", "not kept for later");

    // A rule taken back asks again; a rule that is not there is an error.
    w.raise(&tid, ask("a2", "shell", "ls"));
    let card = the_card(&mut w, &tid);
    w.ok("thread.answer", json!({"thread": tid, "card": card["id"], "option": "always"}));
    let rules = w.ok("thread.show", json!({"thread": tid}))["rules"].clone();
    assert_eq!(w.verdict(&tid, "ls -la"), "allow");
    let r = w.ok("thread.rule.remove", json!({"thread": tid, "rule": rules[0]["id"]}));
    assert_eq!((r["removed"].as_bool(), r["rules"].as_array().unwrap().len()), (Some(true), 0));
    assert_eq!(w.verdict(&tid, "ls -la"), "ask");
    assert!(w.call("thread.rule.remove", json!({"thread": tid, "rule": rules[0]["id"]})).is_err());
    assert!(w.call("thread.rule.remove", json!({"thread": tid})).is_err());
    let log = w.events(&tid);
    assert!(log.iter().any(|e| e == "rule_added ls") && log.iter().any(|e| e == "rule_removed ls"), "{log:?}");
}

#[test]
fn always_on_one_of_several_open_cards_answers_the_others_the_rule_covers() {
    let mut w = world();
    w.project();
    let tid = w.new_thread(json!({"permission": "confirm"}));
    w.status(&tid, "idle");
    for (id, c) in [("k1", "touch a"), ("k2", "touch b"), ("k3", "ls")] {
        assert_eq!(w.raise(&tid, ask(id, "shell", c)), None);
    }
    let read = w.ok("thread.read", json!({"thread": tid}));
    assert_eq!(read["permissions"].as_array().unwrap().len(), 3);
    w.ok("thread.answer", json!({"thread": tid, "card": read["permissions"][0]["id"], "option": "always"}));
    assert_eq!(w.answer_to(&tid, "k1").as_deref(), Some("accept"));
    assert_eq!(w.answer_to(&tid, "k2").as_deref(), Some("accept"), "the same rule answers it");
    assert_eq!(w.answer_to(&tid, "k3"), None, "another program still waits");
    assert_eq!(w.ok("thread.read", json!({"thread": tid}))["permissions"].as_array().unwrap().len(), 1);
    assert!(w.events(&tid).iter().any(|e| e == "auto_allowed confirm · shell: touch b · rule: touch"), "{:?}", w.events(&tid));
}

#[test]
fn edits_inside_the_folder_can_be_allowed_always_and_nothing_outside_and_no_mcp_write() {
    let mut w = world();
    w.project();
    let repo = w.repo.to_string_lossy().to_string();
    let inside = format!("{repo}/src/a.rs");
    let tid = w.new_thread(json!({"permission": "confirm"}));
    w.status(&tid, "idle");
    // What cannot be ruled has no always at all, not even omp's.
    w.raise(&tid, ask("e0", "shell", "bash -c 'rm x'"));
    assert_eq!(labels(&the_card(&mut w, &tid)), ["允许", "拒绝"]);
    w.host_event(&tid, AgentEvent::PermissionResolved { id: "e0".into() });
    // A patch inside the folder: the edit rule.
    w.raise(&tid, edit_ask("e1", &inside));
    let card = the_card(&mut w, &tid);
    assert_eq!(labels(&card)[1], "本会话总是允许改动此文件夹里的文件");
    w.ok("thread.answer", json!({"thread": tid, "card": card["id"], "option": "always"}));
    assert_eq!(w.raise(&tid, edit_ask("e2", &format!("{repo}/src/b.rs"))).as_deref(), Some("accept"));
    // Outside the folder it asks, in confirm too, and offers nothing "always".
    assert_eq!(w.raise(&tid, edit_ask("e3", "/Users/someone/.zshrc")), None);
    assert_eq!(labels(&the_card(&mut w, &tid)), ["允许", "拒绝"]);
    w.host_event(&tid, AgentEvent::PermissionResolved { id: "e3".into() });
    // A command is not an edit.
    assert_eq!(w.verdict(&tid, "touch x"), "ask");

    // An MCP tool that only looks is let through by the tier (nothing to ask); one that writes to the outside world is a
    // danger, and has no "always". (A rule for one exact tool exists for tools the tier cannot tell: `rules` has its tests.)
    let tool = |id: &str, name: &str| PermissionRequest { tool: ToolKind::Other, command: None, ..ask(id, name, "") };
    assert_eq!(w.raise(&tid, tool("m1", "mcp__github__get_pull_request")).as_deref(), Some("accept"));
    assert_eq!(w.raise(&tid, tool("m2", "mcp__github__create_pull_request")), None);
    assert_eq!(labels(&the_card(&mut w, &tid)), ["允许", "拒绝"]);
    // In the full tier a push is a danger without "always" too.
    w.host_event(&tid, AgentEvent::PermissionResolved { id: "m2".into() });
    w.ok("thread.permission", json!({"thread": tid, "permission": "full"}));
    w.raise(&tid, ask("g1", "shell", "git push origin main"));
    let card = the_card(&mut w, &tid);
    assert_eq!((labels(&card), card["danger"].as_str()), (vec!["允许".to_owned(), "拒绝".to_owned()], Some("git_push")));
}

// ---------------- conversation ----------------

#[test]
fn words_wait_in_the_outbox_while_omp_cannot_take_them() {
    let mut w = world();
    w.project();
    let tid = w.new_thread(json!({"text": "first"}));
    w.status(&tid, "idle");
    assert_eq!(w.sends_to(&tid), ["first"]);
    // omp's input is gone: words wait in the outbox, and are not lost.
    w.d.senders.remove(&tid);
    w.ok("thread.send", json!({"thread": tid, "text": "second"}));
    assert_eq!(w.d.store.outbox(&tid).unwrap().len(), 1);
    assert_eq!(w.sends_to(&tid), ["first"]);
}

// ---------------- worktrees, changes, commit, merge (P2) ----------------

mod p2 {
    use super::*;
    use crate::worktree::tests::{repo, sh};

    /// The project `shop` in a git repository on `main` (its `.lila/project.toml` committed).
    pub(super) fn git_world() -> World {
        let mut w = world();
        // omp is the fake one; git is the real one.
        w.d.path_env = format!("{}:{}", w.d.path_env, std::env::var("PATH").unwrap_or_default());
        w.project();
        repo(&w.repo);
        w
    }

    /// A thread (it reports through a stand-in host, so tests can make it idle), in a worktree.
    pub(super) fn in_worktree(w: &mut World) -> String {
        let tid = w.new_thread(json!({"worktree": true, "text": "start"}));
        w.status(&tid, "idle");
        tid
    }

    pub(super) fn worktree_of(w: &World, tid: &str) -> PathBuf {
        PathBuf::from(w.d.store.thread(tid).unwrap().unwrap().cwd)
    }

    pub(super) fn write(dir: &Path, name: &str, text: &str) {
        let p = dir.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    /// A commit in a worktree, as the agent would make one.
    pub(super) fn commit(dir: &Path, file: &str, text: &str, message: &str) {
        write(dir, file, text);
        sh(dir, &["add", "-A"]);
        sh(dir, &["commit", "-q", "-m", message]);
    }

    #[test]
    fn the_project_list_says_whether_the_folder_can_have_worktrees() {
        let mut w = git_world();
        // Asked when the project is loaded: the repository was made after that, so it is still not one.
        assert_eq!(w.ok("project.list", json!({}))[0]["repo"], json!(false));
        w.ok("project.reload", json!({"project": "shop"}));
        assert_eq!(w.ok("project.list", json!({}))[0]["repo"], json!(true));
        assert_eq!(w.ok("project.show", json!({"project": "shop"}))["repo"], json!(true));
    }

    #[test]
    fn a_thread_can_start_in_a_worktree_of_its_own() {
        let mut w = git_world();
        let head = sh(&w.repo, &["rev-parse", "HEAD"]);
        let tid = w.new_thread(json!({"worktree": true, "text": "hello"}));
        let wt = w.d.paths.data.join("worktrees/shop").join(&tid);
        let j = w.ok("thread.show", json!({"thread": tid}));
        assert_eq!((j["worktree"].clone(), j["base_branch"].clone(), j["base_commit"].clone(), j["merged_ms"].clone()), (json!(true), json!("main"), json!(head), Value::Null));
        assert_eq!(j["cwd"], wt.to_string_lossy().as_ref());
        // omp starts there.
        assert_eq!(w.d.agents.runs[0].cwd, wt);
        assert_eq!(sh(&wt, &["symbolic-ref", "--short", "HEAD"]), format!("lila/{tid}"));
        assert_eq!(sh(&wt, &["rev-parse", "HEAD"]), head);
        assert_eq!(sh(&w.repo, &["symbolic-ref", "--short", "HEAD"]), "main", "the project folder stays where it was");
        w.status(&tid, "idle");
        assert_eq!(w.sends_to(&tid), ["hello"]);
        // Not asked for: the project folder, as before.
        let plain = w.new_thread(json!({}));
        let j = w.ok("thread.show", json!({"thread": plain}));
        assert_eq!((j["worktree"].clone(), j["base_branch"].clone(), j["cwd"].clone()), (json!(false), Value::Null, json!(w.repo.to_string_lossy())));
        // It is the thread list's too.
        let list = w.ok("thread.list", json!({"project": "shop"}));
        assert_eq!(list.as_array().unwrap().iter().filter(|t| t["worktree"] == true).count(), 1);
    }

    #[test]
    fn a_worktree_needs_a_repository_on_a_branch_and_leaves_nothing_when_it_fails() {
        // Not a repository.
        let mut w = world();
        w.d.path_env = format!("{}:{}", w.d.path_env, std::env::var("PATH").unwrap_or_default());
        w.project();
        let err = format!("{:#}", w.call("thread.new", json!({"project": "shop", "worktree": true})).unwrap_err());
        assert!(err.contains("not the top of a git repository"), "{err}");
        assert!(w.ok("thread.list", json!({"archived": true})).as_array().unwrap().is_empty());
        // A folder inside a bigger repository.
        let mut w = git_world();
        let inner = w.repo.join("app");
        std::fs::create_dir_all(&inner).unwrap();
        let inner_s = inner.to_string_lossy().to_string();
        w.ok("project.add", json!({"cwd": inner_s, "name": "app"}));
        let err = format!("{:#}", w.call("thread.new", json!({"project": "app", "worktree": true})).unwrap_err());
        assert!(err.contains("not the top of a git repository"), "{err}");
        // Detached HEAD.
        sh(&w.repo, &["checkout", "-q", "--detach"]);
        let err = format!("{:#}", w.call("thread.new", json!({"project": "shop", "worktree": true})).unwrap_err());
        assert!(err.contains("detached HEAD"), "{err}");
        sh(&w.repo, &["checkout", "-q", "main"]);
        // The engine cannot start: the worktree and its branch are not left behind.
        w.d.agents.refuse = true;
        let err = format!("{:#}", w.call("thread.new", json!({"project": "shop", "worktree": true})).unwrap_err());
        assert!(err.contains("start omp"), "{err}");
        w.settle();
        assert!(w.ok("thread.list", json!({"archived": true})).as_array().unwrap().is_empty());
        assert!(sh(&w.repo, &["branch", "--list", "lila/*"]).is_empty());
        assert_eq!(std::fs::read_dir(w.d.paths.data.join("worktrees/shop")).map(|d| d.count()).unwrap_or(0), 0);
    }

    #[test]
    fn changes_list_every_kind_and_the_diff_is_the_conversations_file_diff() {
        let mut w = git_world();
        let tid = in_worktree(&mut w);
        let wt = worktree_of(&w, &tid);
        commit(&wt, "gone.txt", "bye\n", "add gone");
        commit(&wt, "old.txt", "1\n2\n3\n4\n5\n6\n", "add old");
        write(&wt, "a.txt", "a\nmore\n");
        std::fs::remove_file(wt.join("gone.txt")).unwrap();
        std::fs::rename(wt.join("old.txt"), wt.join("new name.txt")).unwrap();
        write(&wt, "fresh.rs", "fn a() {}\n");
        write(&wt, "pic.bin", "p\0q");

        let c = w.ok("thread.changes", json!({"thread": tid}));
        assert_eq!((c["repo"].clone(), c["branch"].clone(), c["worktree"].clone(), c["base_branch"].clone(), c["ahead"].clone()), (json!(true), json!(format!("lila/{tid}")), json!(true), json!("main"), json!(2)));
        assert_eq!(c["commits"][0]["subject"], "add old");
        let file = |p: &str| c["files"].as_array().unwrap().iter().find(|f| f["path"] == p).unwrap_or_else(|| panic!("{p} in {c}")).clone();
        assert_eq!((file("a.txt")["status"].clone(), file("a.txt")["add"].clone(), file("a.txt")["del"].clone()), (json!("modified"), json!(1), json!(0)));
        assert_eq!((file("gone.txt")["status"].clone(), file("gone.txt")["del"].clone()), (json!("deleted"), json!(1)));
        assert_eq!((file("new name.txt")["status"].clone(), file("new name.txt")["old_path"].clone()), (json!("renamed"), json!("old.txt")));
        assert_eq!((file("fresh.rs")["status"].clone(), file("fresh.rs")["add"].clone()), (json!("untracked"), json!(1)));
        assert_eq!((file("pic.bin")["status"].clone(), file("pic.bin")["binary"].clone()), (json!("untracked"), json!(true)));

        let d = w.ok("thread.diff", json!({"thread": tid, "path": "fresh.rs"}));
        assert_eq!(d["diffs"], json!([{"path": "fresh.rs", "lines": ["@@ -0,0 +1 @@", "+fn a() {}"], "status": "untracked"}]));
        let d = w.ok("thread.diff", json!({"thread": tid, "path": "pic.bin"}));
        assert_eq!((d["diffs"][0]["binary"].clone(), d["diffs"][0]["lines"].clone()), (json!(true), json!([])));
        let all = w.ok("thread.diff", json!({"thread": tid}));
        assert_eq!(all["diffs"].as_array().unwrap().len(), 5);
        assert_eq!(all["truncated"], false);
        // The shape the conversation's diff viewer takes.
        let fd: Vec<lila_agent::FileDiff> = serde_json::from_value(all["diffs"].clone()).unwrap();
        assert!(fd.iter().any(|f| f.path == "a.txt" && f.lines == ["@@ -1 +1,2 @@", " a", "+more"]));
        assert!(w.call("thread.diff", json!({"thread": tid, "path": "../a"})).is_err());

        // Not in git: nothing to list.
        let mut n = world();
        n.d.path_env = w.d.path_env.clone();
        n.project();
        let plain = n.new_thread(json!({}));
        let c = n.ok("thread.changes", json!({"thread": plain}));
        assert_eq!((c["repo"].clone(), c["files"].clone()), (json!(false), json!([])));
        assert_eq!(n.ok("thread.diff", json!({"thread": plain}))["diffs"], json!([]));
    }

    #[test]
    fn a_file_is_read_inside_the_threads_folder_only() {
        let mut w = git_world();
        let tid = in_worktree(&mut w);
        let wt = worktree_of(&w, &tid);
        write(&wt, "src/lib.rs", "pub fn f() {}\n");
        let f = w.ok("thread.file", json!({"thread": tid, "path": "src/lib.rs"}));
        assert_eq!(f, json!({"path": "src/lib.rs", "text": "pub fn f() {}\n", "size": 14}));
        write(&wt, "x.bin", "\0\0");
        assert_eq!(w.ok("thread.file", json!({"thread": tid, "path": "x.bin"}))["binary"], true);
        std::os::unix::fs::symlink(&w.repo, wt.join("out")).unwrap();
        for bad in ["../../a.txt", "/etc/passwd", "out/a.txt", "src/../../x", ""] {
            assert!(w.call("thread.file", json!({"thread": tid, "path": bad})).is_err(), "{bad}");
        }
        assert!(w.call("thread.file", json!({"thread": tid})).is_err());
        assert_eq!(std::fs::read_to_string(w.repo.join("a.txt")).unwrap(), "a\n", "the project folder's own file is not the worktree's");
        assert_eq!(w.ok("thread.file", json!({"thread": tid, "path": "a.txt"}))["text"], "a\n");
    }

    #[test]
    fn the_summary_is_counted_when_a_turn_ends_and_pushed_once() {
        let mut w = git_world();
        let tid = in_worktree(&mut w);
        w.settle();
        let wt = worktree_of(&w, &tid);
        let summary = |w: &mut World| w.ok("thread.show", json!({"thread": tid}))["changes"].clone();
        assert_eq!(summary(&mut w), json!({"files": 0, "add": 0, "del": 0}), "counted when it started");
        w.d.take_dirty();

        // Files change during the turn: nothing is counted (and nobody is told) until it ends.
        w.status(&tid, "working");
        write(&wt, "a.txt", "a\nb\nc\n");
        write(&wt, "new.txt", "1\n2\n");
        w.settle();
        assert!(w.d.take_jobs().is_empty());
        assert_eq!(summary(&mut w), json!({"files": 0, "add": 0, "del": 0}));

        w.d.take_dirty();
        w.status(&tid, "idle");
        w.settle();
        assert_eq!(summary(&mut w), json!({"files": 2, "add": 4, "del": 0}));
        assert!(w.d.take_dirty().0.contains(&tid), "the window is told");
        // The same again: no change, no push.
        w.status(&tid, "working");
        w.status(&tid, "idle");
        w.d.take_dirty();
        w.settle();
        assert!(!w.d.take_dirty().0.contains(&tid), "counting again and finding the same is not news");
        // The turn-end event of a host counts too.
        write(&wt, "third.txt", "x\n");
        w.host_event(&tid, AgentEvent::TurnEnded { turn: "tu1".into(), failed: None });
        w.settle();
        assert_eq!(summary(&mut w)["files"], 3);
        // And the push event carries it.
        let ev = w.d.thread_event(&tid, 7).unwrap();
        assert_eq!(ev["changes"]["files"], 3);
        // A folder outside git has none.
        let mut n = world();
        n.d.path_env = w.d.path_env.clone();
        n.project();
        let plain = n.new_thread(json!({}));
        n.settle();
        assert_eq!(n.ok("thread.show", json!({"thread": plain}))["changes"], Value::Null);
    }

    #[test]
    fn asking_for_a_commit_sends_the_agent_a_note_from_lila() {
        let mut w = git_world();
        let tid = in_worktree(&mut w);
        let wt = worktree_of(&w, &tid);
        let err = |w: &mut World| format!("{:#}", w.call("thread.commit", json!({"thread": tid})).unwrap_err());
        assert!(err(&mut w).contains("nothing to commit"));
        write(&wt, "a.txt", "changed\n");
        w.status(&tid, "working");
        assert!(err(&mut w).contains("working now"));
        w.status(&tid, "idle");
        let before = w.sends_to(&tid).len();
        let r = w.ok("thread.commit", json!({"thread": tid}));
        assert_eq!(r, json!({"sent": true, "thread": tid}));
        let sent = w.sends_to(&tid);
        assert_eq!(sent.len(), before + 1);
        let text = sent.last().unwrap();
        assert!(text.starts_with("<lila ") && text.contains("提交"), "{text}");
        assert!(text.contains("git status") && text.contains("git diff") && text.contains("不要推送"), "{text}");
        assert_eq!(lila_agent::model::origin(text), lila_agent::model::Origin::Lila { task: None }, "a quiet line, not a person's bubble");
        assert!(w.events(&tid).iter().any(|e| e.starts_with("commit ")));
        // Not while it waits or is gone.
        w.ok("thread.stop", json!({"thread": tid}));
        assert!(err(&mut w).contains("has ended"));
        // Outside git.
        let mut n = world();
        n.d.path_env = w.d.path_env.clone();
        n.project();
        let plain = n.new_thread(json!({}));
        n.status(&plain, "idle");
        assert!(format!("{:#}", n.call("thread.commit", json!({"thread": plain})).unwrap_err()).contains("not in a git repository"));
    }

    #[test]
    fn merging_puts_the_threads_work_on_the_base_branch_and_ends_the_thread() {
        let mut w = git_world();
        let tid = in_worktree(&mut w);
        let wt = worktree_of(&w, &tid);
        w.ok("thread.rename", json!({"thread": tid, "title": "Add login"}));
        commit(&wt, "login.rs", "fn login() {}\n", "wip login");
        commit(&wt, "login.rs", "fn login() { 1 }\n", "finish login");
        // The base moved meanwhile.
        write(&w.repo, "other.txt", "o\n");
        sh(&w.repo, &["add", "-A"]);
        sh(&w.repo, &["commit", "-q", "-m", "other work"]);
        let r = w.ok("thread.merge", json!({"thread": tid}));
        assert_eq!((r["merged"].clone(), r["commits"].clone(), r["files"].clone(), r["branch"].clone()), (json!(true), json!(2), json!(1), json!("main")));
        assert_eq!(r["commit"], sh(&w.repo, &["rev-parse", "--short", "HEAD"]).as_str());
        assert_eq!(sh(&w.repo, &["log", "--format=%s"]), "Add login\nother work\nfirst");
        assert_eq!(sh(&w.repo, &["log", "-1", "--format=%B"]), "Add login\n\n- wip login\n- finish login");
        assert_eq!(std::fs::read_to_string(w.repo.join("login.rs")).unwrap(), "fn login() { 1 }\n");
        // The thread: its engine ended, archived, merged; the worktree and its branch go (off the loop).
        let t = &r["thread"];
        assert_eq!((t["archived"].clone(), t["state"].clone(), t["merging"].clone(), t["changes"].clone()), (json!(true), json!("ended"), json!(false), Value::Null));
        assert!(t["merged_ms"].is_number());
        assert_eq!(w.d.agents.ended(), 1);
        w.settle();
        assert!(!wt.exists());
        assert!(sh(&w.repo, &["branch", "--list", "lila/*"]).is_empty());
        assert!(!sh(&w.repo, &["worktree", "list"]).contains(&tid));
        assert!(w.events(&tid).iter().any(|e| e == &format!("merged 2 commit(s) into main as {}", r["commit"].as_str().unwrap())), "{:?}", w.events(&tid));
        assert!(w.ok("thread.list", json!({"project": "shop"})).as_array().unwrap().is_empty());
        // It does not come back.
        assert!(format!("{:#}", w.call("thread.resume", json!({"thread": tid})).unwrap_err()).contains("was merged"));
        assert!(format!("{:#}", w.call("thread.merge", json!({"thread": tid})).unwrap_err()).contains("merged already"));
        assert!(format!("{:#}", w.call("thread.commit", json!({"thread": tid})).unwrap_err()).contains("was merged"));
        assert_eq!(w.ok("thread.changes", json!({"thread": tid}))["repo"], false);
    }

    #[test]
    fn a_thread_whose_agent_has_ended_can_be_merged() {
        let mut w = git_world();
        let tid = in_worktree(&mut w);
        commit(&worktree_of(&w, &tid), "b.txt", "b\n", "Add b");
        w.ok("thread.stop", json!({"thread": tid}));
        let ended = w.d.agents.ended();
        let r = w.ok("thread.merge", json!({"thread": tid}));
        assert_eq!(r["merged"], true);
        assert_eq!(w.d.agents.ended(), ended, "no engine to end");
        assert_eq!(sh(&w.repo, &["log", "--format=%s"]), "Add b\nfirst");
    }

    #[test]
    fn one_commit_keeps_its_own_message() {
        let mut w = git_world();
        let tid = in_worktree(&mut w);
        commit(&worktree_of(&w, &tid), "b.txt", "b\n", "Add b\n\nBecause.");
        w.ok("thread.merge", json!({"thread": tid}));
        assert_eq!(sh(&w.repo, &["log", "-1", "--format=%B"]), "Add b\n\nBecause.");
        assert_eq!(sh(&w.repo, &["log", "--format=%s"]), "Add b\nfirst");
    }

    #[test]
    fn merging_is_refused_when_it_would_lose_or_not_do_anything() {
        let mut w = git_world();
        let tid = in_worktree(&mut w);
        let wt = worktree_of(&w, &tid);
        let refused = |w: &mut World, tid: &str, expect: &str| {
            let err = format!("{:#}", w.call("thread.merge", json!({"thread": tid})).unwrap_err());
            assert!(err.contains(expect), "{err}");
            w.settle();
            assert_eq!(w.state(tid), "idle", "the thread is as it was");
            assert_eq!(w.ok("thread.show", json!({"thread": tid}))["merging"], false);
        };
        // A thread that is not in a worktree.
        let plain = w.new_thread(json!({}));
        w.status(&plain, "idle");
        refused(&mut w, &plain, "does not work in a worktree");
        // No commits.
        refused(&mut w, &tid, "nothing to merge");
        // Uncommitted work.
        commit(&wt, "b.txt", "b\n", "Add b");
        write(&wt, "loose.txt", "x");
        refused(&mut w, &tid, "uncommitted changes; commit them first");
        std::fs::remove_file(wt.join("loose.txt")).unwrap();
        // The project folder is on another branch.
        sh(&w.repo, &["checkout", "-q", "-b", "elsewhere"]);
        refused(&mut w, &tid, "on elsewhere, not on main");
        sh(&w.repo, &["checkout", "-q", "main"]);
        // The person's own uncommitted change to a file the merge brings.
        write(&w.repo, "b.txt", "mine\n");
        refused(&mut w, &tid, "uncommitted changes to files this merge changes: b.txt");
        std::fs::remove_file(w.repo.join("b.txt")).unwrap();
        // While it works.
        w.status(&tid, "working");
        refused_running(&mut w, &tid);
        w.status(&tid, "idle");
        // The refusals changed nothing: the work is all still there.
        assert_eq!(sh(&wt, &["log", "--format=%s", "main..HEAD"]), "Add b");
        assert_eq!(sh(&w.repo, &["log", "--format=%s"]), "first");
        assert!(wt.exists());
        // A conflict is recorded in the thread's log.
        write(&w.repo, "b.txt", "conflicting\n");
        sh(&w.repo, &["add", "-A"]);
        sh(&w.repo, &["commit", "-q", "-m", "b on main"]);
        refused(&mut w, &tid, "conflict");
        assert!(w.events(&tid).iter().any(|e| e.starts_with("merge_failed ") && e.contains("b.txt")), "{:?}", w.events(&tid));
        assert_eq!(sh(&wt, &["log", "--format=%s", "main..HEAD"]), "Add b", "its commit is as it was");
        assert_eq!(sh(&wt, &["status", "--porcelain"]), "");
        // The thread goes on: it can still be told to resolve it.
        assert!(w.call("thread.send", json!({"thread": tid, "text": "bring main in"})).is_ok());
    }

    fn refused_running(w: &mut World, tid: &str) {
        let err = format!("{:#}", w.call("thread.merge", json!({"thread": tid})).unwrap_err());
        assert!(err.contains("working now"), "{err}");
    }

    #[test]
    fn nothing_reaches_a_thread_while_it_is_being_merged() {
        let mut w = git_world();
        let tid = in_worktree(&mut w);
        commit(&worktree_of(&w, &tid), "b.txt", "b\n", "Add b");
        w.now += 1000;
        let Begin::Job(job) = w.d.begin("thread.merge", &json!({"thread": tid}), w.now).unwrap() else { panic!("a merge is a job") };
        assert_eq!(w.ok("thread.show", json!({"thread": tid}))["merging"], true);
        for (method, params) in [("thread.send", json!({"thread": tid, "text": "x"})), ("thread.merge", json!({"thread": tid})), ("thread.commit", json!({"thread": tid})), ("thread.resume", json!({"thread": tid}))] {
            let err = format!("{:#}", w.call(method, params).unwrap_err());
            assert!(err.contains("being merged") || err.contains("is running"), "{method}: {err}");
        }
        let result = job.run();
        w.d.finish(job, result, w.now).unwrap();
        assert_eq!(w.ok("thread.show", json!({"thread": tid}))["merging"], false);
    }

    #[test]
    fn archiving_a_worktree_thread_keeps_its_worktree() {
        let mut w = git_world();
        let tid = in_worktree(&mut w);
        let wt = worktree_of(&w, &tid);
        commit(&wt, "b.txt", "b\n", "Add b");
        w.ok("thread.archive", json!({"thread": tid}));
        w.settle();
        assert!(wt.join("b.txt").exists(), "the work is still there");
        assert!(sh(&w.repo, &["branch", "--list", "lila/*"]).contains(&tid));
        let j = w.ok("thread.unarchive", json!({"thread": tid}));
        assert_eq!((j["archived"].clone(), j["worktree"].clone(), j["merged_ms"].clone()), (json!(false), json!(true), Value::Null));
        // And it goes on: resumed in the same worktree.
        w.ok("thread.resume", json!({"thread": tid}));
        assert_eq!(w.d.agents.runs.last().unwrap().cwd, wt);
        // Its folder deleted by hand: said so.
        w.ok("thread.stop", json!({"thread": tid}));
        sh(&w.repo, &["worktree", "remove", "--force", &wt.to_string_lossy()]);
        assert!(format!("{:#}", w.call("thread.resume", json!({"thread": tid})).unwrap_err()).contains("is gone"));
    }

    #[test]
    fn lilad_counts_every_threads_changes_when_it_starts() {
        let mut w = git_world();
        let tid = in_worktree(&mut w);
        write(&worktree_of(&w, &tid), "x.txt", "x\n");
        // A new lilad on the same data.
        let store = Store::open(&w.d.paths.db()).unwrap();
        let mut d = Daemon::new(store, Fake::default(), w.d.paths.clone(), w.d.path_env.clone());
        let jobs = d.take_jobs();
        assert_eq!(jobs.len(), 1);
        for job in jobs {
            let r = job.run();
            d.finish(job, r, 5).unwrap();
        }
        let t = d.store.thread(&tid).unwrap().unwrap();
        assert_eq!(d.thread_json(&t)["changes"], json!({"files": 1, "add": 1, "del": 0}));
    }
}

// ---------------- checkpoints and going back (A1) ----------------

mod a1 {
    use super::p2::{commit, git_world, in_worktree, write};
    use super::*;
    use crate::worktree::tests::sh;
    use lila_agent::{Origin, model::without_notes};

    fn checkpoints(w: &mut World, tid: &str) -> Vec<Value> {
        w.ok("thread.checkpoints", json!({"thread": tid}))["checkpoints"].as_array().unwrap().clone()
    }

    fn user_item(id: &str, text: &str, at_ms: i64) -> AgentEvent {
        AgentEvent::Item { item: Item { id: id.into(), turn: Some("tu".into()), at_ms, state: ItemState::Done, kind: ItemKind::UserMessage { text: text.into(), origin: Origin::Person, attachments: vec![] } } }
    }

    fn refs(w: &World) -> String {
        sh(&w.repo, &["for-each-ref", "--format=%(refname)", "refs/lila"])
    }

    /// A thread in the project folder, idle, whose first message ("go") has been delivered and its turn is over.
    fn after_first_turn(w: &mut World) -> String {
        let tid = w.new_thread(json!({"text": "go"}));
        w.status(&tid, "idle");
        w.status(&tid, "working");
        w.status(&tid, "idle");
        tid
    }

    #[test]
    fn a_turn_waits_for_its_checkpoint_which_holds_the_folder_as_it_was() {
        let mut w = git_world();
        write(&w.repo, "a.txt", "mine, before the turn\n");
        write(&w.repo, "fresh.txt", "untracked\n");
        let tid = w.new_thread(json!({"text": "hello"}));
        w.status_raw(&tid, "idle");
        // The words are held while the folder is saved; lilad's loop is not the one that runs git.
        assert!(w.sends_to(&tid).is_empty(), "omp has not been given the words");
        assert!(w.d.snapshotting.contains(&tid));
        assert_eq!(w.d.store.outbox(&tid).unwrap().len(), 1);
        w.settle_checkpoints();
        assert_eq!(w.sends_to(&tid), ["hello"]);
        assert!(w.d.store.outbox(&tid).unwrap().is_empty() && w.d.snapshotting.is_empty());
        // What omp does next is not in it.
        write(&w.repo, "a.txt", "the agent's edit\n");
        let rows = checkpoints(&mut w, &tid);
        assert_eq!(rows.len(), 1);
        let (c, sha) = (&rows[0], w.d.store.checkpoint(&tid, 1).unwrap().unwrap().sha);
        assert_eq!((c["id"].clone(), c["kind"].clone(), c["title"].clone(), c["item"].clone(), c["restorable"].clone(), c["branch"].clone()), (json!(1), json!("turn"), json!("hello"), Value::Null, json!(true), json!("main")));
        assert_eq!(sh(&w.repo, &["show", &format!("{sha}:a.txt")]), "mine, before the turn");
        assert_eq!(sh(&w.repo, &["show", &format!("{sha}:fresh.txt")]), "untracked");
        assert_eq!(c["head"], sh(&w.repo, &["rev-parse", "--short=7", "HEAD"]).as_str());
        assert_eq!(sh(&w.repo, &["rev-parse", &format!("refs/lila/checkpoints/{tid}/1")]), sha, "kept alive by a ref");
    }

    #[test]
    fn a_checkpoint_is_attached_to_the_message_omp_echoes() {
        let mut w = git_world();
        let tid = after_first_turn(&mut w);
        // omp has not echoed the message yet: the checkpoint has no item.
        assert_eq!(checkpoints(&mut w, &tid)[0]["item"], Value::Null);
        let t = w.now;
        w.host_event(&tid, user_item("u1", "go", t + 500));
        w.host_event(&tid, AgentEvent::TurnEnded { turn: "tu".into(), failed: None });
        // Two messages with the same words, two turns: each finds its own.
        w.ok("thread.send", json!({"thread": tid, "text": "go"}));
        w.settle_checkpoints();
        let t = w.now;
        w.host_event(&tid, user_item("u2", "go", t + 500));
        let read = w.ok("thread.read", json!({"thread": tid}));
        let list: Vec<(i64, Option<&str>)> = read["checkpoints"].as_array().unwrap().iter().map(|c| (c["id"].as_i64().unwrap(), c["item"].as_str())).collect();
        assert_eq!(list, [(2, Some("u2")), (1, Some("u1"))], "newest first, in thread.read as in thread.checkpoints");
        assert_eq!(checkpoints(&mut w, &tid), read["checkpoints"].as_array().unwrap().clone());
        // It is remembered in the row.
        assert_eq!(w.d.store.checkpoint(&tid, 1).unwrap().unwrap().item.as_deref(), Some("u1"));
    }

    #[test]
    fn omp_takes_no_words_while_it_works_so_they_wait_for_their_checkpoint() {
        let mut w = git_world();
        let omp = w.new_thread(json!({"text": "one"}));
        w.status(&omp, "idle");
        w.status(&omp, "working");
        w.ok("thread.send", json!({"thread": omp, "text": "two"}));
        assert_eq!(w.sends_to(&omp), ["one"]);
        assert_eq!(w.d.store.outbox(&omp).unwrap().len(), 1);
        assert_eq!(checkpoints(&mut w, &omp).len(), 1);
        w.status_raw(&omp, "idle");
        assert_eq!(w.sends_to(&omp), ["one"], "not before the folder is saved");
        w.settle_checkpoints();
        assert_eq!(w.sends_to(&omp), ["one", "two"]);
        assert_eq!(checkpoints(&mut w, &omp).len(), 2);
    }

    #[test]
    fn at_most_fifty_checkpoints_are_kept_and_a_failure_never_stops_the_message() {
        let mut w = git_world();
        let tid = w.new_thread(json!({}));
        w.status(&tid, "idle");
        for i in 1..=52 {
            write(&w.repo, "f.txt", &format!("{i}\n"));
            w.ok("thread.send", json!({"thread": tid, "text": format!("turn {i}")}));
            w.status(&tid, "working");
            w.status(&tid, "idle");
        }
        let rows = checkpoints(&mut w, &tid);
        assert_eq!((rows.len(), rows[0]["id"].clone(), rows[49]["id"].clone()), (50, json!(52), json!(3)));
        assert_eq!(refs(&w).lines().count(), 50);
        assert!(w.d.store.checkpoint(&tid, 2).unwrap().is_none());

        // git cannot be run: recorded, and the words go.
        let other = w.new_thread(json!({}));
        w.status(&other, "idle");
        w.d.path_env = String::new();
        w.ok("thread.send", json!({"thread": other, "text": "still goes"}));
        assert_eq!(w.sends_to(&other), ["still goes"]);
        assert!(checkpoints(&mut w, &other).is_empty());
        assert!(w.events(&other).iter().any(|e| e.starts_with("checkpoint_failed ")), "{:?}", w.events(&other));
    }

    #[test]
    fn a_folder_outside_git_has_no_checkpoints() {
        let mut w = world();
        w.project();
        let tid = w.new_thread(json!({"text": "hi"}));
        w.status_raw(&tid, "idle");
        assert_eq!(w.sends_to(&tid), ["hi"], "no job, no wait");
        assert!(w.d.take_jobs().iter().all(|j| !j.is_checkpoint()));
        assert_eq!(w.ok("thread.checkpoints", json!({"thread": tid}))["checkpoints"], json!([]));
        let err = format!("{:#}", w.call("thread.restore", json!({"thread": tid, "checkpoint": 1})).unwrap_err());
        assert!(err.contains("no checkpoint 1"), "{err}");
    }

    #[test]
    fn putting_the_files_back_changes_them_and_the_conversation_gets_a_mark_but_starts_no_turn() {
        let mut w = git_world();
        write(&w.repo, "keep.txt", "keep\n");
        write(&w.repo, "gone.txt", "gone\n");
        sh(&w.repo, &["add", "-A"]);
        sh(&w.repo, &["commit", "-q", "-m", "files"]);
        write(&w.repo, "a.txt", "a, before\n");
        let tid = after_first_turn(&mut w);
        // The agent's turn.
        write(&w.repo, "a.txt", "broken\n");
        std::fs::remove_file(w.repo.join("gone.txt")).unwrap();
        write(&w.repo, "new/file.txt", "made by the agent\n");
        write(&w.repo, "target/cache", "ignored\n");
        let t = w.now;
        w.host_event(&tid, user_item("u1", "go", t + 100));
        w.settle();
        let head = sh(&w.repo, &["rev-parse", "HEAD"]);
        let files = |w: &World| ["a.txt", "gone.txt", "new/file.txt", "target/cache"].map(|f| std::fs::read_to_string(w.repo.join(f)).ok());
        let broken = files(&w);

        // A dry run changes nothing.
        let seq = w.d.store.last_seq(&tid).unwrap();
        let (log, rows, refs_before, status) = (w.events(&tid), checkpoints(&mut w, &tid).len(), refs(&w), sh(&w.repo, &["status", "--porcelain=v2"]));
        let dry = w.ok("thread.restore", json!({"thread": tid, "checkpoint": 1, "dry_run": true}));
        assert_eq!(
            dry,
            json!({"thread": tid, "checkpoint": 1, "dry_run": true, "restored": false, "undo": null, "head_moved": false,
                   "head": {"then": &head[..7], "now": &head[..7]},
                   "changed": ["a.txt"], "recreated": ["gone.txt"], "deleted": ["new/file.txt"],
                   "counts": {"changed": 1, "recreated": 1, "deleted": 1}, "truncated": false})
        );
        assert_eq!((files(&w), w.d.store.last_seq(&tid).unwrap(), w.events(&tid), checkpoints(&mut w, &tid).len(), refs(&w), sh(&w.repo, &["status", "--porcelain=v2"])), (broken.clone(), seq, log, rows, refs_before, status));
        assert!(w.d.store.outbox(&tid).unwrap().is_empty());

        // The real one.
        let sends = w.sends_to(&tid).len();
        let r = w.ok("thread.restore", json!({"thread": tid, "checkpoint": 1}));
        let undo = r["undo"].as_i64().expect("the way back");
        assert_eq!((r["restored"].clone(), r["dry_run"].clone(), r["counts"].clone(), r["head_moved"].clone()), (json!(true), json!(false), dry["counts"].clone(), json!(false)));
        assert_eq!(files(&w), [Some("a, before\n".into()), Some("gone\n".into()), None, Some("ignored\n".into())], "ignored files are left alone");
        assert!(!w.repo.join("new").exists());
        assert_eq!(sh(&w.repo, &["rev-parse", "HEAD"]), head);
        // It is recorded; the conversation has its mark; the agent is told with the next message, and no turn begins.
        assert!(w.events(&tid).iter().any(|e| e.starts_with("restored checkpoint 1 (go): 1 put back, 1 made again, 1 deleted; undo is checkpoint")), "{:?}", w.events(&tid));
        let read = w.ok("thread.read", json!({"thread": tid}));
        let mark = read["items"].as_array().unwrap().iter().find(|i| i["from"] == "restored").expect("a mark in the conversation").clone();
        assert_eq!((mark["kind"].clone(), mark["checkpoint"].clone(), mark["undo"].clone(), mark["state"].clone()), (json!("user_message"), json!(1), json!(undo), json!("done")));
        let rows = read["checkpoints"].as_array().unwrap();
        assert_eq!((rows[0]["id"].clone(), rows[0]["kind"].clone(), rows[0]["item"].clone()), (json!(undo), json!("restore"), Value::Null));
        w.now += 60_000;
        w.d.tick(w.now);
        w.settle_checkpoints();
        assert_eq!(w.sends_to(&tid).len(), sends, "a restore sends the engine nothing by itself");
        assert_eq!(w.state(&tid), "idle");
        let held = w.d.store.outbox(&tid).unwrap();
        assert!(held.len() == 1 && held[0].hold && held[0].text.starts_with("<lila note about=\"restore\">") && held[0].text.contains("go") && held[0].text.contains("read a file again"), "{held:?}");
        // The summary of changes is counted again.
        assert!(w.d.take_jobs().iter().any(|j| j.serial()));
        // The next message takes the note along, in front of it, in one message and one turn.
        w.ok("thread.send", json!({"thread": tid, "text": "try again"}));
        let sent = w.sends_to(&tid);
        assert_eq!(sent.len(), sends + 1);
        assert!(sent.last().unwrap().starts_with("<lila note about=\"restore\">"), "{sent:?}");
        assert_eq!(without_notes(sent.last().unwrap()), "try again");
        assert!(w.d.store.outbox(&tid).unwrap().is_empty());
        // That turn has its checkpoint, and the way back works: the state before the restore.
        write(&w.repo, "a.txt", "worse\n");
        w.ok("thread.restore", json!({"thread": tid, "checkpoint": undo}));
        assert_eq!(files(&w), broken, "as broken as it was, ignored files and all");
        // It all survives lilad starting again.
        let store = Store::open(&w.d.paths.db()).unwrap();
        let mut d = Daemon::new(store, Fake::default(), w.d.paths.clone(), w.d.path_env.clone());
        let read = d.handle("thread.read", &json!({"thread": tid}), w.now + 1000).unwrap();
        assert!(read["items"].as_array().unwrap().iter().filter(|i| i["from"] == "restored").count() == 2 && read["checkpoints"].as_array().unwrap().len() >= 4);
    }

    #[test]
    fn commits_since_the_checkpoint_are_reported_and_stay() {
        let mut w = git_world();
        let tid = after_first_turn(&mut w);
        let before = sh(&w.repo, &["rev-parse", "HEAD"]);
        commit(&w.repo, "b.txt", "committed by the agent\n", "agent work");
        let head = sh(&w.repo, &["rev-parse", "HEAD"]);
        let r = w.ok("thread.restore", json!({"thread": tid, "checkpoint": 1}));
        assert_eq!((r["head_moved"].clone(), r["head"].clone()), (json!(true), json!({"then": &before[..7], "now": &head[..7]})));
        assert_eq!(r["deleted"], json!(["b.txt"]), "the files go back; the commit does not");
        assert_eq!(sh(&w.repo, &["rev-parse", "HEAD"]), head);
        assert_eq!(sh(&w.repo, &["status", "--porcelain"]), "D b.txt", "the panel shows it as a change");
        let note = &w.d.store.outbox(&tid).unwrap()[0].text;
        assert!(note.contains("Commits made since then were not undone"), "{note}");
    }

    #[test]
    fn a_restore_is_refused_while_anything_could_still_change_the_files() {
        let mut w = git_world();
        let tid = after_first_turn(&mut w);
        let other = w.new_thread(json!({}));
        w.status(&other, "idle");
        let refused = |w: &mut World, tid: &str, expect: &str| {
            for dry_run in [true, false] {
                let err = format!("{:#}", w.call("thread.restore", json!({"thread": tid, "checkpoint": 1, "dry_run": dry_run})).unwrap_err());
                assert!(err.contains(expect), "{err}");
            }
            let c = w.ok("thread.checkpoints", json!({"thread": tid}));
            assert_eq!((c["can_restore"].clone(), c["checkpoints"][0]["restorable"].clone()), (json!(false), json!(false)));
            assert!(c["why_not"].as_str().unwrap().contains(expect));
        };
        // Working; waiting for an answer.
        w.status(&tid, "working");
        refused(&mut w, &tid, "working now");
        w.status(&tid, "blocked");
        refused(&mut w, &tid, "waiting for an answer");
        w.status(&tid, "idle");
        // Another thread working in the same folder.
        w.status(&other, "working");
        refused(&mut w, &tid, &format!("{other} is working in the same folder"));
        w.status(&other, "idle");
        // Words not taken yet.
        w.d.senders.remove(&tid);
        w.ok("thread.send", json!({"thread": tid, "text": "queued"}));
        refused(&mut w, &tid, "message waiting");
        // No checkpoint of that number; and not while it is being restored (what the loop sees in between).
        w.d.store.pop_outbox(w.d.store.outbox(&tid).unwrap()[0].id).unwrap();
        let err = format!("{:#}", w.call("thread.restore", json!({"thread": tid, "checkpoint": 99})).unwrap_err());
        assert!(err.contains("no checkpoint 99"), "{err}");
        w.now += 1000;
        let Begin::Job(job) = w.d.begin("thread.restore", &json!({"thread": tid, "checkpoint": 1}), w.now).unwrap() else { panic!("a restore is a job") };
        assert_eq!(w.ok("thread.show", json!({"thread": tid}))["restoring"], true);
        refused(&mut w, &tid, "being restored");
        assert!(w.call("thread.send", json!({"thread": tid, "text": "x"})).is_err());
        let result = job.run();
        w.d.finish(job, result, w.now).unwrap();
        assert_eq!(w.ok("thread.show", json!({"thread": tid}))["restoring"], false);
        // An ended thread can be restored; one whose repository is gone says so and changes nothing.
        w.ok("thread.stop", json!({"thread": tid}));
        w.ok("thread.restore", json!({"thread": tid, "checkpoint": 1, "dry_run": true}));
        std::fs::rename(w.repo.join(".git"), w.repo.join(".git.away")).unwrap();
        let err = format!("{:#}", w.call("thread.restore", json!({"thread": tid, "checkpoint": 1})).unwrap_err());
        assert!(err.contains("not in a git repository"), "{err}");
        assert!(w.events(&tid).iter().any(|e| e.starts_with("restore_failed ")));
        assert_eq!(w.ok("thread.show", json!({"thread": tid}))["restoring"], false);
    }

    #[test]
    fn a_threads_refs_go_when_its_worktree_is_merged_and_when_its_project_is_removed() {
        let mut w = git_world();
        let tid = in_worktree(&mut w);
        w.ok("thread.send", json!({"thread": tid, "text": "more"}));
        commit(&PathBuf::from(w.d.store.thread(&tid).unwrap().unwrap().cwd), "w.txt", "w\n", "work");
        let plain = after_first_turn(&mut w);
        assert!(refs(&w).contains(&format!("checkpoints/{tid}/")) && refs(&w).contains(&format!("checkpoints/{plain}/")), "{}", refs(&w));
        w.ok("thread.merge", json!({"thread": tid}));
        w.settle();
        assert!(!refs(&w).contains(&tid) && refs(&w).contains(&plain), "only the merged thread's refs go:\n{}", refs(&w));
        assert!(w.d.store.checkpoints(&tid).unwrap().is_empty());
        assert!(format!("{:#}", w.call("thread.restore", json!({"thread": tid, "checkpoint": 1})).unwrap_err()).contains("was merged"));
        w.ok("project.remove", json!({"project": "shop"}));
        w.settle();
        assert_eq!(refs(&w), "");
        assert!(w.d.store.checkpoints(&plain).unwrap().is_empty());
    }
}
