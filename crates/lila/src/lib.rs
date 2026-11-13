//! Lila's CLI and daemon as a library, so the desktop app (lila-app) talks to
//! lilad the same way the `lila` command does.

/// The version of lilad's socket protocol. Bump it whenever a method, its parameters or its reply change in a way an older window or `lila`
/// (or an older lilad) would get wrong; `ping` and `status` report it so a stale lilad is noticed (`client::daemon_version`).
/// 3 (A2): `thread.show`/`thread.read` list the thread's `rules`, `thread.rule.remove`, `thread.trust`, `waiting_for` and `trust_folder` on a thread.
/// 4 (omp only): no engines. A thread has no `engine`, `caps` or `terminal`-dialog state (`waiting_for`, `trust_folder`), and a thread from before has `legacy`;
/// `thread.new`, `thread.resume` and `project.set` take no `engine` or `effort`; `thread.trust`, `thread.run`, `hook` and `hook.answer` are gone; `thread.read` has no `writing`.
/// 5 (omp RPC): `thread.read` lists the thread's `questions` (cards beside `permissions`: an agent asking the person), `thread.answer` answers them with
/// `value`, `yes` or `cancel`, `models.list`; a conversation can hold `error` items; the tools' `name` is omp's own (`bash`, `edit`).
pub const PROTOCOL: u32 = 5;

pub mod check;
pub mod changes;
pub mod checkpoint;
pub mod client;
pub mod daemon;
pub mod github;
pub mod host;
pub mod merge;
pub mod models;
pub mod paths;
pub mod policy;
pub mod project;
pub mod rules;
pub mod worktree;
