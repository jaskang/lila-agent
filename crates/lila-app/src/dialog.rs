//! The settings dialogs' frame: a dimmed window, a centered card with page
//! navigation on the left, the page on the right and a footer, plus the
//! grouped rows the pages are built from.

use crate::app::LilaApp;
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::scroll::ScrollableElement;
use gpui::component::{ActiveTheme, Disableable, Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
use gpui::prelude::*;
use gpui::*;
use crate::style;

/// Something a dialog does to the app (a click, a page switch).
pub type Act = Box<dyn Fn(&mut LilaApp, &mut Context<LilaApp>)>;

pub const WIDTH: f32 = 920.;
pub const HEIGHT: f32 = 640.;

/// One entry in the dialog's navigation.
pub struct Nav {
    pub id: &'static str,
    pub icon: IconName,
    pub label: &'static str,
    pub active: bool,
    pub badge: Option<String>,
}

pub struct Shell {
    pub id: &'static str,
    /// Over the navigation: what the dialog is about.
    pub heading: AnyElement,
    pub nav: Vec<(Nav, Act)>,
    pub title: String,
    pub description: String,
    /// The page; it scrolls itself (see [`scroll`]) so a page can split.
    pub body: AnyElement,
    /// Left of the buttons: what saving will do, or an error.
    pub note: Option<AnyElement>,
    pub saving: bool,
    /// The main button: "保存" in settings, "下一步" or "创建并启动" in the wizard.
    pub save_label: &'static str,
    /// Off: the main button can't be pressed yet.
    pub can_save: bool,
    /// A "上一步" button left of cancel.
    pub back: Option<Act>,
    pub on_save: Act,
    pub on_close: Act,
}

/// The dialog's own surface: above the dimmed window.
pub fn surface(_cx: &App) -> Hsla {
    style::popover()
}

/// Grouped rows on the dialog's surface: a faint fill with a hairline.
pub fn card(_cx: &App) -> Hsla {
    style::fill()
}

/// A scrolling column for a page's content.
pub fn scroll(id: impl Into<ElementId>, children: Vec<AnyElement>) -> AnyElement {
    v_flex().id(id).flex_1().min_h_0().overflow_y_scrollbar().px_6().py_5().gap_5().children(children).into_any_element()
}

/// A titled block of rows on a card, divided by hairlines.
pub fn group(title: Option<&str>, rows: Vec<AnyElement>, cx: &App) -> AnyElement {
    let _ = cx;
    let n = rows.len();
    v_flex()
        .gap_2()
        .when_some(title, |d, title| d.child(div().px_1().text_size(px(11.)).font_semibold().text_color(style::muted2()).child(title.to_owned())))
        .child(
            v_flex()
                .rounded(px(10.))
                .border_1()
                .border_color(style::hairline())
                .bg(card(cx))
                .children(rows.into_iter().enumerate().map(|(i, r)| div().when(i + 1 < n, |d| d.border_b_1().border_color(gpui::Hsla::from(rgba(0xffffff0d)))).child(r))),
        )
        .into_any_element()
}

/// A row: what it is on the left, its control on the right.
pub fn row(title: impl Into<SharedString>, about: Option<String>, control: impl IntoElement, cx: &App) -> AnyElement {
    let t = cx.theme();
    h_flex()
        .px_4()
        .py_3()
        .min_h(px(58.))
        .gap_4()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(div().text_size(px(13.)).font_medium().child(title.into()))
                .when_some(about, |d, a| d.child(div().text_xs().text_color(t.muted_foreground).child(a))),
        )
        .child(div().flex_none().child(control))
        .into_any_element()
}

/// A row whose control sits under its title, full width (prompts, long inputs).
pub fn block(title: impl Into<SharedString>, about: Option<String>, control: impl IntoElement, aside: Option<AnyElement>, cx: &App) -> AnyElement {
    let t = cx.theme();
    v_flex()
        .px_4()
        .py_3()
        .gap_2()
        .child(
            h_flex()
                .gap_2()
                .child(v_flex().flex_1().min_w_0().gap_0p5().child(div().text_size(px(13.)).font_medium().child(title.into())).when_some(about, |d, a| d.child(div().text_xs().text_color(t.muted_foreground).child(a))))
                .children(aside),
        )
        .child(control)
        .into_any_element()
}

/// Read-only text in a row's control spot.
pub fn value_text(text: impl Into<SharedString>, cx: &App) -> Div {
    div().text_size(px(13.)).text_color(cx.theme().muted_foreground).max_w(px(360.)).truncate().child(text.into())
}

/// − value + for a number.
pub fn stepper(id: &str, text: String, down: Option<Act>, up: Option<Act>, cx: &mut Context<LilaApp>) -> AnyElement {
    let t = cx.theme().clone();
    let button = |name: String, icon: IconName, f: Option<Act>, cx: &mut Context<LilaApp>| {
        let disabled = f.is_none();
        Button::new(SharedString::from(name)).ghost().xsmall().icon(icon).disabled(disabled).when_some(f, |b, f| b.on_click(cx.listener(move |this, _, _, cx| f(this, cx))))
    };
    let _ = t;
    h_flex()
        .h(px(32.))
        .rounded(px(8.))
        .border_1()
        .border_color(style::hairline())
        .bg(style::well())
        .child(button(format!("{id}-down"), IconName::Minus, down, cx).size(px(30.)))
        .child(div().min_w(px(64.)).px_1().text_center().text_size(px(13.)).child(text))
        .child(button(format!("{id}-up"), IconName::Plus, up, cx).size(px(30.)))
        .into_any_element()
}

impl LilaApp {
    /// Starts moving the open dialog with the mouse (a press on its header or navigation top).
    fn drag_dialog(&mut self, ev: &MouseDownEvent) {
        self.dialog_drag = Some((ev.position, self.dialog_offset));
    }

    pub fn dialog(&mut self, shell: Shell, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let Shell { id, heading, nav, title, description, body, note, saving, save_label, can_save, back, on_save, on_close } = shell;
        let on_close: std::rc::Rc<Act> = std::rc::Rc::new(on_close);
        let (close_x, close_cancel) = (on_close.clone(), on_close);
        let offset = self.dialog_offset;
        let (surface, nav_bg) = (surface(cx), gpui::Hsla::from(rgba(0x0000002e)));
        let nav_items: Vec<AnyElement> = nav
            .into_iter()
            .map(|(n, f)| {
                h_flex()
                    .id(n.id)
                    .h(px(32.))
                    .px_2()
                    .gap_2()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .when(n.active, |d| d.bg(style::accent()).text_color(style::fg()))
                    .when(!n.active, |d| d.text_color(style::fg()).hover(|d| d.bg(style::hover())))
                    .child(Icon::new(n.icon).small().text_color(if n.active { style::fg() } else { style::muted() }))
                    .child(div().flex_1().child(n.label))
                    .when_some(n.badge, |d, b| d.child(div().text_xs().text_color(style::muted()).child(b)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        f(this, cx);
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        // Two shadows: a wide soft one that lifts the card off the dimmed window, a tight one for its edge.
        let shadows = vec![
            BoxShadow { color: gpui::black().opacity(0.55), offset: point(px(0.), px(24.)), blur_radius: px(64.), spread_radius: px(0.), inset: false },
            BoxShadow { color: gpui::black().opacity(0.35), offset: point(px(0.), px(2.)), blur_radius: px(8.), spread_radius: px(0.), inset: false },
        ];
        div()
            .id(SharedString::from(format!("{id}-backdrop")))
            .absolute()
            .inset_0()
            .bg(gpui::black().opacity(0.42))
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                let Some((start, from)) = this.dialog_drag else { return };
                if ev.pressed_button != Some(MouseButton::Left) {
                    this.dialog_drag = None;
                    return;
                }
                this.dialog_offset = from + (ev.position - start);
                cx.notify();
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.dialog_drag = None))
            .child(
                h_flex()
                    .id(id)
                    .relative()
                    .left(offset.x)
                    .top(offset.y)
                    .w(px(WIDTH))
                    .max_w(relative(0.94))
                    .h(px(HEIGHT))
                    .max_h(relative(0.9))
                    .rounded(px(16.))
                    .border_1()
                    .border_color(gpui::Hsla::from(rgba(0xffffff1a)))
                    .bg(surface)
                    .text_color(t.popover_foreground)
                    .shadow(shadows)
                    .occlude()
                    .child(
                        v_flex()
                            .w(px(210.))
                            .flex_none()
                            .h_full()
                            .rounded_l(px(16.))
                            .bg(nav_bg)
                            .border_r_1()
                            .border_color(style::divider())
                            .px(px(10.))
                            .py(px(22.))
                            .gap(px(2.))
                            .child(div().id(SharedString::from(format!("{id}-grip-nav"))).px_2().pt_1().pb_3().on_mouse_down(MouseButton::Left, cx.listener(|this, ev, _, _| this.drag_dialog(ev))).child(heading))
                            .children(nav_items),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(
                                v_flex()
                                    .id(SharedString::from(format!("{id}-grip")))
                                    .flex_none()
                                    .pl_6()
                                    .pr(px(56.))
                                    .pt(px(22.))
                                    .pb_4()
                                    .gap_1()
                                    .border_b_1()
                                    .border_color(style::divider())
                                    .on_mouse_down(MouseButton::Left, cx.listener(|this, ev, _, _| this.drag_dialog(ev)))
                                    .child(div().text_size(px(17.)).font_semibold().child(title))
                                    .when(!description.is_empty(), |d| d.child(div().text_xs().text_color(style::muted()).child(description))),
                            )
                            .child(v_flex().flex_1().min_h_0().child(body))
                            .child(
                                h_flex()
                                    .flex_none()
                                    .px_5()
                                    .py_3()
                                    .gap_2()
                                    .border_t_1()
                                    .border_color(style::divider())
                                    .child(div().flex_1().min_w_0().text_xs().children(note))
                                    .when_some(back, |d, back| {
                                        d.child(Button::new(SharedString::from(format!("{id}-back"))).outline().label("上一步").disabled(saving).on_click(cx.listener(move |this, _, _, cx| {
                                            back(this, cx);
                                            cx.notify();
                                        })))
                                    })
                                    .child(Button::new(SharedString::from(format!("{id}-cancel"))).ghost().label("取消").on_click(cx.listener(move |this, _, _, cx| {
                                        close_cancel(this, cx);
                                        cx.notify();
                                    })))
                                    .child(Button::new(SharedString::from(format!("{id}-save"))).primary().label(save_label).disabled(saving || !can_save).on_click(cx.listener(move |this, _, _, cx| on_save(this, cx)))),
                            ),
                    )
                    .child(div().absolute().top(px(16.)).right(px(14.)).child(Button::new(SharedString::from(format!("{id}-x"))).ghost().xsmall().icon(IconName::Close).tooltip("关闭 (Esc)").on_click(cx.listener(move |this, _, _, cx| {
                        close_x(this, cx);
                        cx.notify();
                    })))),
            )
            .into_any_element()
    }
}


/// A segmented control: one choice of a few, the chosen one filled.
pub fn segmented(id: &str, options: &[&str], selected: usize, pick: impl Fn(&mut LilaApp, usize, &mut Window, &mut Context<LilaApp>) + 'static, cx: &mut Context<LilaApp>) -> AnyElement {
    let t = cx.theme().clone();
    let pick = std::rc::Rc::new(pick);
    let _ = &t;
    h_flex()
        .h(px(32.))
        .p(px(3.))
        .gap(px(2.))
        .rounded(px(8.))
        .border_1()
        .border_color(style::hairline())
        .bg(style::well())
        .children(options.iter().enumerate().map(|(i, label)| {
            let on = i == selected;
            let pick = pick.clone();
            div()
                .id(SharedString::from(format!("{id}-{i}")))
                .h_full()
                .px_3()
                .flex()
                .items_center()
                .rounded(px(6.))
                .text_size(px(13.))
                .font_medium()
                .cursor_pointer()
                // The chosen segment is a raised thumb, not an inverted block.
                .when(on, |d| d.bg(gpui::Hsla::from(rgba(0xffffff29))).text_color(style::fg()).shadow_xs())
                .when(!on, |d| d.text_color(style::muted()).hover(|d| d.text_color(style::fg())))
                .child(label.to_string())
                .on_click(cx.listener(move |this, _, window, cx| {
                    pick(this, i, window, cx);
                    cx.notify();
                }))
        }))
        .into_any_element()
}
