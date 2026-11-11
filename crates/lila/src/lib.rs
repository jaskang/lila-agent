//! Lila's CLI and daemon as a library, so the desktop app (lila-app) talks to
//! lilad the same way the `lila` command does.

pub mod accounts;
pub mod check;
pub mod changes;
pub mod client;
pub mod commands;
pub mod daemon;
pub mod github;
pub mod host;
pub mod magpie;
pub mod merge;
pub mod paths;
pub mod policy;
pub mod project;
pub mod sessions;
pub mod signin;
pub mod usage;
pub mod worktree;
