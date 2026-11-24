//! Desktop window, command palette, and session workspace.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::base::{Disableable as _, InteractiveElementExt as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_kit::component::message_scroller::MessageScrollerState;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, KeyBinding, MouseButton, MouseMoveEvent, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, actions, div,
    prelude::FluentBuilder as _, px,
};

use crate::format::shorten_home_path;
use crate::protocol::ConnectionState;
use crate::store::{AppStore, AppStoreEvent, ToastTone};
use crate::ui::composer::{self, ComposerView};
use crate::ui::pending::PendingPanelView;
use crate::ui::sidebar::render_sidebar;
use crate::ui::theme_setup::ThemePreference;
use crate::ui::transcript::render_transcript;
use crate::ui::{
    BadgeTone, SHELL_INSET, WORKSPACE_BAR_HEIGHT, WORKSPACE_TRAFFIC_LIGHT_INSET, pill_badge,
    theme_setup, window_drag_bar, workspace_card,
};

actions!(
    lila,
    [
        NewSession,
        ToggleSidebar,
        OpenPalette,
        AbortRun,
        PrevSession,
        NextSession
    ]
);

pub fn bind_global_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-n", NewSession, None),
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        KeyBinding::new("cmd-k", OpenPalette, None),
        KeyBinding::new("cmd-.", AbortRun, None),
        KeyBinding::new("cmd-[", PrevSession, None),
        KeyBinding::new("cmd-]", NextSession, None),
    ]);
    composer::bind_keys(cx);
}

enum PaletteAction {
    NewSession,
    AddProject,
    ToggleSidebar,
    ToggleTheme,
    Compact(String),
    Reload(String),
    OpenSession(String),
}

pub struct LilaApp {
    store: Entity<AppStore>,
    focus_handle: FocusHandle,
    scroller: Entity<MessageScrollerState>,
    composer: Entity<ComposerView>,
    pending: Entity<PendingPanelView>,
    sidebar_resizing: bool,
    _subscriptions: Vec<Subscription>,
}

impl LilaApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = AppStore::global(cx);
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);
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
                AppStoreEvent::TranscriptSync {
                    prev_count,
                    next_count,
                } => {
                    let prev = *prev_count;
                    let next = *next_count;
                    scroller_sub.update(cx, |s, cx| {
                        let current = s.item_count();
                        if current != next {
                            if prev == current && next > prev {
                                // Fast path when appending new rows at tail.
                                s.append(next - prev, cx);
                            } else {
                                s.splice(0..current, next, cx);
                            }
                        } else if next > 0 {
                            // Keep heights fresh for streaming tail / live sub-agent updates.
                            let start = next.saturating_sub(4);
                            s.remeasure_items(start..next, cx);
                        }
                    });
                }
                AppStoreEvent::TranscriptPrepend { added_count } => {
                    let added = *added_count;
                    scroller_sub.update(cx, |s, cx| {
                        s.prepend(added, cx);
                    });
                }
                AppStoreEvent::TranscriptRemeasure => {
                    scroller_sub.update(cx, |s, cx| s.remeasure(cx));
                }
                AppStoreEvent::FocusComposer => {
                    let composer = composer_sub.clone();
                    let store = AppStore::global(cx);
                    window.defer(cx, move |window, cx| {
                        let store = store.read(cx);
                        let can_focus = match store.selected_id.as_deref() {
                            None => store.active_project().is_some(),
                            Some(sid) => {
                                let is_readonly = store
                                    .sessions
                                    .get(sid)
                                    .is_some_and(|s| s.parent_id.is_some());
                                let has_pending = store
                                    .main_slot
                                    .state
                                    .as_ref()
                                    .is_some_and(|st| !st.pending.is_empty());
                                !is_readonly && !has_pending
                            }
                        };
                        if can_focus {
                            composer.read(cx).focus_handle(cx).focus(window, cx);
                        }
                    });
                }
                AppStoreEvent::DraftSync { text, .. } => {
                    let t = text.clone();
                    composer_sub.update(cx, |c, cx| c.sync_text(t, window, cx));
                }
            },
        );

        let store_app = store.clone();
        let app_sub = window.observe_window_appearance(move |window, cx| {
            if store_app.read(cx).theme_preference == ThemePreference::System {
                theme_setup::apply_theme_preference(ThemePreference::System, Some(window), cx);
            }
        });

        Self {
            store,
            focus_handle,
            scroller,
            composer,
            pending,
            sidebar_resizing: false,
            _subscriptions: vec![obs, ev_sub, app_sub],
        }
    }

    fn open_command_palette_with_store(store: Entity<AppStore>, window: &mut Window, cx: &mut App) {
        let command = cx.new(|cx| CommandState::new(window, cx));
        let initial_focus = Cell::new(true);
        let state = store.read(cx);
        let connected = state.connection == ConnectionState::Connected;
        let theme_label = format!("切换外观（当前：{}）", state.theme_preference.label());
        let mut actions = vec![vec![
            PaletteAction::NewSession,
            PaletteAction::AddProject,
            PaletteAction::ToggleSidebar,
            PaletteAction::ToggleTheme,
        ]];
        let mut groups = vec![
            CommandGroup::new()
                .label("操作")
                .item(CommandItem::new().label("新建会话").icon(IconName::Plus))
                .item(
                    CommandItem::new()
                        .label("添加项目…")
                        .icon(IconName::FolderOpen)
                        .disabled(!connected),
                )
                .item(
                    CommandItem::new()
                        .label("切换侧栏")
                        .icon(IconName::PanelLeft),
                )
                .item(CommandItem::new().label(theme_label).icon(IconName::Palette)),
        ];

        if let Some(session) = state
            .selected_id
            .as_ref()
            .and_then(|id| state.sessions.get(id))
            && session.parent_id.is_none()
        {
            actions.push(vec![
                PaletteAction::Compact(session.id.clone()),
                PaletteAction::Reload(session.id.clone()),
            ]);
            groups.push(
                CommandGroup::new()
                    .label("当前会话")
                    .item(
                        CommandItem::new()
                            .label("压缩当前会话上下文")
                            .disabled(!connected || state.is_busy()),
                    )
                    .item(
                        CommandItem::new()
                            .label("重新加载技能与 MCP")
                            .disabled(!connected),
                    ),
            );
        }

        let mut sessions: Vec<_> = state.sessions.values().collect();
        sessions.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        let mut session_actions = Vec::with_capacity(sessions.len());
        let mut session_items = Vec::with_capacity(sessions.len());
        for session in sessions {
            let title = if session.title.is_empty() {
                "新会话"
            } else {
                &session.title
            };
            let project = state.projects.iter().find(|p| p.id == session.project_id);
            let label =
                project.map_or_else(|| title.to_owned(), |p| format!("{title} · {}", p.name));
            session_actions.push(PaletteAction::OpenSession(session.id.clone()));
            session_items.push(CommandItem::new().label(label).icon(
                if session.parent_id.is_some() {
                    IconName::Bot
                } else {
                    IconName::SquareTerminal
                },
            ));
        }
        actions.push(session_actions);
        groups.push(CommandGroup::new().label("会话").items(session_items));
        let actions = Rc::new(actions);

        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                command.update(cx, |state, cx| state.focus(window, cx));
            }
            let store = store.clone();
            let actions = actions.clone();
            let palette = groups.iter().cloned().fold(
                Command::new(&command)
                    .bordered(false)
                    .placeholder("搜索会话、项目或命令…")
                    .max_h(px(320.0))
                    .empty(|_, _, cx| {
                        div()
                            .p_4()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("没有匹配的命令或会话")
                    })
                    .footer(|_, _, cx| {
                        h_flex()
                            .justify_end()
                            .gap_3()
                            .pt_2()
                            .border_t_1()
                            .border_color(theme_setup::hairline(cx))
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("↑↓ 选择")
                            .child("↵ 确认")
                    })
                    .on_confirm(move |index, window, cx| {
                        let Some(action) = actions
                            .get(index.section)
                            .and_then(|group| group.get(index.row))
                        else {
                            return;
                        };
                        window.close_dialog(cx);
                        if matches!(action, PaletteAction::ToggleTheme) {
                            let next = store.update(cx, |s, cx| s.cycle_theme_preference(cx));
                            theme_setup::apply_theme_preference(next, Some(window), cx);
                            return;
                        }
                        store.update(cx, |s, cx| match action {
                            PaletteAction::NewSession => s.start_new(None, cx),
                            PaletteAction::AddProject => s.add_project(cx),
                            PaletteAction::ToggleSidebar => s.toggle_sidebar(cx),
                            PaletteAction::Compact(id) => s.compact(id.clone(), cx),
                            PaletteAction::Reload(id) => s.reload(id.clone(), cx),
                            PaletteAction::OpenSession(id) => {
                                s.select_session(Some(id.clone()), cx)
                            }
                            PaletteAction::ToggleTheme => {}
                        });
                    }),
                Command::group,
            );
            dialog.title("命令面板").w(px(500.0)).child(palette)
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
                .px_6()
                .pb(px(56.0))
                .child(
                    v_flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .w(px(42.0))
                                .h(px(42.0))
                                .rounded(px(theme_setup::RADIUS_MD))
                                .bg(theme_setup::subtle_surface_bg(cx))
                                .border_1()
                                .border_color(theme_setup::card_border(cx))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::FolderOpen)
                                        .size(px(18.0))
                                        .text_color(theme_setup::accent_ember(cx)),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(22.0))
                                .font_weight(gpui_kit::FontWeight::NORMAL)
                                .text_color(cx.theme().foreground)
                                .child("选择一个本地项目开始"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Agent 会在该目录中阅读代码、修改文件并执行命令。"),
                        )
                        .child(
                            div().pt_2().child(
                                Button::new("welcome-add-project")
                                    .primary()
                                    .rounded(px(theme_setup::RADIUS_SM))
                                    .disabled(!connected)
                                    .icon(IconName::FolderOpen)
                                    .label("添加项目目录…")
                                    .on_click(move |_, _, cx| {
                                        st_add.update(cx, |s, cx| s.add_project(cx));
                                    }),
                            ),
                        ),
                )
                .into_any_element();
        }

        let Some(proj) = active_proj else {
            return div().flex_1().into_any_element();
        };
        let proj_path = proj.path.clone();
        let short_path = shorten_home_path(&proj_path);

        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .px_6()
            .pb(px(72.0))
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(680.0))
                    .gap(px(16.0))
                    .child(
                        h_flex()
                            .items_center()
                            .px(px(2.0))
                            .gap(px(9.0))
                            .child(
                                div()
                                    .size(px(7.0))
                                    .rounded_full()
                                    .bg(theme_setup::accent_ember(cx)),
                            )
                            .child(
                                div()
                                    .text_size(px(22.0))
                                    .font_weight(gpui_kit::FontWeight::NORMAL)
                                    .text_color(cx.theme().foreground)
                                    .child("要做点什么？"),
                            ),
                    )
                    .child(self.composer.clone())
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .px(px(4.0))
                            .child(
                                h_flex()
                                    .id("new-session-open-dir")
                                    .min_w_0()
                                    .items_center()
                                    .gap(px(5.0))
                                    .cursor_pointer()
                                    .text_color(cx.theme().muted_foreground)
                                    .hover(|s| s.text_color(cx.theme().foreground))
                                    .on_click(move |_, _, _| {
                                        let _ =
                                            crate::shell_bridge::reveal_whitelisted_path(&proj_path);
                                    })
                                    .child(
                                        Icon::new(IconName::Folder)
                                            .size(px(11.0))
                                            .text_color(cx.theme().muted_foreground.opacity(0.6)),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .text_size(px(12.0))
                                            .child(short_path),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .flex_shrink_0()
                                    .items_center()
                                    .gap(px(10.0))
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_size(px(12.0))
                                    .text_color(cx.theme().muted_foreground)
                                    .child("/ 命令")
                                    .child("@ 文件")
                                    .child("⌘K 面板"),
                            ),
                    ),
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
        let active_project_name = active_summary.as_ref().and_then(|s| {
            store
                .projects
                .iter()
                .find(|p| p.id == s.project_id)
                .map(|p| p.name.clone())
        });
        let parent_id = active_summary.as_ref().and_then(|s| s.parent_id.clone());
        let is_readonly_sub = parent_id.is_some();
        let conn = store.connection;
        let conn_detail = store.connection_detail.clone();
        let demo_mode = store.demo_mode;
        let muted_icon = cx.theme().foreground.opacity(0.85);

        let st_pal = self.store.clone();
        let open_palette: Rc<dyn Fn(&mut Window, &mut App)> = Rc::new(move |window, cx| {
            Self::open_command_palette_with_store(st_pal.clone(), window, cx);
        });

        let sidebar_el =
            sidebar_open.then(|| render_sidebar(&self.store, open_palette.clone(), window, cx));

        let st_toggle = self.store.clone();
        let st_new = self.store.clone();
        let st_back = self.store.clone();
        let st_reload = self.store.clone();

        let has_session = active_summary.is_some();
        let title_text = active_summary
            .as_ref()
            .map(|s| {
                if s.title.is_empty() {
                    "新会话".to_owned()
                } else {
                    s.title.clone()
                }
            })
            .unwrap_or_default();

        let workspace_bar = window_drag_bar("workspace-top-bar", WORKSPACE_BAR_HEIGHT, window, cx)
            .pl(px(if sidebar_open {
                12.0
            } else {
                WORKSPACE_TRAFFIC_LIGHT_INSET
            }))
            .pr(px(10.0))
            .when(has_session, |el| {
                el.border_b_1().border_color(theme_setup::hairline(cx))
            })
            .justify_between()
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(px(8.0))
                    .when(!sidebar_open, |el| {
                        el.child(
                            Button::new("ws-open-sb")
                                .ghost()
                                .xsmall()
                                .rounded(px(theme_setup::RADIUS_SM))
                                .icon(Icon::new(IconName::PanelLeft).text_color(muted_icon))
                                .tooltip("展开侧边栏 (⌘B)")
                                .on_click(move |_, _, cx| {
                                    st_toggle.update(cx, |s, cx| s.toggle_sidebar(cx));
                                }),
                        )
                        .child(
                            Button::new("ws-new-session")
                                .ghost()
                                .xsmall()
                                .rounded(px(theme_setup::RADIUS_SM))
                                .icon(Icon::new(IconName::Plus).text_color(muted_icon))
                                .tooltip("新建会话 (⌘N)")
                                .on_click(move |_, _, cx| {
                                    st_new.update(cx, |s, cx| s.start_new(None, cx));
                                }),
                        )
                    })
                    .when_some(parent_id.clone(), |el, pid| {
                        el.child(
                            Button::new("ws-back-parent")
                                .ghost()
                                .xsmall()
                                .rounded(px(theme_setup::RADIUS_SM))
                                .icon(Icon::new(IconName::ArrowLeft).text_color(muted_icon))
                                .tooltip("返回父会话")
                                .on_click(move |_, _, cx| {
                                    st_back.update(cx, |s, cx| {
                                        s.select_session(Some(pid.clone()), cx);
                                    });
                                }),
                        )
                    })
                    .when(has_session, |el| {
                        el.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(14.0))
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .text_color(cx.theme().foreground)
                                .child(title_text),
                        )
                        .when_some(active_project_name, |el, pname| {
                            el.child(
                                div()
                                    .max_w(px(140.0))
                                    .truncate()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_size(px(12.0))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(pname),
                            )
                        })
                        .when(is_readonly_sub, |el| {
                            el.child(pill_badge("只读子会话", BadgeTone::Muted, cx))
                        })
                    }),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(2.0))
                    .when(demo_mode, |el| {
                        el.child(pill_badge("演示模式", BadgeTone::Amber, cx))
                    })
                    .when_some(
                        selected_id.clone().filter(|_| !is_readonly_sub),
                        |el, sid| {
                            el.child(
                                Button::new("ws-reload")
                                    .ghost()
                                    .xsmall()
                                    .rounded(px(theme_setup::RADIUS_SM))
                                    .disabled(conn != ConnectionState::Connected)
                                    .icon(
                                        Icon::new(IconName::RefreshCw)
                                            .size(px(13.0))
                                            .text_color(muted_icon.opacity(0.8)),
                                    )
                                    .tooltip("重新加载技能与 MCP")
                                    .on_click(move |_, _, cx| {
                                        st_reload.update(cx, |s, cx| s.reload(sid.clone(), cx));
                                    }),
                            )
                        },
                    )
                    .when(!sidebar_open, |el| {
                        let pal = open_palette.clone();
                        el.child(
                            Button::new("ws-palette")
                                .ghost()
                                .xsmall()
                                .rounded(px(theme_setup::RADIUS_SM))
                                .icon(
                                    Icon::new(IconName::Search)
                                        .size(px(13.0))
                                        .text_color(muted_icon.opacity(0.8)),
                                )
                                .tooltip("搜索与命令 (⌘K)")
                                .on_click(move |_, window, cx| {
                                    pal(window, cx);
                                }),
                        )
                    }),
            );

        let conn_banner = (conn != ConnectionState::Connected).then(|| {
            let label = match conn {
                ConnectionState::Connecting => "正在连接 Lila 服务…",
                ConnectionState::Disconnected => "连接中断，正在重连…",
                ConnectionState::Incompatible => "Lila 服务版本不兼容",
                ConnectionState::Connected => "",
            };
            let st_retry = self.store.clone();
            let st_log = self.store.clone();
            div().px_3().pt_3().child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded(px(theme_setup::RADIUS_MD))
                    .border_1()
                    .border_color(theme_setup::attention_border(cx))
                    .bg(theme_setup::attention_bg(cx))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .flex_wrap()
                            .items_center()
                            .gap_x_2()
                            .child(div().text_sm().child(label))
                            .when_some(conn_detail, |el, detail| {
                                el.child(
                                    div()
                                        .min_w_0()
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_size(px(12.0))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(detail),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap(px(4.0))
                            .child(
                                Button::new("conn-retry")
                                    .ghost()
                                    .xsmall()
                                    .rounded(px(theme_setup::RADIUS_SM))
                                    .label("立即重试")
                                    .on_click(move |_, _, cx| {
                                        st_retry.read(cx).reconnect_now();
                                    }),
                            )
                            .child(
                                Button::new("conn-log")
                                    .ghost()
                                    .xsmall()
                                    .rounded(px(theme_setup::RADIUS_SM))
                                    .label("查看日志")
                                    .on_click(move |_, _, cx| {
                                        st_log.update(cx, |s, cx| s.open_core_log(cx));
                                    }),
                            ),
                    ),
            )
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
                        .px_6()
                        .pb(px(14.0))
                        .pt(px(8.0))
                        .child(
                            v_flex()
                                .w_full()
                                .max_w(px(theme_setup::CONTENT_MAX_W))
                                .gap(px(8.0))
                                .child(self.pending.clone())
                                .child(if is_readonly_sub {
                                    h_flex()
                                        .w_full()
                                        .px_4()
                                        .py(px(10.0))
                                        .rounded(px(theme_setup::RADIUS_LG))
                                        .border_1()
                                        .border_color(theme_setup::composer_border(cx))
                                        .bg(theme_setup::composer_bg(cx))
                                        .items_center()
                                        .justify_between()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child("可从此子会话创建独立会话，继续对话。"),
                                        )
                                        .child(
                                            Button::new("fork-subsession")
                                                .primary()
                                                .xsmall()
                                                .rounded(px(theme_setup::RADIUS_SM))
                                                .label("继续对话")
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

        let resizing = self.sidebar_resizing;
        let resize_sash = sidebar_open.then(|| {
            div()
                .id("sidebar-resize-sash")
                .w(px(SHELL_INSET))
                .h_full()
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .cursor_col_resize()
                .group("sash-grp")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.sidebar_resizing = true;
                        cx.notify();
                    }),
                )
                .on_double_click(cx.listener(|this, _, _, cx| {
                    this.sidebar_resizing = false;
                    this.store.update(cx, |s, cx| {
                        s.set_sidebar_width(theme_setup::SIDEBAR_DEFAULT_W, true, cx);
                    });
                }))
                .child(
                    div()
                        .w(px(2.0))
                        .h(px(36.0))
                        .rounded_full()
                        .when(resizing, |el| el.bg(theme_setup::accent_ember(cx)))
                        .when(!resizing, |el| {
                            el.opacity(0.0)
                                .bg(theme_setup::hairline(cx))
                                .group_hover("sash-grp", |s| s.opacity(1.0))
                        }),
                )
        });

        h_flex()
            .id("lila-root")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme_setup::shell_bg(cx))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if !this.sidebar_resizing {
                    return;
                }
                if event.pressed_button != Some(MouseButton::Left) {
                    this.sidebar_resizing = false;
                    let cur = this.store.read(cx).sidebar_width;
                    this.store
                        .update(cx, |s, cx| s.set_sidebar_width(cur, true, cx));
                    return;
                }
                let next_w = f32::from(event.position.x);
                this.store
                    .update(cx, |s, cx| s.set_sidebar_width(next_w, false, cx));
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.sidebar_resizing {
                        this.sidebar_resizing = false;
                        let cur = this.store.read(cx).sidebar_width;
                        this.store
                            .update(cx, |s, cx| s.set_sidebar_width(cur, true, cx));
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.sidebar_resizing {
                        this.sidebar_resizing = false;
                        let cur = this.store.read(cx).sidebar_width;
                        this.store
                            .update(cx, |s, cx| s.set_sidebar_width(cur, true, cx));
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &NewSession, _, cx| {
                this.store.update(cx, |s, cx| s.start_new(None, cx));
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.store.update(cx, |s, cx| s.toggle_sidebar(cx));
            }))
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| {
                Self::open_command_palette_with_store(this.store.clone(), window, cx);
            }))
            .on_action(cx.listener(|this, _: &AbortRun, _, cx| {
                let sid = {
                    let st = this.store.read(cx);
                    if st.is_busy() {
                        st.selected_id.clone()
                    } else {
                        None
                    }
                };
                if let Some(sid) = sid {
                    this.store.update(cx, |s, cx| s.abort(sid, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &PrevSession, _, cx| {
                this.store
                    .update(cx, |s, cx| s.select_adjacent_session(-1, cx));
            }))
            .on_action(cx.listener(|this, _: &NextSession, _, cx| {
                this.store
                    .update(cx, |s, cx| s.select_adjacent_session(1, cx));
            }))
            .when_some(sidebar_el, |el, sidebar| el.child(sidebar))
            .when_some(resize_sash, |el, sash| el.child(sash))
            .child(
                v_flex()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .py(px(SHELL_INSET))
                    .pr(px(SHELL_INSET))
                    .when(!sidebar_open, |el| el.pl(px(SHELL_INSET)))
                    .child(
                        workspace_card(cx)
                            .flex_1()
                            .min_h_0()
                            .child(workspace_bar)
                            .when_some(conn_banner, |el, banner| el.child(banner))
                            .child(workspace_content),
                    ),
            )
    }
}
