//! How the window looks: a quiet chrome sidebar (every project, then the
//! team grouped by what needs attention), tabs for the open projects over a
//! terminal card, a side panel with the inbox, the task board and activity one
//! under another, and a status bar.

use crate::app::LilaApp;
use crate::data;
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::input::Input;
use gpui::component::scroll::ScrollableElement;
use gpui::component::spinner::Spinner;
use gpui::component::tag::Tag;
use gpui::component::{ActiveTheme, Icon, IconName, InteractiveElementExt, Sizable, StyledExt, h_flex, v_flex};
use gpui::prelude::*;
use gpui::*;
use serde_json::{Value, json};

pub fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

pub fn status_color(status: &str, cx: &App) -> Hsla {
    let t = cx.theme();
    match status {
        "working" => t.blue,
        "blocked" | "error" => t.red,
        "done" => t.green,
        "idle" => t.muted_foreground,
        _ => t.muted_foreground.opacity(0.5),
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
    div().size(px(7.)).flex_none().rounded_full().bg(color)
}

/// A member's status as a small mark: a spinner while it works, else a dot.
pub fn status_mark(status: &str, cx: &App) -> AnyElement {
    if status == "working" { Spinner::new().xsmall().color(status_color(status, cx)).into_any_element() } else { dot(status_color(status, cx)).into_any_element() }
}

/// A rounded letter tile in a color picked from the name.
fn avatar(name: &str, cx: &App) -> Div {
    let t = cx.theme();
    let colors = [t.blue, t.green, t.yellow, t.magenta, t.cyan, t.red];
    let hash = name.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    let letter: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    div()
        .size(px(20.))
        .flex_none()
        .rounded(px(5.))
        .bg(colors[hash as usize % colors.len()])
        .text_color(gpui::black())
        .text_xs()
        .font_semibold()
        .flex()
        .items_center()
        .justify_center()
        .child(letter)
}

fn label(text: impl Into<SharedString>, cx: &App) -> Div {
    div().text_xs().font_medium().text_color(cx.theme().muted_foreground).child(text.into())
}

/// A sidebar row: a soft highlight when chosen or hovered.
fn row(id: impl Into<ElementId>, active: bool, cx: &App) -> Stateful<Div> {
    let t = cx.theme();
    h_flex()
        .id(id)
        .mx_2()
        .px_2()
        .h(px(30.))
        .gap_2()
        .rounded_md()
        .cursor_pointer()
        .when(active, |d| d.bg(t.sidebar_accent).text_color(t.sidebar_accent_foreground))
        .when(!active, |d| d.hover(|d| d.bg(t.sidebar_accent.opacity(0.6))))
}

/// The room the macOS traffic lights take at the start of the sidebar's top row:
/// they end at 66 px (see main.rs), then a gap before the logo.
pub const TRAFFIC_LIGHTS: f32 = 84.;

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

/// The order a member's state is listed in: what needs you first.
fn group_of(status: &str) -> (usize, &'static str) {
    match status {
        "blocked" | "error" => (0, "等你"),
        "working" => (1, "工作中"),
        "done" => (2, "刚完成"),
        "idle" => (3, "空闲"),
        "unknown" => (4, "启动中"),
        _ => (5, "未运行"),
    }
}

impl LilaApp {
    pub fn sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let shown = self.shown_member().map(|m| s(&m, "id"));
        let selected = self.selected.clone();
        let projects = self.snap.projects.clone();
        let members = self.members();
        let count = members.len();
        let paused = self.snap.status.as_ref().is_some_and(|s| s["paused"] == true);
        let can_start = self.project().is_some() && (members.iter().any(|m| m["status"] == "gone") || !self.snap.daemon);

        let mut groups: Vec<(usize, &str, Vec<Value>)> = Vec::new();
        for m in members {
            let (order, name) = group_of(&s(&m, "status"));
            match groups.iter_mut().find(|(o, _, _)| *o == order) {
                Some((_, _, list)) => list.push(m),
                None => groups.push((order, name, vec![m])),
            }
        }
        groups.sort_by_key(|(o, _, _)| *o);

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .child(
                drag_bar(h_flex().id("sidebar-top"), cx)
                    .h(px(44.))
                    .flex_none()
                    .pl(px(TRAFFIC_LIGHTS))
                    .pr_3()
                    .gap_2()
                    .child(div().size(px(22.)).rounded(px(6.)).bg(theme.primary).text_color(theme.primary_foreground).text_xs().font_bold().flex().items_center().justify_center().child("L"))
                    .child(div().font_semibold().child("Lila"))
                    .child(div().flex_1())
                    .child(Button::new("new-project").ghost().xsmall().icon(IconName::FolderOpen).tooltip("新建项目…").on_click(cx.listener(|this, _, _, cx| this.new_project(cx)))),
            )
            .child(h_flex().px_4().pt_2().pb_1().child(label("项目", cx)))
            .children(projects.into_iter().map(|p| {
                let active = selected.as_deref() == Some(&p.id);
                let (status, inbox) = self.project_summary(&p.id);
                let id = p.id.clone();
                row(SharedString::from(format!("project-{}", p.id)), active, cx)
                    .child(avatar(&p.id, cx))
                    .child(div().text_sm().truncate().min_w_0().child(p.label()))
                    .child(div().flex_1())
                    .when_some(status, |d, st| d.child(status_mark(st, cx)))
                    .when(inbox > 0, |d| d.child(Tag::danger().xsmall().rounded_full().child(inbox.to_string())))
                    .when_some(p.branch.clone().filter(|_| inbox == 0 && status.is_none()), |d, b| d.child(div().text_xs().text_color(theme.muted_foreground).truncate().max_w(px(110.)).child(b)))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_project(id.clone(), cx)))
            }))
            .child(div().mx_4().my_3().h(px(1.)).bg(theme.sidebar_border))
            .when(count > 0, |d| d.child(h_flex().px_4().pt_2().pb_1().gap_1().child(label("成员", cx)).child(div().text_xs().text_color(theme.muted_foreground.opacity(0.7)).child(count.to_string()))))
            .child(
                v_flex().flex_1().min_h_0().overflow_y_scrollbar().gap_1().children(groups.into_iter().map(|(_, name, list)| {
                    let color = status_color(&s(&list[0], "status"), cx);
                    v_flex()
                        .child(h_flex().px_4().pt_1().pb_0p5().gap_1().text_xs().font_medium().child(div().text_color(color).child(name)).child(div().text_color(theme.muted_foreground).child(list.len().to_string())))
                        .children(list.into_iter().map(|m| {
                            let id = s(&m, "id");
                            let status = s(&m, "status");
                            let doing = Some(s(&m, "msg")).filter(|x| !x.is_empty()).or_else(|| m["task"]["title"].as_str().map(String::from)).unwrap_or_else(|| s(&m, "role"));
                            let queued = m["queued"].as_u64().unwrap_or(0);
                            let click = id.clone();
                            let stale = m["stale"] == true;
                            row(SharedString::from(format!("member-{id}")), shown.as_deref() == Some(&id), cx)
                                .child(Icon::new(IconName::Bot).small().text_color(color))
                                .child(div().text_sm().flex_none().child(id.clone()))
                                .when(stale, |d| d.child(Tag::warning().xsmall().child("待重启")))
                                .child(div().text_xs().text_color(theme.muted_foreground).truncate().min_w_0().flex_1().child(format!("· {doing}")))
                                .when(queued > 0, |d| d.child(div().text_xs().text_color(theme.muted_foreground).child(format!("+{queued}"))))
                                .child(status_mark(&status, cx))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.member = Some(click.clone());
                                    cx.notify();
                                }))
                        }))
                })),
            )
            .child(
                v_flex()
                    .py_2()
                    .border_t_1()
                    .border_color(theme.sidebar_border)
                    .when(can_start, |d| {
                        d.child(row("up", false, cx).child(Icon::new(IconName::Play).small()).child(div().text_sm().child("启动团队")).on_click(cx.listener(|this, _, _, cx| this.act("project.up", json!({}), true, cx))))
                    })
                    .when(self.snap.daemon && self.project().is_some(), |d| {
                        d.child(
                            row("pause", false, cx)
                                .child(Icon::new(if paused { IconName::Play } else { IconName::Pause }).small())
                                .child(div().text_sm().child(if paused { "恢复派发" } else { "暂停派发" }))
                                .on_click(cx.listener(move |this, _, _, cx| this.act(if paused { "resume" } else { "pause" }, json!({}), false, cx))),
                        )
                    })
                    .child(row("new-project-row", false, cx).child(Icon::new(IconName::Plus).small()).child(div().text_sm().child("新建项目…")).on_click(cx.listener(|this, _, _, cx| this.new_project(cx)))),
            )
    }

    /// The bar over the terminal: one tab per open project, then the window's actions.
    fn tab_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let selected = self.project().map(|p| p.id);
        let projects: Vec<data::Project> = self.tabs.iter().filter_map(|id| self.snap.projects.iter().find(|p| &p.id == id).cloned()).collect();
        let has_project = selected.is_some();
        drag_bar(h_flex().id("tab-bar"), cx)
            .h(px(44.))
            .flex_none()
            .px_2()
            .gap_1()
            .bg(theme.sidebar)
            .children(projects.into_iter().enumerate().map(|(n, p)| {
                let active = selected.as_deref() == Some(&p.id);
                let (status, inbox) = self.project_summary(&p.id);
                let click = p.id.clone();
                h_flex()
                    .id(SharedString::from(format!("tab-{}", p.id)))
                    .h(px(28.))
                    .px_2()
                    .gap_1p5()
                    .rounded_md()
                    .border_1()
                    .cursor_pointer()
                    .when(active, |d| d.bg(theme.secondary).border_color(theme.border))
                    .when(!active, |d| d.border_color(gpui::transparent_black()).text_color(theme.muted_foreground).hover(|d| d.bg(theme.secondary.opacity(0.5))))
                    .child(avatar(&p.id, cx).size(px(16.)).rounded(px(4.)))
                    .child(div().text_sm().truncate().max_w(px(180.)).child(p.label()))
                    .when_some(status, |d, st| d.child(status_mark(st, cx)))
                    .when(inbox > 0, |d| d.child(Tag::danger().xsmall().rounded_full().child(inbox.to_string())))
                    .when(n < 9 && inbox == 0, |d| d.child(div().text_xs().text_color(theme.muted_foreground.opacity(0.7)).child(format!("⌘{}", n + 1))))
                    .child({
                        let close = p.id.clone();
                        Button::new(SharedString::from(format!("tab-close-{}", p.id))).ghost().xsmall().icon(IconName::Close).tooltip("关闭标签 (⌘W)，团队继续运行").on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.close_tab(&close, cx);
                        }))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.select_project(click.clone(), cx)))
            }))
            .child(div().flex_1())
            .when(has_project, |d| {
                d.child(Button::new("bar-new-task").ghost().small().icon(IconName::Plus).label("新建任务").on_click(cx.listener(|this, _, window, cx| this.compose(None, None, window, cx))))
                    .child(Button::new("bar-settings").ghost().small().icon(IconName::Settings).tooltip("项目设置 (⌘,)").on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))))
            })
            .when(!self.right_open, |d| {
                let inbox = self.snap.inbox.len();
                d.child(
                    Button::new("bar-panel")
                        .ghost()
                        .small()
                        .icon(IconName::PanelRightOpen)
                        .when(inbox > 0, |b| b.label(inbox.to_string()))
                        .tooltip("显示收件箱、任务和动态 (⌘I)")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.right_open = true;
                            cx.notify();
                        })),
                )
            })
    }

    pub fn center(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let member = self.shown_member();
        let cwd = self.project().map(|p| data::tilde(&p.cwd)).unwrap_or_default();
        let address = member.as_ref().map(|m| s(m, "terminal")).filter(|a| !a.is_empty());
        let body: AnyElement = match (address, &self.core_error) {
            (_, Some(e)) => placeholder(e.clone(), cx).into_any_element(),
            (Some(addr), None) => match self.terminal(&addr, window, cx) {
                Some(view) => div().flex_1().min_h_0().child(view).into_any_element(),
                None => placeholder("正在连接 herdr-core…".into(), cx).into_any_element(),
            },
            (None, None) if self.project().is_none() && !self.snap.projects.is_empty() => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_3()
                .text_color(theme.muted_foreground)
                .child(Icon::new(IconName::FolderOpen).large().text_color(theme.muted_foreground))
                .child("从左侧选一个项目打开。")
                .into_any_element(),
            (None, None) if self.project().is_none() => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_3()
                .text_color(theme.muted_foreground)
                .child(Icon::new(IconName::FolderOpen).large().text_color(theme.muted_foreground))
                .child("还没有项目。选一个代码仓库，Lila 会创建 .lila/project.toml 并启动默认团队。")
                .child(Button::new("new-project-big").primary().label("新建项目…").on_click(cx.listener(|this, _, _, cx| this.new_project(cx))))
                .into_any_element(),
            (None, None) => {
                let why = if !self.snap.daemon { "lilad 没有运行" } else { "这个成员没有在运行" };
                v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .text_color(theme.muted_foreground)
                    .child(why)
                    .child(Button::new("start-big").primary().icon(IconName::Play).label("启动团队").on_click(cx.listener(|this, _, _, cx| this.act("project.up", json!({}), true, cx))))
                    .into_any_element()
            }
        };
        let card_header = member.as_ref().map(|m| {
            let status = s(m, "status");
            h_flex()
                .h(px(32.))
                .flex_none()
                .px_3()
                .gap_2()
                .border_b_1()
                .border_color(theme.border)
                .child(Icon::new(IconName::Bot).small().text_color(status_color(&status, cx)))
                .child(div().text_sm().font_semibold().child(s(m, "id")))
                .child(div().text_xs().text_color(theme.muted_foreground).child(match m["effort"].as_str() {
                    Some(e) => format!("{} · {} · {e}", s(m, "role"), s(m, "model")),
                    None => format!("{} · {}", s(m, "role"), s(m, "model")),
                }))
                .child(div().text_xs().text_color(theme.muted_foreground.opacity(0.7)).truncate().child(cwd.clone()))
                .child(div().flex_1())
                .child(div().text_xs().text_color(theme.muted_foreground).truncate().max_w(px(380.)).child(s(m, "msg")))
                .when(m["stale"] == true, |d| {
                    let id = s(m, "id");
                    d.child(div().text_xs().text_color(theme.yellow).child("设置已改")).child(
                        Button::new("restart-member").xsmall().outline().icon(IconName::RefreshCw).label("重启").tooltip("用新设置重启它：当前对话会丢失，手上的任务会重新派给它").on_click(cx.listener(move |this, _, _, cx| {
                            this.act("member.restart", json!({"member": id}), false, cx)
                        })),
                    )
                })
                .child(div().text_xs().text_color(status_color(&status, cx)).child(status_label(&status)))
                .child(status_mark(&status, cx))
        });
        let working = member.as_ref().is_some_and(|m| m["status"] == "working" || m["status"] == "blocked");
        let border = match member.as_ref().map(|m| s(m, "status")).as_deref() {
            Some("blocked") => theme.red.opacity(0.7),
            _ if working => theme.blue.opacity(0.55),
            _ => theme.border,
        };
        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .child(self.tab_bar(cx))
            .when_some(self.notice.clone(), |d, n| {
                d.child(
                    h_flex()
                        .mx_2()
                        .mb_2()
                        .px_3()
                        .py_2()
                        .gap_2()
                        .rounded_md()
                        .border_1()
                        .border_color(theme.red.opacity(0.5))
                        .bg(theme.red.opacity(0.12))
                        .text_sm()
                        .child(Icon::new(IconName::CircleAlert).small().text_color(theme.red))
                        .child(div().flex_1().child(n))
                        .child(Button::new("notice-close").ghost().xsmall().icon(IconName::Close).on_click(cx.listener(|this, _, _, cx| {
                            this.notice = None;
                            cx.notify();
                        }))),
                )
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .mx_2()
                    .mb_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(border)
                    .bg(gpui::Rgba { r: 0x21 as f32 / 255., g: 0x25 as f32 / 255., b: 0x2b as f32 / 255., a: 1.0 })
                    .overflow_hidden()
                    .children(card_header)
                    .child(body),
            )
    }

    pub fn right(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        for i in &self.snap.inbox {
            if i["kind"] == "question" {
                let id = s(i, "id");
                self.answers.entry(id).or_insert_with(|| cx.new(|cx| gpui::component::input::InputState::new(window, cx).placeholder("回答 PM…")));
            }
        }
        let inbox = self.snap.inbox.len();
        let open_tasks = self.snap.tasks.iter().filter(|t| !matches!(t["state"].as_str(), Some("done" | "cancelled"))).count();
        let section = |title: &'static str, count: usize, cx: &App| {
            h_flex().px_4().pt_3().pb_1p5().gap_1().child(label(title, cx)).when(count > 0, |d| d.child(div().text_xs().text_color(cx.theme().muted_foreground.opacity(0.7)).child(count.to_string())))
        };
        let inbox_el = self.inbox_list(cx).into_any_element();
        let tasks_el = self.task_board(cx).into_any_element();
        let log_el = self.activity(cx).into_any_element();
        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .child(
                drag_bar(h_flex().id("right-top"), cx)
                    .h(px(44.))
                    .flex_none()
                    .pl_4()
                    .pr_2()
                    .child(div().text_sm().font_semibold().child("概览"))
                    .child(div().flex_1())
                    .child(Button::new("right-close").ghost().small().icon(IconName::PanelRightClose).tooltip("收起 (⌘I)").on_click(cx.listener(|this, _, _, cx| {
                        this.right_open = false;
                        cx.notify();
                    }))),
            )
            .when_some(self.snap.error.clone(), |d, n| d.child(div().mx_2().mb_2().p_2().rounded_md().bg(theme.yellow.opacity(0.15)).text_xs().child(n)))
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .pb_3()
                    .child(section("需要你", inbox, cx))
                    .child(inbox_el)
                    .child(div().mx_4().mt_3().h(px(1.)).bg(theme.sidebar_border))
                    .child(section("任务", open_tasks, cx))
                    .child(tasks_el)
                    .child(div().mx_4().mt_3().h(px(1.)).bg(theme.sidebar_border))
                    .child(section("动态", 0, cx))
                    .child(log_el),
            )
    }

    fn inbox_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let items = self.snap.inbox.clone();
        if items.is_empty() {
            return h_flex()
                .px_4()
                .gap_2()
                .text_color(theme.muted_foreground)
                .child(Icon::new(IconName::CircleCheck).small().text_color(theme.muted_foreground))
                .child(div().text_sm().child("没有需要你处理的事"));
        }
        let cards: Vec<AnyElement> = items.into_iter().map(|item| self.inbox_card(item, cx).into_any_element()).collect();
        v_flex().px_2().gap_2().children(cards)
    }

    fn task_board(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let tasks = self.snap.tasks.clone();
        let group = |title: &'static str, states: &[&str], limit: usize| {
            let rows: Vec<Value> = tasks.iter().rev().filter(|t| states.contains(&t["state"].as_str().unwrap_or(""))).take(limit).cloned().collect();
            (title, rows)
        };
        let groups = [group("进行中", &["delivered", "in_progress"], 30), group("阻塞", &["blocked"], 30), group("排队", &["queued"], 30), group("最近完成", &["done"], 10)];
        let empty = groups.iter().all(|(_, r)| r.is_empty());
        v_flex()
            .px_2()
            .gap_3()
            .when(empty, |d| d.child(div().px_2().text_sm().text_color(theme.muted_foreground).child("还没有任务。点上方「新建任务」，或者在 PM 的终端里提需求。")))
            .children(groups.into_iter().filter(|(_, rows)| !rows.is_empty()).map(|(title, rows)| {
                v_flex()
                    .gap_0p5()
                    .child(h_flex().px_2().pb_1().gap_1().child(label(title, cx)).child(div().text_xs().text_color(theme.muted_foreground.opacity(0.7)).child(rows.len().to_string())))
                    .children(rows.into_iter().map(|t| {
                        let state = s(&t, "state");
                        let id = s(&t, "id");
                        let open = id.clone();
                        let active = self.task.as_deref() == Some(id.as_str());
                        v_flex()
                            .id(SharedString::from(format!("board-{id}")))
                            .px_2()
                            .py_1p5()
                            .gap_0p5()
                            .rounded_md()
                            .cursor_pointer()
                            .when(active, |d| d.bg(theme.sidebar_accent))
                            .when(!active, |d| d.hover(|d| d.bg(theme.sidebar_accent.opacity(0.6))))
                            .on_click(cx.listener(move |this, _, window, cx| this.open_task(open.clone(), window, cx)))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(crate::detail::state_tag(&state).xsmall().child(id.clone()))
                                    .child(div().text_sm().truncate().flex_1().min_w_0().child(s(&t, "title"))),
                            )
                            .child(div().pl_1().text_xs().text_color(theme.muted_foreground).child(format!("{} → {} · {}", s(&t, "from"), s(&t, "to"), data::clock(t["updated_ms"].as_i64().unwrap_or(0)))))
                    }))
            }))
    }

    fn activity(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let log = self.snap.log.clone();
        if log.is_empty() {
            return v_flex().px_4().text_sm().text_color(theme.muted_foreground).child("还没有动静");
        }
        v_flex().px_4().gap_1().children(log.into_iter().rev().take(40).enumerate().map(|(n, e)| {
            let task = e["task"].as_str().map(String::from);
            h_flex()
                .gap_2()
                .items_start()
                .text_xs()
                .child(div().flex_none().w(px(36.)).text_color(theme.muted_foreground).child(data::clock(e["at_ms"].as_i64().unwrap_or(0))))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            h_flex()
                                .gap_1()
                                .child(div().font_medium().child(s(&e, "actor")))
                                .when_some(task, |d, t| d.child(crate::detail::task_link(&t, &format!("log{n}"), cx)))
                                .child(div().text_color(theme.muted_foreground).child(s(&e, "kind"))),
                        )
                        .child(div().text_color(theme.muted_foreground).truncate().child(s(&e, "detail"))),
                )
        }))
    }

    fn inbox_card(&mut self, item: Value, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let id = s(&item, "id");
        let kind = s(&item, "kind");
        let member = item["member"].as_str().map(String::from);
        let (icon, accent, label_text) = match kind.as_str() {
            "question" => (IconName::CircleAlert, theme.yellow, "PM 在问你"),
            "terminal" => (IconName::SquareTerminal, theme.red, "终端在等你"),
            "startup" => (IconName::TriangleAlert, theme.red, "启动卡住"),
            "stalled" => (IconName::TriangleAlert, theme.yellow, "没有回应"),
            "lost" => (IconName::CircleX, theme.red, "成员退出"),
            "report" => (IconName::CircleCheck, theme.green, "完成报告"),
            _ => (IconName::Info, theme.muted_foreground, "提醒"),
        };
        let options: Vec<String> = item["options"].as_array().into_iter().flatten().filter_map(|o| o.as_str().map(String::from)).collect();
        let input = self.answers.get(&id).cloned();
        let card_id = id.clone();
        v_flex()
            .p_3()
            .gap_2()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(icon).small().text_color(accent))
                    .child(div().text_sm().font_medium().child(label_text))
                    .child(div().flex_1())
                    .when_some(item["task"].as_str().map(String::from), |d, t| d.child(div().text_xs().child(crate::detail::task_link(&t, &format!("inbox-{card_id}"), cx))))
                    .when_some(member.clone(), |d, m| d.child(div().text_xs().text_color(theme.muted_foreground).child(m)))
                    .child(div().text_xs().text_color(theme.muted_foreground.opacity(0.7)).child(data::clock(item["created_ms"].as_i64().unwrap_or(0)))),
            )
            .child(div().text_sm().child(s(&item, "text")))
            .when(!options.is_empty(), |d| {
                d.child(h_flex().gap_1().flex_wrap().children(options.into_iter().enumerate().map(|(n, opt)| {
                    let (id, text) = (id.clone(), opt.clone());
                    Button::new(SharedString::from(format!("opt-{id}-{n}"))).small().outline().label(opt).on_click(cx.listener(move |this, _, window, cx| this.answer(id.clone(), Some(text.clone()), window, cx)))
                })))
            })
            .when_some(input, |d, input| {
                let (id, field) = (card_id.clone(), input.clone());
                d.child(
                    h_flex().gap_1().child(div().flex_1().child(Input::new(&input).small())).child(Button::new(SharedString::from(format!("send-{card_id}"))).small().primary().label("回答").on_click(
                        cx.listener(move |this, _, window, cx| {
                            let text = field.read(cx).value().to_string();
                            if !text.trim().is_empty() {
                                this.answer(id.clone(), Some(text), window, cx);
                            }
                        }),
                    )),
                )
            })
            .when(kind != "question", |d| {
                let open = member.clone().filter(|_| matches!(kind.as_str(), "terminal" | "startup" | "stalled"));
                let id = card_id.clone();
                d.child(
                    h_flex()
                        .gap_1()
                        .when_some(open, |d, m| {
                            d.child(Button::new(SharedString::from(format!("open-{card_id}"))).small().outline().icon(IconName::SquareTerminal).label("打开终端").on_click(cx.listener(move |this, _, _, cx| {
                                this.member = Some(m.clone());
                                cx.notify();
                            })))
                        })
                        .child(div().flex_1())
                        .child(Button::new(SharedString::from(format!("dismiss-{card_id}"))).small().ghost().label("知道了").on_click(cx.listener(move |this, _, window, cx| this.answer(id.clone(), None, window, cx)))),
                )
            })
    }

    pub fn status_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let project = self.project();
        let members = self.members();
        let working = members.iter().filter(|m| m["status"] == "working").count();
        let waiting = members.iter().filter(|m| m["status"] == "blocked").count();
        let paused = self.snap.status.as_ref().is_some_and(|s| s["paused"] == true);
        let item = |text: String| div().text_xs().text_color(theme.muted_foreground).child(text);
        h_flex()
            .h(px(26.))
            .flex_none()
            .px_3()
            .gap_4()
            .bg(theme.sidebar)
            .border_t_1()
            .border_color(theme.sidebar_border)
            .child(h_flex().gap_1p5().child(dot(if self.snap.daemon { theme.green } else { theme.muted_foreground.opacity(0.5) })).child(item(if self.snap.daemon { "lilad".into() } else { "lilad 未运行".into() })))
            .when_some(project.as_ref().map(|p| data::tilde(&p.cwd)), |d, p| d.child(item(p).truncate().max_w(px(360.))))
            .when_some(project.as_ref().and_then(|p| p.branch.clone()), |d, b| d.child(item(format!("⎇ {b}"))))
            .when(!members.is_empty(), |d| d.child(item(format!("{} 个成员 · {working} 个工作中", members.len()))))
            .when(waiting > 0, |d| d.child(div().text_xs().text_color(theme.red).child(format!("{waiting} 个在等你"))))
            .when(paused, |d| d.child(div().text_xs().text_color(theme.yellow).child("已暂停派发")))
            .child(div().flex_1())
            .child(item("⌘N 新建任务 · ⌘1–9 切换标签 · ⌘W 关闭标签 · ⌘, 设置 · ⌘I 概览 · Esc 关闭面板".into()))
    }
}

pub fn placeholder(text: String, cx: &App) -> impl IntoElement {
    div().flex_1().flex().items_center().justify_center().p_8().text_color(cx.theme().muted_foreground).child(text)
}
