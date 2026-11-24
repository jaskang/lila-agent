//! Quiet borderless ghost pickers for project/model/thinking/approval selection
//! and the micro progress-bar context usage meter.

use std::rc::Rc;

use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    Anchor, App, Entity, Focusable as _, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::format::{format_compact_tokens, shorten_home_path};
use crate::protocol::{ApprovalMode, ModelInfo, ThinkingLevel};
use crate::store::AppStore;
use crate::ui::theme_setup;

#[derive(Default)]
struct ModelPickerNav {
    selected_idx: usize,
    last_query: String,
}

pub fn render_project_picker(
    id: &'static str,
    store_entity: Entity<AppStore>,
    disabled: bool,
    cx: &App,
) -> Option<impl IntoElement> {
    let store = store_entity.read(cx);
    let active = store.active_project()?.clone();
    let projects = store.projects.clone();
    let active_id = active.id.clone();
    let label = active.name.clone();
    let tooltip_path = active.path.clone();

    let trigger = Button::new(SharedString::from(format!("{id}-trigger")))
        .ghost()
        .xsmall()
        .rounded(px(theme_setup::RADIUS_SM))
        .disabled(disabled)
        .icon(
            Icon::new(IconName::Folder)
                .size(px(12.0))
                .text_color(cx.theme().muted_foreground),
        )
        .max_w(px(160.0))
        .label(label)
        .dropdown_caret(true)
        .text_color(cx.theme().foreground.opacity(0.9))
        .tooltip(tooltip_path);

    Some(
        Popover::new(SharedString::from(id))
            .anchor(Anchor::BottomLeft)
            .offset(px(6.0))
            .trigger(trigger)
            .content(move |_, _, cx| {
                let mut col = v_flex().w(px(292.0)).gap(px(2.0));
                for project in projects.clone() {
                    let is_curr = project.id == active_id;
                    let st_p = store_entity.clone();
                    let pid = project.id.clone();
                    let pop_ent = cx.entity();
                    col = col.child(
                        h_flex()
                            .id(SharedString::from(format!("proj-opt-{}", project.id)))
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .px_2()
                            .py(px(5.0))
                            .rounded(px(theme_setup::RADIUS_SM))
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
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                                            .text_color(cx.theme().foreground)
                                            .truncate()
                                            .child(project.name),
                                    )
                                    .child(
                                        div()
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .text_size(px(11.5))
                                            .text_color(cx.theme().muted_foreground)
                                            .truncate()
                                            .child(shorten_home_path(&project.path)),
                                    ),
                            )
                            .when(is_curr, |el| {
                                el.child(
                                    Icon::new(IconName::Check)
                                        .size(px(12.0))
                                        .text_color(cx.theme().foreground),
                                )
                            }),
                    );
                }
                let st_more = store_entity.clone();
                let pop_ent2 = cx.entity();
                col = col.child(
                    div()
                        .pt_1()
                        .mt_1()
                        .border_t_1()
                        .border_color(theme_setup::hairline(cx))
                        .child(
                            Button::new("composer-proj-add")
                                .ghost()
                                .xsmall()
                                .rounded(px(theme_setup::RADIUS_SM))
                                .icon(IconName::Plus)
                                .label("添加本地项目…")
                                .on_click(move |_, window, cx| {
                                    pop_ent2.update(cx, |st, cx| st.dismiss(window, cx));
                                    st_more.update(cx, |s, cx| s.add_project(cx));
                                }),
                        ),
                );
                div()
                    .id("composer-proj-scroll")
                    .max_h(px(260.0))
                    .overflow_y_scrollbar()
                    .child(col)
            }),
    )
}

pub fn render_model_picker(
    id: &'static str,
    store_entity: Entity<AppStore>,
    search_input: Entity<InputState>,
    current_model_id: Option<String>,
    disabled: bool,
    on_select: Rc<dyn Fn(ModelInfo, &mut App)>,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<> {
    let store = store_entity.read(cx);
    let target = current_model_id.or_else(|| store.default_model_id.clone());
    let label = if let Some(ref tid) = target
        && let Some(m) = store.models.iter().find(|m| &m.id == tid)
    {
        m.label.clone()
    } else if let Some(tid) = target.clone() {
        tid
    } else if store.models_loading {
        "加载模型…".to_owned()
    } else {
        "选择模型".to_owned()
    };

    let trigger = Button::new(SharedString::from(format!("{id}-trigger")))
        .ghost()
        .xsmall()
        .rounded(px(theme_setup::RADIUS_SM))
        .disabled(disabled)
        .label(label)
        .dropdown_caret(true)
        .text_color(cx.theme().muted_foreground)
        .tooltip("切换模型");

    let nav_state = window.use_keyed_state(
        SharedString::from(format!("{id}-nav-state")),
        cx,
        |_, _| ModelPickerNav::default(),
    );
    let search_open = search_input.clone();
    let nav_open = nav_state.clone();

    Popover::new(SharedString::from(id))
        .anchor(Anchor::BottomLeft)
        .offset(px(6.0))
        .trigger(trigger)
        .on_open_change(move |open, window, cx| {
            if *open {
                nav_open.update(cx, |nav, _| nav.selected_idx = 0);
                search_open.update(cx, |inp, cx| inp.focus_handle(cx).focus(window, cx));
            } else {
                nav_open.update(cx, |nav, _| {
                    nav.selected_idx = 0;
                    nav.last_query.clear();
                });
                search_open.update(cx, |inp, cx| inp.set_value(String::new(), window, cx));
            }
        })
        .content(move |_popover_state, _window, cx| {
            let query = search_input.read(cx).value().trim().to_lowercase();
            let store = store_entity.read(cx);
            let models = store.models.clone();
            let models_error = store.models_error.clone();
            let models_loading = store.models_loading;
            let active_id = target.clone();

            let filtered: Vec<ModelInfo> = models
                .into_iter()
                .filter(|m| {
                    query.is_empty()
                        || m.label.to_lowercase().contains(&query)
                        || m.id.to_lowercase().contains(&query)
                        || m.provider.to_lowercase().contains(&query)
                })
                .take(80)
                .collect();

            if nav_state.read(cx).last_query != query {
                let q_clone = query.clone();
                nav_state.update(cx, |nav, _| {
                    nav.last_query = q_clone;
                    nav.selected_idx = 0;
                });
            }
            let active_idx = if filtered.is_empty() {
                0
            } else {
                nav_state.read(cx).selected_idx.min(filtered.len() - 1)
            };

            let mut list = v_flex().gap(px(2.0));
            if let Some(err) = models_error {
                let st_retry = store_entity.clone();
                list = list.child(
                    v_flex()
                        .items_center()
                        .gap_2()
                        .py_3()
                        .child(div().text_xs().text_color(theme_setup::danger_fg(cx)).child(err))
                        .child(Button::new("retry-models").xsmall().label("重试").on_click(
                            move |_, _, cx| {
                                st_retry.update(cx, |s, cx| s.load_models(cx));
                            },
                        )),
                );
            } else if models_loading && filtered.is_empty() {
                list = list.child(
                    div()
                        .py_4()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("正在加载模型列表…"),
                );
            } else if filtered.is_empty() {
                list = list.child(
                    div()
                        .py_3()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("无匹配模型"),
                );
            } else {
                for (idx, model) in filtered.iter().enumerate() {
                    let is_sel = active_id.as_deref() == Some(model.id.as_str());
                    let is_highlighted = idx == active_idx;
                    let cb = on_select.clone();
                    let m_click = model.clone();
                    let pop_entity = cx.entity();
                    let nav_hover = nav_state.clone();

                    list = list.child(
                        h_flex()
                            .id(SharedString::from(format!("model-opt-{}", model.id)))
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .px_2()
                            .py(px(5.0))
                            .rounded(px(theme_setup::RADIUS_SM))
                            .cursor_pointer()
                            .when(is_highlighted || is_sel, |el| el.bg(cx.theme().accent))
                            .on_hover(move |hovered, _, cx| {
                                if *hovered {
                                    nav_hover.update(cx, |nav, cx| {
                                        if nav.selected_idx != idx {
                                            nav.selected_idx = idx;
                                            cx.notify();
                                        }
                                    });
                                }
                            })
                            .on_click(move |_, window, cx| {
                                cb(m_click.clone(), cx);
                                pop_entity.update(cx, |st, cx| st.dismiss(window, cx));
                            })
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                                            .text_color(cx.theme().foreground)
                                            .truncate()
                                            .child(model.label.clone()),
                                    )
                                    .child(
                                        div()
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .text_size(px(11.5))
                                            .text_color(cx.theme().muted_foreground)
                                            .truncate()
                                            .child(model.provider.clone()),
                                    ),
                            )
                            .when(is_sel, |el| {
                                el.child(
                                    Icon::new(IconName::Check)
                                        .size(px(12.0))
                                        .text_color(cx.theme().foreground),
                                )
                            }),
                    );
                }
            }

            let nav_key = nav_state.clone();
            let filtered_key = filtered;
            let cb_key = on_select.clone();
            let pop_key = cx.entity();

            v_flex()
                .w(px(320.0))
                .gap_2()
                .capture_key_down(move |ev: &KeyDownEvent, window, cx| {
                    let len = filtered_key.len();
                    if len == 0 {
                        return;
                    }
                    match ev.keystroke.key.as_str() {
                        "down" => {
                            nav_key.update(cx, |nav, cx| {
                                nav.selected_idx = (nav.selected_idx + 1) % len;
                                cx.notify();
                            });
                            cx.stop_propagation();
                            window.prevent_default();
                        }
                        "up" => {
                            nav_key.update(cx, |nav, cx| {
                                nav.selected_idx = (nav.selected_idx + len - 1) % len;
                                cx.notify();
                            });
                            cx.stop_propagation();
                            window.prevent_default();
                        }
                        "enter" => {
                            let idx = nav_key.read(cx).selected_idx.min(len - 1);
                            let chosen = filtered_key[idx].clone();
                            cb_key(chosen, cx);
                            pop_key.update(cx, |st, cx| st.dismiss(window, cx));
                            cx.stop_propagation();
                            window.prevent_default();
                        }
                        _ => {}
                    }
                })
                .child(Input::new(&search_input).xsmall())
                .child(
                    div()
                        .id("model-list-scroll")
                        .max_h(px(280.0))
                        .overflow_y_scrollbar()
                        .child(list),
                )
        })
}

pub fn render_thinking_picker(
    id: &'static str,
    current: ThinkingLevel,
    levels: Vec<ThinkingLevel>,
    disabled: bool,
    on_select: Rc<dyn Fn(ThinkingLevel, &mut App)>,
    cx: &App,
) -> Option<impl IntoElement> {
    if levels.len() <= 1 {
        return None;
    }

    let trigger = Button::new(SharedString::from(format!("{id}-trigger")))
        .ghost()
        .xsmall()
        .rounded(px(theme_setup::RADIUS_SM))
        .disabled(disabled)
        .label(current.label())
        .dropdown_caret(true)
        .text_color(cx.theme().muted_foreground)
        .tooltip("推理思考深度");

    Some(
        Popover::new(SharedString::from(id))
            .anchor(Anchor::BottomLeft)
            .offset(px(6.0))
            .trigger(trigger)
            .content(move |_, _, cx| {
                let mut col = v_flex().w(px(148.0)).gap(px(2.0));
                for lvl in levels.clone() {
                    let is_sel = lvl == current;
                    let cb = on_select.clone();
                    let pop_entity = cx.entity();
                    col = col.child(
                        h_flex()
                            .id(SharedString::from(format!("think-opt-{:?}", lvl)))
                            .w_full()
                            .items_center()
                            .justify_between()
                            .px_2()
                            .py(px(5.0))
                            .rounded(px(6.0))
                            .cursor_pointer()
                            .when(is_sel, |el| el.bg(cx.theme().accent))
                            .hover(|s| s.bg(cx.theme().accent))
                            .on_click(move |_, window, cx| {
                                cb(lvl, cx);
                                pop_entity.update(cx, |st, cx| st.dismiss(window, cx));
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().foreground)
                                    .child(lvl.label()),
                            )
                            .when(is_sel, |el| {
                                el.child(
                                    Icon::new(IconName::Check)
                                        .size(px(12.0))
                                        .text_color(cx.theme().foreground),
                                )
                            }),
                    );
                }
                col
            }),
    )
}

pub fn render_approval_picker(
    id: &'static str,
    current: ApprovalMode,
    disabled: bool,
    on_select: Rc<dyn Fn(ApprovalMode, &mut App)>,
    cx: &App,
) -> impl IntoElement {
    let trigger = Button::new(SharedString::from(format!("{id}-trigger")))
        .ghost()
        .xsmall()
        .rounded(px(theme_setup::RADIUS_SM))
        .disabled(disabled)
        .label(current.label())
        .dropdown_caret(true)
        .text_color(cx.theme().muted_foreground)
        .tooltip("工具审批模式");

    Popover::new(SharedString::from(id))
        .anchor(Anchor::BottomLeft)
        .offset(px(6.0))
        .trigger(trigger)
        .content(move |_, _, cx| {
            let mut col = v_flex().w(px(268.0)).gap(px(2.0));
            for mode in ApprovalMode::ALL {
                let is_sel = mode == current;
                let cb = on_select.clone();
                let pop_entity = cx.entity();
                col = col.child(
                    v_flex()
                        .id(SharedString::from(format!("appr-opt-{:?}", mode)))
                        .w_full()
                        .gap(px(2.0))
                        .px_2()
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .when(is_sel, |el| el.bg(cx.theme().accent))
                        .hover(|s| s.bg(cx.theme().accent))
                        .on_click(move |_, window, cx| {
                            cb(mode, cx);
                            pop_entity.update(cx, |st, cx| st.dismiss(window, cx));
                        })
                        .child(
                            h_flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                                        .text_color(cx.theme().foreground)
                                        .child(mode.label()),
                                )
                                .when(is_sel, |el| {
                                    el.child(
                                        Icon::new(IconName::Check)
                                            .size(px(12.0))
                                            .text_color(cx.theme().foreground),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(cx.theme().muted_foreground)
                                .child(mode.hint()),
                        ),
                );
            }
            col
        })
}

pub fn render_context_meter(
    store_entity: Entity<AppStore>,
    session_id: String,
    cx: &App,
) -> Option<impl IntoElement> {
    let s = store_entity.read(cx).main_slot.state.clone()?;
    let pct = s.context.as_ref().and_then(|c| c.percent);
    let clamped_pct = pct.unwrap_or(0.0).clamp(0.0, 100.0) as f32;
    let bar_color: Hsla = if clamped_pct >= 92.0 {
        theme_setup::danger_fg(cx)
    } else if clamped_pct >= 75.0 {
        theme_setup::warning_fg(cx)
    } else {
        cx.theme().muted_foreground.opacity(0.75)
    };

    let label = if s.compacting {
        "压缩中…".to_owned()
    } else if let Some(p) = pct {
        format!("{p:.0}%")
    } else if let Some(ref c) = s.context
        && let Some(t) = c.tokens
    {
        format_compact_tokens(t)
    } else {
        "用量".to_owned()
    };

    let ctx_line = s.context.as_ref().map(|c| {
        let used = c
            .tokens
            .map(format_compact_tokens)
            .unwrap_or_else(|| "—".to_owned());
        let win = format_compact_tokens(c.window);
        let p_str = c.percent.map(|p| format!(" ({p:.0}%)")).unwrap_or_default();
        format!("{used} / {win}{p_str}")
    });
    let cost_line = format!("${:.4}", s.cost);
    let compacting = s.compacting;
    let run_active = s.run_active;

    let trigger = Button::new("ctx-meter-trigger")
        .ghost()
        .xsmall()
        .rounded(px(6.0))
        .tooltip("上下文用量与压缩")
        .child(
            h_flex()
                .gap(px(6.0))
                .items_center()
                .when(pct.is_some(), |el| {
                    let fill_w = (22.0 * (clamped_pct / 100.0)).max(2.0);
                    el.child(
                        div()
                            .w(px(22.0))
                            .h(px(3.5))
                            .rounded_full()
                            .bg(theme_setup::chip_border(cx))
                            .overflow_hidden()
                            .child(div().h_full().w(px(fill_w)).rounded_full().bg(bar_color)),
                    )
                })
                .child(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(12.0))
                        .text_color(if clamped_pct >= 75.0 {
                            bar_color
                        } else {
                            cx.theme().muted_foreground
                        })
                        .child(label),
                ),
        );

    Some(
        Popover::new("ctx-meter-popover")
            .anchor(Anchor::BottomRight)
            .offset(px(6.0))
            .trigger(trigger)
            .content(move |_, _, cx| {
                let st_cmp = store_entity.clone();
                let sid_cmp = session_id.clone();
                let st_rel = store_entity.clone();
                let sid_rel = session_id.clone();
                let pop1 = cx.entity();
                let pop2 = cx.entity();

                v_flex()
                    .w(px(252.0))
                    .gap(px(8.0))
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .text_color(cx.theme().foreground)
                                    .child("上下文与花费"),
                            )
                            .child(
                                div()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_size(px(12.0))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(cost_line.clone()),
                            ),
                    )
                    .when(pct.is_some(), |el| {
                        let fill_w = (228.0 * (clamped_pct / 100.0)).max(3.0);
                        el.child(
                            div()
                                .w_full()
                                .h(px(4.0))
                                .rounded_full()
                                .bg(theme_setup::chip_border(cx))
                                .overflow_hidden()
                                .child(div().h_full().w(px(fill_w)).rounded_full().bg(bar_color)),
                        )
                    })
                    .when_some(ctx_line.clone(), |el, line| {
                        el.child(
                            div()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(px(12.0))
                                .text_color(cx.theme().muted_foreground)
                                .child(line),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .pt_2()
                            .border_t_1()
                            .border_color(theme_setup::hairline(cx))
                            .child(
                                Button::new("ctx-compact")
                                    .xsmall()
                                    .rounded(px(6.0))
                                    .disabled(compacting || run_active)
                                    .label("压缩上下文")
                                    .on_click(move |_, window, cx| {
                                        pop1.update(cx, |st, cx| st.dismiss(window, cx));
                                        st_cmp.update(cx, |s, cx| s.compact(sid_cmp.clone(), cx));
                                    }),
                            )
                            .child(
                                Button::new("ctx-reload")
                                    .ghost()
                                    .xsmall()
                                    .rounded(px(6.0))
                                    .disabled(run_active)
                                    .label("重载能力")
                                    .on_click(move |_, window, cx| {
                                        pop2.update(cx, |st, cx| st.dismiss(window, cx));
                                        st_rel.update(cx, |s, cx| s.reload(sid_rel.clone(), cx));
                                    }),
                            ),
                    )
            }),
    )
}
