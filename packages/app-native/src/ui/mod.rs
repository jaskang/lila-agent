//! GPUI Kit UI modules and reusable frosted-card / badge helpers.

pub mod app;
pub mod composer;
pub mod controls;
pub mod pending;
pub mod sidebar;
pub mod theme_setup;
pub mod transcript;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{App, Div, Hsla, IntoElement, ParentElement as _, Styled as _, div, px, rgb, rgba};

use crate::protocol::SessionStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeTone {
    Muted,
    Blue,
    Amber,
    Green,
    Red,
}

pub fn pill_badge(label: impl Into<gpui_kit::SharedString>, tone: BadgeTone, cx: &App) -> Div {
    let dark = cx.theme().is_dark();
    let (bg, fg): (Hsla, Hsla) = match tone {
        BadgeTone::Muted => (theme_setup::inset_card_bg(cx), cx.theme().muted_foreground),
        BadgeTone::Blue => (
            rgba(0x3b82f626).into(),
            if dark { rgb(0x60a5fa).into() } else { rgb(0x2563eb).into() },
        ),
        BadgeTone::Amber => (
            rgba(0xf59e0b26).into(),
            if dark { rgb(0xfbbf24).into() } else { rgb(0xd97706).into() },
        ),
        BadgeTone::Green => (
            rgba(0x10b98126).into(),
            if dark { rgb(0x34d399).into() } else { rgb(0x059669).into() },
        ),
        BadgeTone::Red => (cx.theme().danger.opacity(0.16), cx.theme().danger),
    };

    div()
        .px(px(7.0))
        .py(px(1.5))
        .rounded_full()
        .bg(bg)
        .text_color(fg)
        .text_size(px(10.0))
        .font_weight(gpui_kit::FontWeight::MEDIUM)
        .child(label.into())
}

pub fn status_dot(status: SessionStatus, cx: &App) -> impl IntoElement {
    let color: Hsla = match status {
        SessionStatus::Running => rgb(0x3b82f6).into(),
        SessionStatus::Waiting => rgb(0xf59e0b).into(),
        SessionStatus::Error => rgb(0xef4444).into(),
        SessionStatus::Idle => cx.theme().muted_foreground.opacity(0.45),
    };
    div().w(px(7.0)).h(px(7.0)).rounded_full().bg(color)
}
