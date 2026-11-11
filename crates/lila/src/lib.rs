//! Lila's CLI and daemon as a library, so the desktop app (lila-app) talks to
//! lilad the same way the `lila` command does.

/// The version of lilad's socket protocol. Bump it whenever a method, its parameters or its reply change in a way an older window or `lila`
/// (or an older lilad) would get wrong; `ping` and `status` report it so a stale lilad is noticed (`client::daemon_version`).
/// 3 (A2): `thread.show`/`thread.read` list the thread's `rules`, `thread.rule.remove`, `thread.trust`, `waiting_for` and `trust_folder` on a thread.
pub const PROTOCOL: u32 = 3;

pub mod accounts;
pub mod check;
pub mod changes;
pub mod checkpoint;
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
pub mod rules;
pub mod sessions;
pub mod signin;
pub mod usage;
pub mod worktree;
