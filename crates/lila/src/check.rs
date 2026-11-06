//! Runs a flow's check command (`[settings] check`) for lilad, off its main loop.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

/// A check lilad wants run; the result comes back as `Daemon::on_check_done`.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckJob {
    pub project: String,
    /// The flow run it belongs to.
    pub anchor: String,
    pub cwd: PathBuf,
    pub cmd: String,
    pub timeout: Duration,
    pub path_env: String,
}

/// What a check returned: its exit code (None: it did not finish) and the end of its output.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckResult {
    pub exit: Option<i32>,
    pub tail: String,
}

impl CheckResult {
    pub fn passed(&self) -> bool {
        self.exit == Some(0)
    }
}

/// Lines and characters of output kept: enough to see the failure, little enough to type into a terminal.
const TAIL_LINES: usize = 40;
const TAIL_CHARS: usize = 3000;

pub fn tail(out: &str) -> String {
    let lines: Vec<&str> = out.lines().collect();
    let kept = lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n");
    let n = kept.chars().count();
    if n <= TAIL_CHARS { kept } else { kept.chars().skip(n - TAIL_CHARS).collect() }
}

/// Runs `job.cmd` with `sh -c` in its folder, stdout and stderr together. Blocks.
pub fn run(job: &CheckJob) -> CheckResult {
    let child = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("exec 2>&1; {}", job.cmd))
        .current_dir(&job.cwd)
        .env("PATH", &job.path_env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // Its own process group, so a timeout stops what it started too.
        .process_group(0)
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return CheckResult { exit: None, tail: format!("could not start `sh -c {}`: {e}", job.cmd) },
    };
    let mut stdout = child.stdout.take().expect("piped");
    // Read as it comes: something the command leaves running may hold the pipe open past its exit.
    let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let (tx, eof) = std::sync::mpsc::channel::<()>();
    let sink = buf.clone();
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        while let Ok(n) = stdout.read(&mut chunk) {
            if n == 0 {
                break;
            }
            sink.lock().unwrap().extend_from_slice(&chunk[..n]);
        }
        let _ = tx.send(());
    });
    let (exit, timed_out) = match child.wait_timeout(job.timeout) {
        Ok(Some(status)) => (status.code(), false),
        Ok(None) => {
            kill_group(&child);
            let _ = child.wait();
            (None, true)
        }
        Err(_) => (None, false),
    };
    let _ = eof.recv_timeout(Duration::from_secs(2));
    let out = String::from_utf8_lossy(&buf.lock().unwrap()).into_owned();
    let mut t = tail(&out);
    if timed_out {
        t.push_str(&format!("\n(stopped after {} s: `check_timeout_secs`)", job.timeout.as_secs()));
    }
    CheckResult { exit, tail: t }
}

/// Stops a check and everything it started: it leads its own process group.
fn kill_group(child: &Child) {
    // SAFETY: killpg only sends a signal; the group is the child's own (`process_group(0)`).
    unsafe {
        libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn job(cmd: &str, timeout_ms: u64) -> CheckJob {
        CheckJob {
            project: "p".into(),
            anchor: "T1".into(),
            cwd: std::env::temp_dir(),
            cmd: cmd.into(),
            timeout: Duration::from_millis(timeout_ms),
            path_env: std::env::var("PATH").unwrap_or_default(),
        }
    }

    #[test]
    fn exit_codes_and_output() {
        let ok = run(&job("echo fine", 5000));
        assert!(ok.passed());
        assert_eq!(ok.tail, "fine");
        let bad = run(&job("echo out; echo err >&2; exit 3", 5000));
        assert_eq!(bad.exit, Some(3));
        assert_eq!(bad.tail, "out\nerr", "stderr comes along");
    }

    #[test]
    fn a_slow_check_is_stopped() {
        let r = run(&job("sleep 5", 300));
        assert_eq!(r.exit, None);
        assert!(r.tail.contains("stopped after"), "{}", r.tail);
    }

    #[test]
    fn what_a_check_leaves_running_does_not_hold_it() {
        let started = Instant::now();
        let r = run(&job("sleep 30 & echo started", 5000));
        assert!(r.passed());
        assert_eq!(r.tail, "started");
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    }

    #[test]
    fn only_the_end_is_kept() {
        let long: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        let t = tail(&long);
        assert!(t.starts_with("line 61\n") && t.ends_with("line 100"), "{t}");
        assert_eq!(tail(&"x".repeat(5000)).len(), TAIL_CHARS);
    }
}
