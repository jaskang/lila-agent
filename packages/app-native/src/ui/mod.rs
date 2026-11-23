//! GPUI Kit UI modules and reusable macOS-style window chrome & indicator helpers.

pub mod app;
pub mod composer;
pub mod controls;
pub mod pending;
pub mod sidebar;
pub mod theme_setup;
pub mod transcript;

use gpui_kit::base::InteractiveElementExt as _;
use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::{
    App, Div, Hsla, InteractiveElement as _, MouseButton, MouseMoveEvent, ParentElement as _,
    SharedString, Stateful, Styled as _, Window, WindowControlArea, div, px, rgb, rgba,
};

use crate::protocol::SessionStatus;

/// Unified top chrome height shared by the full-height sidebar and workspace pane.
pub const TOP_BAR_HEIGHT: f32 = 38.0;
/// Left inset reserved for macOS traffic lights (`x = 14`, 3 lights ≈ 52px + breathing room).
#[cfg(target_os = "macos")]
pub const TRAFFIC_LIGHT_INSET: f32 = 76.0;
#[cfg(not(target_os = "macos"))]
pub const TRAFFIC_LIGHT_INSET: f32 = 12.0;

#[derive(Default)]
struct DragRegionState {
    should_move: bool,
}

/// Renders an interactive top bar region that supports native window dragging and
/// macOS titlebar double-click zoom, while letting clicks on child controls pass
/// through without triggering a window move.
pub fn window_drag_bar(
    id: impl Into<SharedString>,
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
        .h(px(TOP_BAR_HEIGHT))
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
    let dark = cx.theme().is_dark();
    let (bg, fg): (Hsla, Hsla) = match tone {
        BadgeTone::Muted => (theme_setup::subtle_surface_bg(cx), cx.theme().muted_foreground),
        BadgeTone::Blue => (
            rgba(0x0a84ff22).into(),
            if dark { rgb(0x64a8ff).into() } else { rgb(0x0066cc).into() },
        ),
        BadgeTone::Amber => (
            rgba(0xf59e0b22).into(),
            if dark { rgb(0xfbbf24).into() } else { rgb(0xb45309).into() },
        ),
        BadgeTone::Green => (
            rgba(0x30d15820).into(),
            if dark { rgb(0x4ade80).into() } else { rgb(0x15803d).into() },
        ),
        BadgeTone::Red => (cx.theme().danger.opacity(0.15), cx.theme().danger),
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

/// Restrained activity dot: only active/waiting/error draw a colored indicator;
/// idle sessions stay quiet so the sidebar stays calm.
pub fn status_dot(status: SessionStatus, cx: &App) -> Option<Div> {
    let color: Hsla = match status {
        SessionStatus::Running => if cx.theme().is_dark() {
            rgb(0x64a8ff).into()
        } else {
            rgb(0x0071e3).into()
        },
        SessionStatus::Waiting => rgb(0xf59e0b).into(),
        SessionStatus::Error => cx.theme().danger,
        SessionStatus::Idle => return None,
    };
    Some(div().w(px(6.0)).h(px(6.0)).flex_shrink_0().rounded_full().bg(color))
}
