//! The task detail panel: what a task asks, who has it, what came of it, its
//! history, and the person's actions on it (append, cancel, open its terminal).

use crate::app::LilaApp;
use crate::ui::{dot, s, status_color};
use crate::data;
use crate::style::{self, muted, muted2};
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::input::Input;
use gpui::component::scroll::ScrollableElement;
use gpui::component::text::TextView;
use gpui::component::{ActiveTheme, IconName, Sizable, StyledExt, h_flex, v_flex};
use crate::style::Control as _;
use gpui::prelude::*;
use gpui::*;
use serde_json::{Value, json};

/// A task's id (or state word) as a tag in the state's color; queued and cancelled are neutral.
pub fn state_tag(state: &str, text: impl Into<SharedString>) -> Div {
    match state {
        "blocked" => style::tag(style::red(), text),
        "done" => style::tag(style::green(), text),
        "cancelled" | "queued" => style::tag_neutral(text),
        _ => style::tag(style::blue(), text),
    }
}

pub fn state_label(state: &str) -> &'static str {
    match state {
        "queued" => "排队",
        "delivered" => "已送达",
        "in_progress" => "进行中",
        "blocked" => "阻塞",
        "done" => "完成",
        "cancelled" => "已取消",
        _ => "未知",
    }
}

/// One line of a task's history, for the person: who, what, and the words that
/// came with it. lilad's own bookkeeping (terminals, rounds, sign-offs) stays out.
fn event_line(actor: &str, kind: &str, detail: &str) -> (String, Option<String>) {
    let who = match actor {
        "human" => "你",
        "lilad" => "Lila",
        other => other,
    };
    let (what, said) = match kind {
        "created" => ("创建", false),
        "delivered" => ("送达", false),
        "started" => ("开始", false),
        "blocked" => ("卡住", true),
        "done" => ("完成", true),
        "cancelled" => ("取消", true),
        "requeued" => ("退回队列", false),
        "edited" => ("修改", false),
        "nudged" => ("提醒", false),
        "stalled" => ("无回应", false),
        "answered" => ("回答", true),
        "fresh_session" => ("换新会话", false),
        "accepted" => ("通过", false),
        "rejected" => ("打回", true),
        "committed" => ("提交", !detail.contains(' ')),
        "commit_failed" => ("提交失败", true),
        "check" if detail.starts_with("running") => ("开始检查", false),
        "check" if detail == "passed" => ("检查通过", false),
        "check" => ("检查没通过", true),
        "sent_back" => ("返工", false),
        "flow" => ("开始流程", false),
        "flow_done" => ("流程结束", false),
        "flow_failed" => ("流程失败", true),
        "flow_held" => ("流程暂停", false),
        "flow_cancelled" => ("流程取消", false),
        "budget" => ("调整预算", false),
        other => (other, true),
    };
    (format!("{who} {what}"), (said && !detail.is_empty()).then(|| detail.to_owned()))
}

/// A clickable task id that opens it in the panel.
pub fn task_link(id: &str, key: &str, cx: &mut Context<LilaApp>) -> Stateful<Div> {
    let open = id.to_owned();
    div()
        .id(SharedString::from(format!("link-{key}-{id}")))
        .flex_none()
        .cursor_pointer()
        .text_color(cx.theme().blue)
        .hover(|d| d.underline())
        .child(id.to_owned())
        .on_click(cx.listener(move |this, _, _, cx| this.show_task(open.clone(), cx)))
}

fn block(title: &str, _cx: &App) -> Div {
    v_flex().gap_2().child(div().text_xs().font_semibold().text_color(muted()).child(title.to_owned()))
}

/// A flow step as the window names it (`code`: implement → check → verify).
pub fn step_label(action: &str) -> String {
    match action {
        "implement" => "实现".into(),
        "check" => "检查".into(),
        "verify" => "验证".into(),
        "review" => "审阅".into(),
        other => other.to_owned(),
    }
}

/// Lines with a muted bullet each.
fn bullets(items: &[String], color: Hsla) -> Div {
    v_flex().gap_1().text_size(px(13.)).children(items.iter().map(|i| {
        h_flex().gap_2().items_start().child(div().flex_none().text_color(muted2()).child("•")).child(div().flex_1().min_w_0().text_color(color).child(i.clone()))
    }))
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect()
}

/// The flow a task is a step of: its steps (done ticked, the current one blue),
/// the round, and what the last check said.
fn flow_block(f: &Value) -> Div {
    let steps = strings(&f["steps"]);
    let at = f["step"].as_u64().unwrap_or(0) as usize;
    let finished = f["state"] == "done";
    let round = f["round"].as_u64().unwrap_or(0);
    let mut chips = h_flex().gap_1().flex_wrap();
    for (i, st) in steps.iter().enumerate() {
        if i > 0 {
            chips = chips.child(div().text_xs().text_color(muted2()).child("›"));
        }
        let label = step_label(st);
        chips = chips.child(if finished || i < at {
            style::tag(style::green(), format!("✓ {label}"))
        } else if i == at {
            style::tag(style::blue(), format!("● {label}"))
        } else {
            style::tag_neutral(label)
        });
    }
    let head = h_flex()
        .gap(px(6.))
        .child(div().text_xs().font_semibold().text_color(muted()).child("流程"))
        .when(round > 0, |d| d.child(div().text_xs().text_color(muted2()).child(format!("返工 {round}/{}", f["max_rounds"].as_u64().unwrap_or(0)))));
    let check = f["last_check"].as_object().map(|c| {
        let detail = c.get("detail").and_then(Value::as_str).unwrap_or_default();
        let at = data::clock(c.get("at_ms").and_then(Value::as_i64).unwrap_or(0));
        let cmd = f["check"].as_str().unwrap_or_default();
        if detail == "passed" { format!("{at} `{cmd}` 通过") } else { format!("{at} `{cmd}` 没通过：{detail}") }
    });
    v_flex().gap_2().child(head).child(chips).when_some(check, |d, c| d.child(div().text_xs().text_color(muted()).child(c)))
}

impl LilaApp {
    pub fn detail_panel(&mut self, t: Value, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let id = s(&t, "id");
        let state = s(&t, "state");
        let to = s(&t, "to");
        let open = !matches!(state.as_str(), "done" | "cancelled");
        let cwd = self.project().map(|p| p.cwd).unwrap_or_default();
        let to_status = self.members().iter().find(|m| m["id"] == to.as_str()).map(|m| s(m, "status")).unwrap_or_default();
        let input = self.append_input(window, cx);
        let confirming = self.confirm_cancel.as_deref() == Some(id.as_str());

        let meta = h_flex()
            .gap(px(6.))
            .flex_wrap()
            .text_xs()
            .text_color(muted())
            .child(dot(status_color(&to_status, cx)))
            .child(div().text_color(style::fg()).child(to.clone()))
            .child(div().text_color(theme.muted_foreground).child(data::clock(t["updated_ms"].as_i64().unwrap_or(0))))
            .when_some(t["parent"].as_str().map(String::from), |d, p| d.child(div().text_color(theme.muted_foreground).child("·")).child(task_link(&p, "parent", cx)));

        let file = s(&t, "file");
        let actions = h_flex()
            .gap_2()
            .flex_wrap()
            .child({
                let m = to.clone();
                Button::new("detail-terminal").small().small_button().outline().icon(IconName::SquareTerminal).label("打开终端").on_click(cx.listener(move |this, _, _, cx| {
                    this.member = Some(m.clone());
                    cx.notify();
                }))
            })
            .when(!file.is_empty(), |d| {
                let (cwd, file) = (cwd.clone(), file.clone());
                d.child(Button::new("detail-file").small().small_button().ghost().icon(IconName::File).label("任务文件").on_click(move |_, _, _| data::open_path(&cwd, &file)))
            })
            .when(state == "queued" && self.rewrite.is_none(), |d| {
                let task = t.clone();
                d.child(Button::new("detail-rewrite").small().small_button().ghost().label("改写").on_click(cx.listener(move |this, _, window, cx| this.start_rewrite(&task, window, cx))))
            })
            .when(open, |d| {
                let (to, parent) = (to.clone(), id.clone());
                d.child(Button::new("detail-subtask").small().small_button().ghost().label("派子任务").on_click(cx.listener(move |this, _, window, cx| {
                    this.compose(Some(to.clone()), Some(parent.clone()), window, cx)
                })))
            })
            .child(div().flex_1())
            .when(open, |d| {
                let tid = id.clone();
                if confirming {
                    d.child(div().text_xs().text_color(muted()).child(format!("取消后通知 {to} 和 PM"))).child(Button::new("detail-cancel-yes").small().small_button().outline().text_color(gpui::Hsla::from(rgba(0xff6961ff))).label("确认取消").on_click(cx.listener(move |this, _, _, cx| {
                        this.confirm_cancel = None;
                        this.act("task.cancel", json!({"id": tid, "reason": "cancelled by the person in lila-app"}), false, cx);
                    })))
                    .child(Button::new("detail-cancel-no").small().small_button().ghost().label("算了").on_click(cx.listener(|this, _, _, cx| {
                        this.confirm_cancel = None;
                        cx.notify();
                    })))
                } else {
                    d.child(Button::new("detail-cancel").small().small_button().ghost().text_color(gpui::Hsla::from(rgba(0xff6961ff))).label("取消任务").on_click(cx.listener(move |this, _, _, cx| {
                        this.confirm_cancel = Some(tid.clone());
                        cx.notify();
                    })))
                }
            });

        let outcome = match state.as_str() {
            "done" => t["summary"].as_str().map(|x| (style::green(), format!("完成：{x}"))),
            "blocked" => t["block_reason"].as_str().map(|x| (style::red(), format!("阻塞：{x}"))),
            _ => None,
        };

        let decisions = strings(&t["decisions"]);
        let unverified = strings(&t["unverified"]);
        let brief = t["brief"].clone();
        let quote = brief["quote"].as_str().map(String::from);
        let quote_from = brief["quote_from"].as_str().filter(|f| *f != id).map(String::from);
        let accept = strings(&brief["accept"]);
        let out_of_scope = strings(&brief["out_of_scope"]);
        let body = s(&t, "body");
        let artifacts: Vec<String> = t["artifacts"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(String::from)).collect();
        let children: Vec<Value> = t["children"].as_array().cloned().unwrap_or_default();
        let events: Vec<Value> = t["events"].as_array().cloned().unwrap_or_default();

        let rewrite = self.rewrite_form(&id, cx);
        let rewriting = rewrite.is_some();

        crate::ui::drawer_frame(v_flex(), 600.)
            .id("task-detail")
            .text_color(style::fg())
            .child(
                h_flex()
                    .h(px(48.))
                    .pl(px(18.))
                    .pr(px(10.))
                    .gap_2()
                    .flex_none()
                    .border_b_1()
                    .border_color(style::divider())
                    .child(state_tag(&state, state_label(&state)))
                    .child(div().font_semibold().child(id.clone()))
                    .child(div().flex_1())
                    .child(Button::new("detail-close").ghost().xsmall().icon(IconName::Close).tooltip("关闭 (Esc)").on_click(cx.listener(|this, _, _, cx| this.close_task(cx)))),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .px_5()
                    .pt_5()
                    .pb_6()
                    .gap_5()
                    .child(v_flex().gap(px(6.)).child(div().text_size(px(19.)).font_semibold().line_height(relative(1.3)).child(s(&t, "title"))).child(meta))
                    .child(actions)
                    .when_some(outcome, |d, (accent, text)| {
                        d.child(div().px_3().py(px(10.)).rounded(px(8.)).bg(accent.opacity(0.12)).text_color(style::tint(accent, 0.4)).text_size(px(13.)).child(text))
                    })
                    .when(!decisions.is_empty(), |d| d.child(block("决定", cx).child(bullets(&decisions, style::fg()))))
                    .when(!unverified.is_empty(), |d| {
                        d.child(v_flex().gap_2().child(div().text_xs().font_semibold().text_color(style::yellow()).child("未验证")).child(bullets(&unverified, style::fg())))
                    })
                    .when(t["flow"].is_object(), |d| d.child(flow_block(&t["flow"])))
                    .when_some(quote, |d, q| {
                        let head = h_flex().gap(px(6.)).text_xs().child(div().font_semibold().text_color(muted()).child("原话")).when_some(quote_from, |h, from| {
                            h.child(div().text_color(muted2()).child("来自")).child(task_link(&from, "quote", cx))
                        });
                        d.child(v_flex().gap_2().child(head).child(div().border_l_2().border_color(style::blue()).pl_3().py(px(2.)).text_size(px(13.)).line_height(relative(1.6)).child(q)))
                    })
                    .when(!accept.is_empty(), |d| d.child(block("验收标准", cx).child(bullets(&accept, style::fg()))))
                    .when(!out_of_scope.is_empty(), |d| d.child(block("不做", cx).child(bullets(&out_of_scope, muted()))))
                    .when_some(rewrite, |d, form| d.child(form))
                    .when(!rewriting && !body.trim().is_empty(), |d| d.child(div().text_size(px(13.)).child(TextView::markdown(SharedString::from(format!("body-{id}")), body.clone()))))
                    .when(!artifacts.is_empty(), |d| {
                        d.child(block("交付文件", cx).children(artifacts.into_iter().enumerate().map(|(n, a)| {
                            let (cwd, path) = (cwd.clone(), a.clone());
                            div()
                                .id(SharedString::from(format!("artifact-{n}")))
                                .text_size(px(13.))
                                .cursor_pointer()
                                .text_color(style::blue())
                                .hover(|d| d.underline())
                                .child(a)
                                .on_click(move |_, _, _| data::open_path(&cwd, &path))
                        })))
                    })
                    .when(!children.is_empty(), |d| {
                        d.child(block("子任务", cx).children(children.into_iter().map(|c| {
                            let st = s(&c, "state");
                            let cid = s(&c, "id");
                            let open = cid.clone();
                            h_flex()
                                .id(SharedString::from(format!("child-{cid}")))
                                .px_2()
                                .py(px(7.))
                                .gap_2()
                                .rounded(px(style::R_CTL))
                                .cursor_pointer()
                                .hover(|d| d.bg(style::hover()))
                                .text_size(px(13.))
                                .child(state_tag(&st, cid))
                                .child(div().flex_1().min_w_0().truncate().child(s(&c, "title")))
                                .child(div().text_xs().text_color(muted()).flex_none().child(s(&c, "to")))
                                .on_click(cx.listener(move |this, _, _, cx| this.show_task(open.clone(), cx)))
                        })))
                    })
                    .when(!events.is_empty(), |d| d.child(block("经过", cx).child({
                        // A thin line down the left, a dot per event; the latest one is blue.
                        let n = events.len();
                        v_flex().ml_1().pl_4().gap(px(10.)).border_l_1().border_color(gpui::Hsla::from(rgba(0xffffff1a))).children(events.into_iter().enumerate().map(move |(i, e)| {
                            let (line, detail) = event_line(&s(&e, "actor"), &s(&e, "kind"), &s(&e, "detail"));
                            h_flex()
                                .relative()
                                .gap_2()
                                .items_start()
                                .text_xs()
                                .child(div().absolute().left(px(-20.)).top(px(4.)).size(px(7.)).rounded_full().bg(if i + 1 == n { style::blue() } else { gpui::Hsla::from(rgba(0x48484eff)) }))
                                .child(div().flex_none().text_color(muted()).child(data::clock(e["at_ms"].as_i64().unwrap_or(0))))
                                .child(div().flex_none().child(line))
                                .when_some(detail, |d, detail| d.child(div().flex_1().min_w_0().text_color(muted2()).child(detail)))
                        }))
                    })))
                    .when(open, |d| {
                        let tid = id.clone();
                        let field = input.clone();
                        d.child(
                            v_flex()
                                .child(
                                    h_flex().gap(px(6.)).child(div().flex_1().child(Input::new(&input).control())).child(Button::new("detail-append").primary().label("追加").on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            let text = field.read(cx).value().to_string();
                                            if text.trim().is_empty() {
                                                return;
                                            }
                                            field.update(cx, |f, cx| f.set_value("", window, cx));
                                            this.act("task.edit", json!({"id": tid, "append": text}), false, cx);
                                        },
                                    ))),
                                ),
                        )
                    }),
            )
    }
}
