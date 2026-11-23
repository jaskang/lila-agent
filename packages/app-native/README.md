# Lila Native Desktop (`gpui-kit`)

Pure Rust native desktop client for Lila, built with [`gpui-kit`](https://github.com/longbridge/gpui-kit) (GPUI + `gpui-component` + `gpui-base`) while reusing `packages/core` (Node 24 + Pi SDK + `pi-subagents` + MCP) over the local WebSocket protocol (`@lila/protocol` v7).

## Architecture

- **Core host (`src/core_host.rs`)**: Discovers Node >= 22.19, manages `core.lock` / `/health` probing / detached `packages/core` spawning, and reads `core-token`.
- **RPC transport (`src/ws_client.rs`, `src/protocol.rs`)**: `tokio-tungstenite` WebSocket client with `Sec-WebSocket-Protocol: lila.<token>`, automatic reconnect, typed RPC calls, and 16ms frame coalescing.
- **Reactive store (`src/store.rs`)**: GPUI `Entity<AppStore>` managing projects, sessions, transcripts, sub-agent nesting, pending approvals/prompts, drafts, and local preferences (`native-prefs.json`).
- **Native UI (`src/ui/`)**:
  - Blurred macOS window (`WindowBackgroundAppearance::Blurred`) with custom `TitleBar` and frosted cards.
  - Virtualized `MessageScroller` + `TextView::markdown` (with streaming fade, code highlighting, collapsible activity/diff/subagent cards, and rewind).
  - Multi-line `Textarea` composer with `/` slash commands, `@` file mentions, image chips, and searchable model/thinking/approval/context popovers.
  - Native `Dialog` modals (rename, delete, remove project, `⌘K` command palette) and `Notification` toasts.

## Running

From the repository root:

```sh
cargo run --manifest-path packages/app-native/Cargo.toml
```

Run checks and tests:

```sh
cargo check --all-targets --manifest-path packages/app-native/Cargo.toml
cargo test --manifest-path packages/app-native/Cargo.toml
```

## Customizing Themes (Live Reload)

Lila ships with `Lila Light` and `Lila Dark` in [`themes/lila.json`](./themes/lila.json), watched at runtime via `ThemeRegistry::watch_dir`. Edit any color token (including RGBA alpha for frosted surfaces), `radius`, or `shadow` in `themes/lila.json` and save — the running window updates immediately without restarting.
