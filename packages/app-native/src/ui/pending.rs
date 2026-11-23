//! Pending requests card: approval, confirm, select, input, editor, and multi-question ask_user.

use std::collections::HashMap;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::protocol::{AskQuestionAnswer, PendingAnswer, PendingRequest};
use crate::store::AppStore;
use crate::ui::theme_setup;

struct Choice {
    label: String,
    answer: PendingAnswer,
    primary: bool,
    danger: bool,
}

fn build_choices(item: &PendingRequest) -> Vec<Choice> {
    match item.kind.as_str() {
        "approval" => vec![
            Choice {
                label: "允许本次".to_owned(),
                answer: PendingAnswer::Option {
                    value: "allow".to_owned(),
                },
                primary: true,
                danger: false,
            },
            Choice {
                label: "本会话同类自动允许".to_owned(),
                answer: PendingAnswer::Option {
                    value: "session".to_owned(),
                },
                primary: false,
                danger: false,
            },
            Choice {
                label: "拒绝".to_owned(),
                answer: PendingAnswer::Option {
                    value: "deny".to_owned(),
                },
                primary: false,
                danger: true,
            },
        ],
        "confirm" => vec![
            Choice {
                label: "确定".to_owned(),
                answer: PendingAnswer::Confirm { confirmed: true },
                primary: true,
                danger: false,
            },
            Choice {
                label: "取消".to_owned(),
                answer: PendingAnswer::Confirm { confirmed: false },
                primary: false,
                danger: false,
            },
        ],
        "select" => item
            .options
            .iter()
            .enumerate()
            .map(|(i, opt)| Choice {
                label: opt.clone(),
                answer: PendingAnswer::Option { value: opt.clone() },
                primary: i == 0,
                danger: false,
            })
            .collect(),
        _ => Vec::new(),
    }
}

pub struct PendingPanelView {
    store: Entity<AppStore>,
    active_pending_id: Option<String>,
    text_input: Entity<InputState>,
    editor_input: Entity<TextareaState>,
    ask_selected: HashMap<String, Vec<String>>,
    ask_custom_inputs: HashMap<String, Entity<InputState>>,
}

impl PendingPanelView {
    pub fn new(store: Entity<AppStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let text_input = cx.new(|cx| InputState::new(window, cx));
        let editor_input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 8));
        Self {
            store,
            active_pending_id: None,
            text_input,
            editor_input,
            ask_selected: HashMap::new(),
            ask_custom_inputs: HashMap::new(),
        }
    }

    fn sync_pending(
        &mut self,
        first: &PendingRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_pending_id.as_deref() == Some(first.id.as_str()) {
            return;
        }
        self.active_pending_id = Some(first.id.clone());
        self.ask_selected.clear();
        self.ask_custom_inputs.clear();

        if first.kind == "input" {
            let ph = first
                .placeholder
                .clone()
                .unwrap_or_else(|| "请输入…".to_owned());
            self.text_input = cx.new(|cx| InputState::new(window, cx).placeholder(ph));
        } else if first.kind == "editor" {
            let prefill = first.prefill.clone().unwrap_or_default();
            self.editor_input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(3, 8)
                    .default_value(prefill)
            });
        } else if first.kind == "ask" {
            for q in first.questions.as_deref().unwrap_or(&[]) {
                let inp = cx.new(|cx| InputState::new(window, cx).placeholder("其他回答（可选）…"));
                self.ask_custom_inputs.insert(q.id.clone(), inp);
            }
        }
    }
}

impl Render for PendingPanelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (session_id, first, extra) = {
            let st = self.store.read(cx);
            let Some(sid) = st.selected_id.clone() else {
                return div().into_any_element();
            };
            let Some(ref state) = st.main_slot.state else {
                return div().into_any_element();
            };
            let Some(first) = state.pending.first().cloned() else {
                self.active_pending_id = None;
                return div().into_any_element();
            };
            (sid, first, state.pending.len().saturating_sub(1))
        };

        self.sync_pending(&first, window, cx);

        let store = self.store.clone();
        let pid = first.id.clone();

        let mut card = v_flex()
            .w_full()
            .gap_2()
            .p_3()
            .rounded_2xl()
            .border_1()
            .border_color(theme_setup::attention_border(cx))
            .bg(theme_setup::attention_bg(cx))
            .shadow_md()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child(first.title.clone()),
                    )
                    .when(extra > 0, |el| {
                        el.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("还有 {extra} 个待处理")),
                        )
                    }),
            );

        if let Some(msg) = first.message.clone() {
            card = card.child(
                div()
                    .id("pending-msg-scroll")
                    .max_h(px(160.0))
                    .overflow_y_scrollbar()
                    .p_2()
                    .rounded_lg()
                    .bg(theme_setup::card_bg(cx))
                    .text_xs()
                    .text_color(cx.theme().foreground)
                    .child(msg),
            );
        }

        match first.kind.as_str() {
            "approval" | "confirm" | "select" => {
                let mut btn_row = h_flex().flex_wrap().gap_2();
                for (i, choice) in build_choices(&first).into_iter().enumerate() {
                    let st_c = store.clone();
                    let sid_c = session_id.clone();
                    let pid_c = pid.clone();
                    let ans = choice.answer;
                    let mut b = Button::new(SharedString::from(format!("pend-choice-{i}")))
                        .xsmall()
                        .label(choice.label);
                    if choice.primary {
                        b = b.primary();
                    } else if choice.danger {
                        b = b.danger();
                    }
                    b = b.on_click(move |_, _, cx| {
                        st_c.update(cx, |s, cx| {
                            s.respond_pending(sid_c.clone(), pid_c.clone(), ans.clone(), cx);
                        });
                    });
                    btn_row = btn_row.child(b);
                }
                if first.kind == "select" {
                    let st_cancel = store.clone();
                    let sid_cancel = session_id.clone();
                    let pid_cancel = pid.clone();
                    btn_row = btn_row.child(
                        Button::new("pend-select-cancel")
                            .ghost()
                            .xsmall()
                            .label("取消")
                            .on_click(move |_, _, cx| {
                                st_cancel.update(cx, |s, cx| {
                                    s.respond_pending(
                                        sid_cancel.clone(),
                                        pid_cancel.clone(),
                                        PendingAnswer::Cancel,
                                        cx,
                                    );
                                });
                            }),
                    );
                }
                card = card.child(btn_row);
            }
            "input" => {
                let st_ok = store.clone();
                let sid_ok = session_id.clone();
                let pid_ok = pid.clone();
                let inp_ok = self.text_input.clone();

                let st_c = store.clone();
                let sid_c = session_id.clone();
                let pid_c = pid.clone();

                card = card.child(Input::new(&self.text_input).small()).child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("pend-input-cancel")
                                .ghost()
                                .xsmall()
                                .label("取消")
                                .on_click(move |_, _, cx| {
                                    st_c.update(cx, |s, cx| {
                                        s.respond_pending(
                                            sid_c.clone(),
                                            pid_c.clone(),
                                            PendingAnswer::Cancel,
                                            cx,
                                        );
                                    });
                                }),
                        )
                        .child(
                            Button::new("pend-input-submit")
                                .primary()
                                .xsmall()
                                .label("提交")
                                .on_click(move |_, _, cx| {
                                    let val = inp_ok.read(cx).value().to_string();
                                    st_ok.update(cx, |s, cx| {
                                        s.respond_pending(
                                            sid_ok.clone(),
                                            pid_ok.clone(),
                                            PendingAnswer::Text { value: val },
                                            cx,
                                        );
                                    });
                                }),
                        ),
                );
            }
            "editor" => {
                let st_ok = store.clone();
                let sid_ok = session_id.clone();
                let pid_ok = pid.clone();
                let ed_ok = self.editor_input.clone();

                let st_c = store.clone();
                let sid_c = session_id.clone();
                let pid_c = pid.clone();

                card = card.child(Textarea::new(&self.editor_input).small()).child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("pend-ed-cancel")
                                .ghost()
                                .xsmall()
                                .label("取消")
                                .on_click(move |_, _, cx| {
                                    st_c.update(cx, |s, cx| {
                                        s.respond_pending(
                                            sid_c.clone(),
                                            pid_c.clone(),
                                            PendingAnswer::Cancel,
                                            cx,
                                        );
                                    });
                                }),
                        )
                        .child(
                            Button::new("pend-ed-submit")
                                .primary()
                                .xsmall()
                                .label("提交")
                                .on_click(move |_, _, cx| {
                                    let val = ed_ok.read(cx).value().to_string();
                                    st_ok.update(cx, |s, cx| {
                                        s.respond_pending(
                                            sid_ok.clone(),
                                            pid_ok.clone(),
                                            PendingAnswer::Text { value: val },
                                            cx,
                                        );
                                    });
                                }),
                        ),
                );
            }
            "ask" => {
                let mut q_col = v_flex().gap_2();
                let q_list = first.questions.clone().unwrap_or_default();
                for q in &q_list {
                    let mut q_box = v_flex()
                        .gap_1()
                        .p_2()
                        .rounded_lg()
                        .bg(theme_setup::card_bg(cx))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .text_color(cx.theme().foreground)
                                .child(q.question.clone()),
                        );
                    for (oi, opt) in q.options.iter().enumerate() {
                        let checked = self
                            .ask_selected
                            .get(&q.id)
                            .is_some_and(|v| v.contains(opt));
                        let qid = q.id.clone();
                        let opt_str = opt.clone();
                        let multiple = q.multiple;
                        q_box = q_box.child(
                            Checkbox::new(SharedString::from(format!("ask-{}-{oi}", q.id)))
                                .label(opt.clone())
                                .checked(checked)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let entry = this.ask_selected.entry(qid.clone()).or_default();
                                    if multiple {
                                        if let Some(pos) =
                                            entry.iter().position(|x| x == &opt_str)
                                        {
                                            entry.remove(pos);
                                        } else {
                                            entry.push(opt_str.clone());
                                        }
                                    } else if entry.len() == 1 && entry[0] == opt_str {
                                        entry.clear();
                                    } else {
                                        *entry = vec![opt_str.clone()];
                                    }
                                    cx.notify();
                                })),
                        );
                    }
                    if let Some(inp) = self.ask_custom_inputs.get(&q.id) {
                        q_box = q_box.child(Input::new(inp).xsmall());
                    }
                    q_col = q_col.child(q_box);
                }

                let questions = q_list;
                let st_ok = store.clone();
                let sid_ok = session_id.clone();
                let pid_ok = pid.clone();

                let st_c = store.clone();
                let sid_c = session_id.clone();
                let pid_c = pid.clone();

                card = card.child(q_col).child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("pend-ask-cancel")
                                .ghost()
                                .xsmall()
                                .label("取消")
                                .on_click(move |_, _, cx| {
                                    st_c.update(cx, |s, cx| {
                                        s.respond_pending(
                                            sid_c.clone(),
                                            pid_c.clone(),
                                            PendingAnswer::Cancel,
                                            cx,
                                        );
                                    });
                                }),
                        )
                        .child(
                            Button::new("pend-ask-submit")
                                .primary()
                                .xsmall()
                                .label("提交回答")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let answers: Vec<AskQuestionAnswer> = questions
                                        .iter()
                                        .map(|q| {
                                            let selected = this
                                                .ask_selected
                                                .get(&q.id)
                                                .cloned()
                                                .unwrap_or_default();
                                            let custom = this
                                                .ask_custom_inputs
                                                .get(&q.id)
                                                .map(|inp| inp.read(cx).value().trim().to_owned())
                                                .filter(|s| !s.is_empty());
                                            AskQuestionAnswer {
                                                id: q.id.clone(),
                                                selected,
                                                custom,
                                            }
                                        })
                                        .collect();
                                    st_ok.update(cx, |s, cx| {
                                        s.respond_pending(
                                            sid_ok.clone(),
                                            pid_ok.clone(),
                                            PendingAnswer::Ask { answers },
                                            cx,
                                        );
                                    });
                                })),
                        ),
                );
            }
            _ => {}
        }

        card.into_any_element()
    }
}
