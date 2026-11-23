//! Theme initialization, JSON live hot-reload from `themes/`, and Lila's
//! frosted card surface helpers built on top of `gpui-kit` Theme tokens.

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
// Frosted card surface colors derived directly from active Theme JSON tokens
// so editing `themes/lila.json` live-updates cards, bubbles, and glass layers.

pub fn workspace_bg(cx: &App) -> Hsla {
    cx.theme().background
}

pub fn card_bg(cx: &App) -> Hsla {
    cx.theme().popover
}

pub fn inset_card_bg(cx: &App) -> Hsla {
    cx.theme().muted
}

pub fn card_border(cx: &App) -> Hsla {
    cx.theme().border
}

pub fn glass_rim_border(cx: &App) -> Hsla {
    if cx.theme().is_dark() {
        rgba(0xffffff26).into()
    } else {
        rgba(0xffffffc8).into()
    }
}

pub fn user_bubble_bg(cx: &App) -> Hsla {
    cx.theme()
        .ring
        .opacity(if cx.theme().is_dark() { 0.20 } else { 0.12 })
}

pub fn user_bubble_border(cx: &App) -> Hsla {
    cx.theme()
        .ring
        .opacity(if cx.theme().is_dark() { 0.36 } else { 0.30 })
}

pub fn attention_bg(cx: &App) -> Hsla {
    cx.theme()
        .warning
        .opacity(if cx.theme().is_dark() { 0.18 } else { 0.12 })
}

pub fn attention_border(cx: &App) -> Hsla {
    cx.theme()
        .warning
        .opacity(if cx.theme().is_dark() { 0.48 } else { 0.42 })
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
