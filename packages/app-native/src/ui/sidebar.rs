//! Project and session navigation.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    Anchor, App, AppContext as _, Entity, Focusable as _, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::format::{format_relative_time, now_millis};
use crate::protocol::{ConnectionState, Project, SessionSummary};
use crate::store::AppStore;
use crate::ui::theme_setup::ThemePreference;
use crate::ui::{
    BadgeTone, TOP_BAR_HEIGHT, TRAFFIC_LIGHT_INSET, pill_badge, status_dot, theme_setup,
    window_drag_bar,
};

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
    let connected = store.connection == ConnectionState::Connected;
    let has_projects = !projects.is_empty();
    let sidebar_width = store.sidebar_width;
    let theme_pref = store.theme_preference;
    let now = now_millis();

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
    let chrome_icon = cx.theme().foreground.opacity(0.85);

    let top_bar = window_drag_bar("sidebar-top-bar", TOP_BAR_HEIGHT, window, cx)
        .pl(px(TRAFFIC_LIGHT_INSET))
        .pr_1()
        .justify_between()
        .child(
            h_flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    Button::new("sb-toggle-collapse")
                        .ghost()
                        .small()
                        .rounded(px(theme_setup::RADIUS_SM))
                        .icon(Icon::new(IconName::PanelLeft).text_color(chrome_icon))
                        .tooltip("收起侧栏 (⌘B)")
                        .on_click(move |_, _, cx| {
                            st_toggle.update(cx, |s, cx| s.toggle_sidebar(cx));
                        }),
                )
                .child(
                    h_flex()
                        .items_center()
                        .gap(px(5.0))
                        .child(
                            div()
                                .size(px(5.5))
                                .rounded_full()
                                .bg(theme_setup::accent_ember(cx)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .text_color(cx.theme().foreground)
                                .child("Lila"),
                        ),
                ),
        )
        .child(
            h_flex()
                .items_center()
                .gap(px(2.0))
                .child(
                    Button::new("sb-new-session")
                        .ghost()
                        .small()
                        .rounded(px(theme_setup::RADIUS_SM))
                        .icon(Icon::new(IconName::Plus).text_color(chrome_icon))
                        .tooltip("新建会话 (⌘N)")
                        .on_click(move |_, _, cx| {
                            st_new.update(cx, |s, cx| s.start_new(None, cx));
                        }),
                )
                .child(
                    Button::new("sb-open-palette")
                        .ghost()
                        .small()
                        .rounded(px(theme_setup::RADIUS_SM))
                        .icon(Icon::new(IconName::Search).text_color(chrome_icon))
                        .tooltip("搜索与命令 (⌘K)")
                        .on_click(move |_, window, cx| {
                            on_open_palette(window, cx);
                        }),
                ),
        );

    let mut tree = v_flex()
        .w_full()
        .px(px(8.0))
        .pt(px(2.0))
        .pb_3()
        .gap(px(12.0));

    if has_projects {
        for project in projects {
            let is_open = !collapsed_projects.contains(&project.id);
            let project_sessions = by_project.remove(&project.id).unwrap_or_default();

            let mut sess_list = v_flex().gap(px(2.0));
            if project_sessions.is_empty() {
                sess_list = sess_list.child(
                    div()
                        .px(px(10.0))
                        .py_1()
                        .text_size(px(12.0))
                        .text_color(cx.theme().muted_foreground.opacity(0.5))
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
                    let rel_time = format_relative_time(&sess.updated_at, now);

                    let st_sel = store_entity.clone();
                    let sid_sel = sess.id.clone();
                    let sess_group: SharedString = format!("sess-grp-{}", sess.id).into();
                    let leading_dot = status_dot(sess.status, cx);

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
                        .h(px(32.0))
                        .items_center()
                        .gap(px(4.0))
                        .pl(px(9.0))
                        .pr(px(6.0))
                        .rounded(px(theme_setup::RADIUS_SM))
                        .when(is_active, |el| el.bg(theme_setup::pill_active_bg(cx)))
                        .when(!is_active, |el| el.hover(|s| s.bg(cx.theme().list_hover)))
                        .child(
                            h_flex()
                                .id(SharedString::from(format!("sess-btn-{}", sess.id)))
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .items_center()
                                .gap(px(7.0))
                                .cursor_pointer()
                                .on_click(move |_, _, cx| {
                                    st_sel.update(cx, |s, cx| {
                                        s.select_session(Some(sid_sel.clone()), cx);
                                    });
                                })
                                .when_some(leading_dot, |el, dot| {
                                    el.child(
                                        div()
                                            .w(px(10.0))
                                            .h(px(10.0))
                                            .flex()
                                            .flex_shrink_0()
                                            .items_center()
                                            .justify_center()
                                            .child(dot),
                                    )
                                })
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(13.5))
                                        .font_weight(if is_active {
                                            gpui_kit::FontWeight::MEDIUM
                                        } else {
                                            gpui_kit::FontWeight::NORMAL
                                        })
                                        .text_color(if is_active {
                                            cx.theme().foreground
                                        } else {
                                            cx.theme().sidebar_foreground
                                        })
                                        .child(title_str),
                                ),
                        );

                    if sess.pending_count > 0 {
                        row = row.child(pill_badge(
                            sess.pending_count.to_string(),
                            BadgeTone::Amber,
                            cx,
                        ));
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
                                .px(px(4.5))
                                .py(px(1.5))
                                .rounded(px(theme_setup::RADIUS_SM))
                                .cursor_pointer()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(px(11.5))
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

                    let more_btn =
                        Button::new(SharedString::from(format!("sess-more-{}", sess.id)))
                            .ghost()
                            .xsmall()
                            .compact()
                            .rounded(px(6.0))
                            .icon(
                                Icon::new(IconName::Ellipsis)
                                    .size(px(12.5))
                                    .text_color(muted_icon),
                            )
                            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                                let st_r = st_dd_ren.clone();
                                let sr = sess_dd_ren.clone();
                                let st_d = st_dd_del.clone();
                                let sd = sess_dd_del.clone();
                                menu.item(
                                    PopupMenuItem::new("重命名")
                                        .icon(IconName::Replace)
                                        .on_click(move |_, window, cx| {
                                            open_rename_dialog(
                                                st_r.clone(),
                                                sr.clone(),
                                                window,
                                                cx,
                                            );
                                        }),
                                )
                                .separator()
                                .item(
                                    PopupMenuItem::new("删除会话")
                                        .icon(IconName::Delete)
                                        .on_click(move |_, window, cx| {
                                            open_delete_dialog(
                                                st_d.clone(),
                                                sd.clone(),
                                                window,
                                                cx,
                                            );
                                        }),
                                )
                            });

                    // Trailing slot: calm monospace relative timestamp at rest,
                    // smoothly swaps to the action button on row hover.
                    row = row.child(
                        div()
                            .relative()
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .justify_end()
                            .min_w(px(24.0))
                            .when(!rel_time.is_empty(), |el| {
                                el.child(
                                    div()
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_size(px(11.5))
                                        .text_color(cx.theme().muted_foreground)
                                        .group_hover(sess_group.clone(), |s| s.opacity(0.0))
                                        .child(rel_time),
                                )
                            })
                            .child(
                                div()
                                    .absolute()
                                    .right_0()
                                    .opacity(0.0)
                                    .group_hover(sess_group, |s| s.opacity(1.0))
                                    .child(more_btn),
                            ),
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
                                    .h(px(28.0))
                                    .items_center()
                                    .gap(px(6.0))
                                    .px_2()
                                    .rounded(px(theme_setup::RADIUS_SM))
                                    .cursor_pointer()
                                    .when(child_active, |el| el.bg(theme_setup::pill_active_bg(cx)))
                                    .when(!child_active, |el| {
                                        el.hover(|s| s.bg(cx.theme().list_hover))
                                    })
                                    .on_click(move |_, _, cx| {
                                        st_c.update(cx, |s, cx| {
                                            s.select_session(Some(cid.clone()), cx);
                                        });
                                    })
                                    .when_some(status_dot(child.status, cx), |el, dot| {
                                        el.child(dot)
                                    })
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(px(12.5))
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
                .rounded(px(6.0))
                .icon(
                    Icon::new(IconName::Ellipsis)
                        .size(px(12.0))
                        .text_color(muted_icon),
                )
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
                .h(px(28.0))
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
                        .gap(px(5.0))
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            st_tog.update(cx, |s, cx| {
                                s.toggle_project_collapsed(pid_tog.clone(), cx);
                            });
                        })
                        .child(
                            Icon::new(if is_open {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size(px(11.0))
                            .text_color(muted_icon),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.0))
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .text_color(cx.theme().muted_foreground)
                                .child(project.name.clone()),
                        ),
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
                                .rounded(px(6.0))
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
    let st_theme = store_entity.clone();
    let (theme_icon, theme_tooltip) = match theme_pref {
        ThemePreference::System => (IconName::Palette, "外观：跟随系统（点击切换）"),
        ThemePreference::Light => (IconName::Sun, "外观：浅色（点击切换）"),
        ThemePreference::Dark => (IconName::Moon, "外观：深色（点击切换）"),
    };

    let footer = h_flex()
        .h(px(38.0))
        .flex_shrink_0()
        .items_center()
        .justify_between()
        .px_2()
        .border_t_1()
        .border_color(theme_setup::hairline(cx))
        .child(
            Button::new("sb-footer-add-project")
                .ghost()
                .small()
                .rounded(px(theme_setup::RADIUS_SM))
                .disabled(!connected)
                .icon(Icon::new(IconName::FolderOpen).text_color(muted_icon))
                .label("添加项目")
                .tooltip("添加本地项目目录…")
                .on_click(move |_, _, cx| {
                    st_add_footer.update(cx, |s, cx| s.add_project(cx));
                }),
        )
        .child(
            Button::new("sb-footer-theme")
                .ghost()
                .small()
                .rounded(px(theme_setup::RADIUS_SM))
                .icon(Icon::new(theme_icon).text_color(muted_icon))
                .tooltip(theme_tooltip)
                .on_click(move |_, window, cx| {
                    let next = st_theme.update(cx, |s, cx| s.cycle_theme_preference(cx));
                    theme_setup::apply_theme_preference(next, Some(window), cx);
                }),
        );

    v_flex()
        .w(px(sidebar_width))
        .min_w(px(theme_setup::SIDEBAR_MIN_W))
        .max_w(px(theme_setup::SIDEBAR_MAX_W))
        .h_full()
        .flex_shrink_0()
        .bg(theme_setup::sidebar_bg(cx))
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

    let st_enter = store.clone();
    let sid_enter = sid.clone();
    window
        .subscribe(
        &input_state,
        cx,
        move |input, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                let val = input.read(cx).value().to_string();
                st_enter.update(cx, |s, cx| s.rename_session(sid_enter.clone(), val, cx));
                window.close_dialog(cx);
            }
        },
    )
    .detach();

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
                    .child(Button::new("rename-save").primary().label("保存").on_click(
                        move |_, window, cx| {
                            let val = inp_ok.read(cx).value().to_string();
                            st_ok.update(cx, |s, cx| s.rename_session(sid_ok.clone(), val, cx));
                            window.close_dialog(cx);
                        },
                    )),
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
                    .child(Button::new("del-confirm").danger().label("删除").on_click(
                        move |_, window, cx| {
                            st_do.update(cx, |s, cx| s.delete_session(sid_do.clone(), cx));
                            window.close_dialog(cx);
                        },
                    )),
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
