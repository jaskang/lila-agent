//! A member's conversation, drawn natively (architecture v3 §10): what it was
//! asked, what it said and did, and a box to write to it. Claude Code's TUI keeps
//! running underneath; the header switches to it (`tui`).

use crate::app::LilaApp;
use crate::data;
use crate::style::{self, muted, muted2};
use crate::ui::s;
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::input::{Textarea, TextareaState};
use gpui::component::spinner::Spinner;
use gpui::component::text::TextView;
use crate::style::Control as _;
use gpui::component::{Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
use gpui::prelude::*;
use gpui::*;
use serde_json::{Value, json};

/// The shown member's conversation, merged from reads of what changed since `seq`.
pub struct Conv {
    pub project: String,
    pub member: String,
    pub session: Option<String>,
    pub seq: i64,
    pub items: Vec<Value>,
}

impl Conv {
    /// Folds a `conversation.read` reply in: a new session starts over; items
    /// already shown are replaced where they stand, new ones go at the end.
    fn merge(&mut self, v: &Value) {
        let session = v["session"].as_str().map(String::from);
        if session.is_some() && session != self.session {
            self.session = session;
            self.items.clear();
            self.seq = 0;
        }
        for item in v["items"].as_array().into_iter().flatten() {
            match self.items.iter_mut().find(|i| i["id"] == item["id"]) {
                Some(old) => *old = item.clone(),
                None => self.items.push(item.clone()),
            }
        }
        self.seq = self.seq.max(v["seq"].as_i64().unwrap_or(0));
    }
}

/// Which part of the shown member's conversation the next fetch reads.
pub struct ConvWant {
    pub member: String,
    pub session: Option<String>,
    pub after: i64,
}

impl LilaApp {
    /// The key a member's view choice is remembered by.
    fn view_key(&self, member: &str) -> String {
        format!("{}/{member}", self.selected.as_deref().unwrap_or_default())
    }

    /// The member is shown as its conversation (else its terminal).
    pub fn shows_gui(&self, m: &Value) -> bool {
        m["caps"]["gui"] == true && !self.tui.contains(&self.view_key(&s(m, "id")))
    }

    pub fn set_view(&mut self, member: &str, tui: bool, cx: &mut Context<Self>) {
        let key = self.view_key(member);
        if tui { self.tui.insert(key) } else { self.tui.remove(&key) };
        data::save_views(self.paths(), &self.tui);
        self.refresh_now();
        cx.notify();
    }

    /// The header's two-way switch: the conversation Lila draws, or the CLI's own terminal.
    pub fn view_switch(&self, m: &Value, cx: &mut Context<Self>) -> impl IntoElement {
        let gui = self.shows_gui(m);
        let member = s(m, "id");
        let side = |id: &'static str, icon: IconName, tip: &'static str, on: bool, tui: bool| {
            let member = member.clone();
            Button::new(id)
                .ghost()
                .xsmall()
                .icon(Icon::new(icon).text_color(if on { style::fg() } else { muted() }))
                .when(on, |b| b.bg(gpui::Hsla::from(rgba(0xffffff14))))
                .tooltip(tip)
                .on_click(cx.listener(move |this, _, _, cx| this.set_view(&member, tui, cx)))
        };
        h_flex()
            .gap(px(2.))
            .p(px(2.))
            .rounded(px(style::R_CTL))
            .border_1()
            .border_color(style::divider())
            .child(side("view-gui", IconName::GalleryVerticalEnd, "对话", gui, false))
            .child(side("view-tui", IconName::SquareTerminal, "终端", !gui, true))
    }

    /// What the next fetch reads of the shown member's conversation, if it is shown as one.
    pub fn conv_want(&self) -> Option<ConvWant> {
        let m = self.shown_member()?;
        if !self.shows_gui(&m) {
            return None;
        }
        let member = s(&m, "id");
        let same = self.conv.as_ref().filter(|c| Some(&c.project) == self.selected.as_ref() && c.member == member);
        Some(ConvWant { member, session: same.and_then(|c| c.session.clone()), after: same.map_or(0, |c| c.seq) })
    }

    /// A fetched read, for the member it was asked for.
    pub fn take_conv(&mut self, v: Value) {
        let (Some(project), Some(member)) = (self.selected.clone(), v["member"].as_str().map(String::from)) else { return };
        if self.member.as_deref() != Some(member.as_str()) {
            return;
        }
        // Following along: new items keep the end in view.
        let at_end = {
            let h = &self.conv_scroll;
            h.max_offset().y - (-h.offset().y) < px(48.)
        };
        let before = self.conv.as_ref().map(|c| (c.items.len(), c.seq));
        match &mut self.conv {
            Some(c) if c.project == project && c.member == member => c.merge(&v),
            _ => {
                let mut c = Conv { project, member, session: None, seq: 0, items: Vec::new() };
                c.merge(&v);
                self.conv = Some(c);
                self.conv_open.clear();
            }
        }
        let after = self.conv.as_ref().map(|c| (c.items.len(), c.seq));
        if before.is_none() || (at_end && before != after) {
            self.conv_scroll.scroll_to_bottom();
        }
    }

    fn conv_input(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextareaState> {
        self.conv_input.get_or_insert_with(|| cx.new(|cx| TextareaState::new(window, cx).rows(2).placeholder("消息…"))).clone()
    }

    fn send_to_member(&mut self, member: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.conv_input.clone() else { return };
        let text = input.read(cx).value().trim().to_owned();
        if text.is_empty() {
            return;
        }
        input.update(cx, |t, cx| t.set_value("", window, cx));
        self.request("conversation.send", json!({"member": member, "text": text}), cx, |this, _, _| this.refresh_now());
    }

    /// The center card's body for a member shown as its conversation.
    pub fn conversation_view(&mut self, m: &Value, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let member = s(m, "id");
        let input = self.conv_input(window, cx);
        let items: Vec<Value> = self.conv.as_ref().filter(|c| c.member == member).map(|c| c.items.clone()).unwrap_or_default();
        let rows: Vec<AnyElement> = items.iter().filter_map(|i| self.item_row(i, cx)).collect();
        let to = member.clone();
        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                div().id("conversation").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.conv_scroll).child(
                    h_flex().w_full().justify_center().child(v_flex().w_full().max_w(px(760.)).px_5().pt_4().pb_6().gap_3().children(rows)),
                ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_end()
                    .border_t_1()
                    .border_color(style::divider())
                    .on_key_down(cx.listener(move |this, ev: &KeyDownEvent, window, cx| {
                        let k = &ev.keystroke;
                        if k.key == "enter" && k.modifiers.platform {
                            this.send_to_member(to.clone(), window, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(div().flex_1().child(Textarea::new(&input).area()))
                    .child({
                        let to = member.clone();
                        Button::new("conv-send").primary().icon(IconName::ArrowUp).tooltip("⌘↩").on_click(cx.listener(move |this, _, window, cx| this.send_to_member(to.clone(), window, cx)))
                    }),
            )
            .into_any_element()
    }

    fn item_row(&self, i: &Value, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = s(i, "id");
        let text = s(i, "text");
        Some(match i["kind"].as_str()? {
            "user_message" if i["from"] == "lila" => lila_row(&id, &text, i["task"].as_str(), cx),
            "user_message" => h_flex()
                .w_full()
                .justify_end()
                .child(div().max_w(relative(0.8)).px_3().py_2().rounded(px(style::R_CTL)).bg(style::fill()).text_size(px(13.)).line_height(relative(1.55)).child(text))
                .into_any_element(),
            "agent_message" => div()
                .w_full()
                .text_size(px(13.))
                // Inline code as a quiet chip, not the accent color.
                .child(TextView::markdown(SharedString::from(format!("msg-{id}")), text).inline_code_style(StyleRefinement::default().bg(style::fill()).text_color(style::fg())))
                .into_any_element(),
            "thought" => div().w_full().pl_3().border_l_2().border_color(style::divider()).text_xs().text_color(muted2()).child(first_lines(&text, 3)).into_any_element(),
            "tool_call" => self.tool_row(i, cx),
            _ => return None,
        })
    }

    fn tool_row(&self, i: &Value, cx: &mut Context<Self>) -> AnyElement {
        let id = s(i, "id");
        let open = self.conv_open.contains(&id);
        let state = s(i, "state");
        let icon = match i["tool"].as_str() {
            Some("execute") => IconName::SquareTerminal,
            Some("read") => IconName::FileText,
            Some("edit") => IconName::Replace,
            Some("search") => IconName::Search,
            Some("fetch") => IconName::Globe,
            Some("delete") => IconName::Delete,
            _ => IconName::Asterisk,
        };
        let diffs: Vec<Value> = i["diffs"].as_array().cloned().unwrap_or_default();
        let output = i["output"].as_str().map(String::from);
        let has_more = !diffs.is_empty() || output.is_some();
        let toggle = id.clone();
        let head = h_flex()
            .id(SharedString::from(format!("tool-{id}")))
            .h(px(24.))
            .gap_2()
            .px_1()
            .rounded(px(6.))
            .when(has_more, |d| d.cursor_pointer().hover(|d| d.bg(style::hover())))
            .child(Icon::new(icon).size(px(13.)).text_color(muted()))
            .child(div().flex_1().min_w_0().truncate().font_family(lila_term::theme::FONT).text_size(px(12.)).text_color(muted()).child(s(i, "title")))
            .when(state == "running", |d| d.child(Spinner::new().xsmall().color(style::blue())))
            .when(state == "failed", |d| d.child(Icon::new(IconName::CircleX).size(px(13.)).text_color(style::red())))
            .when(has_more, |d| d.child(Icon::new(if open { IconName::ChevronUp } else { IconName::ChevronDown }).size(px(12.)).text_color(muted2())))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.conv_open.remove(&toggle) {
                    this.conv_open.insert(toggle.clone());
                }
                cx.notify();
            }));
        v_flex()
            .w_full()
            .child(head)
            .when(open, |d| {
                d.child(
                    v_flex()
                        .ml(px(26.))
                        .mt_1()
                        .gap_2()
                        .children(diffs.iter().map(diff_block))
                        .when_some(output, |d, o| {
                            d.child(
                                div()
                                    .id(SharedString::from(format!("out-{id}")))
                                    .max_h(px(240.))
                                    .overflow_y_scroll()
                                    .px_3()
                                    .py_2()
                                    .rounded(px(6.))
                                    .bg(style::well())
                                    .font_family(lila_term::theme::FONT)
                                    .text_size(px(12.))
                                    .text_color(muted())
                                    .child(o),
                            )
                        }),
                )
            })
            .into_any_element()
    }
}

/// Something Lila delivered: a task as its id and title (opens it), else the words, muted.
fn lila_row(id: &str, text: &str, task: Option<&str>, cx: &mut Context<LilaApp>) -> AnyElement {
    let inner: Vec<&str> = text.lines().filter(|l| !l.trim_start().starts_with("<lila") && !l.trim_start().starts_with("</lila")).filter(|l| !l.trim().is_empty()).collect();
    match task {
        Some(t) => {
            let open = t.to_owned();
            h_flex()
                .id(SharedString::from(format!("lila-{id}")))
                .gap_2()
                .cursor_pointer()
                .child(style::tag(style::blue(), t.to_owned()))
                .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).font_medium().child(inner.first().copied().unwrap_or_default().to_owned()))
                .on_click(cx.listener(move |this, _, _, cx| this.show_task(open.clone(), cx)))
                .into_any_element()
        }
        None => div().w_full().text_xs().text_color(muted2()).child(first_lines(&inner.join("\n"), 2)).into_any_element(),
    }
}

/// A file's change: its path, then the lines colored by what happened to them.
fn diff_block(d: &Value) -> AnyElement {
    let lines: Vec<String> = d["lines"].as_array().into_iter().flatten().filter_map(|l| l.as_str().map(String::from)).collect();
    v_flex()
        .rounded(px(6.))
        .overflow_hidden()
        .bg(style::well())
        .font_family(lila_term::theme::FONT)
        .text_size(px(12.))
        .child(div().px_3().py_1().text_color(muted()).border_b_1().border_color(style::divider()).child(s(d, "path")))
        .children(lines.into_iter().map(|l| {
            let (fg, bg) = match l.chars().next() {
                Some('+') => (style::tint(style::green(), 0.3), Some(style::green().opacity(0.12))),
                Some('-') => (style::tint(style::red(), 0.3), Some(style::red().opacity(0.12))),
                Some('@') => (muted2(), None),
                _ => (muted(), None),
            };
            div().px_3().text_color(fg).when_some(bg, |d, b| d.bg(b)).child(if l.is_empty() { " ".to_owned() } else { l })
        }))
        .into_any_element()
}

/// The first `n` lines, marked when there is more.
fn first_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= n { text.to_owned() } else { format!("{}…", lines[..n].join("\n")) }
}
