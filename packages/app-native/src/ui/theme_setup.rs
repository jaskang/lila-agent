//! Theme initialization, JSON live hot-reload from `themes/`, and macOS-inspired
//! translucent glass surface tokens built on top of `gpui-kit` Theme colors.

use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode, ThemeRegistry, ThemeSet};
use gpui_kit::{App, Hsla, SharedString, Window, rgba};

const EMBEDDED_THEME_JSON: &str = include_str!("../../themes/lila.json");
const LIGHT_NAME: &str = "Lila Light";
const DARK_NAME: &str = "Lila Dark";

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
    let Ok(set) = serde_json::from_str::<ThemeSet>(EMBEDDED_THEME_JSON) else {
        return;
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
pub fn init_themes(cx: &mut App) {
    apply_embedded_lila_themes(cx);
    Theme::sync_system_appearance(None, cx);

    if let Some(dir) = resolve_themes_dir() {
        let _ = ThemeRegistry::watch_dir(dir, cx, |cx| {
            let light_name = SharedString::from(LIGHT_NAME);
            let dark_name = SharedString::from(DARK_NAME);
            let light = ThemeRegistry::global(cx).themes().get(&light_name).cloned();
            let dark = ThemeRegistry::global(cx).themes().get(&dark_name).cloned();
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
        });
    }
}

pub fn toggle_theme_mode(window: &mut Window, cx: &mut App) {
    let next = if cx.theme().is_dark() {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    };
    Theme::change(next, Some(window), cx);
}

// ---------------------------------------------------------------------------
// Translucent macOS-inspired surface tokens derived from the active Theme JSON.

/// Full-bleed main workspace background.
pub fn workspace_bg(cx: &App) -> Hsla {
    cx.theme().background
}

/// Subtle hairline divider (1px vertical split & faint toolbar bottom line).
pub fn hairline(cx: &App) -> Hsla {
    cx.theme().sidebar_border
}

/// Subtle translucent border for inset blocks, code containers, and cards.
pub fn card_border(cx: &App) -> Hsla {
    cx.theme().border
}

/// Elevated glass surface for popovers, composer, and focused sheets.
pub fn composer_bg(cx: &App) -> Hsla {
    if cx.theme().is_dark() {
        rgba(0xffffff0b).into()
    } else {
        rgba(0xffffffd8).into()
    }
}

/// Hairline rim around the floating composer & interactive glass containers.
pub fn composer_border(cx: &App) -> Hsla {
    if cx.theme().is_dark() {
        rgba(0xffffff1a).into()
    } else {
        rgba(0x00000016).into()
    }
}

/// Soft translucent capsule fill for the selected sidebar item (Apple glass pill).
pub fn pill_active_bg(cx: &App) -> Hsla {
    cx.theme().sidebar_accent
}

/// Subtle glass rim around the selected sidebar capsule.
pub fn pill_active_border(cx: &App) -> Hsla {
    if cx.theme().is_dark() {
        rgba(0xffffff22).into()
    } else {
        rgba(0x00000010).into()
    }
}

/// Whisper-quiet surface for collapsible traces, code blocks, and notices.
pub fn subtle_surface_bg(cx: &App) -> Hsla {
    cx.theme().muted
}

/// Secondary elevated card surface (dialogs / subsession banners / diffs).
pub fn card_bg(cx: &App) -> Hsla {
    if cx.theme().is_dark() {
        rgba(0xffffff08).into()
    } else {
        rgba(0xffffffc8).into()
    }
}

/// Neutral translucent bubble for user messages (monochromatic, zero neon tint).
pub fn user_bubble_bg(cx: &App) -> Hsla {
    if cx.theme().is_dark() {
        rgba(0xffffff13).into()
    } else {
        rgba(0x0000000a).into()
    }
}

pub fn user_bubble_border(cx: &App) -> Hsla {
    if cx.theme().is_dark() {
        rgba(0xffffff16).into()
    } else {
        rgba(0x0000000d).into()
    }
}

/// Restrained attention surface for pending approvals and questions.
pub fn attention_bg(cx: &App) -> Hsla {
    if cx.theme().is_dark() {
        rgba(0x28272be6).into()
    } else {
        rgba(0xffffffeb).into()
    }
}

pub fn attention_border(cx: &App) -> Hsla {
    cx.theme()
        .warning
        .opacity(if cx.theme().is_dark() { 0.42 } else { 0.38 })
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
