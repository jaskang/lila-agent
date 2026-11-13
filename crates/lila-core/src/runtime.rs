//! How omp is started for a thread: through `lila host omp`, in the thread's terminal.

use std::path::Path;

/// The program to run. `argv[0]` is a bare name; the caller resolves it on PATH.
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    pub argv: Vec<String>,
}

/// The CLI a thread needs on PATH.
pub const CLI: &str = "omp";

/// Driven by `lila host omp` in the thread's terminal; lilad adds `--program <omp>`. `session_dir` holds the thread's
/// omp sessions: omp goes on with the last one in it, and starts one there when there is none.
pub fn launch(model: &str, session_dir: &Path) -> Launch {
    let mut argv: Vec<String> = ["lila", "host", "omp"].map(String::from).into();
    if !model.is_empty() {
        argv.extend(["--model".into(), model.into()]);
    }
    argv.extend(["--session-dir".into(), session_dir.to_string_lossy().to_string()]);
    Launch { argv }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omp_runs_through_the_host_in_the_threads_own_sessions() {
        assert_eq!(launch("openai/gpt-6", Path::new("/d/omp/t1")).argv, ["lila", "host", "omp", "--model", "openai/gpt-6", "--session-dir", "/d/omp/t1"]);
        assert_eq!(launch("", Path::new("/d/omp/t1")).argv, ["lila", "host", "omp", "--session-dir", "/d/omp/t1"], "no model: omp's own default");
    }
}
