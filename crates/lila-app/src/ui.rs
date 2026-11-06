//! How the window looks: a graphite glass window with three cards. The
//! sidebar holds the projects, then the team as a tree under the PM; the
//! center card is a header row and the shown member's terminal; the overview
//! holds what needs you, the task board and activity.

use crate::app::LilaApp;
use crate::data;
use crate::style::{self, card, muted, muted2};
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::input::Input;
use gpui::component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui::component::scroll::ScrollableElement;
use gpui::component::spinner::Spinner;
use gpui::component::{Icon, IconName, InteractiveElementExt, Sizable, StyledExt, h_flex, v_flex};
use crate::style::Control as _;
use gpui::prelude::*;
use gpui::*;
use serde_json::{Value, json};
use std::time::Duration;

pub fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

pub fn status_color(status: &str, _cx: &App) -> Hsla {
    match status {
        "working" => style::blue(),
        "blocked" | "error" => style::red(),
        "done" => style::green(),
        "idle" => muted(),
        _ => gpui::Hsla::from(rgba(0xa1a1a180)),
    }
}

pub fn status_label(status: &str) -> &'static str {
    match status {
        "working" => "工作中",
        "blocked" => "等你",
        "done" => "刚完成",
        "idle" => "空闲",
        "error" => "出错",
        "gone" => "未运行",
        _ => "启动中",
    }
}

pub fn dot(color: Hsla) -> Div {
    div().size(px(6.)).flex_none().rounded_full().bg(color)
}

/// A member's status as a small mark: a spinner while it works, else a dot.
pub fn status_mark(status: &str, cx: &App) -> AnyElement {
    if status == "working" { Spinner::new().xsmall().color(status_color(status, cx)).into_any_element() } else { dot(status_color(status, cx)).into_any_element() }
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

/// A section heading in the sidebar (项目, 成员).
fn label(text: impl Into<SharedString>) -> Div {
    div().text_xs().font_semibold().text_color(muted()).child(text.into())
}

/// A card's title row: small title, count, then actions on the right.
fn card_head(title: &'static str, count: usize) -> Div {
    h_flex()
        .h(px(42.))
        .flex_none()
        .pl_4()
        .pr_3()
        .gap(px(6.))
        .child(div().text_size(px(13.)).font_semibold().text_color(gpui::Hsla::from(rgba(0xebebf5cc))).child(title))
        .when(count > 0, |d| d.child(div().text_xs().text_color(muted2()).child(count.to_string())))
        .child(div().flex_1())
}

/// A sidebar row: a soft highlight when chosen or hovered.
fn row(id: impl Into<ElementId>, active: bool) -> Stateful<Div> {
    h_flex()
        .id(id)
        .mx_2()
        .px_2()
        .h(px(style::ROW))
        .mb(px(style::ROW_GAP))
        .gap_2()
        .rounded(px(style::R_CTL))
        .cursor_pointer()
        .when(active, |d| d.bg(style::accent()))
        .when(!active, |d| d.hover(|d| d.bg(style::hover())))
}

/// The first row of the sidebar card (the traffic lights and the app's name)
/// and the center card's project row: as tall as each other, so they line up.
pub const BAR: f32 = 44.;

/// The room the macOS traffic lights take at the start of the sidebar card's
/// first row: they end 76 px from the window's edge (see main.rs).
pub const TRAFFIC_LIGHTS: f32 = 86.;

/// Where the drawers (new task, task detail) sit, inside the window's padding:
/// as tall as the cards, right-aligned.
pub fn drawer_frame(d: Div, width: f32) -> Div {
    let shadow = vec![
        BoxShadow { color: gpui::black().opacity(0.45), offset: point(px(-18.), px(0.)), blur_radius: px(50.), spread_radius: px(0.), inset: false },
        BoxShadow { color: gpui::black().opacity(0.4), offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(1.), inset: false },
    ];
    d.absolute()
        .top(px(style::GAP))
        .right(px(style::GAP))
        .bottom(px(style::GAP))
        .w(px(width))
        .max_w(relative(0.9))
        .rounded(px(style::R_BLOCK))
        .bg(style::popover())
        .border_1()
        .border_color(gpui::Hsla::from(rgba(0xffffff17)))
        .shadow(shadow)
        .overflow_hidden()
        .occlude()
}

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

/// A tile that breathes a red ring while its member waits for you: slow
/// enough to notice without nagging.
fn waiting_ring(tile: Div, id: String) -> AnyElement {
    tile.with_animation(SharedString::from(format!("ring-{id}")), Animation::new(Duration::from_millis(1800)).repeat(), |d, t| {
        let k = (t / 0.7).min(1.);
        let ease = 1. - (1. - k).powi(3);
        d.shadow(vec![BoxShadow { color: style::red().opacity(0.55 * (1. - k)), offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(5. * ease), inset: false }])
    })
    .into_any_element()
}

impl LilaApp {
    /// One member row: the CLI tile, the id, then a second line that leads
    /// with the state in its color and says what the member is on.
    fn member_row(&self, m: &Value, branch: Option<crate::tree::Branch>, shown: Option<&str>, cx: &mut Context<Self>) -> AnyElement {
        let id = s(m, "id");
        let status = s(m, "status");
        let doing = Some(s(m, "msg")).filter(|x| !x.is_empty()).or_else(|| m["task"]["title"].as_str().map(String::from)).unwrap_or_else(|| s(m, "role"));
        let queued = m["queued"].as_u64().unwrap_or(0);
        let stale = m["stale"] == true;
        let click = id.clone();
        let tile = style::cli_tile(&s(m, "runtime"), 22.);
        let tile = if status == "blocked" { waiting_ring(tile, id.clone()) } else { tile.into_any_element() };
        let in_tree = branch.is_some();
        h_flex()
            .id(SharedString::from(format!("member-{id}")))
            .relative()
            .when(in_tree, |d| d.mr_2())
            .when(!in_tree, |d| d.mx_2())
            .px_2()
            .h(px(style::MEMBER_ROW))
            .mb(px(style::MEMBER_GAP))
            .gap(px(10.))
            .rounded(px(style::R_CTL))
            .cursor_pointer()
            .when(shown == Some(id.as_str()), |d| d.bg(style::accent()))
            .when(shown != Some(id.as_str()), |d| d.hover(|d| d.bg(style::hover())))
            .children(branch.map(|b| crate::tree::canvas_for(b).into_any_element()))
            .child(tile)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(1.))
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .child(div().text_size(px(13.)).font_medium().child(id.clone()))
                            .when(stale, |d| d.child(dot(style::yellow())))
                            .when(queued > 0, |d| d.child(div().text_xs().text_color(muted2()).child(format!("+{queued}")))),
                    )
                    .child(div().text_xs().truncate().min_w_0().text_color(muted2()).child(doing)),
            )
            .child(status_mark(&status, cx))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.member = Some(click.clone());
                cx.notify();
            }))
            .into_any_element()
    }

    pub fn sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = self.shown_member().map(|m| s(&m, "id"));
        let selected = self.selected.clone();
        let projects = self.snap.projects.clone();
        let members = self.members();
        // Two levels: the PM at the root, everyone it hands work to hanging off it.
        let pm = members.iter().find(|m| m["owner"] == true).cloned();
        let crew: Vec<Value> = members.iter().filter(|m| m["owner"] != true).cloned().collect();
        let branches = crate::tree::branches(&crew.iter().map(|m| s(m, "status")).collect::<Vec<_>>());
        let pm_row = pm.as_ref().map(|m| self.member_row(m, None, shown.as_deref(), cx));
        let crew_rows: Vec<AnyElement> = crew.iter().zip(branches).map(|(m, b)| self.member_row(m, Some(b), shown.as_deref(), cx)).collect();

        let body = card()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            // The first row: the traffic lights, the app's name, Lila's settings. It drags the window.
            .child(
                drag_bar(h_flex().id("sidebar-top"), cx)
                    .h(px(BAR))
                    .flex_none()
                    .pl(px(TRAFFIC_LIGHTS))
                    .pr(px(10.))
                    .child(div().text_size(px(13.)).font_semibold().child("Lila"))
                    .child(div().flex_1())
                    .child(Button::new("prefs").ghost().xsmall().icon(IconName::Settings).tooltip("设置 (⌘,)").on_click(cx.listener(|this, _, window, cx| this.open_prefs(crate::prefs::PrefPage::Roles, window, cx)))),
            )
            .child(
                h_flex()
                    .pl_4()
                    .pr(px(10.))
                    .py(px(6.))
                    .child(label("项目"))
                    .child(div().flex_1())
                    .child(Button::new("new-project").ghost().xsmall().icon(IconName::Plus).tooltip("新建项目").on_click(cx.listener(|this, _, window, cx| this.new_project(window, cx)))),
            )
            .children(projects.into_iter().map(|p| {
                let active = selected.as_deref() == Some(&p.id);
                let (status, inbox) = self.project_summary(&p.id);
                let id = p.id.clone();
                row(SharedString::from(format!("project-{}", p.id)), active)
                    .child(avatar(&p.id, 20., cx))
                    .child(div().text_size(px(13.)).truncate().min_w_0().child(p.label()))
                    .child(div().flex_1())
                    .when(inbox > 0, |d| d.child(style::pill(style::red(), inbox.to_string())))
                    .when(inbox == 0, |d| d.when_some(status, |d, st| d.child(status_mark(st, cx))))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_project(id.clone(), cx)))
                    .context_menu(self.project_menu(&p, cx))
            }))
            .child(
                v_flex()
                    .id("member-tree")
                    .flex_1()
                    .min_h_0()
                    .when(pm_row.is_some(), |d| d.mt(px(14.)).pt(px(10.)).border_t_1().border_color(style::divider()))
                    .overflow_y_scrollbar()
                    .pb_2()
                    .children(pm_row)
                    .when(!crew_rows.is_empty(), |d| d.child(v_flex().ml(px(27.)).pl(px(14.)).children(crew_rows))),
            );

        v_flex().size_full().text_color(style::fg()).child(body)
    }

    /// The right-click menu of a project (sidebar row or tab).
    fn project_menu(&self, p: &data::Project, cx: &mut Context<Self>) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let weak = cx.entity().downgrade();
        let (id, cwd) = (p.id.clone(), p.cwd.clone());
        let daemon = self.snap.daemon;
        let paused = self.paused_of(&p.id);
        move |menu, _, _| {
            let on = |f: fn(&mut LilaApp, &mut Window, &mut Context<LilaApp>)| {
                let (weak, id) = (weak.clone(), id.clone());
                move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                    let _ = weak.update(cx, |this, cx| {
                        this.select_project(id.clone(), cx);
                        f(this, window, cx);
                    });
                }
            };
            let cwd = cwd.clone();
            menu.item(PopupMenuItem::new("新建任务…").icon(IconName::Plus).on_click(on(|this, window, cx| this.compose(None, None, window, cx))))
                .item(PopupMenuItem::new("项目设置…").icon(IconName::Settings2).on_click(on(|this, window, cx| this.open_settings(window, cx))))
                .when(daemon, |m| {
                    m.item(PopupMenuItem::new(if paused { "恢复派发" } else { "暂停派发" }).icon(if paused { IconName::Play } else { IconName::Pause }).on_click(on(if paused {
                        |this, _, cx| this.act("resume", json!({}), false, cx)
                    } else {
                        |this, _, cx| this.act("pause", json!({}), false, cx)
                    })))
                })
                .separator()
                .item(PopupMenuItem::new("在访达中显示").icon(IconName::FolderOpen).on_click(move |_, _, _| {
                    let _ = std::process::Command::new("open").arg(&cwd).spawn();
                }))
                .separator()
                .item(PopupMenuItem::new("从列表移除…").icon(style::icon("lila/trash.svg")).on_click({
                    let (weak, id) = (weak.clone(), id.clone());
                    move |_, _, cx| {
                        let _ = weak.update(cx, |this, cx| {
                            this.removing = this.snap.projects.iter().find(|p| p.id == id).cloned();
                            cx.notify();
                        });
                    }
                }))
        }
    }

    /// Asks before removing a project from the list: what goes, what stays.
    pub fn remove_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.removing.clone()?;
        let running = self.snap.daemon && self.snap.project.as_deref() == Some(p.id.as_str()) && self.members().iter().any(|m| m["terminal"].is_string());
        let shadows = vec![
            BoxShadow { color: gpui::black().opacity(0.55), offset: point(px(0.), px(24.)), blur_radius: px(64.), spread_radius: px(0.), inset: false },
            BoxShadow { color: gpui::black().opacity(0.35), offset: point(px(0.), px(2.)), blur_radius: px(8.), spread_radius: px(0.), inset: false },
        ];
        let text = format!(
            "{}Lila 会删掉它的任务、动态和收件箱。项目目录和里面的 .lila/ 不动，以后在这个文件夹新建项目可以重新加回来，只是历史没了。",
            if running { "团队会先停止。" } else { "" }
        );
        Some(
            div()
                .id("remove-backdrop")
                .absolute()
                .inset_0()
                .bg(gpui::black().opacity(0.42))
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.removing = None;
                    cx.notify();
                }))
                .child(
                    v_flex()
                        .id("remove-dialog")
                        .w(px(440.))
                        .px(px(22.))
                        .pt(px(20.))
                        .pb(px(16.))
                        .gap(px(10.))
                        .rounded(px(14.))
                        .bg(style::popover())
                        .border_1()
                        .border_color(style::hairline())
                        .shadow(shadows)
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(div().text_size(px(15.)).font_semibold().child(format!("从列表移除「{}」？", p.label())))
                        .child(div().text_size(px(13.)).text_color(muted()).line_height(relative(1.6)).child(text))
                        .child(div().text_xs().text_color(muted2()).child(data::tilde(&p.cwd)))
                        .child(
                            h_flex()
                                .mt(px(6.))
                                .gap_2()
                                .justify_end()
                                .child(Button::new("remove-no").ghost().label("取消").on_click(cx.listener(|this, _, _, cx| {
                                    this.removing = None;
                                    cx.notify();
                                })))
                                .child(Button::new("remove-yes").outline().text_color(style::red()).label("移除").on_click(cx.listener(move |this, _, _, cx| this.remove_project(p.clone(), cx)))),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The center card's first row: the project, the member shown and how it is
    /// doing, then its tools, the project's settings and the overview switch.
    /// Drags the window.
    fn header(&mut self, member: Option<&Value>, terminal: Option<Entity<lila_term::TerminalView>>, cx: &mut Context<Self>) -> impl IntoElement {
        let (open, inbox) = (self.right_open, self.snap.inbox.len());
        let project = self.project();
        let paused = self.snap.status.as_ref().is_some_and(|s| s["paused"] == true);
        let tool = |id: &'static str, icon: IconName, tip: &'static str| Button::new(id).ghost().xsmall().icon(icon).tooltip(tip);
        drag_bar(h_flex().id("project-bar"), cx)
            .h(px(BAR))
            .flex_none()
            .pl(px(14.))
            .pr_2()
            .gap_2()
            .when(project.is_some(), |d| d.border_b_1().border_color(style::divider()))
            .whitespace_nowrap()
            .when_some(project, |d, p| {
                d.child(avatar(&p.id, 20., cx))
                    .child(div().flex_none().max_w(px(220.)).truncate().text_size(px(15.)).font_semibold().child(p.label()))
                    .when(paused, |d| d.child(style::tag(style::yellow(), "已暂停")))
            })
            .when_some(member, |d, m| {
                let id = s(m, "id");
                let spec = [s(m, "role"), s(m, "model"), s(m, "effort")].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
                d.child(Icon::new(IconName::ChevronRight).size(px(14.)).text_color(muted2()))
                    .child(
                        h_flex()
                            .id("member-name")
                            .gap_2()
                            .min_w_0()
                            .child(style::cli_tile(&s(m, "runtime"), 20.))
                            .child(div().text_size(px(13.)).font_medium().child(id.clone()))
                            .tooltip(move |window, cx| gpui::component::tooltip::Tooltip::new(spec.clone()).build(window, cx)),
                    )
                    .child(style::status_pill(&s(m, "status"), cx))
                    .when(m["stale"] == true, |d| {
                        let id = id.clone();
                        d.child(Button::new("restart-member").small().small_button().outline().rounded_full().text_color(style::yellow()).icon(IconName::RefreshCw).label("重启").tooltip("设置改过了：用新设置重启，当前对话会丢失").on_click(cx.listener(move |this, _, _, cx| {
                            this.act("member.restart", json!({"member": id}), false, cx)
                        })))
                    })
            })
            .child(div().flex_1())
            .when_some(member, |d, m| {
                let id = s(m, "id");
                d.when_some(m["context"].as_u64().zip(m["context_limit"].as_u64()), |d, (n, limit)| d.child(context_chip(n, limit)))
                    .when(m["owner"] == true && m["context"].is_u64(), |d| {
                        if self.confirm_refresh {
                            d.child(Button::new("refresh-pm-yes").small().small_button().outline().label("换新会话").tooltip("终端历史会清空，PM 收到项目摘要").on_click(cx.listener(move |this, _, _, cx| {
                                this.confirm_refresh = false;
                                this.act("member.restart", json!({"member": id}), false, cx)
                            })))
                            .child(Button::new("refresh-pm-no").small().small_button().ghost().label("取消").on_click(cx.listener(|this, _, _, cx| {
                                this.confirm_refresh = false;
                                cx.notify();
                            })))
                        } else {
                            d.child(tool("refresh-pm", IconName::RefreshCw, "整理上下文").on_click(cx.listener(|this, _, _, cx| {
                                this.confirm_refresh = true;
                                cx.notify();
                            })))
                        }
                    })
            })
            .when_some(terminal, |d, view| {
                d.child(tool("terminal-find", IconName::Search, "搜索 (⌘F)").on_click(move |_, window, cx| {
                    view.update(cx, |view, cx| view.open_search(window, cx));
                }))
            })
            .when(self.project().is_some(), |d| d.child(tool("bar-settings", IconName::Settings2, "项目设置 (⇧⌘,)").on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx)))))
            .when(self.project().is_some(), |d| d.child(
                div().relative().child(
                    Button::new("bar-panel")
                        .ghost()
                        .size(px(30.))
                        .icon(Icon::new(IconName::PanelRight).text_color(if open { style::fg() } else { muted() }))
                        .when(open, |b| b.bg(gpui::Hsla::from(rgba(0xffffff14))))
                        .tooltip("概览 (⌘I)")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.right_open = !this.right_open;
                            cx.notify();
                        })),
                )
                .when(!open && inbox > 0, |d| d.child(style::pill(style::red(), inbox.to_string()).absolute().top(px(-3.)).right(px(-5.)).h(px(15.)).min_w(px(15.)).text_size(px(10.)))),
            ))
    }

    pub fn center(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let member = self.shown_member();
        let address = member.as_ref().map(|m| s(m, "terminal")).filter(|a| !a.is_empty());
        let mut terminal_view = None;
        let empty = || v_flex().flex_1().items_center().justify_center();
        let body: AnyElement = match (address, &self.core_error) {
            (_, Some(e)) => placeholder(e.clone(), cx).into_any_element(),
            (Some(addr), None) => match self.terminal(&addr, window, cx) {
                Some(view) => {
                    terminal_view = Some(view.clone());
                    div().flex_1().min_h_0().px_1().child(view).into_any_element()
                }
                None => empty().child(Spinner::new().color(muted())).into_any_element(),
            },
            (None, None) if self.snap.projects.is_empty() => empty().child(Button::new("new-project-big").primary().icon(IconName::Plus).label("新建项目").on_click(cx.listener(|this, _, window, cx| this.new_project(window, cx)))).into_any_element(),
            (None, None) if self.project().is_none() => empty().into_any_element(),
            (None, None) => empty().child(Button::new("start-big").primary().icon(IconName::Play).label("启动团队").on_click(cx.listener(|this, _, _, cx| this.act("project.up", json!({}), true, cx)))).into_any_element(),
        };
        let header = self.header(member.as_ref(), terminal_view, cx).into_any_element();
        v_flex().size_full().pr(px(4.)).child(
            card()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(header)
                .when_some(self.notice.clone(), |d, n| {
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
                            .child(Button::new("notice-close").ghost().xsmall().icon(IconName::Close).on_click(cx.listener(|this, _, _, cx| {
                                this.notice = None;
                                cx.notify();
                            }))),
                    )
                })
                .child(body),
        )
    }

    pub fn right(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for i in &self.snap.inbox {
            let placeholder = match i["kind"].as_str() {
                Some("question") => "回答…",
                Some("review") => "返工意见…",
                _ => continue,
            };
            let id = s(i, "id");
            self.answers.entry(id).or_insert_with(|| cx.new(|cx| gpui::component::input::InputState::new(window, cx).placeholder(placeholder)));
        }
        let inbox = self.snap.inbox.len();
        let open_tasks = self.snap.tasks.iter().filter(|t| !matches!(t["state"].as_str(), Some("done" | "cancelled"))).count();
        let inbox_el = self.inbox_list(cx).into_any_element();
        let tasks_el = self.task_board(cx).into_any_element();
        let has_project = self.project().is_some();
        v_flex()
            .size_full()
            .pl(px(3.))
            .gap(px(style::GAP))
            .when_some(self.snap.error.clone(), |d, n| d.child(div().p_2().rounded(px(10.)).bg(style::yellow().opacity(0.15)).text_xs().child(n)))
            .when(inbox > 0, |d| d.child(card().flex_none().max_h(relative(0.45)).flex().flex_col().child(card_head("需要你", inbox)).child(inbox_el)))
            .child(
                card()
                    .flex_1()
                    .min_h(px(170.))
                    .flex()
                    .flex_col()
                    .child(card_head("任务", open_tasks).when(has_project, |d| d.child(style::link("task-new", "新建", true).text_size(px(13.)).on_click(cx.listener(|this, _, window, cx| this.compose(None, None, window, cx))))))
                    .child(tasks_el),
            )
    }

    fn inbox_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.snap.inbox.clone();
        let cards: Vec<AnyElement> = items.into_iter().enumerate().map(|(n, item)| self.inbox_card(item, n > 0, cx).into_any_element()).collect();
        v_flex().id("inbox-list").min_h_0().overflow_y_scrollbar().children(cards)
    }

    fn task_board(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let tasks = self.snap.tasks.clone();
        // Open flow runs by their first task: where each is now.
        let runs: Vec<Value> = self.snap.status.as_ref().and_then(|v| v["runs"].as_array().cloned()).unwrap_or_default();
        let flow_now = |t: &Value| -> Option<String> {
            let anchor = t["run"].as_str()?;
            let r = runs.iter().find(|r| r["anchor"] == anchor)?;
            let round = r["round"].as_u64().unwrap_or(0);
            Some(format!("{}{}", crate::detail::step_label(r["step"].as_str()?), if round > 0 { format!(" · 返工 {round} 次") } else { String::new() }))
        };
        let group = |title: &'static str, states: &[&str], limit: usize| {
            let rows: Vec<Value> = tasks.iter().rev().filter(|t| states.contains(&t["state"].as_str().unwrap_or(""))).take(limit).cloned().collect();
            (title, rows)
        };
        let groups = [group("进行中", &["delivered", "in_progress"], 30), group("阻塞", &["blocked"], 30), group("排队", &["queued"], 30), group("最近完成", &["done"], 10)];
        let sublabel = |t: String| div().text_size(px(11.)).font_semibold().text_color(muted2()).child(t);
        v_flex()
            .id("task-board")
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .px_2()
            .pb(px(10.))
            .gap_3()
            .children(groups.into_iter().filter(|(_, rows)| !rows.is_empty()).map(|(title, rows)| {
                v_flex()
                    .gap(px(2.))
                    .child(div().px_2().pt(px(2.)).pb_1().child(sublabel(title.into())))
                    .children(rows.into_iter().map(|t| {
                        let state = s(&t, "state");
                        let id = s(&t, "id");
                        let open = id.clone();
                        let active = self.task.as_deref() == Some(id.as_str());
                        let step = if state == "done" { None } else { flow_now(&t) };
                        let place = step.unwrap_or_else(|| s(&t, "to"));
                        h_flex()
                            .id(SharedString::from(format!("board-{id}")))
                            .h(px(32.))
                            .px_2()
                            .gap(px(10.))
                            .rounded(px(style::R_CTL))
                            .cursor_pointer()
                            .when(active, |d| d.bg(style::accent()))
                            .when(!active, |d| d.hover(|d| d.bg(style::hover())))
                            .on_click(cx.listener(move |this, _, window, cx| this.open_task(open.clone(), window, cx)))
                            .child(crate::detail::state_tag(&state, id.clone()))
                            .child(div().flex_1().min_w_0().text_size(px(13.)).truncate().child(s(&t, "title")))
                            .child(div().flex_none().max_w(px(120.)).truncate().text_xs().text_color(muted2()).child(place))
                    }))
            }))
    }

    fn inbox_card(&mut self, item: Value, divided: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let id = s(&item, "id");
        let kind = s(&item, "kind");
        let member = item["member"].as_str().map(String::from);
        let (icon, accent, label_text) = match kind.as_str() {
            "question" => (IconName::CircleAlert, style::yellow(), "PM 在问你"),
            "terminal" => (IconName::SquareTerminal, style::red(), "终端在等你"),
            "startup" => (IconName::TriangleAlert, style::red(), "启动卡住"),
            "stalled" => (IconName::TriangleAlert, style::yellow(), "没有回应"),
            "lost" => (IconName::CircleX, style::red(), "成员退出"),
            "report" => (IconName::CircleCheck, style::green(), "完成报告"),
            "review" => (IconName::CircleCheck, style::green(), "等你验收"),
            "budget" => (IconName::TriangleAlert, style::yellow(), "超出预算"),
            "quota" => (IconName::CircleX, style::red(), "额度用完"),
            "runaway" => (IconName::TriangleAlert, style::yellow(), "可能在打转"),
            _ => (IconName::Info, muted(), "提醒"),
        };
        // A review signs off with 通过 or sends back what the person wrote; it has its own row.
        let review = kind == "review";
        let options: Vec<String> = item["options"].as_array().into_iter().flatten().filter_map(|o| o.as_str().map(String::from)).collect();
        let input = self.answers.get(&id).cloned();
        let card_id = id.clone();
        v_flex()
            .px_4()
            .pt(px(if divided { 14. } else { 4. }))
            .pb(px(14.))
            .gap(px(9.))
            .when(divided, |d| d.border_t_1().border_color(style::divider()))
            .child(
                h_flex()
                    .gap(px(7.))
                    .child(Icon::new(icon).small().text_color(accent))
                    .child(div().text_size(px(13.)).font_semibold().child(label_text))
                    .child(div().flex_1())
                    .when_some(item["task"].as_str().map(String::from), |d, t| d.child(div().text_xs().child(crate::detail::task_link(&t, &format!("inbox-{card_id}"), cx)))),
            )
            .child(div().text_size(px(13.)).line_height(relative(1.55)).child(s(&item, "text")))
            .when(!options.is_empty() && !review, |d| {
                d.child(h_flex().gap(px(6.)).flex_wrap().children(options.into_iter().enumerate().map(|(n, opt)| {
                    let (id, text) = (id.clone(), opt.clone());
                    Button::new(SharedString::from(format!("opt-{id}-{n}"))).small().small_button().outline().rounded_full().label(opt).on_click(cx.listener(move |this, _, window, cx| this.answer(id.clone(), Some(text.clone()), window, cx)))
                })))
            })
            .when_some(input, |d, input| {
                let (id, field) = (card_id.clone(), input.clone());
                let accept_id = card_id.clone();
                d.child(
                    h_flex()
                        .gap(px(6.))
                        .when(review, |d| {
                            d.child(Button::new(SharedString::from(format!("accept-{card_id}"))).primary().label("通过").on_click(cx.listener(move |this, _, window, cx| {
                                this.answer(accept_id.clone(), Some("通过".into()), window, cx)
                            })))
                        })
                        .child(div().flex_1().child(Input::new(&input).control()))
                        .child({
                            let send = Button::new(SharedString::from(format!("send-{card_id}"))).label(if review { "打回" } else { "回答" });
                            let send = if review { send.ghost() } else { send.primary() };
                            send.on_click(cx.listener(move |this, _, window, cx| {
                                let text = field.read(cx).value().to_string();
                                if !text.trim().is_empty() {
                                    this.answer(id.clone(), Some(text), window, cx);
                                }
                            }))
                        }),
                )
            })
            .when(!matches!(kind.as_str(), "question" | "review" | "budget"), |d| {
                let open = member.clone().filter(|_| matches!(kind.as_str(), "terminal" | "startup" | "stalled" | "runaway"));
                let id = card_id.clone();
                d.child(
                    h_flex()
                        .gap(px(6.))
                        .when_some(open, |d, m| {
                            d.child(Button::new(SharedString::from(format!("open-{card_id}"))).small().small_button().outline().rounded_full().icon(IconName::SquareTerminal).label("打开终端").on_click(cx.listener(move |this, _, _, cx| {
                                this.member = Some(m.clone());
                                cx.notify();
                            })))
                        })
                        .child(div().flex_1())
                        .child(style::link(SharedString::from(format!("dismiss-{card_id}")), "知道了", false).text_size(px(13.)).on_click(cx.listener(move |this, _, window, cx| this.answer(id.clone(), None, window, cx)))),
                )
            })
    }
}

pub fn placeholder(text: String, _cx: &App) -> impl IntoElement {
    div().flex_1().flex().items_center().justify_center().p_8().text_color(muted()).child(text)
}

/// How full a member's context is: a short bar and the size, against where Lila
/// starts it afresh (the bar's end). Yellow from 80%, orange past it.
fn context_chip(n: u64, limit: u64) -> impl IntoElement {
    let k = |t: u64| if t >= 1_000_000 { format!("{:.1}M", t as f64 / 1e6) } else { format!("{}k", (t + 500) / 1000) };
    let frac = if limit == 0 { 0.0 } else { (n as f32 / limit as f32).min(1.0) };
    let color = if limit > 0 && n >= limit {
        style::orange()
    } else if limit > 0 && n * 5 >= limit * 4 {
        style::yellow()
    } else {
        muted()
    };
    let tip = if limit == 0 { format!("上下文 {}", k(n)) } else { format!("上下文 {} / {}", k(n), k(limit)) };
    h_flex()
        .id("context-chip")
        .flex_none()
        .gap(px(5.))
        .text_xs()
        .text_color(color)
        .child(div().w(px(34.)).h(px(4.)).rounded(px(2.)).bg(gpui::Hsla::from(rgba(0xffffff1a))).child(div().h_full().w(relative(frac)).rounded(px(2.)).bg(color)))
        .child(k(n))
        .tooltip(move |window, cx| gpui::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
}
