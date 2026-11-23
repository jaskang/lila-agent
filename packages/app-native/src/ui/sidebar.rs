//! Left frosted sidebar: project-grouped collapsible session tree, sub-agent nesting,
//! rename/delete/remove-project native dialogs, and core status + RSS footer.

use std::cell::Cell;
use std::collections::HashMap;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Entity, Focusable as _, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px, rgb,
};

use crate::format::format_short_time;
use crate::protocol::{ConnectionState, Project, SessionSummary};
use crate::store::AppStore;
use crate::ui::{status_dot, theme_setup};

pub fn render_sidebar(
    store_entity: &Entity<AppStore>,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<> {
    let store = store_entity.read(cx);
    let projects = store.projects.clone();
    let active_id = store.selected_id.clone();
    let collapsed_projects = store.collapsed_projects.clone();
    let expanded_parents = store.expanded_parents.clone();
    let connection = store.connection;
    let connection_detail = store.connection_detail.clone();
    let core_version = store.core_version.clone();
    let memory_mb = store.memory_mb;

    let mut by_project: HashMap<String, Vec<SessionSummary>> = HashMap::new();
    let mut children_map: HashMap<String, Vec<SessionSummary>> = HashMap::new();
    for s in store.sessions.values() {
        if let Some(pid) = &s.parent_id {
            children_map.entry(pid.clone()).or_default().push(s.clone());
        } else {
            by_project
                .entry(s.project_id.clone())
                .or_default()
                .push(s.clone());
        }
    }
    for list in by_project.values_mut() {
        list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    }
    for list in children_map.values_mut() {
        list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    }

    let st_new = store_entity.clone();
    let st_add = store_entity.clone();

    let header = h_flex()
        .h(px(40.0))
        .px_3()
        .items_center()
        .justify_between()
        .border_b_1()
        .border_color(theme_setup::card_border(cx))
        .child(
            div()
                .text_xs()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .text_color(cx.theme().foreground)
                .child("项目与会话"),
        )
        .child(
            h_flex()
                .items_center()
                .gap_1()
                .child(
                    Button::new("sb-new-session")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Plus)
                        .label("新会话")
                        .on_click(move |_, _, cx| {
                            st_new.update(cx, |s, cx| s.start_new(None, cx));
                        }),
                )
                .child(
                    Button::new("sb-add-project")
                        .ghost()
                        .xsmall()
                        .icon(IconName::FolderOpen)
                        .on_click(move |_, _, cx| {
                            st_add.update(cx, |s, cx| s.add_project(cx));
                        }),
                ),
        );

    let mut tree = v_flex().w_full().p_2().gap_3();

    if projects.is_empty() {
        let st_empty = store_entity.clone();
        tree = tree.child(
            v_flex()
                .items_center()
                .gap_2()
                .p_5()
                .rounded_xl()
                .border_1()
                .border_color(theme_setup::card_border(cx))
                .bg(theme_setup::card_bg(cx))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("还没有项目目录"),
                )
                .child(
                    Button::new("sb-empty-add-project")
                        .primary()
                        .xsmall()
                        .label("添加项目目录…")
                        .on_click(move |_, _, cx| {
                            st_empty.update(cx, |s, cx| s.add_project(cx));
                        }),
                ),
        );
    } else {
        for project in projects {
            let is_open = !collapsed_projects.contains(&project.id);
            let project_sessions = by_project.remove(&project.id).unwrap_or_default();

            let mut sess_list = v_flex().gap(px(2.0)).pl_1();
            if project_sessions.is_empty() {
                sess_list = sess_list.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(px(11.0))
                        .text_color(cx.theme().muted_foreground.opacity(0.75))
                        .child("暂无会话"),
                );
            } else {
                for sess in project_sessions {
                    let is_active = active_id.as_deref() == Some(sess.id.as_str());
                    let subs = children_map.remove(&sess.id).unwrap_or_default();
                    let subs_open = expanded_parents.contains(&sess.id);

                    let title_str = if sess.title.is_empty() {
                        "新会话".to_owned()
                    } else {
                        sess.title.clone()
                    };
                    let time_str = format_short_time(&sess.updated_at);

                    let st_sel = store_entity.clone();
                    let sid_sel = sess.id.clone();

                    let mut row = h_flex()
                        .id(SharedString::from(format!("sess-row-{}", sess.id)))
                        .w_full()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .py(px(6.0))
                        .rounded_xl()
                        .border_1()
                        .border_color(if is_active {
                            theme_setup::glass_rim_border(cx)
                        } else {
                            gpui_kit::transparent_black()
                        })
                        .when(is_active, |el| {
                            el.bg(theme_setup::card_bg(cx)).shadow_xs()
                        })
                        .when(!is_active, |el| {
                            el.hover(|s| s.bg(cx.theme().list_hover))
                        })
                        .child(
                            h_flex()
                                .id(SharedString::from(format!("sess-btn-{}", sess.id)))
                                .flex_1()
                                .min_w_0()
                                .items_center()
                                .gap_2()
                                .cursor_pointer()
                                .on_click(move |_, _, cx| {
                                    st_sel.update(cx, |s, cx| {
                                        s.select_session(Some(sid_sel.clone()), cx);
                                    });
                                })
                                .child(status_dot(sess.status, cx))
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .text_size(px(12.0))
                                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                                .text_color(cx.theme().foreground)
                                                .truncate()
                                                .child(title_str),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(10.0))
                                                .text_color(cx.theme().muted_foreground.opacity(0.85))
                                                .child(time_str),
                                        ),
                                ),
                        );

                    if sess.pending_count > 0 {
                        row = row.child(
                            div()
                                .px(px(6.0))
                                .py(px(1.0))
                                .rounded_full()
                                .bg(rgb(0xf59e0b))
                                .text_color(gpui_kit::white())
                                .text_size(px(10.0))
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(sess.pending_count.to_string()),
                        );
                    }

                    if !subs.is_empty() {
                        let st_sub = store_entity.clone();
                        let sid_sub = sess.id.clone();
                        let label = if subs_open {
                            format!("▾{}", subs.len())
                        } else {
                            format!("▸{}", subs.len())
                        };
                        row = row.child(
                            Button::new(SharedString::from(format!("sub-tog-{}", sess.id)))
                                .ghost()
                                .xsmall()
                                .label(label)
                                .on_click(move |_, _, cx| {
                                    st_sub.update(cx, |s, cx| {
                                        s.toggle_parent_expanded(sid_sub.clone(), cx);
                                    });
                                }),
                        );
                    }

                    if is_active {
                        let st_ren = store_entity.clone();
                        let sess_ren = sess.clone();
                        let st_del = store_entity.clone();
                        let sess_del = sess.clone();

                        row = row
                            .child(
                                Button::new(SharedString::from(format!("ren-{}", sess.id)))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Replace)
                                    .on_click(move |_, window, cx| {
                                        open_rename_dialog(st_ren.clone(), sess_ren.clone(), window, cx);
                                    }),
                            )
                            .child(
                                Button::new(SharedString::from(format!("del-{}", sess.id)))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Delete)
                                    .on_click(move |_, window, cx| {
                                        open_delete_dialog(st_del.clone(), sess_del.clone(), window, cx);
                                    }),
                            );
                    }

                    let mut group = v_flex().gap(px(2.0)).child(row);
                    if subs_open && !subs.is_empty() {
                        let mut sub_col = v_flex()
                            .pl_4()
                            .ml_2()
                            .border_l_1()
                            .border_color(theme_setup::card_border(cx))
                            .gap(px(2.0));
                        for child in subs {
                            let child_active = active_id.as_deref() == Some(child.id.as_str());
                            let st_c = store_entity.clone();
                            let cid = child.id.clone();
                            let ctitle = if child.title.is_empty() {
                                "子代理会话".to_owned()
                            } else {
                                child.title.clone()
                            };
                            sub_col = sub_col.child(
                                h_flex()
                                    .id(SharedString::from(format!("sub-row-{}", child.id)))
                                    .w_full()
                                    .items_center()
                                    .gap_1()
                                    .px_2()
                                    .py_1()
                                    .rounded_lg()
                                    .cursor_pointer()
                                    .when(child_active, |el| el.bg(theme_setup::card_bg(cx)))
                                    .when(!child_active, |el| {
                                        el.hover(|s| s.bg(cx.theme().list_hover))
                                    })
                                    .on_click(move |_, _, cx| {
                                        st_c.update(cx, |s, cx| {
                                            s.select_session(Some(cid.clone()), cx);
                                        });
                                    })
                                    .child(status_dot(child.status, cx))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(11.0))
                                            .text_color(cx.theme().muted_foreground)
                                            .child(ctitle),
                                    ),
                            );
                        }
                        group = group.child(sub_col);
                    }

                    sess_list = sess_list.child(group);
                }
            }

            let st_tog = store_entity.clone();
            let pid_tog = project.id.clone();
            let st_pnew = store_entity.clone();
            let pid_new = project.id.clone();
            let st_prm = store_entity.clone();
            let proj_rm = project.clone();

            let proj_header = h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .px_1()
                .py(px(2.0))
                .child(
                    h_flex()
                        .id(SharedString::from(format!("proj-tog-{}", project.id)))
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap_1()
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            st_tog.update(cx, |s, cx| {
                                s.toggle_project_collapsed(pid_tog.clone(), cx);
                            });
                        })
                        .child(
                            div()
                                .text_size(px(10.0))
                                .text_color(cx.theme().muted_foreground)
                                .child(if is_open { "▾" } else { "▸" }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(11.0))
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .text_color(cx.theme().foreground.opacity(0.85))
                                .child(project.name.clone()),
                        ),
                )
                .child(
                    h_flex()
                        .items_center()
                        .gap(px(2.0))
                        .child(
                            Button::new(SharedString::from(format!("proj-new-{}", project.id)))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Plus)
                                .on_click(move |_, _, cx| {
                                    st_pnew.update(cx, |s, cx| {
                                        s.start_new(Some(pid_new.clone()), cx);
                                    });
                                }),
                        )
                        .child(
                            Button::new(SharedString::from(format!("proj-rm-{}", project.id)))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .on_click(move |_, window, cx| {
                                    open_remove_project_dialog(
                                        st_prm.clone(),
                                        proj_rm.clone(),
                                        window,
                                        cx,
                                    );
                                }),
                        ),
                );

            tree = tree.child(
                Collapsible::new()
                    .open(is_open)
                    .gap_1()
                    .child(proj_header)
                    .content(sess_list),
            );
        }
    }

    let (conn_color, conn_text) = match connection {
        ConnectionState::Connected => (
            rgb(0x10b981),
            if core_version.is_empty() {
                "core 已连接".to_owned()
            } else {
                format!("core v{core_version}")
            },
        ),
        ConnectionState::Connecting => (rgb(0xf59e0b), "正在连接 core…".to_owned()),
        ConnectionState::Incompatible => (rgb(0xef4444), "协议版本不匹配".to_owned()),
        ConnectionState::Disconnected => (
            rgb(0xef4444),
            connection_detail
                .map(|d| format!("已断开：{d}"))
                .unwrap_or_else(|| "已断开，重连中…".to_owned()),
        ),
    };

    let footer = h_flex()
        .items_center()
        .justify_between()
        .border_t_1()
        .border_color(theme_setup::card_border(cx))
        .px_3()
        .py_2()
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(px(6.0))
                .child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(conn_color))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(10.0))
                        .text_color(cx.theme().muted_foreground)
                        .child(conn_text),
                ),
        )
        .child(
            div()
                .text_size(px(10.0))
                .text_color(cx.theme().muted_foreground.opacity(0.8))
                .child(format!("{memory_mb:.0} MB")),
        );

    let _ = window;
    v_flex()
        .w(px(264.0))
        .h_full()
        .rounded_2xl()
        .border_1()
        .border_color(theme_setup::glass_rim_border(cx))
        .bg(cx.theme().sidebar)
        .shadow_md()
        .overflow_hidden()
        .child(header)
        .child(
            div()
                .id("sidebar-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .child(tree),
        )
        .child(footer)
}

fn open_rename_dialog(
    store: Entity<AppStore>,
    sess: SessionSummary,
    window: &mut Window,
    cx: &mut App,
) {
    let input_state = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder("输入新标题…")
            .default_value(sess.title.clone())
    });
    let initial_focus = Cell::new(true);
    let sid = sess.id.clone();

    window.open_dialog(cx, move |dialog, window, cx| {
        if initial_focus.replace(false) {
            input_state.focus_handle(cx).focus(window, cx);
        }
        let st_ok = store.clone();
        let inp_ok = input_state.clone();
        let sid_ok = sid.clone();

        dialog
            .title("重命名会话")
            .child(Input::new(&input_state))
            .footer(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("rename-cancel")
                            .ghost()
                            .label("取消")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("rename-save")
                            .primary()
                            .label("保存")
                            .on_click(move |_, window, cx| {
                                let val = inp_ok.read(cx).value().to_string();
                                st_ok.update(cx, |s, cx| s.rename_session(sid_ok.clone(), val, cx));
                                window.close_dialog(cx);
                            }),
                    ),
            )
    });
}

fn open_delete_dialog(
    store: Entity<AppStore>,
    sess: SessionSummary,
    window: &mut Window,
    cx: &mut App,
) {
    let title = if sess.title.is_empty() {
        "新会话".to_owned()
    } else {
        sess.title.clone()
    };
    let sid = sess.id.clone();

    window.open_dialog(cx, move |dialog, _, cx| {
        let st_do = store.clone();
        let sid_do = sid.clone();
        dialog
            .title("删除会话？")
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("「{title}」将被移入回收目录（trash/）。")),
            )
            .footer(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("del-cancel")
                            .ghost()
                            .label("取消")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("del-confirm")
                            .danger()
                            .label("删除")
                            .on_click(move |_, window, cx| {
                                st_do.update(cx, |s, cx| s.delete_session(sid_do.clone(), cx));
                                window.close_dialog(cx);
                            }),
                    ),
            )
    });
}

fn open_remove_project_dialog(
    store: Entity<AppStore>,
    project: Project,
    window: &mut Window,
    cx: &mut App,
) {
    let name = project.name.clone();
    let pid = project.id.clone();

    window.open_dialog(cx, move |dialog, _, cx| {
        let st_do = store.clone();
        let pid_do = pid.clone();
        dialog
            .title("移除项目？")
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "将从 Lila 列表中移除「{name}」，本地代码与 Pi 会话文件不会被删除。"
                    )),
            )
            .footer(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("rm-proj-cancel")
                            .ghost()
                            .label("取消")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("rm-proj-confirm")
                            .danger()
                            .label("移除")
                            .on_click(move |_, window, cx| {
                                st_do.update(cx, |s, cx| s.remove_project(pid_do.clone(), cx));
                                window.close_dialog(cx);
                            }),
                    ),
            )
    });
}
