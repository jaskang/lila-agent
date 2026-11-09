//! Lila's domain: roles, projects and teams, tasks and their state machine,
//! assignment policy, prompts, launch commands and dispatch decisions.
//! Nothing here does IO, so all of it is unit-tested.

pub mod config;
pub mod dispatch;
pub mod flow;
pub mod inbox;
pub mod message;
pub mod prompt;
pub mod request;
pub mod route;
pub mod runtime;
pub mod task;

pub use config::{Member, ProjectConfig, Role, RoleFile, Runtime, Settings, Team, Tools, expand_ids, validate_id};
pub use task::{Actor, Brief, Outcome, Task, TaskState, Transition};
