//! Lila's CLI and daemon as a library, so the desktop app (lila-app) talks to
//! lilad the same way the `lila` command does.

pub mod client;
pub mod daemon;
pub mod paths;
pub mod roles;
