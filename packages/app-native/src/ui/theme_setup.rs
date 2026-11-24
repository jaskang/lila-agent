//! Light and dark themes, live JSON reload, and Lila-specific surface tokens.

use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode, ThemeRegistry, ThemeSet};
use gpui_kit::{App, BoxShadow, Hsla, SharedString, Window, px, rgba};
use serde::{Deserialize, Serialize};

const EMBEDDED_THEME_JSON: &str = include_str!("../../themes/lila.json");
const LIGHT_NAME: &str = "Lila Light";
const DARK_NAME: &str = "Lila Dark";

/// Disciplined 3-tier corner radius geometry (px).
pub const RADIUS_SM: f32 = 6.0;
pub const RADIUS_MD: f32 = 10.0;
pub const RADIUS_LG: f32 = 12.0;

/// Golden reading column max width (65-75 chars at 14.5px).
pub const CONTENT_MAX_W: f32 = 720.0;

/// Resizable sidebar bounds.
pub const SIDEBAR_MIN_W: f32 = 200.0;
pub const SIDEBAR_MAX_W: f32 = 340.0;
pub const SIDEBAR_DEFAULT_W: f32 = 240.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePreference {
    pub fn next(self) -> Self {
        match self {
            Self::System => Self::Light,
            Self::Light => Self::Dark,
            Self::Dark => Self::System,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "跟随系统外观",
            Self::Light => "浅色外观",
            Self::Dark => "深色外观",
        }
    }
}

/// Locate a writable `themes/` directory (repo workspace or app data dir) for
/// live JSON theme customization.
fn resolve_themes_dir() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("themes"),
        std::env::current_dir()
            .ok()
            .map(|d| d.join("packages/app-native/themes"))
            .unwrap_or_default(),
        crate::core_host::data_dir().join("themes"),
    ];
    candidates.into_iter().find(|p| p.is_dir())
}

fn apply_embedded_lila_themes(cx: &mut App) {
    let set = match serde_json::from_str::<ThemeSet>(EMBEDDED_THEME_JSON) {
        Ok(set) => set,
        Err(err) => {
            eprintln!("[lila-native] failed to parse embedded themes/lila.json: {err}");
            return;
        }
    };
    let mut light = None;
    let mut dark = None;
    for cfg in set.themes {
        let rc = Rc::new(cfg);
        match rc.mode {
            ThemeMode::Light => light = Some(rc),
            ThemeMode::Dark => dark = Some(rc),
        }
    }
    Theme::update(cx, |theme| {
        if let Some(l) = light {
            theme.light_theme = l;
        }
        if let Some(d) = dark {
            theme.dark_theme = d;
        }
    });
}

/// Register Lila's default light/dark themes, sync with system appearance, and
/// watch `themes/` for live JSON edits.
pub fn init_themes(pref: ThemePreference, cx: &mut App) {
    apply_embedded_lila_themes(cx);
    apply_theme_preference(pref, None, cx);

    if let Some(dir) = resolve_themes_dir()
        && let Err(err) = ThemeRegistry::watch_dir(dir.clone(), cx, |cx| {
            let light_name = SharedString::from(LIGHT_NAME);
            let dark_name = SharedString::from(DARK_NAME);
            let light = ThemeRegistry::global(cx).themes().get(&light_name).cloned();
            let dark = ThemeRegistry::global(cx).themes().get(&dark_name).cloned();
            if light.is_none() || dark.is_none() {
                eprintln!(
                    "[lila-native] themes/lila.json reloaded without both '{LIGHT_NAME}' and '{DARK_NAME}'"
                );
            }
            Theme::update(cx, |theme| {
                if let Some(l) = light {
                    theme.light_theme = l;
                }
                if let Some(d) = dark {
                    theme.dark_theme = d;
                }
            });
            let mode = Theme::global(cx).mode;
            Theme::change(mode, None, cx);
        }) {
            eprintln!(
                "[lila-native] failed to watch themes directory {}: {err}",
                dir.display()
            );
    }
}

pub fn apply_theme_preference(pref: ThemePreference, window: Option<&mut Window>, cx: &mut App) {
    match pref {
        ThemePreference::System => Theme::sync_system_appearance(window, cx),
        ThemePreference::Light => Theme::change(ThemeMode::Light, window, cx),
        ThemePreference::Dark => Theme::change(ThemeMode::Dark, window, cx),
    }
}

// ---------------------------------------------------------------------------
// Lila Design System surface & semantic tokens — 100% derived from themes/lila.json

/// Translucent warm glass over the native macOS blurred window material.
pub fn shell_bg(cx: &App) -> Hsla {
    cx.theme().title_bar
}

/// Translucent sidebar surface (inherits shell glass for single-card clarity).
pub fn sidebar_bg(cx: &App) -> Hsla {
    cx.theme().sidebar
}

/// Single elevated workspace paper card surface.
pub fn workspace_bg(cx: &App) -> Hsla {
    cx.theme().group_box
}

/// Subtle hairline divider (1px toolbar bottom rule & internal section seams).
pub fn hairline(cx: &App) -> Hsla {
    cx.theme().sidebar_border
}

/// Hairline border for inset cards, collapsible blocks, and tool containers.
pub fn card_border(cx: &App) -> Hsla {
    cx.theme().border
}

/// Subtle elevation for the floating composer.
pub fn composer_shadow(cx: &App) -> BoxShadow {
    let color = if cx.theme().is_dark() {
        0x0000002c
    } else {
        0x1a19180f
    };
    BoxShadow::new(px(0.0), px(2.0), rgba(color).into()).blur_radius(px(6.0))
}

/// Restraint shadow anchoring the single workspace paper card onto the frosted shell.
pub fn workspace_shadow(cx: &App) -> BoxShadow {
    let color = if cx.theme().is_dark() {
        0x00000038
    } else {
        0x1a191812
    };
    BoxShadow::new(px(0.0), px(1.0), rgba(color).into())
        .blur_radius(px(4.0))
        .spread_radius(px(-1.0))
}

/// Elevation for floating overlays (autocomplete popup).
pub fn overlay_shadow(cx: &App) -> BoxShadow {
    let color = if cx.theme().is_dark() {
        0x0000004a
    } else {
        0x1a191818
    };
    BoxShadow::new(px(0.0), px(6.0), rgba(color).into())
        .blur_radius(px(18.0))
        .spread_radius(px(-2.0))
}

/// Whisper-quiet secondary surface for expanded traces, status strips, and notices.
pub fn subtle_surface_bg(cx: &App) -> Hsla {
    cx.theme().muted
}

/// Recessed code / output / diff trough inside cards (`secondary.background`).
pub fn code_surface_bg(cx: &App) -> Hsla {
    cx.theme().secondary
}

/// Elevated composer surface (`background`).
pub fn composer_bg(cx: &App) -> Hsla {
    cx.theme().background
}

/// Hairline rim around the floating composer & interactive command surfaces.
pub fn composer_border(cx: &App) -> Hsla {
    cx.theme().input
}

/// Fill for selected sidebar rows.
pub fn pill_active_bg(cx: &App) -> Hsla {
    cx.theme().sidebar_accent
}

/// Secondary card surface (dialogs / sub-session banners / embedded panels).
pub fn card_bg(cx: &App) -> Hsla {
    cx.theme().muted
}

/// Quiet chip surface for tags, attachment pills, and status counts.
pub fn chip_bg(cx: &App) -> Hsla {
    cx.theme().accent.opacity(0.65)
}

pub fn chip_border(cx: &App) -> Hsla {
    cx.theme().border
}

/// Keycap surface (`⌘K`, `⌘N`) in menus and hints.
pub fn kbd_bg(cx: &App) -> Hsla {
    cx.theme().accent.opacity(0.6)
}

pub fn kbd_border(cx: &App) -> Hsla {
    cx.theme().border
}

/// Warm bone/slate surface for user message bubbles.
pub fn user_bubble_bg(cx: &App) -> Hsla {
    cx.theme().muted
}

/// Restrained attention surface for pending approvals and questions.
pub fn attention_bg(cx: &App) -> Hsla {
    cx.theme().secondary
}

pub fn attention_border(cx: &App) -> Hsla {
    cx.theme()
        .warning
        .opacity(if cx.theme().is_dark() { 0.28 } else { 0.34 })
}

/// Signature warm terracotta/ember accent (`sidebar.primary.background` in `lila.json`),
/// reserved for subtle brand punctuation and focused state cues. Never used for large fills.
pub fn accent_ember(cx: &App) -> Hsla {
    cx.theme().sidebar_primary
}

/// Semantic foreground colors — all WCAG AA compliant in both Light and Dark themes.
pub fn running_fg(cx: &App) -> Hsla {
    cx.theme().info
}

pub fn warning_fg(cx: &App) -> Hsla {
    cx.theme().warning
}

pub fn success_fg(cx: &App) -> Hsla {
    cx.theme().success
}

pub fn danger_fg(cx: &App) -> Hsla {
    cx.theme().danger
}

/// Semantic diff line backgrounds derived directly from theme semantic tokens.
pub fn diff_add_bg(cx: &App) -> Hsla {
    cx.theme()
        .success
        .opacity(if cx.theme().is_dark() { 0.14 } else { 0.09 })
}

pub fn diff_del_bg(cx: &App) -> Hsla {
    cx.theme()
        .danger
        .opacity(if cx.theme().is_dark() { 0.14 } else { 0.09 })
}

pub fn diff_hunk_bg(cx: &App) -> Hsla {
    cx.theme()
        .info
        .opacity(if cx.theme().is_dark() { 0.12 } else { 0.07 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_theme_json_parses_light_and_dark() {
        let set: ThemeSet = serde_json::from_str(EMBEDDED_THEME_JSON)
            .expect("embedded themes/lila.json should parse as ThemeSet");
        assert!(set.themes.iter().any(|t| t.mode == ThemeMode::Light));
        assert!(set.themes.iter().any(|t| t.mode == ThemeMode::Dark));
    }
}
