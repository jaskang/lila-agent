//! How the window looks: a graphite glass window with two cards. The sidebar (`sidebar.rs`)
//! holds the workspaces, each with its threads; the center card is a header row and the shown
//! thread's conversation or its terminal, or the box that starts a new session.

use crate::app::LilaApp;
use crate::style::{self, card, muted, muted2};
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::menu::{DropdownMenu, PopupMenuItem};
use gpui::component::spinner::Spinner;
use gpui::component::{Disableable as _, Icon, IconName, InteractiveElementExt, Sizable, StyledExt, h_flex, v_flex};
use crate::style::Control as _;
use gpui::prelude::*;
use gpui::*;
use serde_json::{Value, json};

pub fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

pub fn dot(color: Hsla) -> Div {
    div().size(px(6.)).flex_none().rounded_full().bg(color)
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

/// The first row of the sidebar card (the traffic lights, fold the sidebar)
/// and the center card's title row: as tall as each other, so they line up.
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
    /// The center card's first row. A thread: its title, then in grey the engine, model and effort, and on the right
    /// its actions (`header_actions`). A new session: the workspace it will be in. With the sidebar folded away the row
    /// leaves room for the traffic lights and starts with the button that brings it back. Drags the window.
    fn header(&mut self, thread: Option<&Value>, terminal: Option<Entity<lila_term::TerminalView>>, cx: &mut Context<Self>) -> impl IntoElement {
        let quiet = style::quiet(cx);
        let folded = self.sidebar_hidden;
        let bar = drag_bar(h_flex().id("thread-bar"), cx)
            .h(px(BAR))
            .flex_none()
            .pl(px(if folded { TRAFFIC_LIGHTS } else { 14. }))
            .pr_2()
            .gap_2()
            .whitespace_nowrap()
            .when(folded, |d| d.child(Button::new("sidebar-show").custom(quiet).xsmall().icon(IconName::PanelLeftOpen).tooltip("展开侧栏 (⌘B)").on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx)))));
        let Some(t) = thread else {
            let avatar = self.new_in.as_deref().map(|id| avatar(id, 20., cx));
            return bar.children(avatar).child(self.project_switch(cx)).child(div().flex_1());
        };
        let engine = s(t, "engine");
        let info = format!("{} · {}", crate::engines::engine_name(&engine), crate::engines::spec_label(&engine, &s(t, "model"), &s(t, "effort")));
        bar.border_b_1()
            .border_color(style::divider())
            .child(div().flex_none().max_w(relative(0.45)).truncate().text_size(px(13.)).font_semibold().child(s(t, "title")))
            // What runs it, and the branch: they give way first when the card is narrow.
            .child(
                h_flex()
                    .min_w_0()
                    .overflow_hidden()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(muted2())
                    .child(div().flex_none().child(info))
                    .when(t["worktree"] == true, |d| {
                        // A thread in a worktree of its own works on its own branch.
                        d.child(h_flex().flex_none().gap_1().child(style::icon("lila/git-branch.svg").size(px(12.)).text_color(muted2())).child(format!("lila/{}", s(t, "id"))))
                    }),
            )
            .child(div().flex_1())
            .child(h_flex().flex_none().gap_1().children(self.header_actions(t, terminal, cx)))
    }

    /// The buttons at the right end of a thread's first row, left to right.
    fn header_actions(&mut self, t: &Value, terminal: Option<Entity<lila_term::TerminalView>>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let quiet = style::quiet(cx);
        let tool = |id: &'static str, icon: IconName, tip: &'static str| Button::new(id).custom(quiet).xsmall().icon(icon).tooltip(tip);
        let id = s(t, "id");
        // A merged thread is over: its worktree is gone, there is nothing to continue or to show.
        let merged = t["merged_ms"].is_number();
        let ended = t["state"] == "ended" && !merged;
        let mut actions: Vec<AnyElement> = Vec::new();
        // Claude Code's TUI keeps running under its conversation.
        if t["caps"]["tui"] == true && !ended {
            actions.push(self.view_switch(t, cx).into_any_element());
        }
        // Only an ended session has a button: a message to a live or ended one carries on, and ending is in the sidebar menu.
        if ended {
            actions.push(Button::new("resume-thread").small().small_button().primary().icon(IconName::Play).label("继续").on_click(cx.listener(move |this, _, _, cx| this.act("thread.resume", json!({"thread": id}), cx))).into_any_element());
        }
        // Searches the terminal, when it is what is shown.
        if let Some(view) = terminal {
            actions.push(tool("terminal-find", IconName::Search, "搜索 (⌘F)").on_click(move |_, window, cx| view.update(cx, |view, cx| view.open_search(window, cx))).into_any_element());
        }
        // The files and changes buttons open the right panel.
        actions.extend(self.panel_buttons(t, cx));
        let cwd = s(t, "cwd");
        if !cwd.is_empty() && !merged {
            actions.push(
                tool("reveal-cwd", IconName::FolderOpen, "在访达中显示")
                    .on_click(move |_, _, _| {
                        let _ = std::process::Command::new("open").arg(&cwd).spawn();
                    })
                    .into_any_element(),
            );
        }
        actions
    }

    /// The workspace a new session goes to, with a menu of the others when there are any.
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
            (None, _) if self.snap.projects.is_empty() => empty().child(Button::new("add-project-big").primary().icon(IconName::Plus).label("新建工作区").on_click(cx.listener(|this, _, window, cx| this.choose_project(window, cx)))).into_any_element(),
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
    /// the conversation cannot answer it, so it says where to go. For the trust dialog lilad has recognised (`waiting_for`),
    /// the row shows the folder and answers it in place.
    pub(crate) fn terminal_ask(&mut self, t: &Value, cx: &mut Context<Self>) -> Option<AnyElement> {
        if t["waiting"] != "terminal" {
            return None;
        }
        let id = s(t, "id");
        let tui = t["caps"]["tui"] == true;
        let trust = trust_folder(t);
        let strip = h_flex()
            .w_full()
            .px_3()
            .py(px(8.))
            .gap_2()
            .rounded(px(style::R_BLOCK))
            .border_1()
            .border_color(style::yellow().opacity(0.35))
            .bg(style::yellow().opacity(0.06))
            .child(Icon::new(IconName::SquareTerminal).small().text_color(style::yellow()));
        let open = |id: String, primary: bool, cx: &mut Context<Self>| {
            Button::new("open-terminal").small().small_button().when(primary, |b| b.primary()).when(!primary, |b| b.outline()).label("打开终端").on_click(cx.listener(move |this, _, _, cx| this.set_view(&id, true, cx)))
        };
        let Some(folder) = trust else {
            return Some(strip.child(div().flex_1().min_w_0().truncate().text_size(px(13.)).child(format!("{} 在终端里等你", crate::engines::engine_name(&s(t, "engine"))))).when(tui, |d| d.child(open(id, true, cx))).into_any_element());
        };
        let busy = self.trusting.contains(&id);
        let thread = id.clone();
        Some(
            strip
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_2()
                        .child(div().flex_none().text_size(px(13.)).child("信任此文件夹"))
                        // The end of a long path is the folder itself.
                        .child(div().min_w_0().truncate().text_size(px(12.)).font_family(lila_term::theme::FONT).text_color(muted2()).child(crate::data::tail(&crate::data::tilde(&folder), 48))),
                )
                .when(tui, |d| d.child(open(id.clone(), false, cx)))
                .child(Button::new("trust-folder").small().small_button().primary().loading(busy).disabled(busy).label("信任并继续").on_click(cx.listener(move |this, _, _, cx| {
                    let thread = thread.clone();
                    this.trusting.insert(thread.clone());
                    cx.notify();
                    let id = thread.clone();
                    this.call("thread.trust", json!({"thread": id}), cx, move |this, r, cx| {
                        this.trusting.remove(&thread);
                        if let Err(e) = r {
                            this.notice = Some(format!("{e:#}"));
                        }
                        this.refresh_now();
                        cx.notify();
                    });
                })))
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
                            // A rule's words can be long (a chained command): the button keeps to the card, the tooltip has all.
                            .label(crate::engines::clip(&label, 44))
                            .tooltip(label.clone())
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

/// The folder of the trust dialog a thread waits at, when lilad has recognised it (`waiting_for`); an older lilad, or any other
/// reason a terminal waits, says nothing, and the row is the plain "waits in its terminal".
pub fn trust_folder(t: &Value) -> Option<String> {
    (t["waiting"] == "terminal" && t["waiting_for"] == "trust").then(|| s(t, "trust_folder")).filter(|f| !f.is_empty())
}

pub fn placeholder(text: String, _cx: &App) -> impl IntoElement {
    div().flex_1().flex().items_center().justify_center().p_8().text_color(muted()).child(text)
}

#[cfg(test)]
mod tests {
    use super::trust_folder;
    use serde_json::json;

    #[test]
    fn only_a_recognised_trust_dialog_gets_the_trust_row() {
        let waiting = json!({"waiting": "terminal", "waiting_for": "trust", "trust_folder": "/Users/me/shop"});
        assert_eq!(trust_folder(&waiting).as_deref(), Some("/Users/me/shop"));
        // A terminal that waits for something else, an older lilad (nothing said), a dialog without its folder, a thread that waits for a card.
        assert_eq!(trust_folder(&json!({"waiting": "terminal", "waiting_for": null})), None);
        assert_eq!(trust_folder(&json!({"waiting": "terminal"})), None);
        assert_eq!(trust_folder(&json!({"waiting": "terminal", "waiting_for": "trust"})), None);
        assert_eq!(trust_folder(&json!({"waiting": "card", "waiting_for": "trust", "trust_folder": "/x"})), None);
        assert_eq!(trust_folder(&json!({"waiting": null, "waiting_for": "trust", "trust_folder": "/x"})), None);
    }
}
