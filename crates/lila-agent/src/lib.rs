//! Agent runtimes behind one conversation model (docs/architecture-v3.md).
//!
//! `model` is what lilad stores and the window draws: items of a conversation
//! (messages, thoughts, tool calls), permission requests, and what a runtime
//! can do. Each runtime translates its own records into it: `claude` reads
//! Claude Code's transcript (its TUI keeps running; Lila draws the GUI);
//! `codex` speaks `codex app-server` for `lila host codex`; `acp` speaks ACP for
//! `lila host omp` (and any other ACP agent).

pub mod acp;
pub mod claude;
pub mod codex;
pub mod model;

pub use model::*;
