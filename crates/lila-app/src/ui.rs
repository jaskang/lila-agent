//! How the window looks (see `prototype/lila-window.html`): a graphite glass
//! window; the sidebar sits straight on the glass from top to bottom (every
//! project, then the team as a tree under the PM); beside it the tabs of the
//! open projects, then a terminal card and the overview cards (what needs
//! you, the task board, usage, activity), then a status bar card.

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
        .gap_2()
        .rounded(px(style::R_CTL))
        .cursor_pointer()
        .when(active, |d| d.bg(style::accent()))
        .when(!active, |d| d.hover(|d| d.bg(style::hover())))
}

/// The window's bars: the tab row (as tall as the sidebar's top row, where
/// the traffic lights sit; its last 8 px are the gutter to the cards).
pub const TAB_BAR: f32 = 44.;

/// The room the macOS traffic lights take at the start of the sidebar's top row:
/// they end at 72 px from the window's edge (see main.rs).
pub const TRAFFIC_LIGHTS: f32 = 84.;

/// Where the drawers (new task, task detail) sit, inside the window's padding:
/// from under the tab row to above the status bar, right-aligned.
pub fn drawer_frame(d: Div, width: f32) -> Div {
    let shadow = vec![
        BoxShadow { color: gpui::black().opacity(0.45), offset: point(px(-18.), px(0.)), blur_radius: px(50.), spread_radius: px(0.), inset: false },
        BoxShadow { color: gpui::black().opacity(0.4), offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(1.), inset: false },
    ];
    d.absolute()
        .top(px(style::GAP + TAB_BAR))
        .right(px(style::GAP))
        .bottom(px(style::GAP + style::STATUS + style::GAP))
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
        let color = status_color(&status, cx);
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
                            .when(stale, |d| d.child(style::tag(style::yellow(), "待重启")))
                            .when(queued > 0, |d| d.child(div().text_xs().text_color(muted2()).child(format!("+{queued}")))),
                    )
                    .child(h_flex().text_xs().min_w_0().child(div().flex_none().text_color(color).child(status_label(&status))).child(div().truncate().min_w_0().text_color(muted2()).child(format!(" · {doing}")))),
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
        let count = members.len();
        let paused = self.snap.status.as_ref().is_some_and(|s| s["paused"] == true);
        let can_start = self.project().is_some() && (members.iter().any(|m| m["status"] == "gone") || !self.snap.daemon);
        // Two levels: the PM at the root, everyone it hands work to hanging off it.
        let pm = members.iter().find(|m| m["owner"] == true).cloned();
        let crew: Vec<Value> = members.iter().filter(|m| m["owner"] != true).cloned().collect();
        let branches = crate::tree::branches(&crew.iter().map(|m| s(m, "status")).collect::<Vec<_>>());
        let pm_row = pm.as_ref().map(|m| self.member_row(m, None, shown.as_deref(), cx));
        let crew_rows: Vec<AnyElement> = crew.iter().zip(branches).map(|(m, b)| self.member_row(m, Some(b), shown.as_deref(), cx)).collect();

        v_flex()
            .size_full()
            .text_color(style::fg())
            .child(drag_bar(h_flex().id("sidebar-top"), cx).h(px(TAB_BAR - style::GAP)).mb(px(style::GAP)).flex_none().pl(px(TRAFFIC_LIGHTS)))
            .child(
                h_flex()
                    .pl_4()
                    .pr(px(10.))
                    .py(px(6.))
                    .gap(px(5.))
                    .child(label("项目"))
                    .child(div().flex_1())
                    .child(Button::new("new-project").ghost().xsmall().icon(IconName::Plus).tooltip("新建项目…").on_click(cx.listener(|this, _, window, cx| this.new_project(window, cx)))),
            )
            .children(projects.into_iter().map(|p| {
                let active = selected.as_deref() == Some(&p.id);
                let (status, inbox) = self.project_summary(&p.id);
                let id = p.id.clone();
                row(SharedString::from(format!("project-{}", p.id)), active)
                    .child(avatar(&p.id, 20., cx))
                    .child(div().text_size(px(13.)).truncate().min_w_0().child(p.label()))
                    .child(div().flex_1())
                    .when_some(status, |d, st| d.child(status_mark(st, cx)))
                    .when(inbox > 0, |d| d.child(style::pill(style::red(), inbox.to_string())))
                    .when(p.missing, |d| d.child(style::tag(style::yellow(), "缺少配置")).tooltip({
                        let path = data::tilde(&format!("{}/.lila/project.toml", p.cwd));
                        move |window, cx| gpui::component::tooltip::Tooltip::new(format!("找不到 {path}。打开这个项目时会用默认团队重新创建。")).build(window, cx)
                    }))
                    .when_some(p.branch.clone().filter(|_| !p.missing && inbox == 0 && status.is_none()), |d, b| d.child(div().text_xs().text_color(muted2()).truncate().max_w(px(110.)).child(b)))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_project(id.clone(), cx)))
                    .context_menu(self.project_menu(&p, cx))
            }))
            .when(count > 0, |d| d.child(h_flex().mt(px(14.)).pl_4().pr(px(10.)).py(px(6.)).gap(px(5.)).child(label("成员")).child(div().text_xs().text_color(muted2()).child(count.to_string()))))
            .child(
                v_flex()
                    .id("member-tree")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .pb_2()
                    .children(pm_row)
                    .when(!crew_rows.is_empty(), |d| d.child(v_flex().ml(px(27.)).pl(px(14.)).children(crew_rows))),
            )
            .child(
                v_flex()
                    .pt(px(6.))
                    .pb(px(4.))
                    .border_t_1()
                    .border_color(style::divider())
                    .when(can_start, |d| {
                        d.child(row("up", false).child(Icon::new(IconName::Play).small().text_color(style::green())).child(div().text_size(px(13.)).child("启动团队")).on_click(cx.listener(|this, _, _, cx| this.act("project.up", json!({}), true, cx))))
                    })
                    .when(self.snap.daemon && self.project().is_some(), |d| {
                        d.child(
                            row("pause", false)
                                .child(Icon::new(if paused { IconName::Play } else { IconName::Pause }).small().text_color(muted()))
                                .child(div().text_size(px(13.)).child(if paused { "恢复派发" } else { "暂停派发" }))
                                .when(paused, |d| d.child(div().flex_1()).child(style::tag(style::yellow(), "已暂停")))
                                .on_click(cx.listener(move |this, _, _, cx| this.act(if paused { "resume" } else { "pause" }, json!({}), false, cx))),
                        )
                    })
                    .child(
                        row("prefs", false)
                            .child(Icon::new(IconName::Settings).small().text_color(muted()))
                            .child(div().text_size(px(13.)).child("设置"))
                            .child(div().flex_1())
                            .child(div().text_xs().text_color(muted2()).child("⌘,"))
                            .on_click(cx.listener(|this, _, window, cx| this.open_prefs(crate::prefs::PrefPage::Roles, window, cx))),
                    ),
            )
    }

    /// The right-click menu of a project (sidebar row or tab).
    fn project_menu(&self, p: &data::Project, cx: &mut Context<Self>) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let weak = cx.entity().downgrade();
        let (id, cwd, open) = (p.id.clone(), p.cwd.clone(), self.tabs.contains(&p.id));
        move |menu, _, _| {
            let on = |f: fn(&mut LilaApp, &mut Window, &mut Context<LilaApp>)| {
                let (weak, id) = (weak.clone(), id.clone());
                move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                    let _ = weak.update(cx, |this, cx| {
                        this.open_project(id.clone(), cx);
                        f(this, window, cx);
                    });
                }
            };
            let cwd = cwd.clone();
            let close = {
                let (weak, id) = (weak.clone(), id.clone());
                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                    let _ = weak.update(cx, |this, cx| this.close_tab(&id, cx));
                }
            };
            menu.item(PopupMenuItem::new("新建任务…").icon(IconName::Plus).on_click(on(|this, window, cx| this.compose(None, None, window, cx))))
                .item(PopupMenuItem::new("项目设置…").icon(IconName::Settings2).on_click(on(|this, window, cx| this.open_settings(window, cx))))
                .separator()
                .item(PopupMenuItem::new("在访达中显示").icon(IconName::FolderOpen).on_click(move |_, _, _| {
                    let _ = std::process::Command::new("open").arg(&cwd).spawn();
                }))
                .when(open, |m| m.item(PopupMenuItem::new("关闭标签").icon(IconName::Close).on_click(close)))
        }
    }

    /// The row over the cards: one tab per open project, new project, then the overview switch.
    pub fn tab_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.project().map(|p| p.id);
        let projects: Vec<data::Project> = self.tabs.iter().filter_map(|id| self.snap.projects.iter().find(|p| &p.id == id).cloned()).collect();
        let (open, inbox) = (self.right_open, self.snap.inbox.len());
        drag_bar(h_flex().id("tab-bar"), cx)
            .h(px(TAB_BAR))
            .flex_none()
            .pr_1()
            .pb(px(style::GAP))
            .gap(px(2.))
            .children(projects.into_iter().enumerate().map(|(n, p)| {
                let active = selected.as_deref() == Some(&p.id);
                let (status, inbox) = self.project_summary(&p.id);
                let click = p.id.clone();
                let group = SharedString::from(format!("tab-{}", p.id));
                h_flex()
                    .id(group.clone())
                    .group(group.clone())
                    .h(px(28.))
                    .pl_2()
                    .pr(px(6.))
                    .gap(px(7.))
                    .rounded(px(style::R_CTL))
                    .border_1()
                    .cursor_pointer()
                    .when(active, |d| d.bg(gpui::Hsla::from(rgba(0xffffff17))).border_color(style::hairline()).text_color(style::fg()).shadow_xs())
                    .when(!active, |d| d.border_color(gpui::transparent_black()).text_color(muted()).hover(|d| d.bg(style::hover()).text_color(style::fg())))
                    .child(avatar(&p.id, 16., cx))
                    .child(div().text_size(px(13.)).truncate().max_w(px(180.)).child(p.label()))
                    .map(|d| match (inbox, status) {
                        (n, _) if n > 0 => d.child(style::pill(style::red(), n.to_string())),
                        (_, Some(st)) => d.child(status_mark(st, cx)),
                        _ if n < 9 => d.child(div().text_size(px(11.)).text_color(muted2()).child(format!("⌘{}", n + 1))),
                        _ => d,
                    })
                    .child({
                        let close = p.id.clone();
                        div().when(!active, |d| d.opacity(0.).group_hover(group.clone(), |d| d.opacity(1.))).child(
                            Button::new(SharedString::from(format!("tab-close-{}", p.id))).ghost().xsmall().icon(IconName::Close).tooltip("关闭标签 (⌘W)，团队继续运行").on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_tab(&close, cx);
                            })),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.select_project(click.clone(), cx)))
                    .context_menu(self.project_menu(&p, cx))
            }))
            .child(Button::new("tab-new-project").ghost().xsmall().icon(IconName::Plus).tooltip("新建项目…").on_click(cx.listener(|this, _, window, cx| this.new_project(window, cx))))
            .child(div().flex_1())
            .child(
                div().relative().child(
                    Button::new("bar-panel")
                        .ghost()
                        .size(px(30.))
                        .icon(Icon::new(IconName::PanelRight).text_color(if open { style::fg() } else { muted() }))
                        .when(open, |b| b.bg(gpui::Hsla::from(rgba(0xffffff14))))
                        .tooltip(if open { "收起概览 (⌘I)" } else { "显示收件箱、任务和动态 (⌘I)" })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.right_open = !this.right_open;
                            cx.notify();
                        })),
                )
                .when(!open && inbox > 0, |d| d.child(style::pill(style::red(), inbox.to_string()).absolute().top(px(-3.)).right(px(-5.)).h(px(15.)).min_w(px(15.)).text_size(px(10.)))),
            )
    }

    pub fn center(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let member = self.shown_member();
        let address = member.as_ref().map(|m| s(m, "terminal")).filter(|a| !a.is_empty());
        let mut terminal_view = None;
        let icon_tile = |icon: IconName| div().size(px(48.)).rounded(px(12.)).flex().items_center().justify_center().bg(style::fill()).border_1().border_color(style::hairline()).child(Icon::new(icon).large().text_color(muted()));
        let empty = || v_flex().flex_1().items_center().justify_center().gap(px(14.)).p_8().text_size(px(13.)).text_color(muted());
        let body: AnyElement = match (address, &self.core_error) {
            (_, Some(e)) => placeholder(e.clone(), cx).into_any_element(),
            (Some(addr), None) => match self.terminal(&addr, window, cx) {
                Some(view) => {
                    terminal_view = Some(view.clone());
                    div().flex_1().min_h_0().px_1().child(view).into_any_element()
                }
                None => placeholder("正在连接 herdr-core…".into(), cx).into_any_element(),
            },
            (None, None) if self.project().is_none() && !self.snap.projects.is_empty() => empty().child(icon_tile(IconName::FolderOpen)).child("从左侧选一个项目打开。").into_any_element(),
            (None, None) if self.project().is_none() => empty()
                .child(icon_tile(IconName::FolderOpen))
                .child(v_flex().items_center().gap_1().child(div().font_medium().text_color(style::fg()).child("还没有项目")).child(div().max_w(px(340.)).text_center().child("选一个文件夹、挑好团队、写一段简介，PM 就会开始规划。")))
                .child(Button::new("new-project-big").primary().label("新建项目…").on_click(cx.listener(|this, _, window, cx| this.new_project(window, cx))))
                .into_any_element(),
            (None, None) => {
                let why = if !self.snap.daemon { "lilad 没有运行" } else { "这个成员没有在运行" };
                empty()
                    .child(icon_tile(IconName::SquareTerminal))
                    .child(v_flex().items_center().gap_1().child(div().font_medium().text_color(style::fg()).child(why)).child("启动后，每个成员会在这里打开自己的终端。"))
                    .child(Button::new("start-big").primary().icon(IconName::Play).label("启动团队").on_click(cx.listener(|this, _, _, cx| this.act("project.up", json!({}), true, cx))))
                    .into_any_element()
            }
        };
        let card_header = member.as_ref().map(|m| {
            let status = s(m, "status");
            let spec = match m["effort"].as_str() {
                Some(e) => format!("{} · {e}", s(m, "model")),
                None => s(m, "model"),
            };
            h_flex()
                .h(px(38.))
                .flex_none()
                .pl(px(14.))
                .pr_2()
                .gap_2()
                .border_b_1()
                .border_color(style::divider())
                .bg(gpui::Hsla::from(rgba(0xffffff05)))
                .child(style::cli_tile(&s(m, "runtime"), 20.))
                .child(div().text_size(px(13.)).font_semibold().flex_none().child(s(m, "id")))
                .child(div().text_xs().text_color(muted()).flex_none().child(s(m, "role")))
                .child(div().text_xs().text_color(muted2()).truncate().min_w_0().child(spec))
                .child(div().flex_1())
                .child(div().text_xs().text_color(muted()).truncate().max_w(px(360.)).child(s(m, "msg")))
                .when(m["stale"] == true, |d| {
                    let id = s(m, "id");
                    d.child(style::tag(style::yellow(), "设置已改")).child(
                        Button::new("restart-member").small().small_button().outline().rounded_full().icon(IconName::RefreshCw).label("重启").tooltip("用新设置重启它：当前对话会丢失，手上的任务会重新派给它").on_click(cx.listener(move |this, _, _, cx| {
                            this.act("member.restart", json!({"member": id}), false, cx)
                        })),
                    )
                })
                .child(style::status_pill(&status, cx))
                .when_some(terminal_view, |d, view| {
                    d.child(Button::new("terminal-find").ghost().xsmall().icon(IconName::Search).tooltip("搜索终端历史 (⌘F)").on_click(move |_, window, cx| {
                        view.update(cx, |view, cx| view.open_search(window, cx));
                    }))
                })
        });
        // The state shows as a soft ring, not a loud border: the pill in the header carries the word.
        let status = member.as_ref().map(|m| s(m, "status")).unwrap_or_default();
        let (ring, halo) = match status.as_str() {
            "blocked" => (style::red().opacity(0.4), Some(style::red().opacity(0.08))),
            "working" => (style::blue().opacity(0.32), None),
            _ => (style::block_line(), None),
        };
        v_flex()
            .size_full()
            .pr(px(4.))
            .when_some(self.notice.clone(), |d, n| {
                d.child(
                    h_flex()
                        .mb_2()
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
            .child(
                card()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .border_color(ring)
                    .when_some(halo, |d, h| d.shadow(vec![BoxShadow { color: h, offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(3.), inset: false }]))
                    .children(card_header)
                    .child(body),
            )
    }

    pub fn right(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for i in &self.snap.inbox {
            if i["kind"] == "question" {
                let id = s(i, "id");
                self.answers.entry(id).or_insert_with(|| cx.new(|cx| gpui::component::input::InputState::new(window, cx).placeholder("或者写下你的回答…")));
            }
        }
        let inbox = self.snap.inbox.len();
        let open_tasks = self.snap.tasks.iter().filter(|t| !matches!(t["state"].as_str(), Some("done" | "cancelled"))).count();
        let inbox_el = self.inbox_list(cx).into_any_element();
        let tasks_el = self.task_board(cx).into_any_element();
        let log_el = self.activity(cx).into_any_element();
        let usage_el = self.usage_panel(cx);
        let periods = self.usage_periods(cx);
        let has_project = self.project().is_some();
        v_flex()
            .size_full()
            .pl(px(3.))
            .gap(px(style::GAP))
            .when_some(self.snap.error.clone(), |d, n| d.child(div().p_2().rounded(px(10.)).bg(style::yellow().opacity(0.15)).text_xs().child(n)))
            .child(card().flex_none().max_h(relative(0.45)).flex().flex_col().child(card_head("需要你", inbox)).child(inbox_el))
            .child(
                card()
                    .flex_1()
                    .min_h(px(170.))
                    .flex()
                    .flex_col()
                    .child(card_head("任务", open_tasks).when(has_project, |d| d.child(style::link("task-new", "新建", true).text_size(px(13.)).on_click(cx.listener(|this, _, window, cx| this.compose(None, None, window, cx))))))
                    .child(tasks_el),
            )
            .child(card().flex_none().flex().flex_col().child(card_head("用量", 0).child(periods)).child(usage_el))
            .child(card().flex_shrink(1.).flex_basis(relative(0.24)).min_h(px(84.)).flex().flex_col().child(card_head("动态", 0)).child(log_el))
    }

    fn inbox_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.snap.inbox.clone();
        if items.is_empty() {
            return div()
                .child(h_flex().px_4().pb(px(14.)).gap_2().text_color(muted()).child(Icon::new(IconName::CircleCheck).small().text_color(muted())).child(div().text_size(px(13.)).child("没有需要你处理的事")))
                .into_any_element();
        }
        let cards: Vec<AnyElement> = items.into_iter().enumerate().map(|(n, item)| self.inbox_card(item, n > 0, cx).into_any_element()).collect();
        v_flex().id("inbox-list").min_h_0().overflow_y_scrollbar().children(cards).into_any_element()
    }

    fn task_board(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let tasks = self.snap.tasks.clone();
        let group = |title: &'static str, states: &[&str], limit: usize| {
            let rows: Vec<Value> = tasks.iter().rev().filter(|t| states.contains(&t["state"].as_str().unwrap_or(""))).take(limit).cloned().collect();
            (title, rows)
        };
        let groups = [group("进行中", &["delivered", "in_progress"], 30), group("阻塞", &["blocked"], 30), group("排队", &["queued"], 30), group("最近完成", &["done"], 10)];
        let empty = groups.iter().all(|(_, r)| r.is_empty());
        let sublabel = |t: String| div().text_size(px(11.)).font_semibold().text_color(muted2()).child(t);
        v_flex()
            .id("task-board")
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .px_2()
            .pb(px(10.))
            .gap_3()
            .when(empty, |d| d.child(div().px_2().text_size(px(13.)).text_color(muted()).child("还没有任务。按 ⌘N 或在左侧项目上右键「新建任务」，也可以在 PM 的终端里提需求。")))
            .children(groups.into_iter().filter(|(_, rows)| !rows.is_empty()).map(|(title, rows)| {
                v_flex()
                    .gap(px(2.))
                    .child(h_flex().px_2().pt(px(2.)).pb_1().gap(px(5.)).child(sublabel(title.into())).child(sublabel(rows.len().to_string())))
                    .children(rows.into_iter().map(|t| {
                        let state = s(&t, "state");
                        let id = s(&t, "id");
                        let open = id.clone();
                        let active = self.task.as_deref() == Some(id.as_str());
                        h_flex()
                            .id(SharedString::from(format!("board-{id}")))
                            .items_start()
                            .px_2()
                            .py(px(7.))
                            .gap(px(10.))
                            .rounded(px(style::R_CTL))
                            .cursor_pointer()
                            .when(active, |d| d.bg(style::accent()))
                            .when(!active, |d| d.hover(|d| d.bg(style::hover())))
                            .on_click(cx.listener(move |this, _, window, cx| this.open_task(open.clone(), window, cx)))
                            .child(crate::detail::state_tag(&state, id.clone()))
                            .child(v_flex().flex_1().min_w_0().gap(px(1.)).child(div().text_size(px(13.)).truncate().child(s(&t, "title"))).child(div().pl(px(2.)).text_xs().text_color(muted2()).child(format!("{} → {}", s(&t, "from"), s(&t, "to")))))
                            .child(div().pt(px(2.)).text_xs().text_color(muted2()).child(data::clock(t["updated_ms"].as_i64().unwrap_or(0))))
                    }))
            }))
    }

    fn activity(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let log = self.snap.log.clone();
        if log.is_empty() {
            return div().px_4().pb_3().text_size(px(13.)).text_color(muted()).child("还没有动静").into_any_element();
        }
        v_flex()
            .id("activity")
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .px_4()
            .pb(px(10.))
            .children(log.into_iter().rev().take(40).enumerate().map(|(n, e)| {
                let task = e["task"].as_str().map(String::from);
                let detail = s(&e, "detail");
                h_flex()
                    .gap(px(10.))
                    .items_start()
                    .py(px(6.))
                    .text_xs()
                    .when(n > 0, |d| d.border_t_1().border_color(gpui::Hsla::from(rgba(0xffffff0a))))
                    .child(div().flex_none().w(px(34.)).text_color(muted2()).child(data::clock(e["at_ms"].as_i64().unwrap_or(0))))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(div().font_medium().child(s(&e, "actor")))
                                    .when_some(task, |d, t| d.child(crate::detail::task_link(&t, &format!("log{n}"), cx)))
                                    .child(div().text_color(muted()).child(s(&e, "kind"))),
                            )
                            .when(!detail.is_empty(), |d| d.child(div().text_color(muted2()).truncate().child(detail))),
                    )
            }))
            .into_any_element()
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
            _ => (IconName::Info, muted(), "提醒"),
        };
        let options: Vec<String> = item["options"].as_array().into_iter().flatten().filter_map(|o| o.as_str().map(String::from)).collect();
        let input = self.answers.get(&id).cloned();
        let card_id = id.clone();
        let who = [member.clone().unwrap_or_default(), data::clock(item["created_ms"].as_i64().unwrap_or(0))].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
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
                    .when_some(item["task"].as_str().map(String::from), |d, t| d.child(div().text_xs().child(crate::detail::task_link(&t, &format!("inbox-{card_id}"), cx))))
                    .child(div().text_xs().text_color(muted2()).child(who)),
            )
            .child(div().text_size(px(13.)).line_height(relative(1.55)).child(s(&item, "text")))
            .when(!options.is_empty(), |d| {
                d.child(h_flex().gap(px(6.)).flex_wrap().children(options.into_iter().enumerate().map(|(n, opt)| {
                    let (id, text) = (id.clone(), opt.clone());
                    Button::new(SharedString::from(format!("opt-{id}-{n}"))).small().small_button().outline().rounded_full().label(opt).on_click(cx.listener(move |this, _, window, cx| this.answer(id.clone(), Some(text.clone()), window, cx)))
                })))
            })
            .when_some(input, |d, input| {
                let (id, field) = (card_id.clone(), input.clone());
                d.child(
                    h_flex().gap(px(6.)).child(div().flex_1().child(Input::new(&input).control())).child(Button::new(SharedString::from(format!("send-{card_id}"))).primary().label("回答").on_click(cx.listener(move |this, _, window, cx| {
                        let text = field.read(cx).value().to_string();
                        if !text.trim().is_empty() {
                            this.answer(id.clone(), Some(text), window, cx);
                        }
                    }))),
                )
            })
            .when(kind != "question", |d| {
                let open = member.clone().filter(|_| matches!(kind.as_str(), "terminal" | "startup" | "stalled"));
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

    /// The bottom of the main column: where the daemon and the project stand, at a glance.
    pub fn status_bar(&mut self, _cx: &mut Context<Self>) -> impl IntoElement {
        let project = self.project();
        let members = self.members();
        let working = members.iter().filter(|m| m["status"] == "working").count();
        let waiting = members.iter().filter(|m| m["status"] == "blocked").count();
        let paused = self.snap.status.as_ref().is_some_and(|s| s["paused"] == true);
        let daemon = self.snap.daemon;
        let item = || h_flex().flex_none().gap(px(6.));
        let small_icon = |i: Icon| i.size(px(12.)).text_color(muted2());
        card()
            .h(px(style::STATUS))
            .flex_none()
            .mt(px(style::GAP))
            .flex()
            .items_center()
            .pl(px(14.))
            .pr_3()
            .gap_4()
            .text_size(px(11.))
            .text_color(muted())
            .whitespace_nowrap()
            .child(item().child(dot(if daemon { style::green() } else { muted2() })).child(if daemon { "lilad 运行中" } else { "lilad 未运行" }))
            .when_some(project.as_ref().map(|p| data::tilde(&p.cwd)), |d, p| d.child(item().child(small_icon(Icon::new(IconName::Folder))).child(div().truncate().max_w(px(280.)).child(p))))
            .when_some(project.as_ref().and_then(|p| p.branch.clone()), |d, b| d.child(item().child(small_icon(style::icon("lila/branch.svg"))).child(b)))
            .when(project.is_some() && !members.is_empty(), |d| {
                d.child(
                    item()
                        .gap_0()
                        .child(format!("{} 个成员", members.len()))
                        .when(daemon, |d| d.child(" · ").child(div().text_color(style::blue()).child(format!("{working} 个工作中"))))
                        .when(waiting > 0, |d| d.child(" · ").child(div().text_color(style::red()).child(format!("{waiting} 个在等你")))),
                )
            })
            .when(paused, |d| d.child(div().text_color(style::yellow()).child("已暂停派发")))
            .child(div().flex_1())
            .child(h_flex().gap_3().min_w_0().overflow_hidden().text_color(muted2()).children([("⌘N", "新建任务"), ("⌘I", "概览"), ("⇧⌘,", "项目设置"), ("⌘,", "设置")].into_iter().map(|(k, l)| h_flex().flex_none().gap_1().child(style::kbd(k)).child(l))))
    }
}

pub fn placeholder(text: String, _cx: &App) -> impl IntoElement {
    div().flex_1().flex().items_center().justify_center().p_8().text_color(muted()).child(text)
}
