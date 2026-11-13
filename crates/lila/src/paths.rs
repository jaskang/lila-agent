//! Where Lila keeps things. `LILA_HOME` puts everything under one folder (tests, experiments).

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    /// `lila.db`.
    pub data: PathBuf,
    /// The socket and the hosts' logs.
    pub state: PathBuf,
    /// Set when `LILA_HOME` is: the daemon then runs without the shared `lilad` name.
    pub isolated: bool,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => home().join(fallback),
    }
}

impl Paths {
    pub fn from_env() -> Paths {
        if let Some(h) = std::env::var_os("LILA_HOME").filter(|v| !v.is_empty()) {
            let h = PathBuf::from(h);
            return Paths { data: h.join("data"), state: h.join("state"), isolated: true };
        }
        Paths {
            data: xdg("XDG_DATA_HOME", ".local/share").join("lila"),
            state: xdg("XDG_STATE_HOME", ".local/state").join("lila"),
            isolated: false,
        }
    }

    pub fn db(&self) -> PathBuf {
        self.data.join("lila.db")
    }

    pub fn socket(&self) -> Result<PathBuf> {
        let p = self.state.join("lilad.sock");
        // macOS limits Unix socket paths to 104 bytes. A deep state folder gets
        // a socket in the temp folder, named after the state folder.
        if p.as_os_str().len() < 100 {
            return Ok(p);
        }
        let tmp = std::env::temp_dir().join(format!("lila-{:016x}.sock", fnv(p.as_os_str().as_encoded_bytes())));
        if tmp.as_os_str().len() >= 100 {
            bail!("socket path {} is too long for a Unix socket; set LILA_HOME to a shorter folder", p.display());
        }
        Ok(tmp)
    }

    /// A project's threads' worktrees (`worktree`): `<data>/worktrees/<project>/<thread>`.
    pub fn worktrees(&self, project: &str) -> PathBuf {
        self.data.join("worktrees").join(project)
    }

    /// One thread's omp sessions (`omp --session-dir`): what a resumed thread goes on with.
    pub fn omp_sessions(&self, thread: &str) -> PathBuf {
        self.data.join("omp").join(thread)
    }

    /// One thread's worktree.
    pub fn worktree(&self, project: &str, thread: &str) -> PathBuf {
        self.worktrees(project).join(thread)
    }

    pub fn ensure(&self) -> Result<()> {
        for d in [&self.data, &self.state] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3))
}

/// The folder holding `.lila/project.toml`, searching up from `start`.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut cur = Some(start);
    while let Some(dir) = cur {
        if dir.join(".lila").join("project.toml").is_file() {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

/// `name` resolved on `path` (a PATH-style list), or `name` itself if it has a slash.
pub fn which(name: &str, path: &str) -> Option<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    ::which::which_in(name, Some(path), cwd).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_root_upwards() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".lila")).unwrap();
        std::fs::write(dir.path().join(".lila/project.toml"), "").unwrap();
        let deep = dir.path().join("a/b");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(find_project_root(&deep).unwrap(), dir.path());
        assert!(find_project_root(Path::new("/")).is_none());
    }

    #[test]
    fn long_state_folders_get_a_short_socket() {
        let long = PathBuf::from("/".to_string() + &"x".repeat(120));
        let p = Paths { data: long.clone(), state: long, isolated: true };
        let sock = p.socket().unwrap();
        assert!(sock.as_os_str().len() < 100 && sock.to_string_lossy().ends_with(".sock"));
        assert_eq!(sock, p.socket().unwrap(), "stable");
    }

    #[test]
    fn which_resolves_on_path() {
        assert!(which("sh", "/nonexistent:/bin").unwrap().ends_with("sh"));
        assert!(which("definitely-not-a-program", "/bin").is_none());
        assert!(which("/bin/sh", "").is_some());
    }
}
