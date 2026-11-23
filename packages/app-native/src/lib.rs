//! Pure Rust Lila desktop client built with `longbridge/gpui-kit`.
//!
//! Connects directly to `packages/core` over `tokio-tungstenite` WebSocket
//! (with 16ms frame coalescing) and renders a 120fps GPU-accelerated UI with
//! live-reloadable JSON themes (`themes/lila.json`) and macOS window blur.

pub mod core_host;
pub mod format;
pub mod protocol;
pub mod shell_bridge;
pub mod store;
pub mod transcript_layout;
pub mod ui;
pub mod ws_client;
