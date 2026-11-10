//! Lila's domain: the engines a thread can run on, permission tiers, the data a
//! thread is made of, projects' defaults, and the commands that start each
//! engine. Nothing here does IO, so all of it is unit-tested.

pub mod config;
pub mod runtime;
pub mod thread;

pub use config::{Defaults, ProjectConfig, Runtime, model_source, validate_id};
pub use thread::{AgentStatus, Thread, ThreadState, Tier, new_thread_id, valid_thread_id};
