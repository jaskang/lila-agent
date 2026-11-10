//! Lila's CLI and daemon as a library, so the desktop app (lila-app) talks to
//! lilad the same way the `lila` command does.

pub mod accounts;
pub mod check;
pub mod commands;
pub mod client;
pub mod commit;
pub mod daemon;
pub mod github;
pub mod host;
pub mod init;
pub mod magpie;
pub mod paths;
pub mod roles;
pub mod sessions;
pub mod signin;
pub mod settings;
pub mod usage;
pub mod worktree;
