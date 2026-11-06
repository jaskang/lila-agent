//! The window's look, from the HTML prototype (`prototype/lila-window.html`):
//! a graphite frosted-glass window, content on 12 px cards one step lighter
//! than the glass, Apple's dark system colors, fills instead of borders.
//!
//! The gpui-kit theme is set from the same palette (see [`install`]), so the
//! kit's buttons, inputs, menus and selects match the hand-built parts.

use gpui::component::{Icon, IconName, Sizable, StyledExt, Theme, ThemeConfig, ThemeMode, h_flex};
use gpui::prelude::*;
use gpui::*;
use std::borrow::Cow;

/// Window padding, and the gutter between cards (also the resize handle).
pub const GAP: f32 = 8.;
/// Cards.
pub const R_BLOCK: f32 = 12.;
/// Buttons, inputs, rows.
pub const R_CTL: f32 = 8.;
/// A one-line row in the sidebar.
pub const ROW: f32 = 30.;
/// A member's two-line row.
pub const MEMBER_ROW: f32 = 46.;
/// The status bar card.
pub const STATUS: f32 = 34.;

fn c(hex: u32) -> Hsla {
    rgba(hex).into()
}

// ---- text ----
pub fn fg() -> Hsla {
    c(0xf5f5f7ff)
}
/// Secondary label.
pub fn muted() -> Hsla {
    c(0xebebf599)
}
/// Tertiary label: hints, counts, shortcuts.
pub fn muted2() -> Hsla {
    c(0xebebf552)
}

// ---- system colors ----
pub fn blue() -> Hsla {
    c(0x0a84ffff)
}
pub fn green() -> Hsla {
    c(0x30d158ff)
}
pub fn red() -> Hsla {
    c(0xff453aff)
}
pub fn yellow() -> Hsla {
    c(0xffd60aff)
}
/// Claude Code's own orange, for its tile.
pub fn claude() -> Hsla {
    c(0xf0905fff)
}

// ---- fills: white/black at low alpha so they sit on any glass ----
/// Wells: inputs, steppers, segmented controls.
pub fn well() -> Hsla {
    c(0x0000003d)
}
pub fn hover() -> Hsla {
    c(0xffffff0d)
}
/// A chosen row.
pub fn accent() -> Hsla {
    c(0xffffff17)
}
/// Grouped rows, inbox notes.
pub fn fill() -> Hsla {
    c(0xffffff0a)
}
/// Thin dividers inside cards.
pub fn divider() -> Hsla {
    c(0xffffff0f)
}
pub fn hairline() -> Hsla {
    c(0xffffff12)
}

// ---- materials: heavier = more structural ----
/// The window: neutral graphite glass; the wallpaper only tints it.
pub fn glass() -> Hsla {
    c(0x18191cdb)
}
/// Cards: a lighter step of the window's own glass.
pub fn block() -> Hsla {
    c(0xffffff0e)
}
pub fn block_line() -> Hsla {
    c(0xffffff0f)
}
/// Drawers and dialogs float above the cards. Nearly opaque: GPUI can't blur
/// what is under a view, so a see-through panel would show the cards' text.
pub fn popover() -> Hsla {
    c(0x232328fa)
}

/// Default height of inputs, selects, steppers and buttons.
pub const CTL: f32 = 32.;
/// Compact buttons inside cards and rows.
pub const CTL_SM: f32 = 28.;

/// Form controls at the prototype's sizes. gpui-kit's own presets are 14 px
/// text and 24 px for `small`; these apply after them.
pub trait Control: Styled + Sized {
    /// An input or select: 32 tall, 13 px text, 12 px in from the edges.
    fn control(self) -> Self {
        self.h(px(CTL)).px(px(12.)).text_size(px(13.)).rounded(px(R_CTL))
    }
    /// A select: a control on a raised step of glass instead of a well.
    fn select_control(self) -> Self {
        self.control().bg(gpui::Hsla::from(rgba(0xffffff12)))
    }
    /// A multi-line input (its editor pads itself).
    fn area(self) -> Self {
        self.text_size(px(13.)).rounded(px(9.))
    }
    /// A compact labelled button (kit `small` is 24 tall).
    fn small_button(self) -> Self {
        self.h(px(CTL_SM)).px(px(12.))
    }
}

impl<T: Styled> Control for T {}

/// A card: the terminal, each overview section, the status bar.
pub fn card() -> Div {
    div().rounded(px(R_BLOCK)).bg(block()).border_1().border_color(block_line()).overflow_hidden()
}

/// A color mixed toward white: tinted text on a soft wash of the same hue.
pub fn tint(color: Hsla, toward_white: f32) -> Hsla {
    Hsla { l: color.l + (1. - color.l) * toward_white, a: 1., ..color }
}

/// A small label tag: the hue on a soft wash.
pub fn tag(color: Hsla, text: impl Into<SharedString>) -> Div {
    div().h(px(18.)).px(px(6.)).flex_none().flex().items_center().rounded(px(5.)).text_size(px(11.)).font_semibold().whitespace_nowrap().text_color(tint(color, 0.2)).bg(color.opacity(0.18)).child(text.into())
}

/// A neutral tag (ids of queued or cancelled tasks).
pub fn tag_neutral(text: impl Into<SharedString>) -> Div {
    div().h(px(18.)).px(px(6.)).flex_none().flex().items_center().rounded(px(5.)).text_size(px(11.)).font_semibold().text_color(muted()).bg(c(0xffffff14)).child(text.into())
}

/// A round count (unanswered inbox items).
pub fn pill(color: Hsla, text: impl Into<SharedString>) -> Div {
    tag(color, text).rounded(px(9.)).min_w(px(18.)).justify_center().px(px(5.))
}

/// A key cap in the status bar's shortcut hints.
pub fn kbd(text: &'static str) -> Div {
    div().px(px(4.)).py(px(1.)).rounded(px(4.)).text_size(px(10.)).font_medium().text_color(muted()).bg(c(0xffffff12)).child(text)
}

/// Which CLI a member runs, as a small tile: Claude Code's asterisk in its
/// orange, Codex as a neutral prompt.
pub fn cli_tile(runtime: &str, size: f32) -> Div {
    let codex = runtime == "codex";
    let (color, bg, line, icon) = if codex { (c(0xe5e5eaff), c(0xffffff17), c(0xffffff1a), "lila/prompt.svg") } else { (claude(), claude().opacity(0.16), claude().opacity(0.2), "lila/claude.svg") };
    div()
        .size(px(size))
        .flex_none()
        .rounded(px(size * 0.32))
        .bg(bg)
        .border_1()
        .border_color(line)
        .flex()
        .items_center()
        .justify_center()
        .child(Icon::empty().path(icon).size(px(size * 0.6)).text_color(color))
}

/// An icon from Lila's own set (see [`Assets`]).
pub fn icon(name: &'static str) -> Icon {
    Icon::empty().path(name)
}

/// A text action like "+ 新建": the accent color, no chrome.
pub fn link(id: impl Into<ElementId>, text: impl Into<SharedString>, plus: bool) -> Stateful<Div> {
    h_flex()
        .id(id)
        .flex_none()
        .gap(px(3.))
        .text_color(blue())
        .font_medium()
        .cursor_pointer()
        .hover(|d| d.text_color(c(0x409cffff)))
        .when(plus, |d| d.child(Icon::new(IconName::Plus).xsmall()))
        .child(text.into())
}

/// One place that says what state a member is in: a mark and a word on a soft wash.
pub fn status_pill(status: &str, cx: &App) -> Div {
    let color = crate::ui::status_color(status, cx);
    h_flex()
        .h(px(20.))
        .pl(px(7.))
        .pr(px(8.))
        .gap(px(6.))
        .flex_none()
        .rounded(px(10.))
        .text_size(px(11.))
        .font_semibold()
        .text_color(tint(color, 0.15))
        .bg(color.opacity(0.15))
        .child(crate::ui::status_mark(status, cx))
        .child(crate::ui::status_label(status))
}

/// The kit's Lucide subset, plus Lila's own icons under `lila/`.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let own: Option<&'static [u8]> = match path {
            "lila/claude.svg" => Some(include_bytes!("../assets/lila/claude.svg")),
            "lila/prompt.svg" => Some(include_bytes!("../assets/lila/prompt.svg")),
            "lila/branch.svg" => Some(include_bytes!("../assets/lila/branch.svg")),
            _ => None,
        };
        match own {
            Some(bytes) => Ok(Some(Cow::Borrowed(bytes))),
            None => gpui::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        gpui::assets::Assets.list(path)
    }
}

/// The kit's dark theme in Lila's palette.
pub fn install(cx: &mut App) {
    let config = r##"{
        "name": "Lila Graphite",
        "mode": "dark",
        "radius": 8,
        "radius.lg": 12,
        "colors": {
            "background": "#0000003d",
            "foreground": "#f5f5f7",
            "border": "#ffffff12",
            "input.border": "#ffffff14",
            "ring": "#0a84ff",
            "caret": "#f5f5f7",
            "selection.background": "#0a84ff66",
            "muted.background": "#0000003d",
            "muted.foreground": "#ebebf599",
            "accent.background": "#0a84ff",
            "accent.foreground": "#ffffff",
            "popover.background": "#28282e",
            "popover.foreground": "#f5f5f7",
            "primary.background": "#f5f5f7",
            "primary.hover.background": "#ffffff",
            "primary.active.background": "#d6d6dc",
            "primary.foreground": "#111113",
            "secondary.background": "#ffffff14",
            "secondary.hover.background": "#ffffff1f",
            "secondary.active.background": "#ffffff17",
            "secondary.foreground": "#f5f5f7",
            "sidebar.background": "#00000000",
            "sidebar.foreground": "#f5f5f7",
            "sidebar.accent.background": "#ffffff17",
            "sidebar.accent.foreground": "#f5f5f7",
            "sidebar.border": "#ffffff0f",
            "sidebar.primary.background": "#f5f5f7",
            "sidebar.primary.foreground": "#111113",
            "list.background": "#00000000",
            "list.active.background": "#0a84ff33",
            "list.active.border": "#0a84ff",
            "list.hover.background": "#ffffff0d",
            "scrollbar.background": "#00000000",
            "scrollbar.thumb.background": "#ffffff24",
            "scrollbar.thumb.hover.background": "#ffffff38",
            "switch.background": "#ffffff29",
            "danger.background": "#ff453a",
            "danger.foreground": "#ff6961",
            "success.background": "#30d158",
            "success.foreground": "#30d158",
            "warning.background": "#ffd60a",
            "warning.foreground": "#ffd60a",
            "info.background": "#0a84ff",
            "info.foreground": "#409cff",
            "link.foreground": "#0a84ff",
            "link.hover.foreground": "#409cff",
            "link.active.foreground": "#0a84ff",
            "overlay": "#0000006b",
            "window.border": "#ffffff17",
            "tab_bar.background": "#00000000",
            "title_bar.background": "#00000000",
            "status_bar.background": "#00000000",
            "base.red": "#ff453a",
            "base.red.light": "#ff6961",
            "base.green": "#30d158",
            "base.green.light": "#6ad08c",
            "base.blue": "#0a84ff",
            "base.blue.light": "#409cff",
            "base.yellow": "#ffd60a",
            "base.yellow.light": "#ffe066",
            "base.magenta": "#bf5af2",
            "base.magenta.light": "#da8fff",
            "base.cyan": "#64d2ff",
            "base.cyan.light": "#a0e4ff"
        }
    }"##;
    match serde_json::from_str::<ThemeConfig>(config) {
        Ok(config) => {
            Theme::global_mut(cx).dark_theme = std::rc::Rc::new(config);
            Theme::change(ThemeMode::Dark, None, cx);
        }
        Err(e) => eprintln!("[lila-app] theme: {e}"),
    }
}
