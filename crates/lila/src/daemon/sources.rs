//! GitHub jobs that run off lilad's loop (`gh` blocks): looking at a repository's issues and posting a
//! comment. Nothing here knows threads or lilad: P4 (the GitHub panel, a thread from an issue) calls
//! them. No one does yet.

use crate::github::{self, Issue};
use std::path::PathBuf;

/// A look at one source for new issues.
#[derive(Debug, Clone)]
pub struct PollJob {
    pub project: String,
    pub source: usize,
    pub cwd: PathBuf,
    pub repo: Option<String>,
    pub query: String,
    pub path_env: String,
}

impl PollJob {
    pub fn run(&self) -> std::result::Result<Vec<Issue>, String> {
        github::issues(&self.cwd, &self.path_env, self.repo.as_deref(), &self.query).map_err(|e| format!("{e:#}"))
    }
}

/// A comment the person sent.
#[derive(Debug, Clone)]
pub struct PostJob {
    pub project: String,
    /// What the comment is for (an issue's thread).
    pub subject: String,
    pub cwd: PathBuf,
    pub repo: Option<String>,
    pub number: String,
    pub body: String,
    pub path_env: String,
}

impl PostJob {
    pub fn run(&self) -> std::result::Result<String, String> {
        github::comment(&self.cwd, &self.path_env, self.repo.as_deref(), &self.number, &self.body).map_err(|e| format!("{e:#}"))
    }
}
