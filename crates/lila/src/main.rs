//! `lila`: the command line of Lila. The person uses it to add projects and talk to
//! their threads (the window does the same through the same socket); lilad uses it
//! to run omp (`lila host omp`).

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use lila::client;
use lila::paths::Paths;
use serde_json::{Value, json};

#[derive(Parser)]
#[command(name = "lila", version, about = "An agent with an interface of its own: projects, and the threads you have in them.")]
struct Cli {
    /// Print raw JSON instead of text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start lilad if needed, and add this folder as a project.
    Up,
    /// Projects: the folders threads work in.
    Project {
        #[command(subcommand)]
        cmd: ProjectCmd,
    },
    /// The models omp offers (`provider/model`, the ids `--model` and a thread's model take).
    Models {
        /// Ask omp again instead of using the list lilad remembers.
        #[arg(long)]
        refresh: bool,
    },
    /// Threads: conversations with an agent.
    Thread {
        #[command(subcommand)]
        cmd: ThreadCmd,
    },
    /// lilad, the background process that keeps the state and runs omp.
    Daemon {
        #[command(subcommand)]
        cmd: DaemonCmd,
    },
}

#[derive(Subcommand)]
enum ProjectCmd {
    /// The projects.
    List,
    /// Add a folder (default: this one) as a project.
    Add {
        path: Option<std::path::PathBuf>,
        /// The project's id (default: from the folder's name).
        #[arg(long)]
        name: Option<String>,
        /// What the window calls it.
        #[arg(long)]
        title: Option<String>,
    },
    /// Forget a project and its threads (the folder stays).
    Remove { project: Option<String> },
    /// A project, with its defaults.
    Show { project: Option<String> },
    /// Change a project's title or the defaults its new threads start with.
    Set {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        model: Option<String>,
        /// read_only, confirm or full.
        #[arg(long)]
        permission: Option<String>,
    },
}

#[derive(Subcommand)]
enum ThreadCmd {
    /// Start a thread, with a first message.
    New {
        /// The first message.
        text: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        model: Option<String>,
        /// read_only, confirm or full (default: the project's).
        #[arg(long)]
        permission: Option<String>,
        /// A file to send along (repeatable).
        #[arg(long = "file")]
        files: Vec<std::path::PathBuf>,
        /// Work in a git worktree of its own, on its own branch (the project folder must be a repository on a branch).
        #[arg(long)]
        worktree: bool,
    },
    /// The project's threads, or every project's with --all.
    List {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        all: bool,
        /// Include archived threads.
        #[arg(long)]
        archived: bool,
    },
    /// A thread: its state and conversation.
    Show {
        thread: String,
        /// Only items after this seq.
        #[arg(long, default_value_t = 0)]
        after: i64,
    },
    /// Send a message.
    Send {
        thread: String,
        text: String,
        #[arg(long = "file")]
        files: Vec<std::path::PathBuf>,
    },
    /// Stop what the agent is doing this turn.
    Interrupt { thread: String },
    /// End the agent; the conversation stays and can be resumed.
    Stop { thread: String },
    /// Start an ended thread again, on the omp session it had.
    Resume {
        thread: String,
        /// A message to send once it is up.
        text: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        permission: Option<String>,
    },
    /// Hide a thread (ends its agent).
    Archive { thread: String },
    Unarchive { thread: String },
    Rename { thread: String, title: String },
    /// Switch a thread's permission tier: read_only, confirm or full.
    Permission { thread: String, tier: String },
    /// Answer a card. A permission card: allow, deny, always (allow what the card's option says for the rest of the thread), or one of its options.
    /// A question card: a choice (its words or its number), yes or no, or words of your own; --cancel gives no answer.
    Answer {
        thread: String,
        card: String,
        answer: Option<String>,
        /// Give no answer to a question.
        #[arg(long)]
        cancel: bool,
    },
    /// Change the model or effort (at once for a running thread, among the choices omp lists).
    Model {
        thread: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        effort: Option<String>,
    },
    /// What the thread's folder has changed since its HEAD: files, counts, and for a worktree its commits.
    Changes { thread: String },
    /// The diff of the changed files, or of one.
    Diff {
        thread: String,
        /// A path relative to the thread's folder.
        path: Option<String>,
    },
    /// Ask the agent to commit all the changes (it writes the message; it does not push).
    Commit { thread: String },
    /// A worktree thread's work becomes one commit on the branch it started from; the worktree goes and the thread is archived.
    Merge { thread: String },
    /// The snapshots of the thread's folder lilad took before its turns (newest first), with the message each came before.
    Checkpoints { thread: String },
    /// Put the thread's files back to how they were before a turn (the conversation stays; the agent is told with your next message).
    Restore {
        thread: String,
        /// A number from `lila thread checkpoints`.
        checkpoint: i64,
        /// Only say which files would be put back, made again or deleted.
        #[arg(long)]
        dry_run: bool,
    },
    /// What was allowed "always" for the thread (`lila thread answer <id> <card> always`): its rules, or with --remove one taken back.
    Rules {
        thread: String,
        /// The number of the rule to take back.
        #[arg(long)]
        remove: Option<i64>,
    },
    /// What lilad did for a thread on its own: starts, ends, answers by the tier, cards.
    Log {
        thread: String,
        #[arg(short = 'n', default_value_t = 50)]
        limit: u64,
    },
}

#[derive(Subcommand)]
enum DaemonCmd {
    /// Run in the foreground (what `lila up` starts inside herdr-core).
    Run,
    /// Stop the running daemon. The omp of its running threads ends with it.
    Stop,
    /// Whether it is running, and whether it speaks this `lila`'s protocol.
    Status,
    /// Stop it, wait until its socket is gone, start it again from this `lila`. The omp of its running threads ends with it.
    Restart,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("lila: {e:#}");
        std::process::exit(1);
    }
}

fn cwd() -> Result<String> {
    Ok(std::env::current_dir()?.to_string_lossy().to_string())
}

/// `{project}` when named, else the folder we are in (lilad finds the project that holds it).
fn at(project: Option<String>) -> Result<Value> {
    Ok(match project {
        Some(p) => json!({"project": p}),
        None => json!({"cwd": cwd()?}),
    })
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let Some(cmd) = cli.cmd else {
        use clap::CommandFactory;
        Cli::command().print_help()?;
        return Ok(());
    };
    let paths = Paths::from_env();
    let out = |v: Value, text: &dyn Fn(&Value) -> String| {
        if cli.json {
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        } else {
            let t = text(&v);
            if !t.is_empty() {
                println!("{t}");
            }
        }
    };
    let call = |method: &str, params: Value| client::request(&paths, method, params, false);

    match cmd {
        Cmd::Up => {
            let v = client::request(&paths, "project.add", json!({"cwd": cwd()?}), true)?;
            out(v, &fmt_project);
        }
        Cmd::Project { cmd } => match cmd {
            ProjectCmd::List => out(call("project.list", json!({}))?, &|v| v.as_array().into_iter().flatten().map(fmt_project).collect::<Vec<_>>().join("\n")),
            ProjectCmd::Add { path, name, title } => {
                let dir = match path {
                    Some(p) => std::fs::canonicalize(p)?.to_string_lossy().to_string(),
                    None => cwd()?,
                };
                out(client::request(&paths, "project.add", json!({"cwd": dir, "name": name, "title": title}), true)?, &fmt_project)
            }
            ProjectCmd::Remove { project } => out(call("project.remove", at(project)?)?, &|v| format!("{} removed; stopped: {}", v["project"].as_str().unwrap_or("?"), list(&v["stopped"]))),
            ProjectCmd::Show { project } => out(call("project.show", at(project)?)?, &fmt_project),
            ProjectCmd::Set { project, title, model, permission } => {
                let mut p = at(project)?;
                p["title"] = json!(title);
                let defaults: serde_json::Map<String, Value> = [("model", model), ("permission", permission)].into_iter().filter_map(|(k, v)| v.map(|v| (k.to_owned(), json!(v)))).collect();
                p["defaults"] = Value::Object(defaults);
                out(call("project.set", p)?, &fmt_project)
            }
        },
        Cmd::Models { refresh } => out(call("models.list", json!({"refresh": refresh}))?, &|v| {
            let rows: Vec<String> = v["models"].as_array().into_iter().flatten().map(|m| format!("{:<44} {}", m["id"].as_str().unwrap_or(""), m["name"].as_str().unwrap_or(""))).collect();
            if rows.is_empty() { "no models".into() } else { rows.join("\n") }
        }),
        Cmd::Thread { cmd } => thread(cmd, &call, &out)?,
        Cmd::Daemon { cmd: DaemonCmd::Run } => lila::daemon::serve(paths.clone())?,
        Cmd::Daemon { cmd: DaemonCmd::Stop } => {
            if client::daemon_state(&paths)? {
                call("stop", json!({}))?;
                println!("lilad stopped; the omp of its running threads ended with it");
            } else {
                println!("lilad is not running; nothing to stop");
            }
        }
        Cmd::Daemon { cmd: DaemonCmd::Status } => {
            let state = client::daemon_version(&paths);
            if cli.json {
                println!("{}", daemon_status_json(&state));
            } else {
                println!("{}", daemon_status_text(&state, &paths.socket()?.display().to_string()));
            }
            match state {
                client::DaemonVersion::Current(_) => {}
                client::DaemonVersion::NotRunning => std::process::exit(3),
                client::DaemonVersion::Different(_) => std::process::exit(4),
                client::DaemonVersion::Unreachable(e) => bail!("{e}"),
            }
        }
        Cmd::Daemon { cmd: DaemonCmd::Restart } => {
            client::restart_daemon(&paths)?;
            println!("lilad restarted; the omp of its threads ended with the old one");
        }
    }
    Ok(())
}

/// The words of `lila daemon status`.
fn daemon_status_text(state: &client::DaemonVersion, socket: &str) -> String {
    use client::DaemonVersion::*;
    let ours = lila::PROTOCOL;
    let who = |i: &client::DaemonInfo| format!("{socket}, version {}, protocol {}", i.version.as_deref().unwrap_or("unknown"), i.protocol.map_or("unknown".into(), |p| p.to_string()));
    match state {
        Current(i) => format!("lilad is running ({})", who(i)),
        Different(i) => format!(
            "lilad is running ({}), but this lila speaks protocol {ours}: it is an older or newer lilad.\nRestart it with `lila daemon restart`, or `lila daemon stop` (any lila command starts it again). The omp of its running threads ends with it.\nIf it differs again, the `lila` on disk is the old one: `cargo install --path crates/lila`.",
            who(i)
        ),
        NotRunning => "lilad is not running".into(),
        Unreachable(e) => format!("lilad is not reachable: {e}"),
    }
}

fn daemon_status_json(state: &client::DaemonVersion) -> Value {
    use client::DaemonVersion::*;
    let info = |i: &client::DaemonInfo| json!({"pid": i.pid, "version": i.version, "protocol": i.protocol});
    match state {
        Current(i) => json!({"running": true, "current": true, "daemon": info(i), "protocol": lila::PROTOCOL}),
        Different(i) => json!({"running": true, "current": false, "daemon": info(i), "protocol": lila::PROTOCOL}),
        NotRunning => json!({"running": false}),
        Unreachable(e) => json!({"running": null, "error": e}),
    }
}

type Out<'a> = dyn Fn(Value, &dyn Fn(&Value) -> String) + 'a;

fn files_of(files: Vec<std::path::PathBuf>) -> Result<Vec<String>> {
    files.into_iter().map(|f| Ok(std::fs::canonicalize(&f).with_context(|| format!("{}", f.display()))?.to_string_lossy().to_string())).collect()
}

fn thread(cmd: ThreadCmd, call: &dyn Fn(&str, Value) -> Result<Value>, out: &Out) -> Result<()> {
    match cmd {
        ThreadCmd::New { text, project, title, model, permission, files, worktree } => {
            let mut p = at(project)?;
            p["text"] = json!(text);
            p["title"] = json!(title);
            p["model"] = json!(model);
            p["permission"] = json!(permission);
            p["attachments"] = json!(files_of(files)?);
            p["worktree"] = json!(worktree);
            out(call("thread.new", p)?, &fmt_thread)
        }
        ThreadCmd::List { project, all, archived } => {
            let mut p = if all { json!({}) } else { at(project)? };
            p["archived"] = json!(archived);
            out(call("thread.list", p)?, &|v| {
                let rows: Vec<String> = v.as_array().into_iter().flatten().map(thread_line).collect();
                if rows.is_empty() { "no threads".into() } else { rows.join("\n") }
            })
        }
        ThreadCmd::Show { thread, after } => {
            let v = call("thread.read", json!({"thread": thread, "after": after}))?;
            out(v, &fmt_conversation)
        }
        ThreadCmd::Send { thread, text, files } => out(call("thread.send", json!({"thread": thread, "text": text, "attachments": files_of(files)?}))?, &|v| format!("sent to {}", v["thread"].as_str().unwrap_or("?"))),
        ThreadCmd::Interrupt { thread } => out(call("thread.interrupt", json!({"thread": thread}))?, &|v| format!("interrupted {}", v["thread"].as_str().unwrap_or("?"))),
        ThreadCmd::Stop { thread } => out(call("thread.stop", json!({"thread": thread}))?, &fmt_thread),
        ThreadCmd::Resume { thread, text, model, permission } => out(call("thread.resume", json!({"thread": thread, "text": text, "model": model, "permission": permission}))?, &fmt_thread),
        ThreadCmd::Archive { thread } => out(call("thread.archive", json!({"thread": thread}))?, &fmt_thread),
        ThreadCmd::Unarchive { thread } => out(call("thread.unarchive", json!({"thread": thread}))?, &fmt_thread),
        ThreadCmd::Rename { thread, title } => out(call("thread.rename", json!({"thread": thread, "title": title}))?, &fmt_thread),
        ThreadCmd::Permission { thread, tier } => out(call("thread.permission", json!({"thread": thread, "permission": tier}))?, &fmt_thread),
        ThreadCmd::Answer { thread, card, answer, cancel } => {
            let mut p = json!({"thread": thread, "card": card});
            if cancel {
                p["cancel"] = json!(true);
            } else if let Some(answer) = answer {
                // A permission card takes `allow`, `deny` or an option; a question takes `value`. lilad reads the one the card is.
                match answer.as_str() {
                    "allow" => p["allow"] = json!(true),
                    "deny" => p["allow"] = json!(false),
                    other => p["option"] = json!(other),
                }
                p["value"] = json!(answer);
            }
            out(call("thread.answer", p)?, &|v| format!("{} answered", v["card"].as_str().unwrap_or("?")))
        }
        ThreadCmd::Rules { thread, remove } => {
            if let Some(rule) = remove {
                out(call("thread.rule.remove", json!({"thread": thread, "rule": rule}))?, &|v| fmt_rules(&v["rules"]))
            } else {
                out(call("thread.show", json!({"thread": thread}))?, &|v| fmt_rules(&v["rules"]))
            }
        }
        ThreadCmd::Model { thread, model, effort } => out(call("thread.model", json!({"thread": thread, "model": model, "effort": effort}))?, &|_| "set".into()),
        ThreadCmd::Changes { thread } => out(call("thread.changes", json!({"thread": thread}))?, &fmt_changes),
        ThreadCmd::Diff { thread, path } => out(call("thread.diff", json!({"thread": thread, "path": path}))?, &fmt_diffs),
        ThreadCmd::Commit { thread } => out(call("thread.commit", json!({"thread": thread}))?, &|v| format!("asked {} to commit", v["thread"].as_str().unwrap_or("?"))),
        ThreadCmd::Merge { thread } => out(call("thread.merge", json!({"thread": thread}))?, &|v| {
            format!(
                "merged {}: {} commit(s), {} file(s) into {} as {}",
                v["thread"]["id"].as_str().unwrap_or("?"),
                v["commits"],
                v["files"],
                v["branch"].as_str().unwrap_or("?"),
                v["commit"].as_str().unwrap_or("?")
            )
        }),
        ThreadCmd::Checkpoints { thread } => out(call("thread.checkpoints", json!({"thread": thread}))?, &fmt_checkpoints),
        ThreadCmd::Restore { thread, checkpoint, dry_run } => out(call("thread.restore", json!({"thread": thread, "checkpoint": checkpoint, "dry_run": dry_run}))?, &fmt_restore),
        ThreadCmd::Log { thread, limit } => out(call("thread.log", json!({"thread": thread, "limit": limit}))?, &fmt_log),
    }
    Ok(())
}

// ---------------- text output ----------------

fn list(v: &Value) -> String {
    let items: Vec<String> = v.as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect();
    if items.is_empty() { "none".into() } else { items.join(", ") }
}

fn fmt_project(p: &Value) -> String {
    let d = &p["defaults"];
    let mut s = format!(
        "{:<14} {}  [{} · {}]  {} thread(s)",
        p["id"].as_str().unwrap_or(""),
        p["cwd"].as_str().unwrap_or(""),
        d["model"].as_str().filter(|m| !m.is_empty()).unwrap_or("default model"),
        d["permission"].as_str().unwrap_or(""),
        p["threads"].as_u64().unwrap_or(0)
    );
    if let Some(e) = p["error"].as_str() {
        s.push_str(&format!("\n  ! {e}"));
    }
    s
}

fn thread_line(t: &Value) -> String {
    let waiting = t["waiting"].as_str().map(|w| format!(" ({w})")).unwrap_or_default();
    format!(
        "{:<10} {:<9} {:<20} {:<9} {}{}{}",
        t["id"].as_str().unwrap_or(""),
        format!("{}{waiting}", t["state"].as_str().unwrap_or("")),
        t["model"].as_str().unwrap_or(""),
        t["permission"].as_str().unwrap_or(""),
        t["title"].as_str().unwrap_or(""),
        if t["archived"] == true { "  [archived]" } else { "" },
        t["legacy"].as_str().map(|l| format!("  [{l}: cannot be resumed]")).unwrap_or_default()
    ) + &worktree_note(t)
}

/// `  [worktree main]` or `  [merged]`, and the changes it has, at the end of a thread's line.
fn worktree_note(t: &Value) -> String {
    let mut s = String::new();
    if t["merged_ms"].is_number() {
        s.push_str("  [merged]");
    } else if t["worktree"] == true {
        s.push_str(&format!("  [worktree of {}]", t["base_branch"].as_str().unwrap_or("?")));
    }
    if t["merging"] == true {
        s.push_str("  [merging]");
    }
    if let Some(c) = t["changes"].as_object().filter(|c| c["files"].as_u64().unwrap_or(0) > 0) {
        s.push_str(&format!("  {} file(s) +{} -{}", c["files"], c["add"], c["del"]));
    }
    s
}

fn fmt_changes(v: &Value) -> String {
    if v["repo"] != true {
        return "not in a git repository".into();
    }
    let mut s = format!("branch {}", v["branch"].as_str().unwrap_or("(detached HEAD)"));
    if v["worktree"] == true {
        s.push_str(&format!(" (worktree of {}, {} commit(s) ahead)", v["base_branch"].as_str().unwrap_or("?"), v["ahead"].as_u64().unwrap_or(0)));
    }
    for c in v["commits"].as_array().into_iter().flatten() {
        s.push_str(&format!("\n  {} {}", c["sha"].as_str().unwrap_or(""), c["subject"].as_str().unwrap_or("")));
    }
    if v["files"].as_array().is_none_or(|f| f.is_empty()) {
        s.push_str("\nno uncommitted changes");
    }
    for f in v["files"].as_array().into_iter().flatten() {
        let mark = match f["status"].as_str().unwrap_or("") {
            "added" => "A",
            "deleted" => "D",
            "renamed" => "R",
            "untracked" => "?",
            _ => "M",
        };
        let from = f["old_path"].as_str().map(|o| format!("{o} -> ")).unwrap_or_default();
        let counts = if f["binary"] == true { "binary".to_owned() } else { format!("+{} -{}", f["add"], f["del"]) };
        s.push_str(&format!("\n{mark} {counts:<12} {from}{}", f["path"].as_str().unwrap_or("")));
    }
    if v["truncated"] == true {
        s.push_str("\n… more files than are listed");
    }
    s
}

fn fmt_diffs(v: &Value) -> String {
    let mut s = String::new();
    for d in v["diffs"].as_array().into_iter().flatten() {
        let from = d["old_path"].as_str().map(|o| format!(" (from {o})")).unwrap_or_default();
        s.push_str(&format!("--- {} [{}]{from}\n", d["path"].as_str().unwrap_or(""), d["status"].as_str().unwrap_or("")));
        if d["binary"] == true {
            s.push_str("Binary file\n");
        }
        for l in d["lines"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            s.push_str(l);
            s.push('\n');
        }
        if d["truncated"] == true {
            s.push_str("… cut here\n");
        }
    }
    if s.is_empty() { "no changes".into() } else { s.trim_end().to_owned() }
}

fn fmt_thread(t: &Value) -> String {
    thread_line(t)
}

fn fmt_conversation(v: &Value) -> String {
    let mut s = format!("{}\n", thread_line(&v["thread"]));
    for i in v["items"].as_array().into_iter().flatten() {
        let text = |k: &str| i[k].as_str().unwrap_or("").trim().to_owned();
        match i["kind"].as_str().unwrap_or("") {
            "user_message" => s.push_str(&format!("\nyou: {}\n", text("text"))),
            "agent_message" => s.push_str(&format!("\n{}\n", text("text"))),
            "thought" => s.push_str(&format!("\n(thinking) {}\n", text("text").lines().next().unwrap_or(""))),
            "error" => s.push_str(&format!("\n! {}\n", text("text"))),
            "tool_call" => {
                let out = text("output");
                let tail = out.lines().next().map(|l| format!("\n    {l}")).unwrap_or_default();
                s.push_str(&format!("\n$ {} {} [{}]{tail}\n", text("name"), text("title"), i["state"].as_str().unwrap_or("")));
            }
            _ => {}
        }
    }
    for q in v["questions"].as_array().into_iter().flatten() {
        let options: Vec<String> = q["options"].as_array().into_iter().flatten().filter_map(|o| o.as_str()).enumerate().map(|(i, o)| format!("{}) {o}", i + 1)).collect();
        let how = match q["kind"].as_str() {
            Some("choice") => options.join("  "),
            Some("confirm") => "yes / no".to_owned(),
            _ => "words of your own".to_owned(),
        };
        s.push_str(&format!("\n? [{}] {} ({how})\n", q["id"].as_str().unwrap_or(""), q["title"].as_str().unwrap_or("")));
    }
    for c in v["permissions"].as_array().into_iter().flatten() {
        let opts: Vec<&str> = c["options"].as_array().into_iter().flatten().filter_map(|o| o["id"].as_str()).collect();
        s.push_str(&format!("\n? [{}] {} — {} ({})\n", c["id"].as_str().unwrap_or(""), c["title"].as_str().unwrap_or(""), c["why"].as_str().unwrap_or(""), opts.join(" / ")));
    }
    if v["rules"].as_array().is_some_and(|r| !r.is_empty()) {
        s.push_str(&format!("\nallowed always:\n{}\n", fmt_rules(&v["rules"])));
    }
    s.push_str(&format!("\nseq {}", v["seq"]));
    s
}

/// A thread's rules, one a line: the number to take it back by, and what it allows.
fn fmt_rules(rules: &Value) -> String {
    let rows: Vec<String> = rules.as_array().into_iter().flatten().map(|r| format!("  {}  {}", r["id"], r["label"].as_str().unwrap_or(""))).collect();
    if rows.is_empty() { "no rules".into() } else { rows.join("\n") }
}

fn fmt_checkpoints(v: &Value) -> String {
    let rows = v["checkpoints"].as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        return "no checkpoints".into();
    }
    let mut s: Vec<String> = rows
        .iter()
        .map(|c| {
            let title = match c["kind"].as_str() {
                Some("restore") => "(the state before a restore)".to_owned(),
                _ if c["title"].as_str().unwrap_or("").is_empty() => "(no words)".to_owned(),
                _ => c["title"].as_str().unwrap_or("").to_owned(),
            };
            let at = if c["item"].is_null() && c["kind"] == "turn" { "  [message not in the conversation]" } else { "" };
            format!("#{:<4} {}  {} {}{at}", c["id"], clock(c["at_ms"].as_i64().unwrap_or(0)), c["head"].as_str().unwrap_or("-"), title)
        })
        .collect();
    if let Some(why) = v["why_not"].as_str() {
        s.push(format!("cannot restore now: {why}"));
    }
    s.join("\n")
}

fn fmt_restore(v: &Value) -> String {
    let c = &v["counts"];
    let mut s = format!(
        "{} checkpoint {}: {} file(s) put back, {} made again, {} deleted",
        if v["dry_run"] == true { "would restore" } else { "restored" },
        v["checkpoint"],
        c["changed"],
        c["recreated"],
        c["deleted"]
    );
    for (mark, key) in [("M", "changed"), ("+", "recreated"), ("-", "deleted")] {
        for f in v[key].as_array().into_iter().flatten().filter_map(Value::as_str) {
            s.push_str(&format!("\n  {mark} {f}"));
        }
    }
    if v["truncated"] == true {
        s.push_str("\n  … more files than are listed");
    }
    if v["head_moved"] == true {
        s.push_str(&format!("\nHEAD moved since ({} then, {} now); it stays, so commits made since show as changes", v["head"]["then"].as_str().unwrap_or("none"), v["head"]["now"].as_str().unwrap_or("none")));
    }
    if let Some(u) = v["undo"].as_i64() {
        s.push_str(&format!("\nthe state before is checkpoint {u}: `lila thread restore <thread> {u}` undoes this"));
    }
    s
}

fn fmt_log(v: &Value) -> String {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|e| format!("{} {:<8} {} {}", clock(e["at_ms"].as_i64().unwrap_or(0)), e["actor"].as_str().unwrap_or(""), e["kind"].as_str().unwrap_or(""), e["detail"].as_str().unwrap_or("")))
        .collect::<Vec<_>>()
        .join("\n")
}

/// HH:MM:SS in local time.
fn clock(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_millis_opt(ms).single().map(|t| t.format("%H:%M:%S").to_string()).unwrap_or_default()
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_and_diffs_read_as_git_would_show_them() {
        let v = json!({"repo": true, "branch": "lila/t1", "worktree": true, "base_branch": "main", "ahead": 1, "commits": [{"sha": "abc1234", "subject": "Add b"}],
            "files": [{"path": "a.txt", "status": "modified", "add": 2, "del": 1}, {"path": "new.txt", "status": "renamed", "old_path": "old.txt", "add": 0, "del": 0}, {"path": "p.bin", "status": "untracked", "add": 0, "del": 0, "binary": true}]});
        let text = fmt_changes(&v);
        assert_eq!(text.lines().next(), Some("branch lila/t1 (worktree of main, 1 commit(s) ahead)"));
        assert!(text.contains("\n  abc1234 Add b") && text.contains("M +2 -1        a.txt") && text.contains("R +0 -0        old.txt -> new.txt") && text.contains("? binary       p.bin"), "{text}");
        assert_eq!(fmt_changes(&json!({"repo": false})), "not in a git repository");
        let d = fmt_diffs(&json!({"diffs": [{"path": "a.txt", "status": "modified", "lines": ["@@ -1 +1 @@", "-a", "+b"], "truncated": true}, {"path": "p.bin", "status": "added", "lines": [], "binary": true}]}));
        assert_eq!(d, "--- a.txt [modified]\n@@ -1 +1 @@\n-a\n+b\n… cut here\n--- p.bin [added]\nBinary file");
        let line = thread_line(&json!({"id": "t1", "state": "idle", "model": "m", "permission": "full", "title": "T", "worktree": true, "base_branch": "main", "changes": {"files": 2, "add": 5, "del": 1}}));
        assert!(line.ends_with("T  [worktree of main]  2 file(s) +5 -1"), "{line}");
    }
    #[test]
    fn checkpoints_and_restores_read_as_a_list_of_files() {
        let v = json!({"checkpoints": [{"id": 2, "kind": "restore", "at_ms": 0, "title": "x", "item": null, "head": "abc1234"}, {"id": 1, "kind": "turn", "at_ms": 0, "title": "fix the test", "item": null, "head": null}], "why_not": "t1 is working now"});
        let text = fmt_checkpoints(&v);
        assert!(text.contains("(the state before a restore)") && text.contains("fix the test  [message not in the conversation]") && text.ends_with("cannot restore now: t1 is working now"), "{text}");
        assert_eq!(fmt_checkpoints(&json!({"checkpoints": []})), "no checkpoints");
        let r = fmt_restore(&json!({"dry_run": true, "checkpoint": 3, "counts": {"changed": 1, "recreated": 0, "deleted": 1}, "changed": ["a.rs"], "recreated": [], "deleted": ["b.rs"], "head_moved": true, "head": {"then": "abc", "now": null}, "undo": null}));
        assert_eq!(r, "would restore checkpoint 3: 1 file(s) put back, 0 made again, 1 deleted\n  M a.rs\n  - b.rs\nHEAD moved since (abc then, none now); it stays, so commits made since show as changes");
    }

    #[test]
    fn daemon_status_says_how_to_fix_a_different_protocol() {
        let info = |protocol| client::DaemonInfo { pid: Some(1), version: Some("0.1.0".into()), protocol };
        let ok = daemon_status_text(&client::DaemonVersion::Current(info(Some(lila::PROTOCOL))), "/s");
        assert_eq!(ok, format!("lilad is running (/s, version 0.1.0, protocol {})", lila::PROTOCOL));
        let old = daemon_status_text(&client::DaemonVersion::Different(info(None)), "/s");
        assert!(old.contains("protocol unknown") && old.contains("lila daemon restart") && old.contains("lila daemon stop") && old.contains("cargo install"), "{old}");
        assert_eq!(daemon_status_json(&client::DaemonVersion::Different(info(Some(0)))), json!({"running": true, "current": false, "daemon": {"pid": 1, "version": "0.1.0", "protocol": 0}, "protocol": lila::PROTOCOL}));
        assert_eq!(daemon_status_json(&client::DaemonVersion::NotRunning), json!({"running": false}));
    }
}
