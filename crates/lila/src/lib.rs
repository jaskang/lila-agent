//! Lila's CLI and daemon as a library, so the desktop app (lila-app) talks to
//! lilad the same way the `lila` command does.

pub mod check;
pub mod client;
pub mod daemon;
pub mod init;
pub mod paths;
pub mod roles;
pub mod sessions;
pub mod settings;
pub mod usage;
