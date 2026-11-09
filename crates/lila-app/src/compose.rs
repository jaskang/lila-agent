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
    /// The task this one is a subtask of.
    pub parent: Option<String>,
    pub error: Option<String>,
}

/// Rewriting a queued task: its new title and body.
pub struct Rewrite {
    pub id: String,
    pub title: Entity<InputState>,
    pub body: Entity<TextareaState>,
}

impl LilaApp {
    /// Opens the new-task form, for `to` if given (else the owner).
    pub fn compose(&mut self, to: Option<String>, parent: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.close_task(cx);
        let to = to.or_else(|| self.members().iter().find(|m| m["owner"] == true).map(|m| s(m, "id")));
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("标题"));
        let body = cx.new(|cx| TextareaState::new(window, cx).rows(12).placeholder("说明"));
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
        let params = json!({"to": to, "title": title, "body": body, "parent": d.parent});
        let root = d.parent.is_none();
        self.request("task.new", params, cx, move |this, v, cx| {
            this.draft = None;
            // A new request shows as one; a subtask is pushed over the task it is under.
            if root { this.select_request(s(&v, "id"), cx) } else { this.show_task(s(&v, "id"), cx) }
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
                .child(Input::new(&title).control())
                .child(Textarea::new(&body).area())
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
                        .child(div().font_semibold().child(match &parent {
                            Some(p) => format!("{p} 的子任务"),
                            None => "新建任务".into(),
                        }))
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
                        .gap_4()
                        // Who gets it: the PM by default, which splits and hands out the work.
                        .child(h_flex().gap_1().flex_wrap().children(members.iter().map(|m| {
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
                        })))
                        .child(Input::new(&title).control())
                        .child(Textarea::new(&body).area())
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

    /// A project with its team running and nothing asked yet: the box to ask the PM,
    /// and where else work comes from (its sources).
    pub fn first_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let ask = match &self.ask {
            Some(a) => a.clone(),
            None => {
                let a = cx.new(|cx| TextareaState::new(window, cx).rows(5).placeholder("新需求…"));
                a.update(cx, |t, cx| t.focus(window, cx));
                self.ask = Some(a.clone());
                a
            }
        };
        let sources: Vec<Value> = self.snap.status.as_ref().and_then(|v| v["sources"].as_array().cloned()).unwrap_or_default();
        let source_row = |n: usize, src: &Value| {
            let error = src["error"].as_str().map(String::from);
            let when = src["last_ms"].as_i64().map(crate::requests::ago);
            h_flex()
                .id(SharedString::from(format!("first-source-{n}")))
                .gap(px(6.))
                .text_xs()
                .text_color(crate::style::muted())
                .child(gpui::component::Icon::new(gpui::component::IconName::Github).size(px(13.)))
                .child(s(src, "query"))
                .when_some(when.filter(|_| error.is_none()), |d, w| d.child(div().text_color(crate::style::muted2()).child(w)))
                .when_some(error, |d, e| d.child(dot(crate::style::red())).tooltip(move |window, cx| gpui::component::tooltip::Tooltip::new(e.clone()).build(window, cx)))
        };
        // What already waits for the person (a member at its trust prompt, say), below.
        let loose = self.loose_inbox();
        let waiting: Vec<AnyElement> = loose.iter().map(|i| self.loose_row(i, cx)).collect();
        v_flex()
            .flex_1()
            .items_center()
            .px_6()
            .pt(relative(0.18))
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(560.))
                    .gap_3()
                    .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                        let k = &ev.keystroke;
                        if k.key == "enter" && k.modifiers.platform {
                            this.submit_ask(cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(Textarea::new(&ask).area())
                    .child(
                        h_flex()
                            .gap_3()
                            .children(sources.iter().enumerate().map(|(n, src)| source_row(n, src)))
                            .child(div().flex_1())
                            .child(Button::new("first-ask").primary().label("交给助理").tooltip("⌘↩").on_click(cx.listener(|this, _, _, cx| this.submit_ask(cx)))),
                    )
                    .when(!waiting.is_empty(), |d| d.child(v_flex().pt_6().gap(px(2.)).child(crate::requests::sublabel("需要你", waiting.len())).children(waiting))),
            )
            .into_any_element()
    }

    /// The first line is the title (cut when long); the whole text, when there is more, the body.
    fn submit_ask(&mut self, cx: &mut Context<Self>) {
        let Some(ask) = &self.ask else { return };
        let text = ask.read(cx).value().trim().to_owned();
        let Some(first) = text.lines().next().map(str::trim).filter(|l| !l.is_empty()) else { return };
        let Some(to) = self.members().iter().find(|m| m["owner"] == true).map(|m| s(m, "id")) else { return };
        let title = if first.chars().count() > 80 { format!("{}…", first.chars().take(79).collect::<String>()) } else { first.to_owned() };
        let body = if title == text { String::new() } else { text.clone() };
        self.request("task.new", json!({"to": to, "title": title, "body": body}), cx, |this, v, cx| {
            this.ask = None;
            this.select_request(s(&v, "id"), cx);
        });
    }
}
