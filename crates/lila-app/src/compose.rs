//! Writing tasks from the window: a new task for any member, and rewriting
//! one that is still queued.

use crate::app::LilaApp;
use crate::ui::{dot, s, status_color};
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::input::{Input, InputState, Textarea, TextareaState};
use gpui::component::scroll::ScrollableElement;
use gpui::component::{ActiveTheme, Sizable, StyledExt, h_flex, v_flex};
use crate::style::Control as _;
use gpui::prelude::*;
use gpui::*;
use serde_json::{Value, json};

/// The new-task form's fields.
pub struct Draft {
    pub to: Option<String>,
    pub title: Entity<InputState>,
    pub body: Entity<TextareaState>,
    pub parent: Entity<InputState>,
    pub error: Option<String>,
}

/// Rewriting a queued task: its new title and body.
pub struct Rewrite {
    pub id: String,
    pub title: Entity<InputState>,
    pub body: Entity<TextareaState>,
}

fn field(title: &str, cx: &App) -> Div {
    v_flex().gap_1().child(div().text_xs().font_semibold().text_color(cx.theme().muted_foreground).child(title.to_owned()))
}

impl LilaApp {
    /// Opens the new-task form, for `to` if given (else the owner).
    pub fn compose(&mut self, to: Option<String>, parent: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.close_task(cx);
        let to = to.or_else(|| self.members().iter().find(|m| m["owner"] == true).map(|m| s(m, "id")));
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("一句话说清要做什么"));
        let body = cx.new(|cx| TextareaState::new(window, cx).rows(8).placeholder("细节、验收标准、相关文件……（支持 Markdown）"));
        let parent = cx.new(|cx| {
            let p = InputState::new(window, cx).placeholder("可选，例如 T12");
            match &parent {
                Some(v) => p.default_value(v.clone()),
                None => p,
            }
        });
        title.update(cx, |t, cx| t.focus(window, cx));
        self.draft = Some(Draft { to, title, body, parent, error: None });
        cx.notify();
    }

    fn submit_draft(&mut self, cx: &mut Context<Self>) {
        let Some(d) = &self.draft else { return };
        let (Some(to), title) = (d.to.clone(), d.title.read(cx).value().trim().to_owned()) else { return };
        if title.is_empty() {
            if let Some(d) = &mut self.draft {
                d.error = Some("先写个标题".into());
            }
            cx.notify();
            return;
        }
        let body = d.body.read(cx).value().to_string();
        let parent = d.parent.read(cx).value().trim().to_owned();
        let params = json!({"to": to, "title": title, "body": body, "parent": (!parent.is_empty()).then_some(parent)});
        self.request("task.new", params, cx, |this, v, cx| {
            this.draft = None;
            this.show_task(s(&v, "id"), cx);
        });
    }

    /// Starts rewriting a queued task, its fields filled with what it says now.
    pub fn start_rewrite(&mut self, t: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let (title, body) = (s(t, "title"), s(t, "body"));
        let title = cx.new(|cx| InputState::new(window, cx).default_value(title));
        let body = cx.new(|cx| TextareaState::new(window, cx).rows(10).default_value(body));
        self.rewrite = Some(Rewrite { id: s(t, "id"), title, body });
        cx.notify();
    }

    pub fn save_rewrite(&mut self, cx: &mut Context<Self>) {
        let Some(r) = &self.rewrite else { return };
        let title = r.title.read(cx).value().trim().to_owned();
        if title.is_empty() {
            return;
        }
        let params = json!({"id": r.id, "title": title, "body": r.body.read(cx).value().to_string()});
        self.request("task.edit", params, cx, |this, _, cx| {
            this.rewrite = None;
            cx.notify();
        });
    }

    /// The rewrite form for the open task, if it is being rewritten.
    pub fn rewrite_form(&mut self, id: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        let r = self.rewrite.as_ref().filter(|r| r.id == id)?;
        let (title, body) = (r.title.clone(), r.body.clone());
        Some(
            v_flex()
                .gap_2()
                .child(field("标题", cx).child(Input::new(&title).control()))
                .child(field("说明", cx).child(Textarea::new(&body).area()))
                .child(
                    h_flex()
                        .gap_2()
                        .child(Button::new("rewrite-save").small().small_button().primary().label("保存").on_click(cx.listener(|this, _, _, cx| this.save_rewrite(cx))))
                        .child(Button::new("rewrite-discard").small().small_button().ghost().label("放弃").on_click(cx.listener(|this, _, _, cx| {
                            this.rewrite = None;
                            cx.notify();
                        }))),
                )
                .into_any_element(),
        )
    }

    pub fn compose_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme().clone();
        let d = self.draft.as_ref()?;
        let (to, title, body, parent, error) = (d.to.clone(), d.title.clone(), d.body.clone(), d.parent.clone(), d.error.clone());
        let members = self.members();
        let chosen = members.iter().find(|m| Some(s(m, "id")) == to).cloned();
        let hint = chosen.as_ref().map(|m| {
            let status = s(m, "status");
            let busy = m["task"].is_object();
            match (status.as_str(), busy) {
                ("gone", _) => "这个成员没有在运行：任务会排队，等团队启动后再送达。".to_owned(),
                (_, true) => format!("{} 手上有任务，这个会排在后面。", s(m, "id")),
                ("idle" | "done", false) => format!("{} 现在空闲，会马上收到。", s(m, "id")),
                _ => format!("{} 空下来就会收到。", s(m, "id")),
            }
        });
        Some(
            v_flex()
                .id("compose")
                .size_full()
                .text_color(theme.popover_foreground)
                .child(
                    h_flex()
                        .h(px(48.))
                        .pl(px(18.))
                        .pr(px(10.))
                        .gap_2()
                        .flex_none()
                        .border_b_1()
                        .border_color(crate::style::divider())
                        .child(div().font_semibold().child("新建任务"))
                        .when_some(self.draft.as_ref().map(|d| d.parent.read(cx).value().to_string()).filter(|p| !p.trim().is_empty()), |d, p| d.child(div().text_xs().text_color(theme.muted_foreground).child(format!("· 父任务 {p}"))))
                        .child(div().flex_1())
                        .child(Button::new("compose-close").ghost().xsmall().icon(gpui::component::IconName::Close).tooltip("关闭 (Esc)").on_click(cx.listener(|this, _, _, cx| {
                            this.draft = None;
                            cx.notify();
                        }))),
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
                        .child(div().text_xs().text_color(theme.muted_foreground).child(
                            "通常把需求交给 PM，由它拆分和分派；也可以直接派给某个成员。任务以你（human）的名义发出，完成后会出现在收件箱。",
                        ))
                        .child(field("派给", cx).child(h_flex().gap_1().flex_wrap().children(members.iter().map(|m| {
                            let id = s(m, "id");
                            let active = to.as_deref() == Some(id.as_str());
                            let pick = id.clone();
                            let status = s(m, "status");
                            Button::new(SharedString::from(format!("to-{id}")))
                                .small()
                                .when(active, |b| b.primary())
                                .when(!active, |b| b.outline())
                                .rounded_full()
                                .child(h_flex().gap_1().child(dot(status_color(&status, cx))).child(id.clone()))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(d) = &mut this.draft {
                                        d.to = Some(pick.clone());
                                    }
                                    cx.notify();
                                }))
                        }))))
                        .when_some(hint, |d, h| d.child(div().text_xs().text_color(theme.muted_foreground).child(h)))
                        .child(field("标题", cx).child(Input::new(&title).control()))
                        .child(field("说明", cx).child(Textarea::new(&body).area()))
                        .child(field("父任务", cx).child(Input::new(&parent).control()))
                        .when_some(error, |d, e| d.child(div().px_3().py(px(10.)).rounded(px(8.)).bg(theme.red.opacity(0.12)).text_size(px(13.)).child(e))),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .px_4()
                        .py_3()
                        .gap_2()
                        .justify_end()
                        .border_t_1()
                        .border_color(crate::style::divider())
                        .child(Button::new("compose-cancel").ghost().label("取消").on_click(cx.listener(|this, _, _, cx| {
                            this.draft = None;
                            cx.notify();
                        })))
                        .child(Button::new("compose-send").primary().label("派出").on_click(cx.listener(|this, _, _, cx| this.submit_draft(cx)))),
                )
                .into_any_element(),
        )
    }
}
