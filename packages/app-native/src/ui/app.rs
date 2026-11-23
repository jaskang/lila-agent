//! Root `LilaApp` view built with `gpui-kit`: transparent blurred window,
//! custom `TitleBar`, frosted sidebar, rounded workspace card, virtualized
//! transcript, floating composer, ⌘K command palette dialog, and live theme toggle.

use std::cell::Cell;

use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::message_scroller::MessageScrollerState;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, Theme, TitleBar, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    Anchor, App, AppContext as _, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, actions, div,
    prelude::FluentBuilder as _, px,
};

use crate::protocol::ConnectionState;
use crate::store::{AppStore, AppStoreEvent, ToastTone};
use crate::ui::composer::ComposerView;
use crate::ui::pending::PendingPanelView;
use crate::ui::sidebar::render_sidebar;
use crate::ui::theme_setup;
use crate::ui::transcript::render_transcript;

actions!(lila, [NewSession, ToggleSidebar, OpenPalette]);

pub fn bind_global_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-n", NewSession, None),
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        KeyBinding::new("cmd-k", OpenPalette, None),
    ]);
}

pub struct LilaApp {
    store: Entity<AppStore>,
    focus_handle: FocusHandle,
    scroller: Entity<MessageScrollerState>,
    composer: Entity<ComposerView>,
    pending: Entity<PendingPanelView>,
    _subscriptions: Vec<Subscription>,
}

impl LilaApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = AppStore::global(cx);
        let focus_handle = cx.focus_handle();
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        let composer = cx.new(|cx| ComposerView::new(store.clone(), window, cx));
        let pending = cx.new(|cx| PendingPanelView::new(store.clone(), window, cx));

        let obs = cx.observe(&store, |_, _, cx| cx.notify());

        let scroller_sub = scroller.clone();
        let composer_sub = composer.clone();
        let ev_sub = cx.subscribe_in(
            &store,
            window,
            move |_, _, event: &AppStoreEvent, window, cx| match event {
                AppStoreEvent::Toast { tone, text } => {
                    let notif = match tone {
                        ToastTone::Info => Notification::info(text.clone()),
                        ToastTone::Success => Notification::success(text.clone()),
                        ToastTone::Warning => Notification::warning(text.clone()),
                        ToastTone::Error => Notification::error(text.clone()),
                    };
                    window.push_notification(notif, cx);
                }
                AppStoreEvent::TranscriptReset { block_count } => {
                    let count = *block_count;
                    scroller_sub.update(cx, |s, cx| s.reset(count, cx));
                }
                AppStoreEvent::DraftSync { text, .. } => {
                    let t = text.clone();
                    composer_sub.update(cx, |c, cx| c.sync_text(t, window, cx));
                }
            },
        );

        let app_sub = window.observe_window_appearance(|window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
        });

        Self {
            store,
            focus_handle,
            scroller,
            composer,
            pending,
            _subscriptions: vec![obs, ev_sub, app_sub],
        }
    }

    fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let query_input = cx.new(|cx| InputState::new(window, cx).placeholder("搜索会话或命令…"));
        let initial_focus = Cell::new(true);

        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                query_input.focus_handle(cx).focus(window, cx);
            }
            let query = query_input.read(cx).value().trim().to_lowercase();
            let st_read = store.read(cx);
            let selected_id = st_read.selected_id.clone();

            struct PaletteEntry {
                id: String,
                group: &'static str,
                label: String,
                shortcut: Option<&'static str>,
            }

            let mut entries = vec![
                PaletteEntry {
                    id: "act:new".to_owned(),
                    group: "操作",
                    label: "新建对话".to_owned(),
                    shortcut: Some("⌘N"),
                },
                PaletteEntry {
                    id: "act:add-project".to_owned(),
                    group: "操作",
                    label: "添加本地项目…".to_owned(),
                    shortcut: None,
                },
                PaletteEntry {
                    id: "act:toggle-sidebar".to_owned(),
                    group: "操作",
                    label: "切换侧边栏".to_owned(),
                    shortcut: Some("⌘B"),
                },
                PaletteEntry {
                    id: "act:toggle-theme".to_owned(),
                    group: "操作",
                    label: "切换浅色 / 深色主题".to_owned(),
                    shortcut: None,
                },
            ];

            if let Some(ref sid) = selected_id {
                entries.push(PaletteEntry {
                    id: format!("act:compact:{sid}"),
                    group: "当前会话",
                    label: "压缩当前会话上下文".to_owned(),
                    shortcut: None,
                });
                entries.push(PaletteEntry {
                    id: format!("act:reload:{sid}"),
                    group: "当前会话",
                    label: "重新加载技能与 MCP".to_owned(),
                    shortcut: None,
                });
            }

            let mut sessions: Vec<_> = st_read.sessions.values().cloned().collect();
            sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            for s in sessions.into_iter().take(40) {
                let title = if s.title.is_empty() {
                    "新会话".to_owned()
                } else {
                    s.title
                };
                entries.push(PaletteEntry {
                    id: format!("sess:{}", s.id),
                    group: "会话",
                    label: title,
                    shortcut: None,
                });
            }

            let filtered: Vec<_> = entries
                .into_iter()
                .filter(|e| {
                    query.is_empty()
                        || e.label.to_lowercase().contains(&query)
                        || e.group.contains(&query)
                })
                .collect();

            let mut list = v_flex().gap(px(2.0));
            if filtered.is_empty() {
                list = list.child(
                    div()
                        .py_6()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("没有匹配的命令或会话"),
                );
            } else {
                let mut last_group: Option<&str> = None;
                for item in filtered {
                    if last_group != Some(item.group) {
                        last_group = Some(item.group);
                        list = list.child(
                            div()
                                .px_2()
                                .pt_2()
                                .pb_1()
                                .text_size(px(10.0))
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_color(cx.theme().muted_foreground)
                                .child(item.group),
                        );
                    }
                    let st_run = store.clone();
                    let raw = item.id.clone();
                    list = list.child(
                        h_flex()
                            .id(SharedString::from(format!("pal-{}", item.id)))
                            .w_full()
                            .items_center()
                            .justify_between()
                            .px_2()
                            .py(px(6.0))
                            .rounded_lg()
                            .cursor_pointer()
                            .hover(|s| s.bg(cx.theme().accent))
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                if raw == "act:new" {
                                    st_run.update(cx, |s, cx| s.start_new(None, cx));
                                } else if raw == "act:add-project" {
                                    st_run.update(cx, |s, cx| s.add_project(cx));
                                } else if raw == "act:toggle-sidebar" {
                                    st_run.update(cx, |s, cx| s.toggle_sidebar(cx));
                                } else if raw == "act:toggle-theme" {
                                    theme_setup::toggle_theme_mode(window, cx);
                                } else if let Some(sid) = raw.strip_prefix("act:compact:") {
                                    st_run.update(cx, |s, cx| s.compact(sid.to_owned(), cx));
                                } else if let Some(sid) = raw.strip_prefix("act:reload:") {
                                    st_run.update(cx, |s, cx| s.reload(sid.to_owned(), cx));
                                } else if let Some(sid) = raw.strip_prefix("sess:") {
                                    st_run.update(cx, |s, cx| {
                                        s.select_session(Some(sid.to_owned()), cx);
                                    });
                                }
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .text_color(cx.theme().foreground)
                                    .child(item.label),
                            )
                            .when_some(item.shortcut, |el, sc| {
                                el.child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(sc),
                                )
                            }),
                    );
                }
            }

            dialog
                .title("命令面板")
                .w(px(520.0))
                .child(
                    v_flex()
                        .gap_2()
                        .child(Input::new(&query_input))
                        .child(
                            div()
                                .id("palette-scroll")
                                .max_h(px(340.0))
                                .overflow_y_scrollbar()
                                .child(list),
                        ),
                )
        });
    }

    fn render_new_session_center(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let projects = store.projects.clone();
        let active_proj = store.active_project().cloned();
        let connected = store.connection == ConnectionState::Connected;

        if projects.is_empty() {
            let st_add = self.store.clone();
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_8()
                .child(
                    v_flex()
                        .w_full()
                        .max_w(px(520.0))
                        .items_center()
                        .gap_4()
                        .p_8()
                        .rounded_2xl()
                        .border_1()
                        .border_color(theme_setup::glass_rim_border(cx))
                        .bg(theme_setup::card_bg(cx))
                        .shadow_lg()
                        .child(
                            div()
                                .text_xl()
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_color(cx.theme().foreground)
                                .child("欢迎使用 Lila"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("先添加一个本地项目目录，即可开始与 Agent 协作。"),
                        )
                        .child(
                            Button::new("welcome-add-project")
                                .primary()
                                .disabled(!connected)
                                .label("选择项目目录…")
                                .on_click(move |_, _, cx| {
                                    st_add.update(cx, |s, cx| s.add_project(cx));
                                }),
                        ),
                )
                .into_any_element();
        }

        let Some(proj) = active_proj else {
            return div().flex_1().into_any_element();
        };
        let proj_id = proj.id.clone();
        let proj_name = proj.name.clone();
        let proj_path = proj.path.clone();
        let st_pop = self.store.clone();

        let project_selector = Popover::new("new-session-project-picker")
            .anchor(Anchor::TopLeft)
            .offset(px(6.0))
            .trigger(
                Button::new("new-proj-trigger")
                    .small()
                    .icon(IconName::FolderOpen)
                    .label(proj_name),
            )
            .content(move |_, _, cx| {
                let mut col = v_flex().w(px(300.0)).gap(px(2.0));
                for p in projects.clone() {
                    let is_curr = p.id == proj_id;
                    let st_p = st_pop.clone();
                    let pid = p.id.clone();
                    let pop_ent = cx.entity();
                    col = col.child(
                        v_flex()
                            .id(SharedString::from(format!("np-opt-{}", p.id)))
                            .w_full()
                            .px_2()
                            .py(px(6.0))
                            .rounded_md()
                            .cursor_pointer()
                            .when(is_curr, |el| el.bg(cx.theme().accent))
                            .hover(|s| s.bg(cx.theme().accent))
                            .on_click(move |_, window, cx| {
                                st_p.update(cx, |s, cx| {
                                    s.update_new_session(|ns| ns.project_id = Some(pid.clone()), cx);
                                });
                                pop_ent.update(cx, |st, cx| st.dismiss(window, cx));
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .text_color(cx.theme().foreground)
                                    .child(p.name),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(cx.theme().muted_foreground)
                                    .truncate()
                                    .child(p.path),
                            ),
                    );
                }
                let st_more = st_pop.clone();
                let pop_ent2 = cx.entity();
                col = col.child(
                    Button::new("np-add-more")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Plus)
                        .label("添加本地项目…")
                        .on_click(move |_, window, cx| {
                            pop_ent2.update(cx, |st, cx| st.dismiss(window, cx));
                            st_more.update(cx, |s, cx| s.add_project(cx));
                        }),
                );
                div()
                    .id("np-scroll")
                    .max_h(px(280.0))
                    .overflow_y_scrollbar()
                    .child(col)
            });

        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .px_6()
            .py_8()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(740.0))
                    .gap_4()
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .text_color(cx.theme().foreground)
                                    .child("开始新对话"),
                            )
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(project_selector)
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(proj_path),
                                    ),
                            ),
                    )
                    .child(self.composer.clone()),
            )
            .into_any_element()
    }
}

impl Focusable for LilaApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for LilaApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let sidebar_open = store.sidebar_open;
        let selected_id = store.selected_id.clone();
        let active_summary = selected_id
            .as_deref()
            .and_then(|id| store.sessions.get(id).cloned());
        let parent_id = active_summary.as_ref().and_then(|s| s.parent_id.clone());
        let is_readonly_sub = parent_id.is_some();
        let conn = store.connection;
        let conn_detail = store.connection_detail.clone();
        let is_dark = cx.theme().is_dark();

        let title_text = active_summary
            .as_ref()
            .map(|s| {
                if s.title.is_empty() {
                    "新会话".to_owned()
                } else {
                    s.title.clone()
                }
            })
            .unwrap_or_else(|| "Lila".to_owned());

        let st_toggle = self.store.clone();
        let st_back = self.store.clone();

        let title_bar = TitleBar::new().child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .pr_3()
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("tb-toggle-sb")
                                .ghost()
                                .xsmall()
                                .icon(if sidebar_open {
                                    IconName::PanelLeftClose
                                } else {
                                    IconName::PanelLeftOpen
                                })
                                .on_click(move |_, _, cx| {
                                    st_toggle.update(cx, |s, cx| s.toggle_sidebar(cx));
                                }),
                        )
                        .when_some(parent_id.clone(), |el, pid| {
                            el.child(
                                Button::new("tb-back-parent")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::ArrowLeft)
                                    .label("返回父会话")
                                    .on_click(move |_, _, cx| {
                                        st_back.update(cx, |s, cx| {
                                            s.select_session(Some(pid.clone()), cx);
                                        });
                                    }),
                            )
                        })
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .text_color(cx.theme().foreground)
                                .truncate()
                                .child(title_text),
                        ),
                )
                .child(
                    h_flex()
                        .items_center()
                        .gap_1()
                        .child(
                            Button::new("tb-theme-toggle")
                                .ghost()
                                .xsmall()
                                .icon(if is_dark { IconName::Sun } else { IconName::Moon })
                                .on_click(|_, window, cx| {
                                    theme_setup::toggle_theme_mode(window, cx);
                                }),
                        )
                        .child(
                            Button::new("tb-palette")
                                .ghost()
                                .xsmall()
                                .icon(IconName::Search)
                                .label("⌘K")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_command_palette(window, cx);
                                })),
                        ),
                ),
        );

        let conn_banner = (conn != ConnectionState::Connected).then(|| {
            let label = match conn {
                ConnectionState::Connecting => "正在连接 core…",
                ConnectionState::Disconnected => "与 core 断开连接，正在重连…",
                ConnectionState::Incompatible => "core 协议版本不兼容",
                ConnectionState::Connected => "",
            };
            h_flex()
                .w_full()
                .px_4()
                .py(px(6.0))
                .bg(cx.theme().muted)
                .border_b_1()
                .border_color(theme_setup::card_border(cx))
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .text_color(cx.theme().foreground)
                        .child(label),
                )
                .when_some(conn_detail, |el, d| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(d),
                    )
                })
        });

        let workspace_content = if let Some(sid) = selected_id.clone() {
            let st_fork = self.store.clone();
            let sid_fork = sid.clone();

            v_flex()
                .flex_1()
                .min_h_0()
                .child(render_transcript(
                    self.store.clone(),
                    self.scroller.clone(),
                    cx,
                ))
                .child(
                    v_flex()
                        .w_full()
                        .items_center()
                        .px_4()
                        .pb_4()
                        .pt_2()
                        .child(
                            v_flex()
                                .w_full()
                                .max_w(px(820.0))
                                .gap_2()
                                .child(self.pending.clone())
                                .child(if is_readonly_sub {
                                    h_flex()
                                        .w_full()
                                        .p_3()
                                        .rounded_2xl()
                                        .border_1()
                                        .border_color(theme_setup::glass_rim_border(cx))
                                        .bg(theme_setup::card_bg(cx))
                                        .shadow_md()
                                        .items_center()
                                        .justify_between()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(
                                                    "子代理会话为只读视图，可派生成独立会话继续对话。",
                                                ),
                                        )
                                        .child(
                                            Button::new("fork-subsession")
                                                .primary()
                                                .xsmall()
                                                .label("在此基础上继续对话")
                                                .on_click(move |_, _, cx| {
                                                    st_fork.update(cx, |s, cx| {
                                                        s.continue_subsession(sid_fork.clone(), cx);
                                                    });
                                                }),
                                        )
                                        .into_any_element()
                                } else {
                                    self.composer.clone().into_any_element()
                                }),
                        ),
                )
                .into_any_element()
        } else {
            self.render_new_session_center(cx).into_any_element()
        };

        let sidebar_el = sidebar_open.then(|| render_sidebar(&self.store, window, cx));

        v_flex()
            .id("lila-root")
            .track_focus(&self.focus_handle)
            .size_full()
            .on_action(cx.listener(|this, _: &NewSession, _, cx| {
                this.store.update(cx, |s, cx| s.start_new(None, cx));
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.store.update(cx, |s, cx| s.toggle_sidebar(cx));
            }))
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| {
                this.open_command_palette(window, cx);
            }))
            .child(title_bar)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .p_2()
                    .gap_2()
                    .when_some(sidebar_el, |el, sb| el.child(sb))
                    .child(
                        v_flex()
                            .flex_1()
                            .h_full()
                            .min_w_0()
                            .rounded_2xl()
                            .border_1()
                            .border_color(theme_setup::glass_rim_border(cx))
                            .bg(theme_setup::workspace_bg(cx))
                            .shadow_md()
                            .overflow_hidden()
                            .when_some(conn_banner, |el, b| el.child(b))
                            .child(workspace_content),
                    ),
            )
    }
}
