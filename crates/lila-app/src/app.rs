//! The main window: team on the left, a member's terminal in the middle,
//! what needs you, the task board and recent activity on the right.

use crate::data::{self, Project, Snapshot};
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::input::{Input, InputState};
use gpui::component::tag::Tag;
use gpui::component::scroll::ScrollableElement;
use gpui::component::{ActiveTheme, Sizable, StyledExt, h_flex, v_flex};
use gpui::prelude::*;
use gpui::*;
use lila::paths::Paths;
use lila_term::{Core, TerminalView};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

pub struct LilaApp {
    paths: Paths,
    snap: Snapshot,
    /// The project shown; `None` follows the first one.
    selected: Option<String>,
    /// The member whose terminal fills the middle; `None` is the owner.
    member: Option<String>,
    core: Option<Arc<Core>>,
    core_error: Option<String>,
    terminals: HashMap<String, Entity<TerminalView>>,
    answers: HashMap<String, Entity<InputState>>,
    /// The last action's error, shown until the next one.
    notice: Option<String>,
    _poll: Task<()>,
}

fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

impl LilaApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let paths = Paths::from_env();
        let poll = cx.spawn_in(window, async move |this, cx| {
            loop {
                let Ok((paths, selected)) = this.update(cx, |this, _| (this.paths.clone(), this.selected.clone())) else { return };
                let snap = cx.background_executor().spawn(async move { data::fetch(&paths, selected.as_deref()) }).await;
                if this.update(cx, |this, cx| this.apply(snap, cx)).is_err() {
                    return;
                }
                cx.background_executor().timer(Duration::from_millis(1000)).await;
            }
        });
        cx.spawn(async move |this, cx| {
            let core = cx.background_executor().spawn(async { Core::connect() }).await;
            let _ = this.update(cx, |this, cx| {
                match core {
                    Ok(c) => this.core = Some(c),
                    Err(e) => this.core_error = Some(format!("cannot connect to herdr-core: {e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        LilaApp { paths, snap: Snapshot::default(), selected: None, member: None, core: None, core_error: None, terminals: HashMap::new(), answers: HashMap::new(), notice: None, _poll: poll }
    }

    fn apply(&mut self, snap: Snapshot, cx: &mut Context<Self>) {
        if self.selected.is_none() {
            self.selected = snap.projects.first().map(|p| p.id.clone());
        }
        let open: Vec<String> = snap.inbox.iter().map(|i| s(i, "id")).collect();
        self.answers.retain(|id, _| open.contains(id));
        self.snap = snap;
        cx.notify();
    }

    fn project(&self) -> Option<Project> {
        let id = self.selected.as_deref()?;
        self.snap.projects.iter().find(|p| p.id == id).cloned()
    }

    fn members(&self) -> Vec<Value> {
        self.snap.status.as_ref().and_then(|s| s["members"].as_array().cloned()).unwrap_or_default()
    }

    /// The member shown in the middle: the chosen one, else the owner.
    fn shown_member(&self) -> Option<Value> {
        let members = self.members();
        let chosen = self.member.as_ref().and_then(|id| members.iter().find(|m| m["id"] == id.as_str()).cloned());
        chosen.or_else(|| members.iter().find(|m| m["owner"] == true).cloned())
    }

    /// Runs one request off the UI thread, then refreshes.
    fn act(&mut self, method: &'static str, params: Value, autostart: bool, cx: &mut Context<Self>) {
        let Some(project) = self.project() else { return };
        let paths = self.paths.clone();
        self.notice = None;
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { data::act(&paths, &project, method, params, autostart) }).await;
            let _ = this.update(cx, |this, cx| {
                if let Err(e) = r {
                    this.notice = Some(format!("{e:#}"));
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Pick a folder, make it a project if needed, start its team.
    fn new_project(&mut self, cx: &mut Context<Self>) {
        let paths = self.paths.clone();
        let picked = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("用这个文件夹".into()) });
        self.notice = None;
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(dirs))) = picked.await else { return };
            let Some(dir) = dirs.into_iter().next() else { return };
            let r = cx.background_executor().spawn(async move { data::create_and_start(&paths, &dir) }).await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(id) if !id.is_empty() => {
                        this.selected = Some(id);
                        this.member = None;
                    }
                    Ok(_) => {}
                    Err(e) => this.notice = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn answer(&mut self, id: String, text: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(input) = self.answers.get(&id) {
            input.update(cx, |s, cx| s.set_value("", window, cx));
        }
        self.act("answer", json!({"id": id, "text": text}), false, cx);
    }

    fn terminal(&mut self, address: &str, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<TerminalView>> {
        let core = self.core.clone()?;
        // A view whose stream ended while its member still runs (a lost
        // connection) is replaced on the next look.
        let alive = self.members().iter().any(|m| m["terminal"] == address && m["status"] != "gone");
        let stale = self.terminals.get(address).is_some_and(|v| v.read(cx).ended().is_some() && alive);
        if stale || !self.terminals.contains_key(address) {
            let view = cx.new(|cx| TerminalView::new(core, address.to_owned(), false, window, cx));
            self.terminals.insert(address.to_owned(), view);
        }
        self.terminals.get(address).cloned()
    }
}

fn status_color(status: &str, cx: &App) -> Hsla {
    let t = cx.theme();
    match status {
        "working" => t.blue,
        "blocked" | "error" => t.red,
        "done" => t.green,
        "idle" => t.muted_foreground,
        _ => t.border,
    }
}

fn status_label(status: &str) -> &'static str {
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

fn dot(color: Hsla) -> Div {
    div().size(px(8.)).flex_none().rounded_full().bg(color)
}

fn section(title: impl Into<SharedString>, count: Option<usize>, cx: &App) -> Div {
    h_flex()
        .gap_2()
        .px_3()
        .pt_3()
        .pb_1()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .child(title.into())
        .when_some(count.filter(|n| *n > 0), |d, n| d.child(Tag::danger().small().child(n.to_string())))
}

impl LilaApp {
    fn sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let selected = self.selected.clone();
        let shown = self.shown_member().map(|m| s(&m, "id"));
        let projects = self.snap.projects.clone();
        let members = self.members();
        let paused = self.snap.status.as_ref().is_some_and(|s| s["paused"] == true);
        let any_gone = members.iter().any(|m| m["status"] == "gone") || (!self.snap.daemon && self.project().is_some());
        v_flex()
            .w(px(260.))
            .h_full()
            .flex_none()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(
                h_flex()
                    .h(px(40.))
                    .px_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.sidebar_border)
                    .child(div().font_semibold().child("Lila"))
                    .child(div().flex_1())
                    .child(dot(if self.snap.daemon { theme.green } else { theme.border }))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(if self.snap.daemon { "lilad 运行中" } else { "lilad 未运行" })),
            )
            .child(
                h_flex()
                    .pr_2()
                    .child(section("项目", None, cx))
                    .child(div().flex_1())
                    .child(Button::new("new-project").xsmall().ghost().label("新建项目…").on_click(cx.listener(|this, _, _, cx| this.new_project(cx)))),
            )
            .children(projects.into_iter().map(|p| {
                let active = selected.as_deref() == Some(&p.id);
                let id = p.id.clone();
                div()
                    .id(SharedString::from(format!("project-{}", p.id)))
                    .mx_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .when(active, |d| d.bg(theme.sidebar_accent).text_color(theme.sidebar_accent_foreground))
                    .hover(|d| d.bg(theme.sidebar_accent))
                    .child(div().text_sm().child(p.id.clone()))
                    .child(div().text_xs().text_color(theme.muted_foreground).truncate().child(p.cwd.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(id.clone());
                        this.member = None;
                        cx.notify();
                    }))
            }))
            .child(section("团队", None, cx))
            .child(
                v_flex().flex_1().overflow_y_scrollbar().children(members.into_iter().map(|m| {
                    let id = s(&m, "id");
                    let status = s(&m, "status");
                    let active = shown.as_deref() == Some(&id);
                    let task = m["task"].as_object().map(|t| format!("{} {}", t["id"].as_str().unwrap_or(""), t["title"].as_str().unwrap_or("")));
                    let msg = s(&m, "msg");
                    let queued = m["queued"].as_u64().unwrap_or(0);
                    let click = id.clone();
                    v_flex()
                        .id(SharedString::from(format!("member-{id}")))
                        .mx_2()
                        .px_2()
                        .py_1p5()
                        .gap_0p5()
                        .rounded_md()
                        .cursor_pointer()
                        .when(active, |d| d.bg(theme.sidebar_accent))
                        .hover(|d| d.bg(theme.sidebar_accent))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.member = Some(click.clone());
                            cx.notify();
                        }))
                        .child(
                            h_flex()
                                .gap_2()
                                .child(dot(status_color(&status, cx)))
                                .child(div().text_sm().font_medium().child(id.clone()))
                                .child(div().text_xs().text_color(theme.muted_foreground).truncate().child(s(&m, "role")))
                                .child(div().flex_1())
                                .child(div().text_xs().text_color(status_color(&status, cx)).child(status_label(&status)))
                                .when(queued > 0, |d| d.child(Tag::secondary().small().child(format!("+{queued}")))),
                        )
                        .when_some(task, |d, t| d.child(div().pl_4().text_xs().truncate().child(t)))
                        .when(!msg.is_empty(), |d| d.child(div().pl_4().text_xs().text_color(theme.muted_foreground).truncate().child(format!("↳ {msg}"))))
                })),
            )
            .child(
                h_flex()
                    .p_2()
                    .gap_2()
                    .border_t_1()
                    .border_color(theme.sidebar_border)
                    .when(any_gone, |d| {
                        d.child(Button::new("up").small().primary().label("启动团队").on_click(cx.listener(|this, _, _, cx| this.act("project.up", json!({}), true, cx))))
                    })
                    .when(self.snap.daemon, |d| {
                        d.child(
                            Button::new("pause").small().ghost().label(if paused { "恢复派发" } else { "暂停派发" }).on_click(cx.listener(move |this, _, _, cx| {
                                this.act(if paused { "resume" } else { "pause" }, json!({}), false, cx)
                            })),
                        )
                    }),
            )
    }

    fn center(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let member = self.shown_member();
        let header = member.as_ref().map(|m| {
            let status = s(m, "status");
            h_flex()
                .h(px(40.))
                .px_3()
                .gap_2()
                .border_b_1()
                .border_color(theme.border)
                .child(dot(status_color(&status, cx)))
                .child(div().font_semibold().child(s(m, "id")))
                .child(div().text_sm().text_color(theme.muted_foreground).child(format!("{} · {}", s(m, "role"), s(m, "model"))))
                .child(div().flex_1())
                .child(div().text_xs().text_color(theme.muted_foreground).truncate().max_w(px(420.)).child(s(m, "msg")))
                .child(div().text_xs().text_color(theme.muted_foreground).child(s(m, "terminal")))
        });
        let address = member.as_ref().map(|m| s(m, "terminal")).filter(|a| !a.is_empty());
        let body: AnyElement = match (address, &self.core_error) {
            (_, Some(e)) => placeholder(e.clone(), cx).into_any_element(),
            (Some(addr), None) => match self.terminal(&addr, window, cx) {
                Some(view) => div().flex_1().min_h_0().child(view).into_any_element(),
                None => placeholder("正在连接 herdr-core…".into(), cx).into_any_element(),
            },
            (None, None) if self.project().is_none() => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_3()
                .text_color(theme.muted_foreground)
                .child("还没有项目。选一个代码仓库，Lila 会在里面创建 .lila/project.toml 并启动默认团队。")
                .child(Button::new("new-project-big").primary().label("新建项目…").on_click(cx.listener(|this, _, _, cx| this.new_project(cx))))
                .into_any_element(),
            (None, None) => {
                let why = if !self.snap.daemon {
                    "lilad 没有运行：点左下角「启动团队」。"
                } else {
                    "这个成员没有在运行：点左下角「启动团队」。"
                };
                placeholder(why.into(), cx).into_any_element()
            }
        };
        v_flex().flex_1().min_w_0().h_full().children(header).child(body)
    }

    fn right(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let inbox = self.snap.inbox.clone();
        let tasks = self.snap.tasks.clone();
        let log = self.snap.log.clone();
        for i in &inbox {
            if i["kind"] == "question" {
                let id = s(i, "id");
                self.answers.entry(id).or_insert_with(|| cx.new(|cx| InputState::new(window, cx).placeholder("回答 Owner…")));
            }
        }
        let group = |title: &str, states: &[&str], limit: usize| {
            let rows: Vec<Value> = tasks.iter().rev().filter(|t| states.contains(&t["state"].as_str().unwrap_or(""))).take(limit).cloned().collect();
            (title.to_owned(), rows)
        };
        let inbox_empty = inbox.is_empty();
        let inbox_count = inbox.len();
        let cards: Vec<AnyElement> = inbox.into_iter().map(|item| self.inbox_card(item, cx).into_any_element()).collect();
        let groups = [
            group("进行中", &["delivered", "in_progress"], 20),
            group("阻塞", &["blocked"], 20),
            group("排队", &["queued"], 20),
            group("最近完成", &["done"], 8),
        ];
        v_flex()
            .w(px(380.))
            .h_full()
            .flex_none()
            .border_l_1()
            .border_color(theme.border)
            .bg(theme.background)
            .overflow_y_scrollbar()
            .when_some(self.notice.clone(), |d, n| d.child(div().m_2().p_2().rounded_md().bg(theme.danger).text_color(theme.danger_foreground).text_xs().child(n)))
            .when_some(self.snap.error.clone(), |d, n| d.child(div().m_2().p_2().rounded_md().bg(theme.warning).text_color(theme.warning_foreground).text_xs().child(n)))
            .child(section("需要你", Some(inbox_count), cx))
            .when(inbox_empty, |d| d.child(div().px_3().pb_2().text_sm().text_color(theme.muted_foreground).child("没有需要你处理的事")))
            .children(cards)
            .child(section("任务", None, cx))
            .children(groups.into_iter().filter(|(_, rows)| !rows.is_empty()).map(|(title, rows)| {
                v_flex()
                    .px_3()
                    .pb_2()
                    .gap_1()
                    .child(div().text_xs().text_color(theme.muted_foreground).child(format!("{title} · {}", rows.len())))
                    .children(rows.into_iter().map(|t| {
                        let state = s(&t, "state");
                        let tag = match state.as_str() {
                            "blocked" => Tag::danger(),
                            "done" => Tag::success(),
                            "queued" => Tag::secondary(),
                            _ => Tag::info(),
                        };
                        h_flex()
                            .gap_2()
                            .text_sm()
                            .child(tag.small().child(s(&t, "id")))
                            .child(div().text_xs().text_color(theme.muted_foreground).flex_none().child(format!("{}→{}", s(&t, "from"), s(&t, "to"))))
                            .child(div().truncate().child(s(&t, "title")))
                    }))
            }))
            .child(section("动态", None, cx))
            .child(v_flex().px_3().pb_3().gap_0p5().children(log.into_iter().rev().take(30).map(|e| {
                let task = e["task"].as_str().map(|t| format!("{t} ")).unwrap_or_default();
                h_flex()
                    .gap_2()
                    .text_xs()
                    .child(div().flex_none().text_color(theme.muted_foreground).child(data::clock(e["at_ms"].as_i64().unwrap_or(0))))
                    .child(div().flex_none().font_medium().child(s(&e, "actor")))
                    .child(div().truncate().text_color(theme.muted_foreground).child(format!("{task}{} {}", s(&e, "kind"), s(&e, "detail"))))
            })))
    }

    fn inbox_card(&mut self, item: Value, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let id = s(&item, "id");
        let kind = s(&item, "kind");
        let member = item["member"].as_str().map(String::from);
        let (tag, label) = match kind.as_str() {
            "question" => (Tag::warning(), "提问"),
            "terminal" => (Tag::danger(), "终端等你"),
            "startup" => (Tag::danger(), "启动卡住"),
            "stalled" => (Tag::warning(), "没有回应"),
            "lost" => (Tag::danger(), "成员退出"),
            "report" => (Tag::success(), "完成报告"),
            _ => (Tag::secondary(), "提醒"),
        };
        let options: Vec<String> = item["options"].as_array().into_iter().flatten().filter_map(|o| o.as_str().map(String::from)).collect();
        let input = self.answers.get(&id).cloned();
        let card_id = id.clone();
        v_flex()
            .mx_3()
            .mb_2()
            .p_2()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.secondary)
            .child(
                h_flex()
                    .gap_2()
                    .child(tag.small().child(label))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(id.clone()))
                    .when_some(member.clone(), |d, m| d.child(div().text_xs().text_color(theme.muted_foreground).child(m))),
            )
            .child(div().text_sm().child(s(&item, "text")))
            .when(!options.is_empty(), |d| {
                d.child(h_flex().gap_1().flex_wrap().children(options.into_iter().enumerate().map(|(n, opt)| {
                    let (id, text) = (id.clone(), opt.clone());
                    Button::new(SharedString::from(format!("opt-{id}-{n}")))
                        .small()
                        .outline()
                        .label(opt)
                        .on_click(cx.listener(move |this, _, window, cx| this.answer(id.clone(), Some(text.clone()), window, cx)))
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
                            d.child(Button::new(SharedString::from(format!("open-{card_id}"))).small().outline().label("打开终端").on_click(cx.listener(move |this, _, _, cx| {
                                this.member = Some(m.clone());
                                cx.notify();
                            })))
                        })
                        .child(
                            Button::new(SharedString::from(format!("dismiss-{card_id}")))
                                .small()
                                .ghost()
                                .label("知道了")
                                .on_click(cx.listener(move |this, _, window, cx| this.answer(id.clone(), None, window, cx))),
                        ),
                )
            })
    }
}

fn placeholder(text: String, cx: &App) -> impl IntoElement {
    div().flex_1().flex().items_center().justify_center().p_8().text_color(cx.theme().muted_foreground).child(text)
}

impl Render for LilaApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        h_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.sidebar(cx))
            .child(self.center(window, cx))
            .child(self.right(window, cx))
    }
}
