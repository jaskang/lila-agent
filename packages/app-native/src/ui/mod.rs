//! GPUI Kit UI modules and reusable macOS-style window chrome & indicator helpers.

pub mod app;
pub mod composer;
pub mod controls;
pub mod pending;
pub mod sidebar;
pub mod theme_setup;
pub mod transcript;

use gpui_kit::base::InteractiveElementExt as _;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    App, Div, Hsla, InteractiveElement as _, MouseButton, MouseMoveEvent, ParentElement as _,
    SharedString, Stateful, Styled as _, Window, WindowControlArea, div, px,
};

use crate::protocol::SessionStatus;

pub const TOP_BAR_HEIGHT: f32 = 44.0;
pub const WORKSPACE_BAR_HEIGHT: f32 = 38.0;
pub const SHELL_INSET: f32 = 6.0;

/// Leading space for the native macOS traffic lights when controls sit flush to window left.
#[cfg(target_os = "macos")]
pub const TRAFFIC_LIGHT_INSET: f32 = 76.0;
#[cfg(not(target_os = "macos"))]
pub const TRAFFIC_LIGHT_INSET: f32 = 4.0;

/// Leading space inside the inset workspace card when the sidebar is collapsed.
#[cfg(target_os = "macos")]
pub const WORKSPACE_TRAFFIC_LIGHT_INSET: f32 = 72.0;
#[cfg(not(target_os = "macos"))]
pub const WORKSPACE_TRAFFIC_LIGHT_INSET: f32 = 10.0;

/// Single elevated workspace paper card with `12px` radius and 1px hairline border.
pub fn workspace_card(cx: &App) -> Div {
    v_flex()
        .w_full()
        .rounded(px(theme_setup::RADIUS_LG))
        .border_1()
        .border_color(theme_setup::card_border(cx))
        .bg(theme_setup::workspace_bg(cx))
        .shadow(vec![theme_setup::workspace_shadow(cx)])
        .overflow_hidden()
}

#[derive(Default)]
struct DragRegionState {
    should_move: bool,
}

/// Renders an interactive top bar region that supports native window dragging and
/// macOS titlebar double-click zoom, while letting clicks on child controls pass
/// through without triggering a window move.
pub fn window_drag_bar(
    id: impl Into<SharedString>,
    height: f32,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let key = id.into();
    let state = window.use_keyed_state(
        SharedString::from(format!("drag-state-{key}")),
        cx,
        |_, _| DragRegionState::default(),
    );

    h_flex()
        .id(key)
        .h(px(height))
        .w_full()
        .flex_shrink_0()
        .items_center()
        .window_control_area(WindowControlArea::Drag)
        .on_double_click(|_, window, _| {
            #[cfg(target_os = "macos")]
            window.titlebar_double_click();
            #[cfg(not(target_os = "macos"))]
            window.zoom_window();
        })
        .on_mouse_down_out(window.listener_for(&state, |st, _, _, _| {
            st.should_move = false;
        }))
        .on_mouse_down(
            MouseButton::Left,
            window.listener_for(&state, |st, _, window, cx| {
                st.should_move = !window.default_prevented();
                gpui_kit::component::GlobalState::suppress_text_selection(cx);
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            window.listener_for(&state, |st, _, _, _| {
                st.should_move = false;
            }),
        )
        .on_mouse_move(window.listener_for(
            &state,
            |st, event: &MouseMoveEvent, window, cx| {
                if event.pressed_button != Some(MouseButton::Left) {
                    st.should_move = false;
                    return;
                }
                if st.should_move {
                    st.should_move = false;
                    gpui_kit::base::TextSelection::end(window, cx);
                    cx.stop_active_drag(window);
                    window.start_window_move();
                }
            },
        ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeTone {
    Muted,
    Blue,
    Amber,
    Green,
    Red,
}

pub fn pill_badge(label: impl Into<SharedString>, tone: BadgeTone, cx: &App) -> Div {
    let tint_alpha = if cx.theme().is_dark() { 0.16 } else { 0.11 };
    let (bg, fg): (Hsla, Hsla) = match tone {
        BadgeTone::Muted => (theme_setup::chip_bg(cx), cx.theme().muted_foreground),
        BadgeTone::Blue => {
            let c = theme_setup::running_fg(cx);
            (c.opacity(tint_alpha), c)
        }
        BadgeTone::Amber => {
            let c = theme_setup::warning_fg(cx);
            (c.opacity(tint_alpha), c)
        }
        BadgeTone::Green => {
            let c = theme_setup::success_fg(cx);
            (c.opacity(tint_alpha), c)
        }
        BadgeTone::Red => {
            let c = theme_setup::danger_fg(cx);
            (c.opacity(tint_alpha), c)
        }
    };

    div()
        .px(px(6.0))
        .py(px(1.5))
        .rounded(px(theme_setup::RADIUS_SM))
        .bg(bg)
        .text_color(fg)
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(px(11.0))
        .font_weight(gpui_kit::FontWeight::MEDIUM)
        .child(label.into())
}

/// Monospace keyboard shortcut badge (`⌘K`, `⌘N`) with hairline outline.
pub fn kbd_chip(label: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .px(px(5.0))
        .py(px(1.0))
        .rounded(px(theme_setup::RADIUS_SM))
        .border_1()
        .border_color(theme_setup::kbd_border(cx))
        .bg(theme_setup::kbd_bg(cx))
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(px(11.0))
        .text_color(cx.theme().muted_foreground.opacity(0.85))
        .child(label.into())
}

/// Restrained activity dot: only active/waiting/error draw a colored indicator;
/// idle sessions stay quiet so the sidebar stays calm.
pub fn status_dot(status: SessionStatus, cx: &App) -> Option<Div> {
    let color: Hsla = match status {
        SessionStatus::Running => theme_setup::running_fg(cx),
        SessionStatus::Waiting => theme_setup::warning_fg(cx),
        SessionStatus::Error => theme_setup::danger_fg(cx),
        SessionStatus::Idle => return None,
    };
    Some(div().w(px(6.0)).h(px(6.0)).flex_shrink_0().rounded_full().bg(color))
}
