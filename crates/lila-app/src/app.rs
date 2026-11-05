//! The main window: team on the left, a member's terminal in the middle,
//! what needs you, the task board and recent activity on the right.

use crate::data::{self, Project, Snapshot};
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::input::{Input, InputState};
use gpui::component::tag::Tag;
use gpui::component::scroll::ScrollableElement;
use gpui::component::{ActiveTheme, Sizable, StyledExt, h_flex, h_resizable, resizable_panel, v_flex, v_resizable};
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
    pub member: Option<String>,
    core: Option<Arc<Core>>,
    core_error: Option<String>,
    /// The last screen text of each member's terminal, by address (for the thumbnails).
    previews: HashMap<String, String>,
    terminals: HashMap<String, Entity<TerminalView>>,
    answers: HashMap<String, Entity<InputState>>,
    /// The last action's error, shown until the next one.
    notice: Option<String>,
    /// The task open in the detail panel.
    pub task: Option<String>,
    append: Option<Entity<InputState>>,
    /// The task whose "cancel" was pressed once (press again to confirm).
    pub confirm_cancel: Option<String>,
    _poll: Task<()>,
    _previews: Task<()>,
}

pub fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

impl LilaApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let paths = Paths::from_env();
        // lilad says when anything changed; refresh then. A slow poll covers a
        // daemon that is down or restarting.
        let (wake_tx, wake) = async_channel::unbounded::<()>();
        let watch_paths = paths.clone();
        std::thread::spawn(move || {
            loop {
                if let Ok(events) = lila::client::subscribe(&watch_paths) {
                    let _ = wake_tx.send_blocking(());
                    for _ in events {
                        if wake_tx.send_blocking(()).is_err() {
                            return;
                        }
                    }
                }
                if wake_tx.is_closed() {
                    return;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        });
        let poll = cx.spawn_in(window, async move |this, cx| {
            loop {
                let Ok((paths, selected, task)) = this.update(cx, |this, _| (this.paths.clone(), this.selected.clone(), this.task.clone())) else { return };
                let snap = cx.background_executor().spawn(async move { data::fetch(&paths, selected.as_deref(), task.as_deref()) }).await;
                if std::env::var_os("LILA_APP_DEBUG").is_some() {
                    eprintln!("[lila-app] {} refreshed: {} tasks", data::clock_ms(), snap.tasks.len());
                }
                if this.update(cx, |this, cx| this.apply(snap, cx)).is_err() {
                    return;
                }
                let woken = std::pin::pin!(wake.recv());
                let timeout = std::pin::pin!(cx.background_executor().timer(Duration::from_secs(5)));
                futures::future::select(woken, timeout).await;
                // Changes come in bursts (a delivery, then a status): take them together.
                cx.background_executor().timer(Duration::from_millis(80)).await;
                while wake.try_recv().is_ok() {}
            }
        });
        // Thumbnails: every member's screen as text, read every 1.5 s.
        let previews = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(1500)).await;
                let Ok((core, terminals)) = this.update(cx, |this, _| {
                    let shown = this.shown_member().map(|m| s(&m, "terminal"));
                    let terminals: Vec<String> = this.members().iter().map(|m| s(m, "terminal")).filter(|t| !t.is_empty() && Some(t) != shown.as_ref()).collect();
                    (this.core.clone(), terminals)
                }) else {
                    return;
                };
                let Some(core) = core else { continue };
                let read = cx
                    .background_executor()
                    .spawn(async move {
                        terminals
                            .into_iter()
                            .filter_map(|t| core.call("terminal.read", json!({"terminal": t, "lines": 0})).ok().map(|r| (t, r["text"].as_str().unwrap_or_default().to_owned())))
                            .collect::<Vec<_>>()
                    })
                    .await;
                if this.update(cx, |this, cx| {
                    this.previews.extend(read);
                    cx.notify();
                })
                .is_err()
                {
                    return;
                }
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
        LilaApp { paths, snap: Snapshot::default(), selected: None, member: None, core: None, core_error: None, terminals: HashMap::new(), answers: HashMap::new(), notice: None, task: None, append: None, confirm_cancel: None, previews: HashMap::new(), _poll: poll, _previews: previews }
    }

    fn apply(&mut self, snap: Snapshot, cx: &mut Context<Self>) {
        if self.selected.is_none() {
            self.selected = snap.projects.first().map(|p| p.id.clone());
        }
        let open: Vec<String> = snap.inbox.iter().map(|i| s(i, "id")).collect();
        self.answers.retain(|id, _| open.contains(id));
        // A detail fetched for another task (the panel moved on meanwhile) is dropped.
        let mut snap = snap;
        if snap.detail.as_ref().and_then(|d| d["id"].as_str()) != self.task.as_deref() {
            snap.detail = self.snap.detail.take().filter(|d| d["id"].as_str() == self.task.as_deref());
        }
        self.snap = snap;
        cx.notify();
    }

    pub fn project(&self) -> Option<Project> {
        let id = self.selected.as_deref()?;
        self.snap.projects.iter().find(|p| p.id == id).cloned()
    }

    pub fn members(&self) -> Vec<Value> {
        self.snap.status.as_ref().and_then(|s| s["members"].as_array().cloned()).unwrap_or_default()
    }

    /// The member shown in the middle: the chosen one, else the owner.
    fn shown_member(&self) -> Option<Value> {
        let members = self.members();
        let chosen = self.member.as_ref().and_then(|id| members.iter().find(|m| m["id"] == id.as_str()).cloned());
        chosen.or_else(|| members.iter().find(|m| m["owner"] == true).cloned())
    }

    /// Runs one request off the UI thread, then refreshes.
    pub fn act(&mut self, method: &'static str, params: Value, autostart: bool, cx: &mut Context<Self>) {
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

    /// Shows a task in the detail panel; its details load with the next poll (and now).
    pub fn open_task(&mut self, id: String, _window: &mut Window, cx: &mut Context<Self>) {
        if self.task.as_deref() != Some(&id) {
            self.snap.detail = None;
            self.confirm_cancel = None;
        }
        self.task = Some(id.clone());
        let (paths, project) = (self.paths.clone(), self.project());
        cx.spawn(async move |this, cx| {
            let Some(p) = project else { return };
            let v = cx.background_executor().spawn(async move { data::act(&paths, &p, "task.show", json!({"id": id}), false) }).await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(v) = v
                    && this.task.as_deref() == v["id"].as_str()
                {
                    this.snap.detail = Some(v);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn close_task(&mut self, cx: &mut Context<Self>) {
        self.task = None;
        self.snap.detail = None;
        self.confirm_cancel = None;
        cx.notify();
    }

    pub fn append_input(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        self.append.get_or_insert_with(|| cx.new(|cx| InputState::new(window, cx).placeholder("补充要求或修正…"))).clone()
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

pub fn status_color(status: &str, cx: &App) -> Hsla {
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

pub fn dot(color: Hsla) -> Div {
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
            .size_full()
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
                        this.close_task(cx);
                    }))
            }))
            .child(section("团队", None, cx))
            .child(
                v_flex().flex_1().overflow_y_scrollbar().children(members.into_iter().map(|m| {
                    let id = s(&m, "id");
                    let status = s(&m, "status");
                    let active = shown.as_deref() == Some(&id);
                    let task = m["task"].as_object().map(|t| (t["id"].as_str().unwrap_or("").to_owned(), t["title"].as_str().unwrap_or("").to_owned()));
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
                                .child(div().text_xs().text_color(if matches!(status.as_str(), "gone" | "unknown") { theme.muted_foreground } else { status_color(&status, cx) }).child(status_label(&status)))
                                .when(queued > 0, |d| d.child(Tag::secondary().small().child(format!("+{queued}")))),
                        )
                        .when_some(task, |d, (tid, title)| {
                            d.child(h_flex().pl_4().gap_1().text_xs().child(crate::detail::task_link(&tid, "member", cx)).child(div().truncate().child(title)))
                        })
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
        let main = v_flex().size_full().children(header).child(body);
        let thumbs = self.thumbnails(cx);
        match thumbs {
            Some(strip) => v_resizable("center")
                .child(resizable_panel().child(main))
                .child(resizable_panel().size(px(170.)).size_range(px(90.)..px(420.)).fixed().child(strip))
                .into_any_element(),
            None => main.into_any_element(),
        }
    }

    /// The other members' screens, small: a glance at what each is doing. Click one to open it.
    fn thumbnails(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let shown = self.shown_member().map(|m| s(&m, "id"));
        let others: Vec<Value> = self.members().into_iter().filter(|m| Some(s(m, "id")) != shown).collect();
        if others.is_empty() {
            return None;
        }
        let cards = others.into_iter().map(|m| {
            let (id, status, terminal) = (s(&m, "id"), s(&m, "status"), s(&m, "terminal"));
            let text = self.previews.get(&terminal).map(|t| last_lines(t, 14)).unwrap_or_default();
            let open = id.clone();
            v_flex()
                .id(SharedString::from(format!("thumb-{id}")))
                .flex_1()
                .min_w_0()
                .h_full()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .bg(gpui::Rgba { r: 0x21 as f32 / 255., g: 0x25 as f32 / 255., b: 0x2b as f32 / 255., a: 1.0 })
                .overflow_hidden()
                .cursor_pointer()
                .hover(|d| d.border_color(theme.ring))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.member = Some(open.clone());
                    cx.notify();
                }))
                .child(
                    h_flex()
                        .px_2()
                        .py_1()
                        .gap_2()
                        .flex_none()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(dot(status_color(&status, cx)))
                        .child(div().text_xs().font_semibold().child(id.clone()))
                        .child(div().text_xs().text_color(theme.muted_foreground).truncate().child(s(&m, "msg")))
                        .child(div().flex_1())
                        .child(div().text_xs().text_color(theme.muted_foreground).child(status_label(&status))),
                )
                .child(
                    // Newest line at the bottom; when the card is short, the oldest lines go.
                    div()
                        .flex()
                        .flex_col_reverse()
                        .flex_1()
                        .min_h_0()
                        .overflow_hidden()
                        .px_2()
                        .pb_1()
                        .font_family(lila_term::theme::FONT)
                        .text_size(px(9.))
                        .line_height(px(11.))
                        .text_color(theme.muted_foreground)
                        .children(text.into_iter().rev().map(|l| div().flex_none().whitespace_nowrap().overflow_hidden().child(l))),
                )
        });
        Some(h_flex().size_full().p_2().gap_2().bg(theme.background).border_t_1().border_color(theme.border).children(cards).into_any_element())
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
            .size_full()
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
                        let id = s(&t, "id");
                        let open = id.clone();
                        let active = self.task.as_deref() == Some(id.as_str());
                        h_flex()
                            .id(SharedString::from(format!("board-{id}")))
                            .gap_2()
                            .px_1()
                            .rounded_sm()
                            .text_sm()
                            .cursor_pointer()
                            .when(active, |d| d.bg(theme.list_active))
                            .hover(|d| d.bg(theme.list_hover))
                            .on_click(cx.listener(move |this, _, window, cx| this.open_task(open.clone(), window, cx)))
                            .child(crate::detail::state_tag(&state).small().child(id.clone()))
                            .child(div().text_xs().text_color(theme.muted_foreground).flex_none().child(format!("{}→{}", s(&t, "from"), s(&t, "to"))))
                            .child(div().truncate().child(s(&t, "title")))
                    }))
            }))
            .child(section("动态", None, cx))
            .child(v_flex().px_3().pb_3().gap_0p5().children(log.into_iter().rev().take(30).enumerate().map(|(n, e)| {
                let task = e["task"].as_str().map(String::from);
                h_flex()
                    .gap_2()
                    .text_xs()
                    .child(div().flex_none().text_color(theme.muted_foreground).child(data::clock(e["at_ms"].as_i64().unwrap_or(0))))
                    .child(div().flex_none().font_medium().child(s(&e, "actor")))
                    .when_some(task, |d, t| d.child(crate::detail::task_link(&t, &format!("log{n}"), cx)))
                    .child(div().truncate().text_color(theme.muted_foreground).child(format!("{} {}", s(&e, "kind"), s(&e, "detail"))))
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
                    .when_some(member.clone(), |d, m| d.child(div().text_xs().text_color(theme.muted_foreground).child(m)))
                    .when_some(item["task"].as_str().map(String::from), |d, t| d.child(div().text_xs().child(crate::detail::task_link(&t, &format!("inbox-{card_id}"), cx)))),
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
        let detail = self.task.as_ref().and(self.snap.detail.clone());
        let loading = self.task.is_some() && detail.is_none();
        div()
            .size_full()
            .relative()
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" && this.task.is_some() {
                    this.close_task(cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                h_flex()
                    .size_full()
                    .bg(theme.background)
                    .text_color(theme.foreground)
                    .child(
                        h_resizable("main")
                            .child(resizable_panel().size(px(260.)).size_range(px(200.)..px(460.)).fixed().child(self.sidebar(cx).into_any_element()))
                            .child(resizable_panel().child(self.center(window, cx).into_any_element()))
                            .child(resizable_panel().size(px(380.)).size_range(px(300.)..px(720.)).fixed().child(self.right(window, cx).into_any_element())),
                    ),
            )
            .when_some(detail, |d, t| d.child(self.detail_panel(t, window, cx)))
            .when(loading, |d| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .right_0()
                        .bottom_0()
                        .w(px(560.))
                        .bg(theme.popover)
                        .border_l_1()
                        .border_color(theme.border)
                        .shadow_lg()
                        .occlude()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(theme.muted_foreground)
                        .child("正在读取任务…"),
                )
            })
    }
}

/// The last `n` lines with text on a screen, without trailing blanks.
fn last_lines(text: &str, n: usize) -> Vec<String> {
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let end = lines.iter().rposition(|l| !l.is_empty()).map_or(0, |i| i + 1);
    lines[..end].iter().rev().take(n).rev().map(|l| l.chars().take(160).collect()).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn last_lines_skips_the_blank_bottom() {
        assert_eq!(super::last_lines("a\nb  \n\nc\n\n   \n", 2), ["", "c"]);
        assert_eq!(super::last_lines("a\nb\nc", 5), ["a", "b", "c"]);
        assert!(super::last_lines("\n\n", 3).is_empty());
    }
}
