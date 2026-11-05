//! `lila`: run an agent team on herdr-core. The person uses it to start projects
//! and answer the inbox; members use it to start, finish, block and hand on tasks.

mod client;
mod daemon;
mod paths;
mod roles;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use paths::Paths;
use serde_json::{Value, json};

#[derive(Parser)]
#[command(name = "lila", version, about = "Run a team of coding agents: you talk to the owner, the owner runs the team.")]
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
        /// A first request for the owner, as task T-from-you.
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
    /// (owner) Ask the person a question; the answer comes back as <lila answer>.
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
    /// Stop delivering tasks (notes still go out).
    Pause,
    /// Resume delivering tasks.
    Resume,
    /// The daemon.
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
    Start {
        id: String,
    },
    Done {
        id: String,
        #[arg(long)]
        summary: String,
        /// A deliverable or report (repeatable).
        #[arg(long = "file")]
        files: Vec<String>,
    },
    Block {
        id: String,
        #[arg(long)]
        reason: String,
    },
    Cancel {
        id: String,
        #[arg(long)]
        reason: Option<String>,
    },
    Show {
        id: String,
    },
    Ls {
        /// Only tasks assigned to or by you.
        #[arg(long)]
        mine: bool,
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
        Cmd::Pause => out(call("pause", json!({}))?, &|_| "paused: no new tasks are delivered".into()),
        Cmd::Resume => out(call("resume", json!({}))?, &|_| "resumed".into()),
        Cmd::Daemon { cmd: DaemonCmd::Run } => daemon::serve(paths.clone(), client::hc_bin())?,
        Cmd::Daemon { cmd: DaemonCmd::Stop } => {
            call("stop", json!({}))?;
            println!("lilad stopped; members keep running");
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

fn task(cmd: TaskCmd, call: &dyn Fn(&str, Value) -> Result<Value>, out: &Out) -> Result<()> {
    match cmd {
        TaskCmd::New { to, title, body, body_file, parent, priority } => {
            let body = match body_file {
                Some(f) => std::fs::read_to_string(&f).with_context(|| format!("read {}", f.display()))?,
                None => body.unwrap_or_default(),
            };
            let v = call("task.new", json!({"to": to, "title": title, "body": body, "parent": parent, "priority": priority}))?;
            out(v, &|v| {
                let when = if v["state"] == "delivered" { "delivered now" } else { "queued until they are free" };
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
        TaskCmd::Done { id, summary, files } => out(call("task.done", json!({"id": id, "summary": summary, "files": files}))?, &|v| {
            let to = if v["from"] == "human" { "the person".to_string() } else { v["from"].as_str().unwrap_or("?").to_string() };
            format!("{} done; {to} was told", v["id"].as_str().unwrap_or("?"))
        }),
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
    let file = cwd.join(".lila").join("project.toml");
    if file.exists() {
        bail!("{} already exists", file.display());
    }
    let name = match name {
        Some(n) => n,
        None => slug(&cwd.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "project".into())),
    };
    lila_core::config::validate_id(&name).map_err(|e| anyhow::anyhow!("project name {name:?}: {e}; pass --name"))?;
    std::fs::create_dir_all(file.parent().unwrap())?;
    std::fs::write(&file, default_project(&name))?;
    let written = roles::install_defaults(&paths.roles())?;
    println!("created {}", file.display());
    if !written.is_empty() {
        println!("installed roles in {}: {}", paths.roles().display(), written.join(", "));
    }
    println!("next: edit the team if you like, then `lila up --brief \"what you want built\"`");
    Ok(())
}

fn slug(s: &str) -> String {
    let mut out: String = s.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let out = out.trim_matches('-').to_string();
    let out = if out.starts_with(|c: char| c.is_ascii_lowercase()) { out } else { format!("p-{out}") };
    out.chars().take(24).collect::<String>().trim_end_matches('-').to_string()
}

fn default_project(name: &str) -> String {
    format!(
        r#"name = "{name}"
owner = "owner"

[[members]]
id = "owner"
role = "owner"

[[members]]
id = "architect"
role = "architect"

[[members]]
id = "engineer"
role = "engineer"

[[members]]
id = "tester"
role = "tester"

# Who may hand work to whom (the owner may assign to anyone).
# Without an entry, a member follows its role's `can_assign`.
# [assign]
# engineer = ["tester"]

[settings]
# Non-owner members working at once. Raise it if your subscription allows.
max_working = 1
# Minutes a member may sit idle on an open task before a reminder.
stall_minutes = 5
# Fix cycles two members may bounce one chain between them before the owner decides.
max_rounds = 3
"#
    )
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
        s.push_str(&format!("brief sent to the owner as {t}\n"));
    }
    if !v["started"].as_array().is_none_or(|a| a.is_empty()) {
        s.push_str("First start in a folder: Claude Code asks whether you trust it. `lila inbox` tells you if a member is waiting.\n");
    }
    s.push_str("Talk to the owner in its terminal (herdr-app, or `hc attach lila-<project>-owner`).");
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
    let body = v["body"].as_str().unwrap_or("").trim();
    if !body.is_empty() {
        s.push_str(&format!("\n{body}\n"));
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

/// HH:MM:SS in UTC offset-free local-ish form: good enough for a log.
fn clock(ms: i64) -> String {
    let secs = ms / 1000;
    let offset = local_offset_secs();
    let t = (secs + offset).rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)
}

fn local_offset_secs() -> i64 {
    // `date +%z` is cheap and avoids a timezone dependency.
    std::process::Command::new("date")
        .arg("+%z")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|z| {
            let z = z.trim();
            let sign = if z.starts_with('-') { -1 } else { 1 };
            let digits = z.trim_start_matches(['+', '-']);
            if digits.len() != 4 {
                return None;
            }
            Some(sign * (digits[..2].parse::<i64>().ok()? * 3600 + digits[2..].parse::<i64>().ok()? * 60))
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("My Shop!"), "my-shop");
        assert_eq!(slug("lila-agent"), "lila-agent");
        assert_eq!(slug("2048"), "p-2048");
    }

    #[test]
    fn default_project_builds_with_default_roles() {
        let dir = tempfile::tempdir().unwrap();
        roles::install_defaults(dir.path()).unwrap();
        let roles = roles::load(dir.path()).unwrap();
        let cfg = lila_core::ProjectConfig::parse(&default_project("shop")).unwrap();
        let team = lila_core::Team::build(&cfg, &roles).unwrap();
        assert_eq!(team.members.len(), 4);
        assert_eq!(team.assign["engineer"], ["tester"]);
        assert_eq!(team.assign["tester"], ["engineer"]);
    }
}
