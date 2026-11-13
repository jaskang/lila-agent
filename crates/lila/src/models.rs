//! The models omp offers, asked of a short-lived `omp --mode rpc` (`get_available_models`): a new thread has no omp
//! of its own yet when the person chooses its model.

use anyhow::{Context, Result, bail};
use lila_agent::omp::{Omp, Options};
use lila_agent::{AgentEvent, Choice};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long omp may take to name its models (it looks for them while it starts).
const WAIT: Duration = Duration::from_secs(30);

/// The models `program` (omp) offers, as `provider/model` ids with their names.
pub fn list(program: &Path, path_env: &str, cwd: &Path) -> Result<Vec<Choice>> {
    let mut child = std::process::Command::new(program)
        .args(["--mode", "rpc", "--no-session", "--no-extensions", "--no-skills", "--no-rules", "--no-lsp"])
        .current_dir(cwd)
        .env("PATH", path_env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("start {}", program.display()))?;
    let (Some(mut stdin), Some(out)) = (child.stdin.take(), child.stdout.take()) else { bail!("no pipes to omp") };
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                return;
            }
        }
    });
    let found = ask(&mut stdin, &rx, cwd);
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    found
}

fn ask(stdin: &mut impl Write, rx: &mpsc::Receiver<String>, cwd: &Path) -> Result<Vec<Choice>> {
    let mut omp = Omp::new(Options { cwd: cwd.to_string_lossy().to_string(), run: "models".into() });
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let Ok(line) = rx.recv_timeout(left) else { bail!("omp did not name its models in {}s", WAIT.as_secs()) };
        let step = omp.on_line(&line, 0);
        for frame in &step.send {
            writeln!(stdin, "{frame}")?;
        }
        for ev in step.events {
            match ev {
                AgentEvent::Config { models, .. } if !models.is_empty() => return Ok(models),
                AgentEvent::Item { item } if matches!(item.kind, lila_agent::ItemKind::Error { .. }) => bail!("omp stopped"),
                _ => {}
            }
        }
    }
}
