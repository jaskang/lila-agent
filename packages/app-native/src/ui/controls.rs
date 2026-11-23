//! Session controls: searchable ModelPicker, ThinkingPicker, ApprovalPicker, and ContextMeter
//! styled as quiet, low-contrast macOS controls with native dropdown carets and vector icons.

use std::rc::Rc;

use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    Anchor, App, Entity, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _, px, rgb,
};

use crate::format::format_compact_tokens;
use crate::protocol::{ApprovalMode, ModelInfo, ThinkingLevel};
use crate::store::AppStore;
use crate::ui::theme_setup;

pub fn render_model_picker(
    id: &'static str,
    store_entity: Entity<AppStore>,
    search_input: Entity<InputState>,
    current_model_id: Option<String>,
    disabled: bool,
    on_select: Rc<dyn Fn(ModelInfo, &mut App)>,
    cx: &App,
) -> impl IntoElement {
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
        .disabled(disabled)
        .label(label)
        .dropdown_caret(true)
        .text_color(cx.theme().muted_foreground)
        .tooltip("切换模型");

    Popover::new(SharedString::from(id))
        .anchor(Anchor::BottomLeft)
        .offset(px(6.0))
        .trigger(trigger)
        .content(move |popover_state, _window, cx| {
            let query = search_input.read(cx).value().trim().to_lowercase();
            let store = store_entity.read(cx);
            let models = store.models.clone();
            let models_error = store.models_error.clone();
            let models_loading = store.models_loading;
            let active_id = target.clone();

            let mut list = v_flex().gap(px(2.0));
            if let Some(err) = models_error {
                let st_retry = store_entity.clone();
                list = list.child(
                    v_flex()
                        .items_center()
                        .gap_2()
                        .py_3()
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().danger)
                                .child(err),
                        )
                        .child(
                            Button::new("retry-models")
                                .xsmall()
                                .label("重试")
                                .on_click(move |_, _, cx| {
                                    st_retry.update(cx, |s, cx| s.load_models(cx));
                                }),
                        ),
                );
            } else if models_loading && models.is_empty() {
                list = list.child(
                    div()
                        .py_4()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("正在加载模型列表…"),
                );
            } else {
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

                if filtered.is_empty() {
                    list = list.child(
                        div()
                            .py_3()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("无匹配模型"),
                    );
                } else {
                    for model in filtered {
                        let is_sel = active_id.as_deref() == Some(model.id.as_str());
                        let cb = on_select.clone();
                        let m_click = model.clone();
                        let pop_entity = cx.entity();
                        let _ = popover_state;

                        list = list.child(
                            h_flex()
                                .id(SharedString::from(format!("model-opt-{}", model.id)))
                                .w_full()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .px_2()
                                .py(px(5.0))
                                .rounded_md()
                                .cursor_pointer()
                                .when(is_sel, |el| el.bg(cx.theme().accent))
                                .hover(|s| s.bg(cx.theme().accent))
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
                                                .child(model.label),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(10.0))
                                                .text_color(cx.theme().muted_foreground)
                                                .truncate()
                                                .child(model.provider),
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
            }

            v_flex()
                .w(px(320.0))
                .gap_2()
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
                let mut col = v_flex().w(px(144.0)).gap(px(2.0));
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
                            .rounded_md()
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
                        .rounded_md()
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
                                .text_size(px(10.5))
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
    let high_pct = pct.is_some_and(|p| p >= 80.0);

    let label = if s.compacting {
        "压缩中…".to_owned()
    } else if let Some(p) = pct {
        format!("{:.0}%", p)
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
        let p_str = c
            .percent
            .map(|p| format!(" ({p:.0}%)"))
            .unwrap_or_default();
        format!("上下文：{used} / {win}{p_str}")
    });
    let cost_line = format!("累计花费：${:.4}", s.cost);
    let compacting = s.compacting;
    let run_active = s.run_active;

    let trigger = Button::new("ctx-meter-trigger")
        .ghost()
        .xsmall()
        .label(label)
        .text_color(if high_pct {
            rgb(0xf59e0b).into()
        } else {
            cx.theme().muted_foreground
        })
        .tooltip("上下文用量与压缩");

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
                    .w(px(244.0))
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(cx.theme().foreground)
                            .child("会话用量"),
                    )
                    .when_some(ctx_line.clone(), |el, line| {
                        el.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(line),
                        )
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(cost_line.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .pt_2()
                            .border_t_1()
                            .border_color(theme_setup::hairline(cx))
                            .child(
                                Button::new("ctx-compact")
                                    .xsmall()
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
