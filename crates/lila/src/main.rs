//! `lila`: the command line of Lila. The person uses it to add projects and talk to
//! their threads (the window does the same through the same socket); the engines
//! use it for their hooks and hosts.

use anyhow::{Context, Result};
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
    /// Threads: conversations with an agent.
    Thread {
        #[command(subcommand)]
        cmd: ThreadCmd,
    },
    /// Tokens each thread used, from the session records Claude Code and Codex keep here.
    Usage {
        /// Only the last N days (default: all).
        #[arg(long)]
        days: Option<u64>,
    },
    /// Subscription accounts and how much of each usage window is used.
    Accounts,
    /// lilad, the background process that keeps the state and runs the engines.
    Daemon {
        #[command(subcommand)]
        cmd: DaemonCmd,
    },
    /// A Claude Code hook in a thread's session (its settings file runs this).
    #[command(hide = true)]
    Hook { event: String },
    /// Runs a thread's agent through its protocol (lilad starts it in the thread's terminal).
    #[command(hide = true)]
    Host {
        #[command(subcommand)]
        cmd: HostCmd,
    },
}

#[derive(Subcommand)]
enum HostCmd {
    /// Codex through `codex app-server`, in the current folder.
    Codex {
        /// The codex program.
        #[arg(long)]
        program: std::path::PathBuf,
        #[arg(long)]
        model: String,
        #[arg(long)]
        effort: Option<String>,
        #[arg(long)]
        sandbox: Option<String>,
        /// When Codex asks first: untrusted, on-request or never.
        #[arg(long)]
        approval: Option<String>,
        /// A system prompt for the thread.
        #[arg(long)]
        prompt_file: Option<std::path::PathBuf>,
        /// A thread to go on with.
        #[arg(long)]
        thread: Option<String>,
        /// More arguments for `codex app-server`.
        #[arg(last = true)]
        extra: Vec<String>,
    },
    /// An ACP agent (omp: `omp acp`), in the current folder.
    Omp {
        /// The omp program.
        #[arg(long)]
        program: std::path::PathBuf,
        #[arg(long)]
        model: Option<String>,
        /// A system prompt for the thread.
        #[arg(long)]
        prompt_file: Option<std::path::PathBuf>,
        /// A session to go on with.
        #[arg(long)]
        session: Option<String>,
        /// More options for omp (before `acp`).
        #[arg(last = true)]
        extra: Vec<String>,
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
        /// claude, codex or omp.
        #[arg(long)]
        engine: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        effort: Option<String>,
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
        /// claude, codex or omp (default: the project's).
        #[arg(long)]
        engine: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        effort: Option<String>,
        /// read_only, confirm or full (default: the project's).
        #[arg(long)]
        permission: Option<String>,
        /// A file to send along (repeatable).
        #[arg(long = "file")]
        files: Vec<std::path::PathBuf>,
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
    /// Start an ended thread again, on the session it had.
    Resume {
        thread: String,
        /// A message to send once it is up.
        text: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        effort: Option<String>,
        #[arg(long)]
        permission: Option<String>,
    },
    /// Hide a thread (ends its agent).
    Archive { thread: String },
    Unarchive { thread: String },
    Rename { thread: String, title: String },
    /// Switch a thread's permission tier: read_only, confirm or full.
    Permission { thread: String, tier: String },
    /// Answer a permission card: allow, deny, or dismiss (Claude Code then asks in its terminal).
    Answer { thread: String, card: String, answer: String },
    /// Change the model or effort (a Codex or ACP thread at once; a Claude Code one when ended).
    Model {
        thread: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        effort: Option<String>,
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
    /// Stop the running daemon. Threads keep running.
    Stop,
    /// Whether it is running.
    Status,
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
            ProjectCmd::Set { project, title, engine, model, effort, permission } => {
                let mut p = at(project)?;
                p["title"] = json!(title);
                let defaults: serde_json::Map<String, Value> = [("engine", engine), ("model", model), ("effort", effort), ("permission", permission)].into_iter().filter_map(|(k, v)| v.map(|v| (k.to_owned(), json!(v)))).collect();
                p["defaults"] = Value::Object(defaults);
                out(call("project.set", p)?, &fmt_project)
            }
        },
        Cmd::Thread { cmd } => thread(cmd, &call, &out)?,
        Cmd::Usage { days } => usage(&paths, days, cli.json)?,
        Cmd::Accounts => accounts(&paths, cli.json)?,
        Cmd::Daemon { cmd: DaemonCmd::Run } => lila::daemon::serve(paths.clone(), client::hc_bin())?,
        Cmd::Hook { event } => client::hook(&paths, &event)?,
        Cmd::Host { cmd: HostCmd::Codex { program, model, effort, sandbox, approval, prompt_file, thread, extra } } => {
            let instructions = match prompt_file {
                Some(f) => std::fs::read_to_string(&f).with_context(|| format!("read {}", f.display()))?,
                None => String::new(),
            };
            let cwd = cwd()?;
            let args = std::iter::once("app-server".to_owned()).chain(extra).collect();
            let opts = lila_agent::codex::Options { cwd: cwd.clone(), model, effort, sandbox, approval, instructions, thread };
            lila::host::run(&paths, lila::host::Agent { program, args, cwd }, lila_agent::codex::AppServer::new(opts))?
        }
        Cmd::Host { cmd: HostCmd::Omp { program, model, prompt_file, session, extra } } => {
            let cwd = cwd()?;
            // omp's own options go before `acp` (P0 S3).
            let mut args = Vec::new();
            if let Some(f) = prompt_file {
                args.extend(["--append-system-prompt".to_owned(), std::fs::read_to_string(&f).with_context(|| format!("read {}", f.display()))?]);
            }
            if let Some(m) = model.filter(|m| !m.is_empty()) {
                args.extend(["--model".to_owned(), m]);
            }
            args.extend(extra);
            args.push("acp".into());
            let acp = lila_agent::acp::Acp::new(lila_agent::acp::Options { cwd: cwd.clone(), session });
            lila::host::run(&paths, lila::host::Agent { program, args, cwd }, acp)?
        }
        Cmd::Daemon { cmd: DaemonCmd::Stop } => {
            if client::daemon_state(&paths)? {
                call("stop", json!({}))?;
                println!("lilad stopped; threads keep running");
            } else {
                println!("lilad is not running; nothing to stop");
            }
        }
        Cmd::Daemon { cmd: DaemonCmd::Status } => {
            if client::daemon_state(&paths)? {
                println!("lilad is running ({})", paths.socket()?.display());
            } else {
                println!("lilad is not running");
                std::process::exit(3);
            }
        }
    }
    Ok(())
}

type Out<'a> = dyn Fn(Value, &dyn Fn(&Value) -> String) + 'a;

fn files_of(files: Vec<std::path::PathBuf>) -> Result<Vec<String>> {
    files.into_iter().map(|f| Ok(std::fs::canonicalize(&f).with_context(|| format!("{}", f.display()))?.to_string_lossy().to_string())).collect()
}

fn thread(cmd: ThreadCmd, call: &dyn Fn(&str, Value) -> Result<Value>, out: &Out) -> Result<()> {
    match cmd {
        ThreadCmd::New { text, project, title, engine, model, effort, permission, files } => {
            let mut p = at(project)?;
            p["text"] = json!(text);
            p["title"] = json!(title);
            p["engine"] = json!(engine);
            p["model"] = json!(model);
            p["effort"] = json!(effort);
            p["permission"] = json!(permission);
            p["attachments"] = json!(files_of(files)?);
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
        ThreadCmd::Resume { thread, text, model, effort, permission } => out(call("thread.resume", json!({"thread": thread, "text": text, "model": model, "effort": effort, "permission": permission}))?, &fmt_thread),
        ThreadCmd::Archive { thread } => out(call("thread.archive", json!({"thread": thread}))?, &fmt_thread),
        ThreadCmd::Unarchive { thread } => out(call("thread.unarchive", json!({"thread": thread}))?, &fmt_thread),
        ThreadCmd::Rename { thread, title } => out(call("thread.rename", json!({"thread": thread, "title": title}))?, &fmt_thread),
        ThreadCmd::Permission { thread, tier } => out(call("thread.permission", json!({"thread": thread, "permission": tier}))?, &fmt_thread),
        ThreadCmd::Answer { thread, card, answer } => {
            let mut p = json!({"thread": thread, "card": card});
            match answer.as_str() {
                "allow" => p["allow"] = json!(true),
                "deny" => p["allow"] = json!(false),
                "dismiss" => p["dismiss"] = json!(true),
                other => p["option"] = json!(other),
            }
            out(call("thread.answer", p)?, &|v| format!("{} answered", v["card"].as_str().unwrap_or("?")))
        }
        ThreadCmd::Model { thread, model, effort } => out(call("thread.model", json!({"thread": thread, "model": model, "effort": effort}))?, &|_| "set".into()),
        ThreadCmd::Log { thread, limit } => out(call("thread.log", json!({"thread": thread, "limit": limit}))?, &fmt_log),
    }
    Ok(())
}

fn accounts(paths: &Paths, as_json: bool) -> Result<()> {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let list = lila::accounts::load(&paths.accounts())?;
    let rows: Vec<(lila::accounts::Account, lila::accounts::AccountUsage)> = list.into_iter().map(|a| {
        let u = lila::accounts::usage(&a, &home);
        (a, u)
    }).collect();
    if as_json {
        let v: Vec<Value> = rows
            .iter()
            .map(|(a, u)| {
                json!({"id": a.id, "runtime": a.runtime.to_string(), "name": a.name, "dir": a.dir, "current": a.current, "plan": u.plan, "seen_ms": u.seen_ms,
                    "windows": u.windows.iter().map(|w| json!({"name": w.name, "used_percent": w.used_percent, "resets_ms": w.resets_ms})).collect::<Vec<_>>()})
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    for (a, u) in rows {
        let mark = if a.current { "*" } else { " " };
        println!("{mark} {:<16} {:<6} {} {}", a.id, a.runtime, a.name, u.plan.unwrap_or_default());
        for w in &u.windows {
            println!("      {:<8} {:>5.1}%  resets {}", w.name, w.used_percent, clock(w.resets_ms));
        }
        if u.windows.is_empty() {
            println!("      no usage recorded yet");
        }
    }
    // magpie's accounts, read through magpie (their tokens stay with it).
    let path = std::env::var("PATH").unwrap_or_default();
    if let Some(bin) = lila::magpie::find(&path) {
        match lila::magpie::accounts(&bin) {
            Ok(list) => {
                for m in list {
                    let mark = if m.active { "*" } else { " " };
                    println!("{mark} {:<16} {:<6} {} {} (magpie)", m.user, m.runtime, m.usage.plan.unwrap_or_default(), if m.active { "signed in" } else { "" });
                    for w in &m.usage.windows {
                        println!("      {:<8} {:>5.1}%  resets {}", w.name, w.used_percent, clock(w.resets_ms));
                    }
                }
            }
            Err(e) => println!("magpie: {e:#}"),
        }
    }
    Ok(())
}

fn usage(paths: &Paths, days: Option<u64>, as_json: bool) -> Result<()> {
    use lila::usage::{Owners, Scanner, Sources, short};
    let project = client::request(paths, "project.show", at(None)?, false)?;
    let name = project["id"].as_str().context("no project")?.to_owned();
    let root = project["cwd"].as_str().context("no folder")?.to_owned();
    let threads = client::request(paths, "thread.list", json!({"project": name, "archived": true}), false)?;
    let owners: Owners = threads.as_array().into_iter().flatten().filter_map(|t| Some((t["session"].as_str()?.to_owned(), (name.clone(), t["id"].as_str()?.to_owned())))).collect();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as i64;
    let since = days.map(|d| now - d as i64 * 86_400_000);
    // Threads also work in the project's worktrees.
    let slots = lila::worktree::existing(&paths.worktrees(&name));
    let cwds: Vec<String> = std::iter::once(std::path::PathBuf::from(root)).chain(slots).map(|p| p.to_string_lossy().to_string()).collect();
    let u = Scanner::default().usage(&Sources::from_env(), &name, &cwds, &owners, since);
    if as_json {
        let rows: Vec<Value> = u
            .threads
            .iter()
            .map(|m| json!({"thread": m.thread, "runtime": m.runtime, "sessions": m.sessions, "input": m.tokens.input, "cache_write": m.tokens.cache_write, "cache_read": m.tokens.cache_read, "output": m.tokens.output, "total": m.tokens.total(), "models": m.models.keys().collect::<Vec<_>>()}))
            .collect();
        let limits: Vec<Value> = u.codex_limits.iter().map(|l| json!({"used_percent": l.used_percent, "window_minutes": l.window_minutes, "resets_at_ms": l.resets_at_ms, "seen_ms": l.seen_ms, "plan": l.plan})).collect();
        println!("{}", serde_json::to_string_pretty(&json!({"project": name, "threads": rows, "codex_limits": limits}))?);
        return Ok(());
    }
    if u.threads.is_empty() {
        println!("no sessions of {name}'s threads found{}", if days.is_some() { " in that time" } else { "" });
    }
    for m in &u.threads {
        let t = &m.tokens;
        let models: Vec<&str> = m.models.keys().map(String::as_str).collect();
        println!("{:<10} {:>7}  in {:>6}  cache w {:>6} r {:>7}  out {:>6}  {} session(s)  {}", m.thread, short(t.total()), short(t.input), short(t.cache_write), short(t.cache_read), short(t.output), m.sessions, models.join(", "));
    }
    if !u.threads.is_empty() {
        println!("{:<10} {:>7}", "total", short(u.total.total()));
    }
    for l in &u.codex_limits {
        println!("codex: {:.0}% of the {}-minute window used, resets in {} min", l.used_percent, l.window_minutes, ((l.resets_at_ms - now) / 60_000).max(0));
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
        "{:<14} {}  [{} {} {} · {}]  {} thread(s)",
        p["id"].as_str().unwrap_or(""),
        p["cwd"].as_str().unwrap_or(""),
        d["engine"].as_str().unwrap_or(""),
        d["model"].as_str().unwrap_or(""),
        d["effort"].as_str().unwrap_or("-"),
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
        "{:<10} {:<9} {:<6} {:<20} {:<9} {}{}",
        t["id"].as_str().unwrap_or(""),
        format!("{}{waiting}", t["state"].as_str().unwrap_or("")),
        t["engine"].as_str().unwrap_or(""),
        t["model"].as_str().unwrap_or(""),
        t["permission"].as_str().unwrap_or(""),
        t["title"].as_str().unwrap_or(""),
        if t["archived"] == true { "  [archived]" } else { "" }
    )
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
            "tool_call" => {
                let out = text("output");
                let tail = out.lines().next().map(|l| format!("\n    {l}")).unwrap_or_default();
                s.push_str(&format!("\n$ {} {} [{}]{tail}\n", text("name"), text("title"), i["state"].as_str().unwrap_or("")));
            }
            _ => {}
        }
    }
    if let Some(w) = v["writing"].as_str() {
        s.push_str(&format!("\n… {w}\n"));
    }
    for c in v["permissions"].as_array().into_iter().flatten() {
        let opts: Vec<&str> = c["options"].as_array().into_iter().flatten().filter_map(|o| o["id"].as_str()).collect();
        s.push_str(&format!("\n? [{}] {} — {} ({})\n", c["id"].as_str().unwrap_or(""), c["title"].as_str().unwrap_or(""), c["why"].as_str().unwrap_or(""), opts.join(" / ")));
    }
    s.push_str(&format!("\nseq {}", v["seq"]));
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


