//! `lila`: run an agent team on herdr-core. The person uses it to start projects
//! and answer the inbox; members use it to start, finish, block and hand on tasks.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use lila::paths::Paths;
use lila::{client, daemon, roles};
use serde_json::{Value, json};

#[derive(Parser)]
#[command(name = "lila", version, about = "Run a team of coding agents: you talk to the PM, the PM runs the team.")]
struct Cli {
    /// Print raw JSON instead of text.
    #[arg(long, global = true)]
    json: bool,
    /// Print the protocol guide for agents and exit.
    #[arg(long)]
    skill: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create .lila/project.toml here with the default team, and install the default roles.
    Init {
        /// Project name (default: this folder's name).
        #[arg(long)]
        name: Option<String>,
    },
    /// List roles, or install the defaults that are missing.
    Roles {
        #[command(subcommand)]
        cmd: Option<RolesCmd>,
    },
    /// Start this project's team (and lilad if needed).
    Up {
        /// A first request for the PM, as a task from you.
        #[arg(long)]
        brief: Option<String>,
    },
    /// Stop this project's members. Tasks stay; open ones are re-sent after the next `up`.
    Down,
    /// Who is doing what.
    Status,
    /// Create, update and inspect tasks.
    Task {
        #[command(subcommand)]
        cmd: TaskCmd,
    },
    /// (PM) Ask the person a question; the answer comes back as <lila answer>.
    Ask {
        question: String,
        #[arg(long = "option")]
        options: Vec<String>,
        #[arg(long)]
        task: Option<String>,
    },
    /// What needs you: questions, blocked terminals, reports.
    Inbox {
        /// Include resolved items.
        #[arg(long)]
        all: bool,
    },
    /// Answer a question, or dismiss any other inbox item (omit the text).
    Answer { id: String, text: Option<String> },
    /// Recent activity.
    Log {
        #[arg(short = 'n', default_value_t = 30)]
        limit: u64,
        #[arg(long)]
        task: Option<String>,
    },
    /// Tokens each member used, from the session records Claude Code and Codex keep here.
    Usage {
        /// Only the last N days (default: all).
        #[arg(long)]
        days: Option<u64>,
    },
    /// Subscription accounts and how much of each usage window is used.
    Accounts,
    /// Stop delivering tasks (notes still go out).
    Pause,
    /// Resume delivering tasks.
    Resume,
    /// lilad, the background process that delivers tasks and keeps the state.
    Daemon {
        #[command(subcommand)]
        cmd: DaemonCmd,
    },
}

#[derive(Subcommand)]
enum RolesCmd {
    /// Install default roles that are missing (never overwrites).
    Init,
}

#[derive(Subcommand)]
enum TaskCmd {
    /// Assign a task to a member.
    New {
        #[arg(long)]
        to: String,
        #[arg(long)]
        title: String,
        #[arg(long, conflicts_with = "body_file")]
        body: Option<String>,
        #[arg(long)]
        body_file: Option<std::path::PathBuf>,
        #[arg(long)]
        parent: Option<String>,
        /// Lower runs first (default 0).
        #[arg(long, default_value_t = 0, allow_negative_numbers = true)]
        priority: i64,
        /// The person's own words, unchanged; every step of the chain sees them.
        #[arg(long)]
        quote: Option<String>,
        /// A checkable outcome that counts as done (repeatable).
        #[arg(long)]
        accept: Vec<String>,
        /// Something not to do (repeatable).
        #[arg(long)]
        out_of_scope: Vec<String>,
        /// The flow it runs through (`code`, `quick`, the project's own), or `none`.
        /// Default: the project's flow, for work from the PM or the person.
        #[arg(long)]
        flow: Option<String>,
        /// Tokens the flow may use (300k, 1.5M); over it, it waits for the person between steps.
        #[arg(long, value_parser = parse_tokens)]
        budget: Option<u64>,
    },
    /// Change a task you assigned: rewrite it while queued, or --append an update once delivered.
    Edit {
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, conflicts_with = "body_file")]
        body: Option<String>,
        #[arg(long)]
        body_file: Option<std::path::PathBuf>,
        /// Added under "Update from <you>"; sent to the member if it already has the task.
        #[arg(long)]
        append: Option<String>,
    },
    /// Say you started a task delivered to you.
    Start {
        id: String,
    },
    /// Finish a task delivered to you.
    Done {
        id: String,
        #[arg(long)]
        summary: String,
        /// A deliverable or report (repeatable).
        #[arg(long = "file")]
        files: Vec<String>,
        /// A choice you made that the task did not settle (repeatable).
        #[arg(long = "decision")]
        decisions: Vec<String>,
        /// Something you did not check (repeatable).
        #[arg(long)]
        unverified: Vec<String>,
        /// For a verification step: pass or fail.
        #[arg(long, value_parser = ["pass", "fail"])]
        verdict: Option<String>,
    },
    /// Stop on a task: say exactly what you need. Whoever assigned it hears.
    Block {
        id: String,
        #[arg(long)]
        reason: String,
    },
    /// Withdraw a task you assigned.
    Cancel {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    /// One task with its history and files.
    Show {
        id: String,
    },
    /// Open tasks.
    Ls {
        /// Only tasks assigned to or by you.
        #[arg(long)]
        mine: bool,
        /// Only tasks in this state (queued, delivered, in_progress, blocked, done, cancelled).
        #[arg(long)]
        state: Option<String>,
        /// Include closed tasks.
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
enum DaemonCmd {
    /// Run in the foreground (what `lila up` starts inside herdr-core).
    Run,
    /// Stop the running daemon. Members keep running.
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

fn run() -> Result<()> {
    let cli = Cli::parse();
    if cli.skill {
        print!("{}", include_str!("skill.md"));
        return Ok(());
    }
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
        Cmd::Init { name } => init(&paths, name)?,
        Cmd::Roles { cmd: Some(RolesCmd::Init) } => {
            let written = roles::install_defaults(&paths.roles())?;
            println!("{}", if written.is_empty() { "all default roles already exist".into() } else { format!("installed: {}", written.join(", ")) });
        }
        Cmd::Roles { cmd: None } => {
            let roles = roles::load(&paths.roles())?;
            if roles.is_empty() {
                println!("no roles in {}; run `lila roles init`", paths.roles().display());
            }
            for r in roles.values() {
                let f = &r.file;
                println!("{:<10} {:<28} {} {} {}", r.id, f.name, f.runtime, f.model, f.effort.as_deref().unwrap_or(""));
            }
        }
        Cmd::Up { brief } => {
            roles::install_defaults(&paths.roles())?;
            let v = client::request(&paths, "project.up", json!({"brief": brief}), true)?;
            out(v, &fmt_up);
        }
        Cmd::Down => out(call("project.down", json!({}))?, &|v| format!("stopped: {}", list(&v["stopped"]))),
        Cmd::Status => out(call("status", json!({}))?, &fmt_status),
        Cmd::Task { cmd } => task(cmd, &call, &out)?,
        Cmd::Ask { question, options, task } => {
            out(call("ask", json!({"question": question, "options": options, "task": task}))?, &|v| format!("asked as {}. {}", v["id"].as_str().unwrap_or("?"), v["note"].as_str().unwrap_or("")))
        }
        Cmd::Inbox { all } => out(call("inbox", json!({"all": all}))?, &fmt_inbox),
        Cmd::Answer { id, text } => out(call("answer", json!({"id": id, "text": text}))?, &|v| format!("{} resolved", v["id"].as_str().unwrap_or("?"))),
        Cmd::Log { limit, task } => out(call("log", json!({"limit": limit, "task": task}))?, &fmt_log),
        Cmd::Usage { days } => usage(days, cli.json)?,
        Cmd::Accounts => accounts(&paths, cli.json)?,
        Cmd::Pause => out(call("pause", json!({}))?, &|_| "paused: no new tasks are delivered".into()),
        Cmd::Resume => out(call("resume", json!({}))?, &|_| "resumed".into()),
        Cmd::Daemon { cmd: DaemonCmd::Run } => daemon::serve(paths.clone(), client::hc_bin())?,
        Cmd::Daemon { cmd: DaemonCmd::Stop } => {
            if client::daemon_state(&paths)? {
                call("stop", json!({}))?;
                println!("lilad stopped; members keep running");
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

/// "300k", "1.5M", "250000".
fn parse_tokens(s: &str) -> Result<u64, String> {
    let t = s.trim().to_lowercase();
    let (num, mul) = match t.chars().last() {
        Some('k') => (&t[..t.len() - 1], 1e3),
        Some('m') => (&t[..t.len() - 1], 1e6),
        _ => (t.as_str(), 1.0),
    };
    let n: f64 = num.trim().parse().map_err(|_| format!("{s:?}: give tokens like 300k or 1.5M"))?;
    if n <= 0.0 {
        return Err("a budget must be above 0".into());
    }
    Ok((n * mul).round() as u64)
}

type Out<'a> = dyn Fn(Value, &dyn Fn(&Value) -> String) + 'a;

fn task(cmd: TaskCmd, call: &dyn Fn(&str, Value) -> Result<Value>, out: &Out) -> Result<()> {
    match cmd {
        TaskCmd::New { to, title, body, body_file, parent, priority, quote, accept, out_of_scope, flow, budget } => {
            let body = match body_file {
                Some(f) => std::fs::read_to_string(&f).with_context(|| format!("read {}", f.display()))?,
                None => body.unwrap_or_default(),
            };
            let v = call(
                "task.new",
                json!({"to": to, "title": title, "body": body, "parent": parent, "priority": priority, "quote": quote, "accept": accept, "out_of_scope": out_of_scope, "flow": flow, "budget": budget}),
            )?;
            out(v, &|v| {
                let when = match v["held"].as_str() {
                    Some(why) => format!("queued: {why}"),
                    None if v["state"] == "delivered" => "delivered now".into(),
                    None => "queued until they are free".into(),
                };
                format!("{} → {}: {} ({when})", v["id"].as_str().unwrap_or("?"), v["to"].as_str().unwrap_or("?"), v["title"].as_str().unwrap_or(""))
            });
        }
        TaskCmd::Edit { id, title, body, body_file, append } => {
            let body = match body_file {
                Some(f) => Some(std::fs::read_to_string(&f).with_context(|| format!("read {}", f.display()))?),
                None => body,
            };
            let v = call("task.edit", json!({"id": id, "title": title, "body": body, "append": append}))?;
            out(v, &|v| {
                let how = if v["update_sent"] == true { format!("the update goes to {}", v["to"].as_str().unwrap_or("?")) } else { "rewritten before delivery".into() };
                format!("{} edited; {how}", v["id"].as_str().unwrap_or("?"))
            });
        }
        TaskCmd::Start { id } => out(call("task.start", json!({"id": id}))?, &|v| format!("{} in progress", v["id"].as_str().unwrap_or("?"))),
        TaskCmd::Done { id, summary, files, decisions, unverified, verdict } => out(
            call("task.done", json!({"id": id, "summary": summary, "files": files, "decisions": decisions, "unverified": unverified, "verdict": verdict}))?,
            &|v| {
            let to = if v["from"] == "human" { "the person".to_string() } else { v["from"].as_str().unwrap_or("?").to_string() };
            format!("{} done; {to} was told", v["id"].as_str().unwrap_or("?"))
            },
        ),
        TaskCmd::Block { id, reason } => out(call("task.block", json!({"id": id, "reason": reason}))?, &|v| {
            format!("{} blocked; {} was told. Stop here until you hear back.", v["id"].as_str().unwrap_or("?"), v["from"].as_str().unwrap_or("?"))
        }),
        TaskCmd::Cancel { id, reason } => out(call("task.cancel", json!({"id": id, "reason": reason}))?, &|v| format!("{} cancelled", v["id"].as_str().unwrap_or("?"))),
        TaskCmd::Show { id } => out(call("task.show", json!({"id": id}))?, &fmt_task_show),
        TaskCmd::Ls { mine, state, all } => out(call("task.ls", json!({"mine": mine, "state": state, "all": all}))?, &|v| {
            let rows: Vec<String> = v.as_array().into_iter().flatten().map(task_line).collect();
            if rows.is_empty() { "no tasks".into() } else { rows.join("\n") }
        }),
    }
    Ok(())
}

fn init(paths: &Paths, name: Option<String>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let out = lila::init::init_project(paths, &cwd, name)?;
    println!("created {}", out.file.display());
    if !out.roles_installed.is_empty() {
        println!("installed roles in {}: {}", paths.roles().display(), out.roles_installed.join(", "));
    }
    println!("next: edit the team if you like, then `lila up --brief \"what you want built\"`");
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

fn usage(days: Option<u64>, as_json: bool) -> Result<()> {
    use lila::usage::{Scanner, Sources, short};
    let cwd = std::env::current_dir()?;
    let root = lila::paths::find_project_root(&cwd).context("no .lila/project.toml here or above")?;
    let text = std::fs::read_to_string(root.join(".lila/project.toml"))?;
    let name = lila_core::ProjectConfig::parse(&text).map_err(anyhow::Error::msg)?.name;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as i64;
    let since = days.map(|d| now - d as i64 * 86_400_000);
    let u = Scanner::default().usage(&Sources::from_env(), &name, &root.to_string_lossy(), since);
    if as_json {
        let members: Vec<Value> = u
            .members
            .iter()
            .map(|m| json!({"member": m.member, "runtime": m.runtime, "sessions": m.sessions, "input": m.tokens.input, "cache_write": m.tokens.cache_write, "cache_read": m.tokens.cache_read, "output": m.tokens.output, "total": m.tokens.total(), "models": m.models.keys().collect::<Vec<_>>()}))
            .collect();
        let limits: Vec<Value> = u.codex_limits.iter().map(|l| json!({"used_percent": l.used_percent, "window_minutes": l.window_minutes, "resets_at_ms": l.resets_at_ms, "seen_ms": l.seen_ms, "plan": l.plan})).collect();
        println!("{}", serde_json::to_string_pretty(&json!({"project": name, "members": members, "codex_limits": limits}))?);
        return Ok(());
    }
    if u.members.is_empty() {
        println!("no sessions of {name}'s members found{}", if days.is_some() { " in that time" } else { "" });
    }
    for m in &u.members {
        let t = &m.tokens;
        let models: Vec<&str> = m.models.keys().map(String::as_str).collect();
        println!("{:<14} {:>7}  in {:>6}  cache w {:>6} r {:>7}  out {:>6}  {} session(s)  {}", m.member, short(t.total()), short(t.input), short(t.cache_write), short(t.cache_read), short(t.output), m.sessions, models.join(", "));
    }
    if !u.members.is_empty() {
        println!("{:<14} {:>7}", "total", short(u.total.total()));
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

fn task_line(t: &Value) -> String {
    format!(
        "{:<5} {:<11} {} → {:<10} {}",
        t["id"].as_str().unwrap_or(""),
        t["state"].as_str().unwrap_or(""),
        t["from"].as_str().unwrap_or(""),
        t["to"].as_str().unwrap_or(""),
        t["title"].as_str().unwrap_or("")
    )
}

fn fmt_up(v: &Value) -> String {
    let mut s = format!("project {} ({})\n", v["project"].as_str().unwrap_or("?"), v["cwd"].as_str().unwrap_or(""));
    for m in v["started"].as_array().into_iter().flatten() {
        s.push_str(&format!(
            "  started {:<10} in {:<5} {} {}\n",
            m["member"].as_str().unwrap_or(""),
            m["terminal"].as_str().unwrap_or(""),
            m["runtime"].as_str().unwrap_or(""),
            m["model"].as_str().unwrap_or("")
        ));
    }
    for m in v["running"].as_array().into_iter().flatten() {
        s.push_str(&format!("  running {:<10} in {}\n", m["member"].as_str().unwrap_or(""), m["terminal"].as_str().unwrap_or("")));
    }
    if let Some(t) = v["brief_task"].as_str() {
        s.push_str(&format!("brief sent to the PM as {t}\n"));
    }
    if !v["started"].as_array().is_none_or(|a| a.is_empty()) {
        s.push_str("First start in a folder: Claude Code asks whether you trust it. `lila inbox` tells you if a member is waiting.\n");
    }
    s.push_str("Talk to the PM in its terminal (lila-app, herdr-app, or `hc attach lila-<project>-pm`).");
    s
}

fn fmt_status(v: &Value) -> String {
    let mut s = format!("{}{}\n", v["project"].as_str().unwrap_or("?"), if v["paused"] == true { "  (paused)" } else { "" });
    for m in v["members"].as_array().into_iter().flatten() {
        let task = m["task"].as_object().map(|t| format!("{} {} [{}]", t["id"].as_str().unwrap_or(""), t["title"].as_str().unwrap_or(""), t["state"].as_str().unwrap_or(""))).unwrap_or_default();
        let queued = m["queued"].as_u64().unwrap_or(0);
        let queued = if queued > 0 { format!(" +{queued} queued") } else { String::new() };
        let msg = m["msg"].as_str().filter(|s| !s.is_empty()).map(|s| format!("\n             ↳ {s}")).unwrap_or_default();
        s.push_str(&format!(
            "  {:<10} {:<8} {:<5} {:<18} {task}{queued}{msg}\n",
            m["id"].as_str().unwrap_or(""),
            m["status"].as_str().unwrap_or(""),
            m["terminal"].as_str().unwrap_or("-"),
            m["model"].as_str().unwrap_or(""),
        ));
    }
    for r in v["runs"].as_array().into_iter().flatten() {
        let round = r["round"].as_u64().filter(|n| *n > 0).map(|n| format!(", fix round {n}")).unwrap_or_default();
        s.push_str(&format!(
            "  flow {} {} \"{}\": {} ({}{round})\n",
            r["flow"].as_str().unwrap_or(""),
            r["anchor"].as_str().unwrap_or(""),
            r["title"].as_str().unwrap_or(""),
            r["step"].as_str().unwrap_or(""),
            r["state"].as_str().unwrap_or(""),
        ));
    }
    let held = v["held_for_review"].as_u64().unwrap_or(0);
    if held > 0 {
        s.push_str(&format!("  {held} new task(s) wait: {} results wait for the person's review\n", v["unreviewed"].as_u64().unwrap_or(0)));
    }
    let folder = v["held_for_folder"].as_u64().unwrap_or(0);
    if folder > 0 {
        let open: Vec<&str> = v["open_chains"].as_array().into_iter().flatten().filter_map(|c| c.as_str()).collect();
        s.push_str(&format!("  {folder} task(s) wait for the project folder: chain {} has it until the person signs it off\n", open.join(", ")));
    }
    if v["codex_paused_until"].is_i64() {
        s.push_str("  Codex's usage limit is used up: its members get no work until it resets\n");
    }
    let open = v["open_tasks"].as_array().map(|a| a.len()).unwrap_or(0);
    s.push_str(&format!("{open} open task(s)"));
    let inbox = v["inbox"].as_u64().unwrap_or(0);
    if inbox > 0 {
        s.push_str(&format!(", {inbox} item(s) need the person (`lila inbox`)"));
    }
    s
}

fn fmt_inbox(v: &Value) -> String {
    let items = v.as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        return "nothing needs you".into();
    }
    items
        .iter()
        .map(|i| {
            let mut s = format!("{:<4} [{}] {}", i["id"].as_str().unwrap_or(""), i["kind"].as_str().unwrap_or(""), i["text"].as_str().unwrap_or(""));
            if let Some(opts) = i["options"].as_array().filter(|o| !o.is_empty()) {
                s.push_str(&format!("\n     options: {}", opts.iter().filter_map(|o| o.as_str()).collect::<Vec<_>>().join(" / ")));
            }
            if let Some(a) = i["answer"].as_str() {
                s.push_str(&format!("\n     answered: {a}"));
            }
            s
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn fmt_log(v: &Value) -> String {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|e| {
            let task = e["task"].as_str().map(|t| format!("{t} ")).unwrap_or_default();
            format!("{} {:<10} {task}{} {}", clock(e["at_ms"].as_i64().unwrap_or(0)), e["actor"].as_str().unwrap_or(""), e["kind"].as_str().unwrap_or(""), e["detail"].as_str().unwrap_or(""))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn fmt_task_show(v: &Value) -> String {
    let mut s = format!("{}  {}\n", v["id"].as_str().unwrap_or(""), v["title"].as_str().unwrap_or(""));
    s.push_str(&format!("state {} · {} → {}", v["state"].as_str().unwrap_or(""), v["from"].as_str().unwrap_or(""), v["to"].as_str().unwrap_or("")));
    if let Some(p) = v["parent"].as_str() {
        s.push_str(&format!(" · parent {p}"));
    }
    s.push_str(&format!(" · {}\n", v["file"].as_str().unwrap_or("")));
    if let Some(sum) = v["summary"].as_str() {
        s.push_str(&format!("summary: {sum}\n"));
    }
    if let Some(r) = v["block_reason"].as_str().filter(|_| v["state"] == "blocked") {
        s.push_str(&format!("blocked: {r}\n"));
    }
    let files = list(&v["artifacts"]);
    if files != "none" {
        s.push_str(&format!("files: {files}\n"));
    }
    for (key, head) in [("decisions", "decided"), ("unverified", "not checked")] {
        for item in v[key].as_array().into_iter().flatten().filter_map(Value::as_str) {
            s.push_str(&format!("{head}: {item}\n"));
        }
    }
    let body = v["body"].as_str().unwrap_or("").trim();
    if !body.is_empty() {
        s.push_str(&format!("\n{body}\n"));
    }
    if let Some(q) = v["quote"].as_str() {
        s.push_str("\nthe person's words:\n");
        s.extend(q.lines().map(|l| format!("> {l}\n")));
    }
    for (key, head) in [("accept", "done when"), ("out_of_scope", "out of scope")] {
        let items: Vec<&str> = v[key].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        if !items.is_empty() {
            s.push_str(&format!("\n{head}:\n"));
            s.extend(items.iter().map(|i| format!("- {i}\n")));
        }
    }
    for c in v["children"].as_array().into_iter().flatten() {
        s.push_str(&format!("\n  sub {}", task_line(c)));
    }
    s.push('\n');
    for e in v["events"].as_array().into_iter().flatten() {
        s.push_str(&format!("\n{} {} {} {}", clock(e["at_ms"].as_i64().unwrap_or(0)), e["actor"].as_str().unwrap_or(""), e["kind"].as_str().unwrap_or(""), e["detail"].as_str().unwrap_or("")));
    }
    s
}

/// HH:MM:SS in local time.
fn clock(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_millis_opt(ms).single().map(|t| t.format("%H:%M:%S").to_string()).unwrap_or_default()
}


#[cfg(test)]
mod budget_tests {
    #[test]
    fn budgets_read_like_people_write_them() {
        assert_eq!(super::parse_tokens("300k"), Ok(300_000));
        assert_eq!(super::parse_tokens("1.5M"), Ok(1_500_000));
        assert_eq!(super::parse_tokens("250000"), Ok(250_000));
        assert!(super::parse_tokens("lots").is_err() && super::parse_tokens("0").is_err());
    }
}
