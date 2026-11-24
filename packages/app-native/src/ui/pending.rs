//! Pending requests card: approval, confirm, select, input, editor, and multi-question
//! ask_user. Styled as a 12px-radius calm surface with restrained amber hairline framing.

use std::collections::HashMap;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::text::{SelectionFormat, TextView};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, Focusable as _, InteractiveElement as _,
    IntoElement, KeyDownEvent, ParentElement as _, Render, SharedString, Styled as _, Subscription,
    Window, div, prelude::FluentBuilder as _, px,
};

use crate::protocol::{AskQuestionAnswer, PendingAnswer, PendingRequest};
use crate::store::AppStore;
use crate::ui::transcript::plain_text_to_markdown;
use crate::ui::{kbd_chip, theme_setup};

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
    focus_handle: FocusHandle,
    active_pending_id: Option<String>,
    text_input: Entity<InputState>,
    _text_sub: Option<Subscription>,
    editor_input: Entity<TextareaState>,
    ask_selected: HashMap<String, Vec<String>>,
    ask_custom_inputs: HashMap<String, Entity<InputState>>,
}

impl PendingPanelView {
    pub fn new(store: Entity<AppStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let text_input = cx.new(|cx| InputState::new(window, cx));
        let editor_input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 8));
        Self {
            store,
            focus_handle,
            active_pending_id: None,
            text_input,
            _text_sub: None,
            editor_input,
            ask_selected: HashMap::new(),
            ask_custom_inputs: HashMap::new(),
        }
    }

    fn sync_pending(
        &mut self,
        session_id: &str,
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
        self._text_sub = None;

        if first.kind == "input" {
            let ph = first
                .placeholder
                .clone()
                .unwrap_or_else(|| "请输入…".to_owned());
            self.text_input = cx.new(|cx| InputState::new(window, cx).placeholder(ph));
            let st_ok = self.store.clone();
            let sid_ok = session_id.to_owned();
            let pid_ok = first.id.clone();
            self._text_sub = Some(cx.subscribe_in(
                &self.text_input,
                window,
                move |_, input, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        let val = input.read(cx).value().to_string();
                        st_ok.update(cx, |s, cx| {
                            s.respond_pending(
                                sid_ok.clone(),
                                pid_ok.clone(),
                                PendingAnswer::Text { value: val },
                                cx,
                            );
                        });
                    }
                },
            ));
            let focus_inp = self.text_input.clone();
            window.defer(cx, move |window, cx| {
                focus_inp.focus_handle(cx).focus(window, cx);
            });
        } else if first.kind == "editor" {
            let prefill = first.prefill.clone().unwrap_or_default();
            self.editor_input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(3, 8)
                    .default_value(prefill)
            });
            let focus_ed = self.editor_input.clone();
            window.defer(cx, move |window, cx| {
                focus_ed.focus_handle(cx).focus(window, cx);
            });
        } else if first.kind == "ask" {
            for q in first.questions.as_deref().unwrap_or(&[]) {
                let inp = cx.new(|cx| InputState::new(window, cx).placeholder("其他回答（可选）…"));
                self.ask_custom_inputs.insert(q.id.clone(), inp);
            }
            let fh = self.focus_handle.clone();
            window.defer(cx, move |window, cx| {
                fh.focus(window, cx);
            });
        } else {
            let fh = self.focus_handle.clone();
            window.defer(cx, move |window, cx| {
                fh.focus(window, cx);
            });
        }
    }

    fn submit_primary(&self, session_id: String, first: &PendingRequest, cx: &mut Context<Self>) {
        let answer = match first.kind.as_str() {
            "approval" | "confirm" | "select" => build_choices(first)
                .into_iter()
                .find(|c| c.primary)
                .map(|c| c.answer),
            "input" => Some(PendingAnswer::Text {
                value: self.text_input.read(cx).value().to_string(),
            }),
            "editor" => Some(PendingAnswer::Text {
                value: self.editor_input.read(cx).value().to_string(),
            }),
            "ask" => {
                let answers: Vec<AskQuestionAnswer> = first
                    .questions
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|q| {
                        let selected = self.ask_selected.get(&q.id).cloned().unwrap_or_default();
                        let custom = self
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
                Some(PendingAnswer::Ask { answers })
            }
            _ => None,
        };
        if let Some(ans) = answer {
            let pid = first.id.clone();
            self.store.update(cx, |s, cx| {
                s.respond_pending(session_id, pid, ans, cx);
            });
        }
    }

    fn cancel_current(&self, session_id: String, first: &PendingRequest, cx: &mut Context<Self>) {
        let answer = match first.kind.as_str() {
            "approval" => PendingAnswer::Option {
                value: "deny".to_owned(),
            },
            "confirm" => PendingAnswer::Confirm { confirmed: false },
            _ => PendingAnswer::Cancel,
        };
        let pid = first.id.clone();
        self.store.update(cx, |s, cx| {
            s.respond_pending(session_id, pid, answer, cx);
        });
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mods = &event.keystroke.modifiers;
        let key = event.keystroke.key.as_str();
        let is_cmd_enter =
            key == "enter" && (mods.platform || mods.control) && !mods.alt && !mods.shift;
        let is_escape =
            key == "escape" && !mods.platform && !mods.control && !mods.alt && !mods.shift;

        if !is_cmd_enter && !is_escape {
            return;
        }

        let current = {
            let st = self.store.read(cx);
            st.selected_id.clone().and_then(|sid| {
                st.main_slot
                    .state
                    .as_ref()
                    .and_then(|s| s.pending.first().cloned())
                    .map(|p| (sid, p))
            })
        };
        let Some((sid, first)) = current else {
            return;
        };

        if is_cmd_enter {
            self.submit_primary(sid, &first, cx);
        } else {
            self.cancel_current(sid, &first, cx);
        }
        cx.stop_propagation();
        window.prevent_default();
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

        self.sync_pending(&session_id, &first, window, cx);

        let store = self.store.clone();
        let pid = first.id.clone();

        let card = v_flex()
            .id(SharedString::from(format!("pending-card-{pid}")))
            .track_focus(&self.focus_handle)
            .capture_key_down(cx.listener(Self::handle_key_down))
            .w_full()
            .gap(px(8.0))
            .px(px(14.0))
            .py(px(11.0))
            .rounded(px(theme_setup::RADIUS_LG))
            .border_1()
            .border_color(theme_setup::attention_border(cx))
            .bg(theme_setup::attention_bg(cx))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(px(7.0))
                            .child(
                                Icon::new(IconName::TriangleAlert)
                                    .size(px(13.0))
                                    .text_color(theme_setup::warning_fg(cx)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .text_color(cx.theme().foreground)
                                    .child(first.title.clone()),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap(px(6.0))
                            .child(kbd_chip("⌘↵", cx))
                            .child(kbd_chip("Esc", cx))
                            .when(extra > 0, |el| {
                                el.child(
                                    div()
                                        .px(px(6.0))
                                        .py(px(1.5))
                                        .rounded(px(theme_setup::RADIUS_SM))
                                        .bg(theme_setup::chip_bg(cx))
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_size(px(11.5))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!("+{extra}")),
                                )
                            }),
                    ),
            );

        let mut body = v_flex().gap_2();
        let mut footer = None;
        if let Some(msg) = first.message.clone() {
            let msg_copy = msg.clone();
            body = body.child(
                v_flex()
                    .rounded(px(theme_setup::RADIUS_SM))
                    .border_1()
                    .border_color(theme_setup::card_border(cx))
                    .bg(theme_setup::code_surface_bg(cx))
                    .overflow_hidden()
                    .child(
                        h_flex()
                            .items_start()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .id(SharedString::from(format!("pending-msg-scroll-{pid}")))
                                    .flex_1()
                                    .min_w_0()
                                    .max_h(px(160.0))
                                    .overflow_y_scrollbar()
                                    .px(px(10.0))
                                    .py_2()
                                    .when(first.kind == "approval", |el| {
                                        el.font_family(cx.theme().mono_font_family.clone())
                                    })
                                    .text_size(px(12.5))
                                    .text_color(cx.theme().foreground)
                                    .child(
                                        TextView::markdown(
                                            SharedString::from(format!("pending-msg-md-{pid}")),
                                            plain_text_to_markdown(&msg),
                                        )
                                        .selectable(true)
                                        .selection_format(SelectionFormat::Plain),
                                    ),
                            )
                            .child(
                                div().p(px(4.0)).flex_shrink_0().child(
                                    Clipboard::new(SharedString::from(format!(
                                        "copy-pending-msg-{pid}"
                                    )))
                                    .value(msg_copy)
                                    .tooltip("复制内容"),
                                ),
                            ),
                    ),
            );
        }

        match first.kind.as_str() {
            "approval" | "confirm" | "select" => {
                let mut btn_row = h_flex().flex_wrap().gap(px(6.0));
                for (i, choice) in build_choices(&first).into_iter().enumerate() {
                    let st_c = store.clone();
                    let sid_c = session_id.clone();
                    let pid_c = pid.clone();
                    let ans = choice.answer;
                    let mut b = Button::new(SharedString::from(format!("pend-choice-{i}")))
                        .small()
                        .rounded(px(theme_setup::RADIUS_SM))
                        .label(choice.label);
                    if choice.primary {
                        b = b.primary();
                    } else if choice.danger {
                        b = b.ghost().text_color(cx.theme().danger);
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
                            .small()
                            .rounded(px(theme_setup::RADIUS_SM))
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
                body = body.child(btn_row);
            }
            "input" => {
                let st_ok = store.clone();
                let sid_ok = session_id.clone();
                let pid_ok = pid.clone();
                let inp_ok = self.text_input.clone();

                let st_c = store.clone();
                let sid_c = session_id.clone();
                let pid_c = pid.clone();

                body = body.child(Input::new(&self.text_input).small()).child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("pend-input-cancel")
                                .ghost()
                                .small()
                                .rounded(px(theme_setup::RADIUS_SM))
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
                                .small()
                                .rounded(px(theme_setup::RADIUS_SM))
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

                body = body.child(Textarea::new(&self.editor_input).small()).child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("pend-ed-cancel")
                                .ghost()
                                .small()
                                .rounded(px(theme_setup::RADIUS_SM))
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
                                .small()
                                .rounded(px(theme_setup::RADIUS_SM))
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
                        .gap(px(5.0))
                        .p(px(10.0))
                        .rounded(px(theme_setup::RADIUS_MD))
                        .border_1()
                        .border_color(theme_setup::card_border(cx))
                        .bg(theme_setup::subtle_surface_bg(cx))
                        .child(
                            div()
                                .text_sm()
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
                                        if let Some(pos) = entry.iter().position(|x| x == &opt_str)
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
                        q_box = q_box.child(Input::new(inp).small());
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

                body = body.child(q_col);
                footer = Some(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("pend-ask-cancel")
                                .ghost()
                                .small()
                                .rounded(px(theme_setup::RADIUS_SM))
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
                                .small()
                                .rounded(px(theme_setup::RADIUS_SM))
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

        card.child(
            body.id(SharedString::from(format!("pending-body-{pid}")))
                .max_h(window.viewport_size().height * 0.32)
                .overflow_y_scrollbar(),
        )
        .when_some(footer, |el, actions| el.child(actions))
        .into_any_element()
    }
}
