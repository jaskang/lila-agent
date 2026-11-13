//! The conversation model lilad stores and the window draws, and omp's translation into it (docs/architecture-v3.md).
//!
//! `model` is items of a conversation (messages, thoughts, tool calls), permission
//! requests, questions, and what omp can do; `omp` speaks omp's RPC mode for `lila host omp`.

pub mod model;
pub mod omp;

pub use model::*;
pub use omp::Step;
