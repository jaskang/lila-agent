//! The task detail panel: what a task asks, who has it, what came of it, its
//! history, and the person's actions on it (append, cancel, open its terminal).

use crate::app::{LilaApp, dot, s, status_color};
use crate::data;
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::input::Input;
use gpui::component::scroll::ScrollableElement;
use gpui::component::tag::Tag;
use gpui::component::text::TextView;
use gpui::component::{ActiveTheme, Sizable, StyledExt, h_flex, v_flex};
use gpui::prelude::*;
use gpui::*;
use serde_json::{Value, json};

pub fn state_tag(state: &str) -> Tag {
    match state {
        "blocked" => Tag::danger(),
        "done" => Tag::success(),
        "cancelled" => Tag::secondary(),
        "queued" => Tag::secondary(),
        _ => Tag::info(),
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

fn event_label(kind: &str) -> &str {
    match kind {
        "created" => "创建",
        "delivered" => "送达",
        "started" => "开始",
        "blocked" => "阻塞",
        "done" => "完成",
        "cancelled" => "取消",
        "requeued" => "退回队列",
        "edited" => "修改",
        "nudged" => "提醒",
        "stalled" => "无回应",
        "answered" => "回答",
        other => other,
    }
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
        .on_click(cx.listener(move |this, _, window, cx| this.open_task(open.clone(), window, cx)))
}

fn block(title: &str, cx: &App) -> Div {
    v_flex().gap_1().child(div().text_xs().font_semibold().text_color(cx.theme().muted_foreground).child(title.to_owned()))
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
            .gap_2()
            .flex_wrap()
            .text_sm()
            .child(div().text_color(theme.muted_foreground).child(format!("{} →", s(&t, "from"))))
            .child(dot(status_color(&to_status, cx)))
            .child(div().font_medium().child(to.clone()))
            .when_some(t["parent"].as_str().map(String::from), |d, p| {
                d.child(div().text_color(theme.muted_foreground).child("· 父任务")).child(task_link(&p, "parent", cx))
            })
            .child(div().text_color(theme.muted_foreground).child(format!("· 创建 {} · 更新 {}", data::clock(t["created_ms"].as_i64().unwrap_or(0)), data::clock(t["updated_ms"].as_i64().unwrap_or(0)))));

        let file = s(&t, "file");
        let actions = h_flex()
            .gap_2()
            .flex_wrap()
            .child({
                let m = to.clone();
                Button::new("detail-terminal").small().outline().label(format!("打开 {to} 的终端")).on_click(cx.listener(move |this, _, _, cx| {
                    this.member = Some(m.clone());
                    cx.notify();
                }))
            })
            .when(!file.is_empty(), |d| {
                let (cwd, file) = (cwd.clone(), file.clone());
                d.child(Button::new("detail-file").small().ghost().label("打开任务文件").on_click(move |_, _, _| data::open_path(&cwd, &file)))
            })
            .when(open, |d| {
                let tid = id.clone();
                if confirming {
                    d.child(Button::new("detail-cancel-yes").small().danger().label("确认取消").on_click(cx.listener(move |this, _, _, cx| {
                        this.confirm_cancel = None;
                        this.act("task.cancel", json!({"id": tid, "reason": "cancelled by the person in lila-app"}), false, cx);
                    })))
                    .child(Button::new("detail-cancel-no").small().ghost().label("算了").on_click(cx.listener(|this, _, _, cx| {
                        this.confirm_cancel = None;
                        cx.notify();
                    })))
                } else {
                    d.child(Button::new("detail-cancel").small().ghost().label("取消任务").on_click(cx.listener(move |this, _, _, cx| {
                        this.confirm_cancel = Some(tid.clone());
                        cx.notify();
                    })))
                }
            });

        let outcome = match state.as_str() {
            "done" => t["summary"].as_str().map(|x| (theme.green, format!("完成：{x}"))),
            "blocked" => t["block_reason"].as_str().map(|x| (theme.red, format!("阻塞：{x}"))),
            _ => None,
        };

        let body = s(&t, "body");
        let artifacts: Vec<String> = t["artifacts"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(String::from)).collect();
        let children: Vec<Value> = t["children"].as_array().cloned().unwrap_or_default();
        let events: Vec<Value> = t["events"].as_array().cloned().unwrap_or_default();

        let append_hint = if state == "queued" { "还在排队：追加的内容会直接写进任务，送达时一起发出。".to_owned() } else { format!("已经送达：追加的内容会作为更新发给 {to}。") };

        v_flex()
            .id("task-detail")
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .w(px(560.))
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_l_1()
            .border_color(theme.border)
            .shadow_lg()
            .occlude()
            .child(
                h_flex()
                    .h(px(40.))
                    .px_3()
                    .gap_2()
                    .flex_none()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(state_tag(&state).small().child(state_label(&state)))
                    .child(div().font_semibold().child(id.clone()))
                    .child(div().flex_1())
                    .child(Button::new("detail-close").small().ghost().label("关闭").on_click(cx.listener(|this, _, _, cx| this.close_task(cx)))),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .p_4()
                    .gap_4()
                    .child(div().text_lg().font_semibold().child(s(&t, "title")))
                    .child(meta)
                    .child(actions)
                    .when_some(outcome, |d, (accent, text)| {
                        d.child(div().p_2().rounded_md().border_l_2().border_color(accent).bg(accent.opacity(0.12)).text_sm().child(text))
                    })
                    .child(block("说明", cx).child(if body.trim().is_empty() {
                        div().text_sm().text_color(theme.muted_foreground).child("（没有正文）").into_any_element()
                    } else {
                        div().text_sm().child(TextView::markdown(SharedString::from(format!("body-{id}")), body.clone())).into_any_element()
                    }))
                    .when(!artifacts.is_empty(), |d| {
                        d.child(block("交付文件", cx).children(artifacts.into_iter().enumerate().map(|(n, a)| {
                            let (cwd, path) = (cwd.clone(), a.clone());
                            div()
                                .id(SharedString::from(format!("artifact-{n}")))
                                .text_sm()
                                .cursor_pointer()
                                .text_color(theme.blue)
                                .hover(|d| d.underline())
                                .child(a)
                                .on_click(move |_, _, _| data::open_path(&cwd, &path))
                        })))
                    })
                    .when(!children.is_empty(), |d| {
                        d.child(block("交出去的子任务", cx).children(children.into_iter().map(|c| {
                            let st = s(&c, "state");
                            h_flex()
                                .gap_2()
                                .text_sm()
                                .child(state_tag(&st).small().child(state_label(&st)))
                                .child(task_link(&s(&c, "id"), "child", cx))
                                .child(div().text_xs().text_color(theme.muted_foreground).flex_none().child(format!("→ {}", s(&c, "to"))))
                                .child(div().truncate().child(s(&c, "title")))
                        })))
                    })
                    .child(block("经过", cx).children(events.into_iter().map(|e| {
                        h_flex()
                            .gap_2()
                            .items_start()
                            .text_sm()
                            .child(div().flex_none().w(px(44.)).text_color(theme.muted_foreground).child(data::clock(e["at_ms"].as_i64().unwrap_or(0))))
                            .child(div().flex_none().w(px(72.)).font_medium().truncate().child(s(&e, "actor")))
                            .child(div().flex_none().w(px(56.)).text_color(theme.muted_foreground).child(event_label(&s(&e, "kind")).to_owned()))
                            .child(div().flex_1().min_w_0().child(s(&e, "detail")))
                    })))
                    .when(open, |d| {
                        let tid = id.clone();
                        let field = input.clone();
                        d.child(
                            block("追加说明", cx)
                                .child(div().text_xs().text_color(theme.muted_foreground).child(append_hint))
                                .child(
                                    h_flex().gap_1().child(div().flex_1().child(Input::new(&input).small())).child(Button::new("detail-append").small().primary().label("追加").on_click(cx.listener(
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
