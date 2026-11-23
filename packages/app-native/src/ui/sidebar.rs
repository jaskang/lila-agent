//! Full-height macOS translucent sidebar: unified traffic-light top chrome,
//! quiet project groups with hover-only actions, liquid-glass pill session rows
//! with context/dropdown menus, and a minimal icon-only footer.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    Anchor, App, AppContext as _, Entity, Focusable as _, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px, rgb,
};

use crate::protocol::{ConnectionState, Project, SessionSummary};
use crate::store::AppStore;
use crate::ui::{TRAFFIC_LIGHT_INSET, status_dot, theme_setup, window_drag_bar};

pub fn render_sidebar(
    store_entity: &Entity<AppStore>,
    on_open_palette: Rc<dyn Fn(&mut Window, &mut App)>,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<> {
    let store = store_entity.read(cx);
    let projects = store.projects.clone();
    let active_id = store.selected_id.clone();
    let collapsed_projects = store.collapsed_projects.clone();
    let expanded_parents = store.expanded_parents.clone();
    let connection = store.connection;
    let is_dark = cx.theme().is_dark();

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

    let st_toggle = store_entity.clone();
    let st_new = store_entity.clone();
    let muted_icon = cx.theme().muted_foreground;

    // Unified top chrome aligned with macOS traffic lights: icon-only, zero visual weight.
    let top_bar = window_drag_bar("sidebar-top-bar", window, cx)
        .pl(px(TRAFFIC_LIGHT_INSET))
        .pr_2()
        .justify_between()
        .child(
            Button::new("sb-toggle-collapse")
                .ghost()
                .xsmall()
                .icon(Icon::new(IconName::PanelLeft).text_color(muted_icon))
                .tooltip("收起侧边栏 (⌘B)")
                .on_click(move |_, _, cx| {
                    st_toggle.update(cx, |s, cx| s.toggle_sidebar(cx));
                }),
        )
        .child(
            h_flex()
                .items_center()
                .gap(px(2.0))
                .child(
                    Button::new("sb-open-palette")
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(IconName::Search).text_color(muted_icon))
                        .tooltip("搜索与命令 (⌘K)")
                        .on_click(move |_, window, cx| {
                            on_open_palette(window, cx);
                        }),
                )
                .child(
                    Button::new("sb-new-session")
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(IconName::Plus).text_color(muted_icon))
                        .tooltip("新建对话 (⌘N)")
                        .on_click(move |_, _, cx| {
                            st_new.update(cx, |s, cx| s.start_new(None, cx));
                        }),
                ),
        );

    let mut tree = v_flex().w_full().px_2().pt_1().pb_2().gap(px(14.0));

    if projects.is_empty() {
        let st_empty = store_entity.clone();
        tree = tree.child(
            v_flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_8()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground.opacity(0.8))
                        .child("暂无项目"),
                )
                .child(
                    Button::new("sb-empty-add-project")
                        .ghost()
                        .xsmall()
                        .icon(IconName::FolderOpen)
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

            let mut sess_list = v_flex().gap(px(2.0));
            if project_sessions.is_empty() {
                sess_list = sess_list.child(
                    div()
                        .px(px(10.0))
                        .py_1()
                        .text_size(px(11.0))
                        .text_color(cx.theme().muted_foreground.opacity(0.55))
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

                    let st_sel = store_entity.clone();
                    let sid_sel = sess.id.clone();
                    let sess_group: SharedString = format!("sess-grp-{}", sess.id).into();

                    let leading_icon = if let Some(dot) = status_dot(sess.status, cx) {
                        div()
                            .w(px(14.0))
                            .h(px(14.0))
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .justify_center()
                            .child(dot)
                    } else {
                        div()
                            .w(px(14.0))
                            .h(px(14.0))
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(IconName::SquareTerminal)
                                    .size(px(13.0))
                                    .text_color(if is_active {
                                        cx.theme().foreground.opacity(0.85)
                                    } else {
                                        cx.theme().muted_foreground.opacity(0.65)
                                    }),
                            )
                    };

                    let st_ctx_ren = store_entity.clone();
                    let sess_ctx_ren = sess.clone();
                    let st_ctx_del = store_entity.clone();
                    let sess_ctx_del = sess.clone();

                    let st_dd_ren = store_entity.clone();
                    let sess_dd_ren = sess.clone();
                    let st_dd_del = store_entity.clone();
                    let sess_dd_del = sess.clone();

                    let mut row = h_flex()
                        .id(SharedString::from(format!("sess-row-{}", sess.id)))
                        .group(sess_group.clone())
                        .w_full()
                        .h(px(30.0))
                        .items_center()
                        .gap(px(4.0))
                        .pl(px(9.0))
                        .pr(px(4.0))
                        .rounded_full()
                        .border_1()
                        .border_color(if is_active {
                            theme_setup::pill_active_border(cx)
                        } else {
                            gpui_kit::transparent_black()
                        })
                        .when(is_active, |el| el.bg(theme_setup::pill_active_bg(cx)))
                        .when(!is_active, |el| {
                            el.hover(|s| s.bg(cx.theme().list_hover))
                        })
                        .child(
                            h_flex()
                                .id(SharedString::from(format!("sess-btn-{}", sess.id)))
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .items_center()
                                .gap(px(8.0))
                                .cursor_pointer()
                                .on_click(move |_, _, cx| {
                                    st_sel.update(cx, |s, cx| {
                                        s.select_session(Some(sid_sel.clone()), cx);
                                    });
                                })
                                .child(leading_icon)
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(12.5))
                                        .font_weight(if is_active {
                                            gpui_kit::FontWeight::MEDIUM
                                        } else {
                                            gpui_kit::FontWeight::NORMAL
                                        })
                                        .text_color(if is_active {
                                            cx.theme().foreground
                                        } else {
                                            cx.theme().foreground.opacity(0.82)
                                        })
                                        .child(title_str),
                                ),
                        );

                    if sess.pending_count > 0 {
                        row = row.child(
                            div()
                                .px(px(5.0))
                                .py(px(1.0))
                                .rounded_full()
                                .bg(rgb(0xf59e0b))
                                .text_color(gpui_kit::white())
                                .text_size(px(9.5))
                                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                .child(sess.pending_count.to_string()),
                        );
                    }

                    if !subs.is_empty() {
                        let st_sub = store_entity.clone();
                        let sid_sub = sess.id.clone();
                        let chevron = if subs_open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        };
                        row = row.child(
                            h_flex()
                                .id(SharedString::from(format!("sub-tog-{}", sess.id)))
                                .items_center()
                                .gap(px(2.0))
                                .px(px(5.0))
                                .py(px(2.0))
                                .rounded_full()
                                .cursor_pointer()
                                .text_size(px(10.0))
                                .text_color(cx.theme().muted_foreground)
                                .hover(|s| {
                                    s.bg(cx.theme().accent).text_color(cx.theme().foreground)
                                })
                                .on_click(move |_, _, cx| {
                                    st_sub.update(cx, |s, cx| {
                                        s.toggle_parent_expanded(sid_sub.clone(), cx);
                                    });
                                })
                                .child(Icon::new(chevron).size(px(10.0)))
                                .child(subs.len().to_string()),
                        );
                    }

                    let more_btn = Button::new(SharedString::from(format!("sess-more-{}", sess.id)))
                        .ghost()
                        .xsmall()
                        .compact()
                        .rounded(px(999.0))
                        .icon(Icon::new(IconName::Ellipsis).size(px(13.0)).text_color(muted_icon))
                        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                            let st_r = st_dd_ren.clone();
                            let sr = sess_dd_ren.clone();
                            let st_d = st_dd_del.clone();
                            let sd = sess_dd_del.clone();
                            menu.item(
                                PopupMenuItem::new("重命名")
                                    .icon(IconName::Replace)
                                    .on_click(move |_, window, cx| {
                                        open_rename_dialog(st_r.clone(), sr.clone(), window, cx);
                                    }),
                            )
                            .separator()
                            .item(
                                PopupMenuItem::new("删除会话")
                                    .icon(IconName::Delete)
                                    .on_click(move |_, window, cx| {
                                        open_delete_dialog(st_d.clone(), sd.clone(), window, cx);
                                    }),
                            )
                        });

                    row = row.child(
                        div()
                            .opacity(0.0)
                            .group_hover(sess_group, |s| s.opacity(1.0))
                            .child(more_btn),
                    );

                    let row_with_ctx = row.context_menu(move |menu, _, _| {
                        let st_r = st_ctx_ren.clone();
                        let sr = sess_ctx_ren.clone();
                        let st_d = st_ctx_del.clone();
                        let sd = sess_ctx_del.clone();
                        menu.item(
                            PopupMenuItem::new("重命名")
                                .icon(IconName::Replace)
                                .on_click(move |_, window, cx| {
                                    open_rename_dialog(st_r.clone(), sr.clone(), window, cx);
                                }),
                        )
                        .separator()
                        .item(
                            PopupMenuItem::new("删除会话")
                                .icon(IconName::Delete)
                                .on_click(move |_, window, cx| {
                                    open_delete_dialog(st_d.clone(), sd.clone(), window, cx);
                                }),
                        )
                    });

                    let mut group = v_flex().gap(px(2.0)).child(row_with_ctx);
                    if subs_open && !subs.is_empty() {
                        let mut sub_col = v_flex()
                            .pl_3()
                            .ml(px(14.0))
                            .border_l_1()
                            .border_color(theme_setup::hairline(cx))
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
                                    .h(px(26.0))
                                    .items_center()
                                    .gap(px(6.0))
                                    .px_2()
                                    .rounded_full()
                                    .cursor_pointer()
                                    .when(child_active, |el| {
                                        el.bg(theme_setup::pill_active_bg(cx))
                                    })
                                    .when(!child_active, |el| {
                                        el.hover(|s| s.bg(cx.theme().list_hover))
                                    })
                                    .on_click(move |_, _, cx| {
                                        st_c.update(cx, |s, cx| {
                                            s.select_session(Some(cid.clone()), cx);
                                        });
                                    })
                                    .child(if let Some(dot) = status_dot(child.status, cx) {
                                        dot.into_any_element()
                                    } else {
                                        Icon::new(IconName::Bot)
                                            .size(px(11.0))
                                            .text_color(cx.theme().muted_foreground.opacity(0.65))
                                            .into_any_element()
                                    })
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(11.5))
                                            .text_color(if child_active {
                                                cx.theme().foreground
                                            } else {
                                                cx.theme().muted_foreground
                                            })
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
            let st_pnew_ctx = store_entity.clone();
            let pid_new_ctx = project.id.clone();
            let st_prm_dd = store_entity.clone();
            let proj_rm_dd = project.clone();
            let st_prm_ctx = store_entity.clone();
            let proj_rm_ctx = project.clone();
            let proj_path_dd = project.path.clone();
            let proj_path_ctx = project.path.clone();
            let proj_group: SharedString = format!("proj-grp-{}", project.id).into();

            let proj_more = Button::new(SharedString::from(format!("proj-more-{}", project.id)))
                .ghost()
                .xsmall()
                .compact()
                .rounded(px(999.0))
                .icon(Icon::new(IconName::Ellipsis).size(px(12.0)).text_color(muted_icon))
                .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                    let st_r = st_prm_dd.clone();
                    let pr = proj_rm_dd.clone();
                    let pp = proj_path_dd.clone();
                    menu.item(
                        PopupMenuItem::new("在访达中显示")
                            .icon(IconName::FolderOpen)
                            .on_click(move |_, _, _| {
                                let _ = crate::shell_bridge::reveal_whitelisted_path(&pp);
                            }),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("移除项目")
                            .icon(IconName::Delete)
                            .on_click(move |_, window, cx| {
                                open_remove_project_dialog(st_r.clone(), pr.clone(), window, cx);
                            }),
                    )
                });

            let proj_header = h_flex()
                .id(SharedString::from(format!("proj-hdr-{}", project.id)))
                .group(proj_group.clone())
                .w_full()
                .h(px(24.0))
                .items_center()
                .justify_between()
                .pl(px(9.0))
                .pr(px(2.0))
                .child(
                    h_flex()
                        .id(SharedString::from(format!("proj-tog-{}", project.id)))
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap(px(4.0))
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            st_tog.update(cx, |s, cx| {
                                s.toggle_project_collapsed(pid_tog.clone(), cx);
                            });
                        })
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(11.0))
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .text_color(cx.theme().muted_foreground.opacity(0.82))
                                .child(project.name.clone()),
                        )
                        .when(!is_open, |el| {
                            el.child(
                                Icon::new(IconName::ChevronRight)
                                    .size(px(10.0))
                                    .text_color(cx.theme().muted_foreground.opacity(0.6)),
                            )
                        }),
                )
                .child(
                    h_flex()
                        .items_center()
                        .gap(px(1.0))
                        .opacity(0.0)
                        .group_hover(proj_group, |s| s.opacity(1.0))
                        .child(
                            Button::new(SharedString::from(format!("proj-new-{}", project.id)))
                                .ghost()
                                .xsmall()
                                .compact()
                                .rounded(px(999.0))
                                .icon(
                                    Icon::new(IconName::Plus)
                                        .size(px(12.0))
                                        .text_color(muted_icon),
                                )
                                .tooltip("在此项目新建会话")
                                .on_click(move |_, _, cx| {
                                    st_pnew.update(cx, |s, cx| {
                                        s.start_new(Some(pid_new.clone()), cx);
                                    });
                                }),
                        )
                        .child(proj_more),
                )
                .context_menu(move |menu, _, _| {
                    let st_n = st_pnew_ctx.clone();
                    let pn = pid_new_ctx.clone();
                    let st_r = st_prm_ctx.clone();
                    let pr = proj_rm_ctx.clone();
                    let pp = proj_path_ctx.clone();
                    menu.item(
                        PopupMenuItem::new("新建会话")
                            .icon(IconName::Plus)
                            .on_click(move |_, _, cx| {
                                st_n.update(cx, |s, cx| s.start_new(Some(pn.clone()), cx));
                            }),
                    )
                    .item(
                        PopupMenuItem::new("在访达中显示")
                            .icon(IconName::FolderOpen)
                            .on_click(move |_, _, _| {
                                let _ = crate::shell_bridge::reveal_whitelisted_path(&pp);
                            }),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("移除项目")
                            .icon(IconName::Delete)
                            .on_click(move |_, window, cx| {
                                open_remove_project_dialog(st_r.clone(), pr.clone(), window, cx);
                            }),
                    )
                });

            tree = tree.child(
                Collapsible::new()
                    .open(is_open)
                    .gap(px(3.0))
                    .child(proj_header)
                    .content(sess_list),
            );
        }
    }

    let st_add_footer = store_entity.clone();
    let conn_indicator = (connection != ConnectionState::Connected).then(|| {
        let (dot_color, label) = match connection {
            ConnectionState::Connecting => (rgb(0xf59e0b), "连接中…"),
            ConnectionState::Incompatible => (rgb(0xef4444), "版本不匹配"),
            ConnectionState::Disconnected => (rgb(0xef4444), "重连中…"),
            ConnectionState::Connected => (rgb(0x30d158), ""),
        };
        h_flex()
            .items_center()
            .gap(px(5.0))
            .pl_1()
            .child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(dot_color))
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(cx.theme().muted_foreground)
                    .child(label),
            )
    });

    // Whisper-quiet bottom utility row matching native macOS minimalism.
    let footer = h_flex()
        .h(px(36.0))
        .flex_shrink_0()
        .items_center()
        .justify_between()
        .px_2()
        .child(
            h_flex()
                .items_center()
                .gap(px(4.0))
                .child(
                    Button::new("sb-footer-add-project")
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(IconName::FolderOpen).text_color(muted_icon))
                        .tooltip("添加本地项目…")
                        .on_click(move |_, _, cx| {
                            st_add_footer.update(cx, |s, cx| s.add_project(cx));
                        }),
                )
                .when_some(conn_indicator, |el, ind| el.child(ind)),
        )
        .child(
            Button::new("sb-footer-theme")
                .ghost()
                .xsmall()
                .icon(
                    Icon::new(if is_dark { IconName::Sun } else { IconName::Moon })
                        .text_color(muted_icon),
                )
                .tooltip(if is_dark {
                    "切换到浅色外观"
                } else {
                    "切换到深色外观"
                })
                .on_click(|_, window, cx| {
                    theme_setup::toggle_theme_mode(window, cx);
                }),
        );

    v_flex()
        .w(px(244.0))
        .h_full()
        .flex_shrink_0()
        .bg(cx.theme().sidebar)
        .border_r_1()
        .border_color(theme_setup::hairline(cx))
        .overflow_hidden()
        .child(top_bar)
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
