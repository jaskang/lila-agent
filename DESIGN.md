# Lila — Design System Reference
> Warm Atelier (Light) & Warm Graphite (Dark) — a single elevated `12px`-radius paper canvas framed beside a full-height translucent macOS blurred sidebar, 100% theme-driven semantic tokens, disciplined 3-tier radii, and a single terracotta Ember accent reserved for quiet brand punctuation.

**Theme:** Dual-mode native (`Lila Light` & `Lila Dark`) with persisted 3-state preference (`跟随系统` / `浅色` / `深色`), live hot-reloaded from `packages/app-native/themes/lila.json`.

**Platform & Runtime:** Native macOS desktop client built in Rust with `gpui-kit` (`GPUI` + `gpui-component` + `gpui-base`) using `WindowBackgroundAppearance::Blurred`.

---

## Layout Skeleton — Single Paper Canvas

- **Left Sidebar (`sidebar.rs`)**: Full-height translucent macOS `Blurred` glass (`sidebar.background`), no nested card wrapper, resizable between `200px` and `340px` (`SIDEBAR_DEFAULT_W = 240px`) via a `6px` vertical sash (double-click resets to `240px`).
  - Top bar (`44px`): traffic-light inset (`76px`), collapse toggle (`⌘B`), Ember dot + `Lila`, New Session (`⌘N`), and Search (`⌘K`).
  - Session rows (`32px`, `6px` radius): no repetitive terminal icons on idle rows; only show a `6px` semantic dot for `Running` / `Waiting` / `Error`.
  - Footer (`38px`): `添加项目` + 3-state appearance toggle (`Palette` / `Sun` / `Moon`).
- **Right Workspace (`app.rs` + `ui/mod.rs`)**: Single `workspace_card` (`12px` radius, `1px` hairline border, subtle shadow), inset by `SHELL_INSET = 6px`.
  - Embedded workspace top bar (`38px`): sits inside the top of the workspace paper card (hairline bottom divider only in active sessions; `72px` leading inset when sidebar is collapsed).
  - Golden reading width: `CONTENT_MAX_W = 720px` shared across transcript rows, thinking footer, pending approval card, and composer.
  - Centered New Session empty stage (`max_w = 680px`): Ember dot + `22px` Regular `"要做点什么？"` + Composer (with integrated borderless Project picker) + clickable `short_path` (reveals in Finder).

---

## Tokens — Colors (100% Theme-Driven)

Zero hardcoded `rgb()`/`rgba()` values exist in UI components; every color derives from `packages/app-native/themes/lila.json` via `packages/app-native/src/ui/theme_setup.rs`.

### Light Theme (`Lila Light`) — Warm Atelier

| Role | Hex | Token (`lila.json` / `theme_setup.rs`) |
|------|-----|----------------------------------------|
| Frosted Shell | `#f4f3efe8` | `title_bar.background` / `shell_bg` |
| Translucent Sidebar | `#f4f3efe6` | `sidebar.background` / `sidebar_bg` |
| Workspace Paper | `#ffffff` | `group_box.background` / `workspace_bg` |
| Elevated Composer | `#ffffff` | `background` / `composer_bg` |
| Subtle Surface / User Bubble | `#f2f1ed` | `muted.background` / `subtle_surface_bg` / `user_bubble_bg` |
| Code Trough / Attention | `#f7f6f3` | `secondary.background` / `code_surface_bg` / `attention_bg` |
| Active Sidebar Row | `#0000000e` | `sidebar.accent.background` / `pill_active_bg` |
| Primary Ink | `#1a1918` | `foreground` |
| Muted Ink | `#66645e` | `muted.foreground` |
| Hairline / Card Border | `#00000010` / `#00000014` | `sidebar.border` / `border` |
| Composer Rim | `#00000018` | `input.border` / `composer_border` |
| Ember Accent | `#d97757` | `sidebar.primary.background` / `accent_ember` |
| Running Info (AA) | `#0066cc` | `info.background` / `running_fg` |
| Warning Amber (AA) | `#9a5b00` | `warning.background` / `warning_fg` |
| Success Forest (AA) | `#166534` | `success.background` / `success_fg` |
| Danger Crimson (AA) | `#c81e1e` | `danger.background` / `danger_fg` |

### Dark Theme (`Lila Dark`) — Warm Graphite

| Role | Hex | Token (`lila.json` / `theme_setup.rs`) |
|------|-----|----------------------------------------|
| Frosted Shell | `#1c1c1ae8` | `title_bar.background` / `shell_bg` |
| Translucent Sidebar | `#1c1c1ae8` | `sidebar.background` / `sidebar_bg` |
| Workspace Paper | `#232321` | `group_box.background` / `workspace_bg` |
| Elevated Composer | `#2b2b28` | `background` / `composer_bg` |
| Subtle Surface / User Bubble | `#2c2c29` | `muted.background` / `subtle_surface_bg` / `user_bubble_bg` |
| Code Trough / Attention | `#1d1d1b` | `secondary.background` / `code_surface_bg` / `attention_bg` |
| Active Sidebar Row | `#ffffff14` | `sidebar.accent.background` / `pill_active_bg` |
| Primary Ink | `#ecebe6` | `foreground` |
| Muted Ink | `#9e9b93` | `muted.foreground` |
| Hairline / Card Border | `#ffffff0e` / `#ffffff12` | `sidebar.border` / `border` |
| Composer Rim | `#ffffff16` | `input.border` / `composer_border` |
| Ember Accent | `#e08364` | `sidebar.primary.background` / `accent_ember` |
| Running Info (AA) | `#60a5fa` | `info.background` / `running_fg` |
| Warning Amber (AA) | `#fbbf24` | `warning.background` / `warning_fg` |
| Success Emerald (AA) | `#4ade80` | `success.background` / `success_fg` |
| Danger Coral (AA) | `#f87171` | `danger.background` / `danger_fg` |

---

## Tokens — Typography, Radii & Spacing

- **Body (`font.size`)**: `14.5px` (user bubble & assistant prose)
- **Monospace (`mono_font.size`)**: `12.5px` (code, diffs, tool outputs, paths, telemetry)
- **3-Tier Radii (`theme_setup.rs`)**:
  - `RADIUS_SM = 6.0` — buttons, ghost pickers, sidebar rows, badges, code troughs, kbd chips
  - `RADIUS_MD = 10.0` — user bubbles, autocomplete overlay, connection banner, collapsible cards
  - `RADIUS_LG = 12.0` — single workspace paper card, floating composer, pending approval card
- **Reading Width**: `CONTENT_MAX_W = 720.0`

---

## Interaction & Keyboard Flow

- **Global Shortcuts**: `⌘N` new session, `⌘B` toggle sidebar, `⌘K` command palette, `⌘[` / `⌘]` previous/next session, `⌘.` abort active run.
- **Composer (`composer.rs` + `controls.rs`)**:
  - Floating `deferred` overlay for `/` slash commands and `@` file mentions (zero layout shift).
  - Full image attachment pipeline: `⌘V` clipboard image/file paste, drag-and-drop with `drag.border` feedback, and `+` native file picker.
  - Borderless ghost triggers for Project, keyboard-navigable Model search (`↑`/`↓`/`↵`), Thinking level, Approval mode, and Context meter.
  - `Esc` closes autocomplete first, otherwise aborts an active run.
- **Pending Approvals & Prompts (`pending.rs`)**:
  - Auto-focuses input/editor/card on arrival; `Enter` submits single-line `input`, `⌘↵` confirms primary/submits, `Esc` denies/cancels.
  - Approval/prompt body is selectable (`TextView::markdown`) with one-click `Clipboard` copy.
- **Transcript (`transcript.rs`)**:
  - Borderless warm user bubbles, quiet collapsed activity & tool rows, selectable & copyable tool outputs and `Write` file previews, semantic diff tints (`diff_add_bg` / `diff_del_bg` / `diff_hunk_bg`).
- **Connection Recovery (`app.rs` + `ws_client.rs`)**:
  - Banner exposes `立即重试` (wakes reconnect backoff immediately) and `查看日志` (opens core log).

---

## Offline UI Preview & Live Theme Reload

```sh
# Launch immediately with rich offline demo data (no packages/core required)
cargo run --manifest-path packages/app-native/Cargo.toml -- --demo
```

Edit `packages/app-native/themes/lila.json` while running to live-reload colors, typography, and radii across the entire UI.
