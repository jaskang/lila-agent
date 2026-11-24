# Lila Native Desktop (`gpui-kit`)

Pure Rust native desktop client for Lila, built with [`gpui-kit`](https://github.com/longbridge/gpui-kit) (GPUI + `gpui-component` + `gpui-base`) while reusing `packages/core` (Node 24 + Pi SDK + `pi-subagents` + MCP) over the local WebSocket protocol (`@lila/protocol` v7).

## Architecture

- **Core host (`src/core_host.rs`)**: Discovers Node >= 22.19, manages `core.lock` / `/health` probing / detached `packages/core` spawning, and reads `core-token`.
- **RPC transport (`src/ws_client.rs`, `src/protocol.rs`)**: `tokio-tungstenite` WebSocket client with `Sec-WebSocket-Protocol: lila.<token>`, automatic reconnect, typed RPC calls, and 16ms frame coalescing.
- **Reactive store (`src/store.rs`)**: GPUI `Entity<AppStore>` managing projects, sessions, transcripts, sub-agent nesting, pending approvals/prompts, drafts, and local preferences (`native-prefs.json`).
- **Native UI (`src/ui/`)**:
  - Single-paper-canvas macOS window (`WindowBackgroundAppearance::Blurred`): full-height translucent resizable sidebar on the left (`200–340px`, double-click sash to reset), single `12px`-radius workspace paper card on the right with its top bar embedded inside.
  - Virtualized `MessageScroller` (`720px` golden reading width) + `TextView::markdown` (streaming fade, code copy, collapsible quiet activity/diff/subagent cards, selectable tool & write previews, and rewind).
  - Multi-line `Textarea` composer with floating `/` slash command and `@` file overlays (zero layout shift), full image attachment pipeline (paste / drag-and-drop / file picker), and borderless ghost pickers (Project, keyboard-searchable Model, Thinking, Approval, Context meter).
  - Keyboard-first UX: `⌘N` new session, `⌘B` sidebar, `⌘K` command palette, `⌘[` / `⌘]` adjacent sessions, `⌘.` / `Esc` abort run, `⌘↵` / `Esc` on auto-focused pending approval cards, Enter to save rename dialogs.

## Running

From the repository root, with Rust, Bun, and Node.js 22.19 or later installed:

```sh
bun install
bun run dev
```

`bun run dev` builds the protocol and starts the Rust client. The client starts or reuses the local core. Use `bun run build` to build the protocol and debug executable without opening a window. To build an optimized executable, run `cargo build --release --manifest-path packages/app-native/Cargo.toml` after building the protocol.

### Offline UI Demo Mode (`--demo`)

Preview and iterate on UI/themes instantly without starting `packages/core`:

```sh
cargo run --manifest-path packages/app-native/Cargo.toml -- --demo
```

The demo fixture populates projects, active/running/waiting sessions, streaming sub-agent cards, inline diffs, code blocks, and an interactive pending approval card.

Run native checks and tests:

```sh
cargo check --all-targets --manifest-path packages/app-native/Cargo.toml
cargo test --manifest-path packages/app-native/Cargo.toml
```

## Customizing Themes (Live Reload)

All UI colors are 100% theme-driven via [`themes/lila.json`](./themes/lila.json) (hot-reloaded by `ThemeRegistry::watch_dir`) and surfaced through `src/ui/theme_setup.rs`. Appearance supports a persisted 3-state preference (`跟随系统` / `浅色` / `深色`). See [DESIGN.md](../../DESIGN.md) for the complete Warm Atelier / Warm Graphite design specification.
