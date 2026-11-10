//! How the window looks: a graphite glass window with two cards. The sidebar (`sidebar.rs`)
//! holds the projects, each with its threads; the center card is a header row and the shown
//! thread's conversation or its terminal, or the box that starts a new session.

use crate::app::LilaApp;
use crate::style::{self, card, muted, muted2};
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::menu::{DropdownMenu, PopupMenuItem};
use gpui::component::spinner::Spinner;
use gpui::component::{Icon, IconName, InteractiveElementExt, Sizable, StyledExt, h_flex, v_flex};
use crate::style::Control as _;
use gpui::prelude::*;
use gpui::*;
use serde_json::{Value, json};

pub fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

/// A thread's state (`starting`, `running`, `waiting`, `idle`, `ended`) as a color.
pub fn status_color(state: &str, _cx: &App) -> Hsla {
    match state {
        "running" => style::blue(),
        "waiting" => style::red(),
        "idle" => muted(),
        _ => gpui::Hsla::from(rgba(0xa1a1a180)),
    }
}

pub fn status_label(state: &str) -> &'static str {
    match state {
        "running" => "工作中",
        "waiting" => "等你",
        "idle" => "空闲",
        "ended" => "已结束",
        _ => "启动中",
    }
}

pub fn dot(color: Hsla) -> Div {
    div().size(px(6.)).flex_none().rounded_full().bg(color)
}

/// A thread's state as a small mark: a spinner while it works, else a dot.
pub fn status_mark(status: &str, cx: &App) -> AnyElement {
    if status == "running" { Spinner::new().xsmall().color(status_color(status, cx)).into_any_element() } else { dot(status_color(status, cx)).into_any_element() }
}

/// A tinted letter tile, its hue picked from the name: the hue on a soft wash.
pub fn avatar(name: &str, size: f32, _cx: &App) -> Div {
    let colors = [style::blue(), style::green(), style::yellow(), gpui::Hsla::from(rgba(0xbf5af2ff)), gpui::Hsla::from(rgba(0x64d2ffff)), style::red()];
    let hash = name.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    let color = colors[hash as usize % colors.len()];
    let letter: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    div()
        .size(px(size))
        .flex_none()
        .rounded(px(size * 0.3))
        .bg(color.opacity(0.26))
        .border_1()
        .border_color(color.opacity(0.3))
        .text_color(style::tint(color, 0.45))
        .text_size(px((size * 0.55).round()))
        .font_semibold()
        .flex()
        .items_center()
        .justify_center()
        .child(letter)
}

/// The first row of the sidebar card (the traffic lights and the app's name)
/// and the center card's project row: as tall as each other, so they line up.
pub const BAR: f32 = 44.;

/// The room the macOS traffic lights take at the start of the sidebar card's
/// first row: they end 76 px from the window's edge (see main.rs).
pub const TRAFFIC_LIGHTS: f32 = 86.;

/// A top bar that moves the window like a title bar: press and move to drag,
/// double-click to zoom (as the system setting says). A press that doesn't move
/// stays a click for the buttons inside.
pub fn drag_bar(bar: Stateful<Div>, cx: &mut Context<LilaApp>) -> Stateful<Div> {
    bar.window_control_area(WindowControlArea::Drag)
        .on_double_click(|_, window, _| window.titlebar_double_click())
        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, _| this.drag_armed = true))
        .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.drag_armed = false))
        .on_mouse_down_out(cx.listener(|this, _, _, _| this.drag_armed = false))
        .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, _| {
            if this.drag_armed && ev.pressed_button == Some(MouseButton::Left) {
                this.drag_armed = false;
                window.start_window_move();
            }
        }))
}

impl LilaApp {
    /// The center card's first row. A thread: its engine, title and how it runs, then its tools (the view, the
    /// permission tier, interrupt, end or resume). A new session: the project it will be in. Drags the window.
    fn header(&mut self, thread: Option<&Value>, terminal: Option<Entity<lila_term::TerminalView>>, cx: &mut Context<Self>) -> impl IntoElement {
        let quiet = style::quiet(cx);
        let tool = |id: &'static str, icon: Icon, tip: &'static str| Button::new(id).custom(quiet).xsmall().icon(icon).tooltip(tip);
        let bar = drag_bar(h_flex().id("thread-bar"), cx).h(px(BAR)).flex_none().pl(px(14.)).pr_2().gap_2().whitespace_nowrap();
        let Some(t) = thread else {
            let avatar = self.new_in.as_deref().map(|id| avatar(id, 20., cx));
            return bar.children(avatar).child(self.project_switch(cx)).child(div().flex_1());
        };
        let id = s(t, "id");
        let ended = t["state"] == "ended";
        let tier = s(t, "permission");
        let spec = crate::engines::spec_label(&s(t, "engine"), &s(t, "model"), &s(t, "effort"));
        let weak = cx.entity().downgrade();
        bar.border_b_1()
            .border_color(style::divider())
            .child(style::cli_tile(&s(t, "engine"), 20.))
            .child(div().min_w_0().truncate().text_size(px(13.)).font_semibold().child(s(t, "title")))
            .child(div().flex_none().text_size(px(12.)).text_color(muted2()).child(spec))
            .child(style::status_pill(&s(t, "state"), cx))
            .child(div().flex_1())
            // Claude Code's TUI keeps running under its conversation.
            .when(t["caps"]["tui"] == true && !ended, |d| d.child(self.view_switch(t, cx)))
            .child(self.tier_button("tier", &tier, Anchor::TopRight, cx, {
                let id = id.clone();
                move |key, cx| {
                    let _ = weak.update(cx, |this, cx| this.act("thread.permission", json!({"thread": id, "permission": key}), cx));
                }
            }))
            .when(t["state"] == "running", |d| {
                let id = id.clone();
                d.child(tool("interrupt-thread", Icon::new(IconName::Square), "中断 (⌘.)").on_click(cx.listener(move |this, _, _, cx| this.act("thread.interrupt", json!({"thread": id}), cx))))
            })
            .map(|d| {
                let id = id.clone();
                if ended {
                    d.child(Button::new("resume-thread").small().small_button().primary().icon(IconName::Play).label("继续").on_click(cx.listener(move |this, _, _, cx| this.act("thread.resume", json!({"thread": id}), cx))))
                } else {
                    d.child(tool("stop-thread", style::icon("lila/power.svg"), "结束").on_click(cx.listener(move |this, _, _, cx| this.act("thread.stop", json!({"thread": id}), cx))))
                }
            })
            .when_some(terminal, |d, view| {
                d.child(tool("terminal-find", Icon::new(IconName::Search), "搜索 (⌘F)").on_click(move |_, window, cx| {
                    view.update(cx, |view, cx| view.open_search(window, cx));
                }))
            })
    }

    /// The project a new session goes to, with a menu of the others when there are any.
    fn project_switch(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let quiet = style::quiet(cx);
        let Some(p) = self.new_in.as_deref().and_then(|id| self.project(id)) else { return div().into_any_element() };
        let many = self.snap.projects.len() > 1;
        let weak = cx.entity().downgrade();
        let others: Vec<(String, String)> = self.snap.projects.iter().map(|p| (p.id.clone(), p.label())).collect();
        let now = p.id.clone();
        Button::new("new-project")
            .custom(quiet)
            .small()
            .label(p.label())
            .when(many, |b| b.dropdown_caret(true))
            .dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, _| {
                if others.len() > 1 {
                    for (id, label) in &others {
                        let (weak, id2) = (weak.clone(), id.clone());
                        menu = menu.item(PopupMenuItem::new(label.clone()).checked(*id == now).on_click(move |_, window, cx| {
                            let _ = weak.update(cx, |this, cx| this.show_new(id2.clone(), window, cx));
                        }));
                    }
                }
                menu
            })
            .into_any_element()
    }

    pub fn center(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let quiet = style::quiet(cx);
        let thread = self.shown_thread();
        let address = thread.as_ref().map(|t| s(t, "terminal")).filter(|a| !a.is_empty());
        let mut terminal_view = None;
        let empty = || v_flex().flex_1().items_center().justify_center().gap_3();
        // Shown as its conversation (the default), else its terminal.
        let gui = thread.as_ref().filter(|t| self.shows_gui(t)).cloned();
        let body: AnyElement = match (&thread, address) {
            _ if gui.is_some() => self.conversation_view(gui.as_ref().unwrap(), window, cx),
            (Some(_), Some(addr)) => match (self.core_error.clone(), self.terminal(&addr, window, cx)) {
                (Some(e), _) => placeholder(e, cx).into_any_element(),
                (None, Some(view)) => {
                    terminal_view = Some(view.clone());
                    div().flex_1().min_h_0().px_1().child(view).into_any_element()
                }
                (None, None) => empty().child(Spinner::new().color(muted())).into_any_element(),
            },
            (Some(_), None) => placeholder("这个会话已经结束".into(), cx).into_any_element(),
            (None, _) if !self.snap.daemon => empty().child(Spinner::new().color(muted())).child(div().text_color(muted()).child("正在连接 lilad…")).into_any_element(),
            (None, _) if self.snap.projects.is_empty() => empty().child(Button::new("add-project-big").primary().icon(IconName::Plus).label("添加项目").on_click(cx.listener(|this, _, window, cx| this.choose_project(window, cx)))).into_any_element(),
            (None, _) => match self.new_in.clone() {
                Some(p) => self.new_session_view(&p, window, cx),
                None => empty().child(Spinner::new().color(muted())).into_any_element(),
            },
        };
        let header = self.header(thread.as_ref(), terminal_view, cx).into_any_element();
        v_flex().size_full().child(
            card()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(header)
                .when_some(self.notice.clone().or(self.snap.error.clone()), |d, n| {
                    d.child(
                        h_flex()
                            .mx_2()
                            .mt_2()
                            .px_3()
                            .py_2()
                            .gap_2()
                            .rounded(px(10.))
                            .bg(style::red().opacity(0.12))
                            .text_size(px(13.))
                            .child(Icon::new(IconName::CircleAlert).small().text_color(style::red()))
                            .child(div().flex_1().child(n))
                            .child(Button::new("notice-close").custom(quiet).xsmall().icon(IconName::Close).on_click(cx.listener(|this, _, _, cx| {
                                this.notice = None;
                                cx.notify();
                            }))),
                    )
                })
                .child(body),
        )
    }

    /// The center card for a session not started: the box to write its first message in, in the middle.
    fn new_session_view(&mut self, project: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let boxed = self.new_session_box(project, window, cx);
        let key = crate::composer::new_key(project);
        v_flex()
            .flex_1()
            .min_h_0()
            .items_center()
            .justify_center()
            // Files dropped anywhere on the card go with the first message.
            .drag_over::<ExternalPaths>(|d, _, _, _| d.bg(style::blue().opacity(0.06)))
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| this.add_files(&key, paths.paths().iter().cloned(), cx)))
            .child(div().w_full().max_w(px(760.)).px_5().pb(px(80.)).child(boxed))
            .into_any_element()
    }

    /// The row above the box when the engine waits in its terminal (Claude Code's "trust this folder", a login):
    /// the conversation cannot answer it, so it says where to go.
    pub(crate) fn terminal_ask(&mut self, t: &Value, cx: &mut Context<Self>) -> Option<AnyElement> {
        if t["waiting"] != "terminal" {
            return None;
        }
        let id = s(t, "id");
        let tui = t["caps"]["tui"] == true;
        Some(
            h_flex()
                .w_full()
                .px_3()
                .py(px(8.))
                .gap_2()
                .rounded(px(style::R_BLOCK))
                .border_1()
                .border_color(style::yellow().opacity(0.35))
                .bg(style::yellow().opacity(0.06))
                .child(Icon::new(IconName::SquareTerminal).small().text_color(style::yellow()))
                .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).child(format!("{} 在终端里等你", crate::engines::engine_name(&s(t, "engine")))))
                .when(tui, |d| d.child(Button::new("open-terminal").small().small_button().primary().label("打开终端").on_click(cx.listener(move |this, _, _, cx| this.set_view(&id, true, cx)))))
                .into_any_element(),
        )
    }

    /// A permission card on the shown thread: what it wants to do, why the person is asked, and the answers.
    pub(crate) fn permission_card(&mut self, thread: &str, card: &Value, cx: &mut Context<Self>) -> AnyElement {
        let id = s(card, "id");
        let danger = !card["danger"].is_null();
        let options: Vec<(String, String, String)> = card["options"].as_array().into_iter().flatten().map(|o| (s(o, "id"), s(o, "label"), s(o, "kind"))).collect();
        let tui = self.shown_thread().is_some_and(|t| t["caps"]["tui"] == true);
        let detail = Some(s(card, "detail")).filter(|d| !d.is_empty() && *d != s(card, "title"));
        let paths: Vec<String> = card["paths"].as_array().into_iter().flatten().filter_map(|p| p.as_str().map(String::from)).collect();
        let why = Some(s(card, "why")).filter(|d| !d.is_empty());
        let mut first_allow = true;
        v_flex()
            .px_4()
            .py(px(12.))
            .gap(px(9.))
            .child(
                h_flex()
                    .gap(px(7.))
                    .child(Icon::new(IconName::TriangleAlert).small().text_color(if danger { style::red() } else { style::yellow() }))
                    .child(div().text_size(px(13.)).font_semibold().child("要你允许"))
                    .children(why.map(|w| div().flex_1().min_w_0().truncate().text_xs().text_color(muted()).child(w))),
            )
            .child(
                div()
                    .id(SharedString::from(format!("perm-cmd-{id}")))
                    .max_h(px(120.))
                    .overflow_y_scroll()
                    .px_3()
                    .py(px(7.))
                    .rounded(px(6.))
                    .bg(style::well())
                    .font_family(lila_term::theme::FONT)
                    .text_size(px(12.))
                    .line_height(relative(1.5))
                    .child(s(card, "title")),
            )
            .children(detail.map(|d| div().text_xs().text_color(muted()).child(d)))
            .children((!paths.is_empty()).then(|| div().text_xs().text_color(muted()).child(paths.join("\n"))))
            .child(
                h_flex()
                    .gap(px(6.))
                    .flex_wrap()
                    .children(options.into_iter().enumerate().map(|(n, (opt, label, kind))| {
                        let (thread, card, opt) = (thread.to_owned(), id.clone(), opt.clone());
                        // The first way to say yes is the main button; the rest are quieter.
                        let main = kind == "allow" && std::mem::take(&mut first_allow);
                        Button::new(SharedString::from(format!("perm-{id}-{n}")))
                            .small()
                            .small_button()
                            .rounded_full()
                            .when(main, |b| b.primary())
                            .when(!main, |b| b.outline())
                            .label(label)
                            .on_click(cx.listener(move |this, _, _, cx| this.act("thread.answer", json!({"thread": thread, "card": card, "option": opt}), cx)))
                    }))
                    // Claude Code can ask in its own terminal instead.
                    .when(tui, |d| {
                        let (thread, card) = (thread.to_owned(), id.clone());
                        d.child(style::link(SharedString::from(format!("perm-term-{id}")), "在终端里回答", false).text_size(px(13.)).on_click(cx.listener(move |this, _, _, cx| {
                            this.act("thread.answer", json!({"thread": thread, "card": card, "dismiss": true}), cx)
                        })))
                    }),
            )
            .into_any_element()
    }
}

pub fn placeholder(text: String, _cx: &App) -> impl IntoElement {
    div().flex_1().flex().items_center().justify_center().p_8().text_color(muted()).child(text)
}
