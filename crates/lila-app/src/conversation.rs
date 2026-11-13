//! A thread's conversation, drawn natively (architecture v3 §10): what it was
//! asked, what it said and did, and a box to write to it.
//!
//! The list is gpui-kit's `MessageScroller`, a virtual list: only the rows in view
//! are laid out, so a few hundred items cost the same as a few. What it shows is
//! `LilaApp::conv_rows`, derived from the items (a run of tool calls and thoughts
//! is one row); `sync_conv` keeps the list's own bookkeeping in step with it.

use crate::app::LilaApp;
use crate::data;
use crate::style::{self, muted, muted2};
use crate::ui::s;
use gpui::component::clipboard::Clipboard;
use gpui::component::collapsible::Collapsible;
use gpui::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui::component::spinner::Spinner;
use gpui::component::text::TextView;
use gpui::component::{Icon, IconName, Sizable, h_flex, v_flex};
use gpui::prelude::*;
use gpui::*;
use serde_json::Value;
use std::ops::Range;

/// The shown thread's conversation, merged from reads of what changed since `seq`.
pub struct Conv {
    pub thread: String,
    pub session: Option<String>,
    pub seq: i64,
    pub items: Vec<Value>,
    /// How many times `items[i]` was replaced (a tool call that finished): rows measured
    /// before that are measured again.
    ver: Vec<u32>,
    /// The model and effort it runs on, and what the person may change (`thread.read`'s `config`).
    pub config: Value,
    /// The cards it waits on: its permissions and its questions (`thread.read`'s `permissions` and `questions`; a question has `"ask": "question"`).
    pub cards: Vec<Value>,
    /// The folder's checkpoints, newest first (`thread.read`'s `checkpoints`): a message of the person's
    /// is the one whose `item` is its id. Whole in every read, so each read replaces them.
    pub checkpoints: Vec<Value>,
    /// What was allowed "always" for the thread (`thread.read`'s `rules`): `{id, kind, key, label}`. Whole in every read.
    pub rules: Vec<Value>,
}

impl Conv {
    pub fn new(thread: String) -> Self {
        Conv { thread, session: None, seq: 0, items: Vec::new(), ver: Vec::new(), config: Value::Null, cards: Vec::new(), checkpoints: Vec::new(), rules: Vec::new() }
    }

    /// Folds a `thread.read` reply in: a new session starts over; items
    /// already shown are replaced where they stand, new ones go at the end.
    pub fn merge(&mut self, v: &Value) {
        let session = v["thread"]["session"].as_str().map(String::from);
        if session.is_some() && session != self.session {
            self.session = session;
            self.items.clear();
            self.ver.clear();
            self.seq = 0;
        }
        for item in v["items"].as_array().into_iter().flatten() {
            match self.items.iter().position(|i| i["id"] == item["id"]) {
                Some(at) => {
                    self.items[at] = item.clone();
                    self.ver[at] += 1;
                }
                None => {
                    self.items.push(item.clone());
                    self.ver.push(0);
                }
            }
        }
        self.seq = self.seq.max(v["seq"].as_i64().unwrap_or(0));
        self.config = v["config"].clone();
        self.cards = v["permissions"].as_array().into_iter().chain(v["questions"].as_array()).flatten().cloned().collect();
        self.checkpoints = v["checkpoints"].as_array().cloned().unwrap_or_default();
        self.rules = v["rules"].as_array().cloned().unwrap_or_default();
    }
}

/// A message the person just sent, shown at the end of its conversation before the record has it (`composer::send_message`).
/// It goes when the record has a message of theirs with the same words, or when the send failed.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub id: u64,
    pub thread: String,
    pub text: String,
    /// Absolute paths of the files that went with it.
    pub files: Vec<String>,
    /// When it was sent (ms): a message of the record older than this is not it.
    pub at_ms: i64,
    /// When lilad took it (ms), once it did.
    pub taken_ms: Option<i64>,
}

/// The record's clock and ours are the same machine's; this much apart is still the same moment.
const SKEW_MS: i64 = 10_000;
/// A message lilad took that the record still has not shown after this long is given up on (its words came out different).
const GIVE_UP_MS: i64 = 120_000;

/// Which of `pending` (of `thread`, oldest first) the record `items` already has: a message of the person
/// with the same words, from about that moment on; each message of the record answers one. The rest of
/// `pending` is not in the record yet.
pub fn is_in_record(pending: &[&Pending], items: &[Value]) -> Vec<bool> {
    let mut claimed = vec![false; items.len()];
    pending
        .iter()
        .map(|p| {
            let found = items.iter().enumerate().position(|(i, it)| {
                !claimed[i]
                    && it["kind"] == "user_message"
                    && it["from"] == "person"
                    && it["text"].as_str().is_some_and(|t| t.trim() == p.text)
                    && it["at_ms"].as_i64().is_none_or(|at| at == 0 || at >= p.at_ms - SKEW_MS)
            });
            if let Some(i) = found {
                claimed[i] = true;
            }
            found.is_some()
        })
        .collect()
}

/// Does the end of the conversation already show that the agent works? (A step under way, which its row shows
/// turning.) Otherwise a running turn gets its own turning mark at the end.
fn shows_work(conv: Option<&Conv>) -> bool {
    let Some(c) = conv else { return false };
    let last = c.items.iter().rposition(|i| i["kind"] == "user_message" && i["from"] == "person");
    c.items.iter().skip(last.map_or(0, |l| l + 1)).any(|i| i["kind"] == "tool_call" && i["state"] == "running")
}

/// What a row of the list is. The kind says which row it is (rows of the same kind
/// keep their measured height); `ver` changes when what it shows does.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    kind: RowKind,
    /// The items it shows.
    span: Range<usize>,
    ver: u64,
}

#[derive(Clone, Debug, PartialEq)]
enum RowKind {
    Item(usize),
    /// A run of tool calls and thoughts between two messages, by the index of its first.
    Steps(usize),
    /// A message just sent, not in the record yet (its id).
    Pending(u64),
    /// Only a spinner: the thread works and has put nothing down since the person's last message.
    Busy,
    /// A permission card the thread waits on (its id).
    Ask(String),
}

/// The rows the list shows: one per message, one per run of steps between messages,
/// the messages just sent (or just a spinner while the thread works and has said nothing yet),
/// and the cards the thread waits on.
fn build_rows(conv: Option<&Conv>, pending: &[&Pending], running: bool, asks: &[String]) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    if let Some(c) = conv {
        for (i, item) in c.items.iter().enumerate() {
            let ver = u64::from(c.ver[i]);
            match item["kind"].as_str() {
                Some("tool_call" | "thought") => match rows.last_mut() {
                    Some(Row { kind: RowKind::Steps(_), span, ver: v }) => {
                        span.end = i + 1;
                        // The count is part of what it shows, so a growing run is measured again.
                        *v += ver + 1;
                    }
                    _ => rows.push(Row { kind: RowKind::Steps(i), span: i..i + 1, ver: ver + 1 }),
                },
                Some("user_message" | "agent_message" | "error") => rows.push(Row { kind: RowKind::Item(i), span: i..i + 1, ver }),
                _ => {}
            }
        }
    }
    rows.extend(pending.iter().map(|p| Row { kind: RowKind::Pending(p.id), span: 0..0, ver: p.text.len() as u64 + p.files.len() as u64 }));
    // A card means it waits for the person, not that it works.
    if (running || !pending.is_empty()) && asks.is_empty() && (!pending.is_empty() || !shows_work(conv)) {
        rows.push(Row { kind: RowKind::Busy, span: 0..0, ver: 0 });
    }
    rows.extend(asks.iter().map(|id| Row { kind: RowKind::Ask(id.clone()), span: 0..0, ver: 0 }));
    rows
}

/// More than this many lines of a message are folded, until asked for.
const FOLD_LINES: usize = 8;
/// Characters (CJK twice) that fit a line of a message bubble, roughly.
const LINE_WIDTH: usize = 64;

/// Does the text run past `FOLD_LINES` lines once wrapped (an estimate: the layout is not known here)?
fn is_long(text: &str) -> bool {
    let rows: usize = text.lines().map(|l| l.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum::<usize>().div_ceil(LINE_WIDTH).max(1)).sum();
    rows > FOLD_LINES
}

/// A tool call's `+added −removed` over all its files.
fn diff_counts(diffs: &[Value]) -> (usize, usize) {
    let mut n = (0, 0);
    for line in diffs.iter().flat_map(|d| d["lines"].as_array().into_iter().flatten()).filter_map(Value::as_str) {
        match line.as_bytes().first() {
            Some(b'+') => n.0 += 1,
            Some(b'-') => n.1 += 1,
            _ => {}
        }
    }
    n
}

fn tool_icon(tool: Option<&str>) -> IconName {
    match tool {
        Some("execute") => IconName::SquareTerminal,
        Some("read") => IconName::FileText,
        Some("edit") => IconName::Replace,
        Some("search") => IconName::Search,
        Some("fetch") => IconName::Globe,
        Some("delete") => IconName::Delete,
        _ => IconName::Asterisk,
    }
}

impl LilaApp {
    /// What the next fetch reads of the shown thread's conversation.
    pub fn conv_want(&self) -> Option<data::Want> {
        let t = self.shown_thread()?;
        let thread = s(&t, "id");
        let after = self.conv.as_ref().filter(|c| c.thread == thread).map_or(0, |c| c.seq);
        Some(data::Want { thread, after })
    }

    /// A fetched read, for the thread it was asked for.
    pub fn take_conv(&mut self, v: Value) {
        let Some(thread) = v["thread"]["id"].as_str().map(String::from) else { return };
        if self.thread.as_deref() != Some(thread.as_str()) {
            return;
        }
        match &mut self.conv {
            Some(c) if c.thread == thread => c.merge(&v),
            _ => {
                let mut c = Conv::new(thread);
                c.merge(&v);
                self.conv = Some(c);
                self.conv_open.clear();
            }
        }
        self.settle_pending();
    }

    /// Drops the messages just sent that the record has now (or that it never will).
    pub fn settle_pending(&mut self) {
        let now = data::now_ms();
        let mut gone = std::collections::HashSet::new();
        if let Some(c) = &self.conv {
            let mine: Vec<&Pending> = self.pending.iter().filter(|p| p.thread == c.thread).collect();
            let seen = is_in_record(&mine, &c.items);
            gone.extend(mine.iter().zip(seen).filter(|(_, seen)| *seen).map(|(p, _)| p.id));
        }
        gone.extend(self.pending.iter().filter(|p| p.taken_ms.is_some_and(|t| now - t > GIVE_UP_MS)).map(|p| p.id));
        self.pending.retain(|p| !gone.contains(&p.id));
    }

    /// The cards the shown thread waits on from the person (a permission), under its conversation.
    fn conv_asks(&self) -> Vec<String> {
        self.conv.as_ref().filter(|c| Some(&c.thread) == self.thread.as_ref()).map(|c| c.cards.iter().map(|k| s(k, "id")).collect()).unwrap_or_default()
    }

    /// Brings the list in step with what is shown: another thread or session starts it
    /// over; otherwise from the first row that is not the one it had, rows are replaced,
    /// and rows before it whose content changed are measured again. Following the end
    /// is the list's own: it stays there while the person is there.
    pub fn sync_conv(&mut self, cx: &mut Context<Self>) {
        let conv = self.conv.as_ref().filter(|c| Some(&c.thread) == self.thread.as_ref());
        let key = conv.map(|c| (c.thread.clone(), c.session.clone()));
        let shown = self.thread.as_deref().unwrap_or_default();
        let pending: Vec<&Pending> = self.pending.iter().filter(|p| p.thread == shown).collect();
        // The thread works, or its files are being put back: the end of the conversation turns.
        let running = self.shown_thread().is_some_and(|t| crate::restore::turning(&t, self.restore.going.contains(&s(&t, "id"))));
        let rows = build_rows(conv, &pending, running, &self.conv_asks());
        let list = self.conv_list.clone();
        if key != self.conv_key {
            self.conv_key = key;
            list.update(cx, |l, cx| l.reset(rows.len(), cx));
        } else if rows != self.conv_rows {
            let old = &self.conv_rows;
            let same = old.iter().zip(&rows).take_while(|(a, b)| a.kind == b.kind).count();
            list.update(cx, |l, cx| {
                for i in (0..same).filter(|&i| old[i].ver != rows[i].ver) {
                    l.remeasure_items(i..i + 1, cx);
                }
                if same < old.len() || same < rows.len() {
                    l.splice(same..old.len(), rows.len() - same, cx);
                }
            });
        }
        self.conv_rows = rows;
    }

    /// Opens or closes what is folded under `key` (a tool call, a run of steps, a long message)
    /// in the row that shows `item`, and measures that row again.
    fn toggle_open(&mut self, key: &str, item: usize, cx: &mut Context<Self>) {
        if !self.conv_open.remove(key) {
            self.conv_open.insert(key.to_owned());
        }
        if let Some(at) = self.conv_rows.iter().position(|r| r.span.contains(&item)) {
            self.conv_list.update(cx, |l, cx| {
                l.remeasure_items(at..at + 1, cx);
            });
        }
        cx.notify();
    }

    /// `--features shots`: puts the conversation in a state a screenshot can show, from
    /// `LILA_APP_CONV`: `top`, `row:N` (scroll to row N), `open:N` (open row N), `openall:N`
    /// (open row N and every tool call in it). `focus` and `key:<keystroke>` (the message box's cursor and keys) are
    /// done by the caller, outside any update of the app.
    #[cfg(feature = "shots")]
    pub fn dev_conv(&mut self, spec: &str, cx: &mut Context<Self>) {
        for step in spec.split(',') {
            let (cmd, n) = step.split_once(':').map_or((step, 0), |(c, n)| (c, n.parse::<usize>().unwrap_or(0)));
            match cmd {
                "top" => {
                    self.conv_list.update(cx, |l, cx| l.scroll_to_item(0, cx));
                }
                "row" => {
                    self.conv_list.update(cx, |l, cx| l.scroll_to_item(n, cx));
                }
                "open" | "openall" => {
                    let Some(row) = self.conv_rows.get(n).cloned() else { continue };
                    let Some(conv) = &self.conv else { continue };
                    let ids: Vec<(usize, String)> = row.span.clone().map(|i| (i, s(&conv.items[i], "id"))).collect();
                    for (i, id) in ids.into_iter().filter(|(i, _)| cmd == "openall" || *i == row.span.start) {
                        if !self.conv_open.contains(&id) {
                            self.toggle_open(&id, i, cx);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    pub fn conv_list_state(cx: &mut Context<Self>) -> Entity<MessageScrollerState> {
        cx.new(|cx| MessageScrollerState::new(0, cx))
    }

    /// The center card's body for a thread: its conversation and the box under it.
    pub fn conversation_view(&mut self, t: &Value, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let thread = s(t, "id");
        let composer = self.composer_view(t, window, cx);
        self.sync_conv(cx);
        // The rows draw themselves out of the app's state, when the list asks for them.
        let app = cx.entity();
        let scroller = MessageScroller::new("conversation", self.conv_list.clone(), move |ix, window, cx| app.update(cx, |this, cx| this.conv_row(ix, window, cx)))
            .with_list_style(StyleRefinement::default().pt(px(16.)).pb(px(24.)))
            // The rows' own gap (the kit's is 32).
            .with_row_style(StyleRefinement::default().px(px(0.)).pb(px(12.)))
            .with_jump_button_label("回到最新")
            .with_jump_button_style(StyleRefinement::default().bg(style::popover()))
            .with_jump_button_renderer(|b| b.xsmall().label("回到最新"));
        v_flex()
            .flex_1()
            .min_h_0()
            // Files dropped anywhere on the conversation go with the next message.
            .drag_over::<ExternalPaths>(|d, _, _, _| d.bg(style::blue().opacity(0.06)))
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| this.add_files(&thread, paths.paths().iter().cloned(), cx)))
            .child(div().flex_1().min_h_0().child(scroller))
            .child(composer)
            .into_any_element()
    }

    /// One row of the list, centered in a column.
    fn conv_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.conv_rows.get(ix).cloned() else { return div().into_any_element() };
        let body = match &row.kind {
            RowKind::Item(i) => self.item_row(*i, cx),
            RowKind::Steps(_) => Some(self.steps_row(row.span.clone(), window, cx)),
            RowKind::Pending(id) => self.pending.iter().find(|p| p.id == *id).map(pending_row),
            RowKind::Busy => Some(busy_row()),
            RowKind::Ask(id) => {
                let thread = self.thread.clone().unwrap_or_default();
                let found = self.conv.as_ref().and_then(|c| c.cards.iter().find(|k| k["id"] == id.as_str()).cloned());
                found.map(|k| {
                    let card = if k["ask"] == "question" { self.question_card(&thread, &k, window, cx) } else { self.permission_card(&thread, &k, cx) };
                    div().w_full().px_1().rounded(px(style::R_BLOCK)).border_1().border_color(style::yellow().opacity(0.35)).bg(style::yellow().opacity(0.06)).child(card).into_any_element()
                })
            }
        };
        h_flex().w_full().justify_center().child(div().w_full().max_w(px(760.)).px_5().children(body)).into_any_element()
    }

    fn item_row(&self, at: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let i = self.conv.as_ref()?.items.get(at)?;
        let id = s(i, "id");
        let text = s(i, "text");
        Some(match i["kind"].as_str()? {
            "error" => h_flex().w_full().gap(px(7.)).items_start().child(div().pt(px(2.)).child(Icon::new(IconName::CircleX).size(px(13.)).text_color(style::red()))).child(div().flex_1().min_w_0().text_size(px(12.)).line_height(relative(1.5)).text_color(style::tint(style::red(), 0.2)).child(text)).into_any_element(),
            "user_message" if i["from"] == "lila" => self.lila_row(at, &id, &text, cx),
            "user_message" if i["from"] == "restored" => self.restored_row(&id, i["undo"].as_i64(), cx),
            "user_message" => {
                let (body, more) = self.folded(at, &id, &text, div().px_3().py_2().rounded(px(style::R_CTL)).bg(style::fill()).text_size(px(13.)).line_height(relative(1.55)), cx);
                let files = sent_files(&id, &i["attachments"]);
                let column = v_flex().max_w(relative(0.8)).items_end().gap_1().children(files).when(!text.is_empty(), |d| d.child(body)).children(more);
                // Only the person's own words can be gone back to. The button has its place beside the bubble from the start (it is only
                // seen on hovering the row), so nothing moves when it shows.
                let back = (i["from"] == "person").then(|| self.restore_target(&id)).flatten();
                match back {
                    Some(n) => {
                        let thread = self.conv.as_ref().map(|c| c.thread.clone()).unwrap_or_default();
                        let group = SharedString::from(format!("said-{id}"));
                        let button = self.restore_button(&id, &thread, n, cx);
                        let shown = self.restore.checking.as_ref().is_some_and(|(t, c)| *t == thread && *c == n);
                        h_flex()
                            .group(group.clone())
                            .w_full()
                            .justify_end()
                            .items_start()
                            .gap_1()
                            .child(div().flex_none().mt(px(6.)).opacity(if shown { 1. } else { 0. }).group_hover(group, |d| d.opacity(1.)).child(button))
                            .child(column)
                            .into_any_element()
                    }
                    None => h_flex().w_full().justify_end().child(column).into_any_element(),
                }
            }
            "agent_message" => {
                let group = SharedString::from(format!("reply-{id}"));
                let copy = text.clone();
                div()
                    .group(group.clone())
                    .relative()
                    .w_full()
                    .text_size(px(13.))
                    // Selectable; inline code as a quiet chip, not the accent color; every code block carries a copy button.
                    .child(markdown(SharedString::from(format!("msg-{id}")), text).selectable(true).code_block_actions(|block, _, _| Clipboard::new("copy-code").value(block.code())))
                    // The whole reply's copy button: in the corner, on hovering the reply.
                    .child(
                        div()
                            .absolute()
                            .top(px(-6.))
                            .right(px(-4.))
                            .rounded(px(6.))
                            .bg(style::popover())
                            .opacity(0.)
                            .group_hover(group, |d| d.opacity(1.))
                            .child(Clipboard::new(SharedString::from(format!("copy-{id}"))).value(copy).tooltip("复制")),
                    )
                    .into_any_element()
            }
            _ => return None,
        })
    }

    /// `body` holding the text, cut to `FOLD_LINES` lines when longer, and the control that opens it.
    fn folded(&self, at: usize, id: &str, text: &str, body: Div, cx: &mut Context<Self>) -> (Div, Option<AnyElement>) {
        let long = is_long(text);
        let open = self.conv_open.contains(id);
        let body = body.when(long && !open, |d| d.line_clamp(FOLD_LINES).text_ellipsis()).child(text.to_owned());
        let key = id.to_owned();
        let more = long.then(|| {
            h_flex()
                .id(SharedString::from(format!("fold-{id}")))
                .h(px(22.))
                .px_1()
                .gap_1()
                .items_center()
                .justify_end()
                .cursor_pointer()
                .text_xs()
                .text_color(muted2())
                .hover(|d| d.text_color(muted()))
                .child(if open { "收起" } else { "展开" })
                .child(Icon::new(if open { IconName::ChevronUp } else { IconName::ChevronDown }).size(px(12.)))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_open(&key, at, cx)))
                .into_any_element()
        });
        (body, more)
    }

    /// Something the CLI itself printed (a slash command's result): the words, muted.
    fn lila_row(&self, at: usize, id: &str, text: &str, cx: &mut Context<Self>) -> AnyElement {
        let inner: Vec<&str> = text.lines().filter(|l| !l.trim_start().starts_with("<lila") && !l.trim_start().starts_with("</lila")).filter(|l| !l.trim().is_empty()).collect();
        let (body, more) = self.folded(at, id, &inner.join("\n"), div().w_full().text_xs().text_color(muted2()), cx);
        v_flex().w_full().child(body).children(more).into_any_element()
    }

    /// A run of tool calls and thoughts between two messages: one line that counts them by kind
    /// (or names the step under way), and under it, opened, each as its own row.
    fn steps_row(&self, span: Range<usize>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(conv) = &self.conv else { return div().into_any_element() };
        let items = &conv.items[span.clone()];
        let first = span.start;
        let key = s(&items[0], "id");
        let open = self.conv_open.contains(&key);
        let count = |f: &dyn Fn(&Value) -> bool| items.iter().filter(|i| f(i)).count();
        let tool = |kind: &'static [&'static str]| move |i: &Value| i["kind"] == "tool_call" && kind.contains(&i["tool"].as_str().unwrap_or("other"));
        let parts: Vec<(Icon, &str, usize)> = [
            (Icon::new(IconName::FileText), "读", count(&tool(&["read"]))),
            (Icon::new(IconName::Replace), "改", count(&tool(&["edit"]))),
            (Icon::new(IconName::SquareTerminal), "执行", count(&tool(&["execute"]))),
            (Icon::new(IconName::Search), "搜索", count(&tool(&["search"]))),
            (Icon::new(IconName::Globe), "网页", count(&tool(&["fetch"]))),
            (Icon::new(IconName::Delete), "删除", count(&tool(&["delete"]))),
            (Icon::new(IconName::Asterisk), "其他", count(&|i: &Value| i["kind"] == "tool_call" && !["read", "edit", "execute", "search", "fetch", "delete"].contains(&i["tool"].as_str().unwrap_or("other")))),
            (style::icon("lila/brain.svg"), "思考", count(&|i: &Value| i["kind"] == "thought")),
        ]
        .into_iter()
        .filter(|p| p.2 > 0)
        .collect();
        let failed = count(&|i: &Value| i["state"] == "failed");
        let running = items.iter().rev().find(|i| i["kind"] == "tool_call" && i["state"] == "running");
        let toggle = key.clone();
        let head = h_flex()
            .id(SharedString::from(format!("steps-{key}")))
            .h(px(24.))
            .gap_3()
            .px_1()
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|d| d.bg(style::hover()))
            .when_some(running, |d, r| {
                // The step under way, by its own words, and a spinner.
                d.child(Icon::new(tool_icon(r["tool"].as_str())).size(px(13.)).text_color(muted()))
                    .child(div().flex_1().min_w_0().truncate().font_family(crate::style::MONO).text_size(px(12.)).text_color(muted()).child(s(r, "title")))
                    .child(Spinner::new().xsmall().color(style::blue()))
            })
            .when(running.is_none(), |d| {
                d.children(parts.into_iter().map(|(icon, label, n)| {
                    h_flex().gap(px(5.)).child(icon.size(px(13.)).text_color(muted2())).child(div().text_size(px(12.)).text_color(muted()).child(format!("{label} {n}")))
                }))
                .child(div().flex_1())
            })
            .when(failed > 0, |d| d.child(h_flex().gap(px(4.)).child(Icon::new(IconName::CircleX).size(px(13.)).text_color(style::red())).child(div().text_size(px(12.)).text_color(style::tint(style::red(), 0.2)).child(format!("{failed}")))))
            .child(Icon::new(if open { IconName::ChevronUp } else { IconName::ChevronDown }).size(px(12.)).text_color(muted2()))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_open(&toggle, first, cx)));
        // Opened, each step as its own line: the tool calls keep their output and diffs.
        let steps = v_flex().gap(px(2.)).pt_1().children(items.iter().enumerate().map(|(n, i)| match i["kind"].as_str() {
            Some("tool_call") => self.tool_row(first + n, i, window, cx),
            _ => div().w_full().pl_3().my_1().border_l_2().border_color(style::divider()).text_xs().text_color(muted2()).line_clamp(3).text_ellipsis().child(s(i, "text")).into_any_element(),
        }));
        Collapsible::new().open(open).w_full().child(head).content(steps).into_any_element()
    }

    fn tool_row(&self, at: usize, i: &Value, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let id = s(i, "id");
        let open = self.conv_open.contains(&id);
        let state = s(i, "state");
        let icon = tool_icon(i["tool"].as_str());
        let diffs: Vec<Value> = i["diffs"].as_array().cloned().unwrap_or_default();
        let (added, removed) = diff_counts(&diffs);
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
            .child(div().flex_1().min_w_0().truncate().font_family(crate::style::MONO).text_size(px(12.)).text_color(muted()).child(s(i, "title")))
            .when(added + removed > 0, |d| {
                d.child(
                    h_flex()
                        .flex_none()
                        .gap(px(6.))
                        .font_family(crate::style::MONO)
                        .text_size(px(11.))
                        .child(div().text_color(style::tint(style::green(), 0.3)).child(format!("+{added}")))
                        .child(div().text_color(style::tint(style::red(), 0.3)).child(format!("−{removed}"))),
                )
            })
            .when(state == "running", |d| d.child(Spinner::new().xsmall().color(style::blue())))
            .when(state == "failed", |d| d.child(Icon::new(IconName::CircleX).size(px(13.)).text_color(style::red())))
            .when(has_more, |d| d.child(Icon::new(if open { IconName::ChevronUp } else { IconName::ChevronDown }).size(px(12.)).text_color(muted2())))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_open(&toggle, at, cx)));
        v_flex()
            .w_full()
            .child(head)
            .when(open, |d| {
                d.child(
                    v_flex()
                        .ml(px(26.))
                        .mt_1()
                        .gap_2()
                        .children((!diffs.is_empty()).then(|| diff_view(&id, &diffs, window, cx)))
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
                                    .font_family(crate::style::MONO)
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

/// The files that went with a message, as small cards in a row: a picture by its picture, the rest by name.
fn sent_files(id: &str, list: &Value) -> Option<AnyElement> {
    use gpui::component::attachment::{Attachment, AttachmentContent, AttachmentGroup, AttachmentMedia, AttachmentTitle};
    let files = list.as_array().filter(|a| !a.is_empty())?;
    Some(
        AttachmentGroup::new(SharedString::from(format!("sent-{id}")))
            .gap_1()
            .children(files.iter().enumerate().map(|(ix, f)| {
                let path = s(f, "path");
                let name = if f["name"].as_str().is_some_and(|n| !n.is_empty()) { s(f, "name") } else { path.rsplit('/').next().unwrap_or(&path).to_owned() };
                let shown = f["kind"] == "image" && std::path::Path::new(&path).is_file();
                let media = if shown { AttachmentMedia::new().src(std::path::PathBuf::from(&path)) } else { AttachmentMedia::new().child(if f["kind"] == "image" { style::icon("lila/image.svg") } else { Icon::new(IconName::FileText) }.text_color(muted())) };
                Attachment::new().id(SharedString::from(format!("sent-{id}-{ix}"))).xsmall().tooltip(path.clone()).media(media).content(AttachmentContent::new().title(AttachmentTitle::new(name)))
            }))
            .into_any_element(),
    )
}

/// A thread's words as markdown; inline code as a quiet chip, not the accent color.
fn markdown(id: SharedString, text: String) -> TextView {
    TextView::markdown(id, text).inline_code_style(StyleRefinement::default().bg(style::fill()).text_color(style::fg()))
}

/// What the person just sent, as it will look once the record has it.
fn pending_row(p: &Pending) -> AnyElement {
    let list: Vec<Value> = p.files.iter().filter_map(|f| serde_json::to_value(lila_agent::Attachment::file(f)).ok()).collect();
    let files = sent_files(&format!("pending-{}", p.id), &Value::Array(list));
    let body = div().px_3().py_2().rounded(px(style::R_CTL)).bg(style::fill()).text_size(px(13.)).line_height(relative(1.55)).when(is_long(&p.text), |d| d.line_clamp(FOLD_LINES).text_ellipsis()).child(p.text.clone());
    h_flex().w_full().justify_end().child(v_flex().max_w(relative(0.8)).items_end().gap_1().children(files).when(!p.text.is_empty(), |d| d.child(body))).into_any_element()
}

/// Just a spinner, where the reply will start: it works, and has put nothing down yet.
fn busy_row() -> AnyElement {
    h_flex().w_full().h(px(20.)).items_start().child(div().pt(px(3.)).child(Spinner::new().xsmall().color(style::blue()))).into_any_element()
}

/// The diff of a tool call, in gpui-kit's viewer: the files as one unified patch.
pub(crate) fn diff_view(id: &str, diffs: &[Value], window: &mut Window, cx: &mut App) -> AnyElement {
    use gpui::component::ActiveTheme as _;
    use gpui::component::diff::{Diff, DiffFile, DiffHunkSeparator, DiffState};
    let mut patch = String::new();
    let mut rows = 0;
    for d in diffs {
        let path = d["path"].as_str().unwrap_or("file");
        let path = path.trim_start_matches('/');
        patch.push_str(&format!("--- a/{path}\n+++ b/{path}\n"));
        for l in d["lines"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            patch.push_str(l);
            patch.push('\n');
            // A hunk header is a thin divider here, not a row.
            if !l.starts_with("@@") {
                rows += 1;
            }
        }
    }
    let Ok(files) = DiffFile::parse(&patch) else { return div().children(diffs.iter().map(diff_block)).into_any_element() };
    let state = window.use_keyed_state(SharedString::from(format!("diff-{id}")), cx, |_, cx| DiffState::new(files, cx));
    let line = f32::from(cx.theme().mono_font_size) * 1.6;
    // The viewer owns its scrolling and needs a height: its rows' own, up to a cap.
    let height = rows as f32 * line + if diffs.len() > 1 { diffs.len() as f32 * 36. } else { 0. } + 8.;
    div()
        .w_full()
        .h(px(height.min(360.)))
        .rounded(px(6.))
        .overflow_hidden()
        .child(Diff::new(&state).header_visible(diffs.len() > 1).hunk_separator(DiffHunkSeparator::Simple))
        .into_any_element()
}

/// A file's change: its path, then the lines colored by what happened to them.
fn diff_block(d: &Value) -> AnyElement {
    let lines: Vec<String> = d["lines"].as_array().into_iter().flatten().filter_map(|l| l.as_str().map(String::from)).collect();
    v_flex()
        .rounded(px(6.))
        .overflow_hidden()
        .bg(style::well())
        .font_family(crate::style::MONO)
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

/// Where the person stopped a turn: a quiet mark, not a message.
#[cfg(test)]
mod tests {
    use super::{Conv, Pending, RowKind, build_rows, diff_counts, is_in_record, is_long};
    use serde_json::json;

    fn read(items: serde_json::Value) -> serde_json::Value {
        json!({"thread": {"id": "t", "session": "s"}, "seq": 1, "items": items})
    }

    fn item(kind: &str, id: &str) -> serde_json::Value {
        match kind {
            "tool_call" => json!({"id": id, "kind": kind, "tool": "read", "title": id, "state": "done"}),
            _ => json!({"id": id, "kind": kind, "text": id, "state": "done"}),
        }
    }

    #[test]
    fn a_run_of_steps_between_messages_is_one_row() {
        let mut c = Conv::new("t".into());
        c.merge(&read(json!([item("user_message", "u"), item("thought", "t"), item("tool_call", "a"), item("tool_call", "b"), item("agent_message", "r"), item("tool_call", "c")])));
        let rows = build_rows(Some(&c), &[], false, &["card".into()]);
        let shape: Vec<(RowKind, std::ops::Range<usize>)> = rows.into_iter().map(|r| (r.kind, r.span)).collect();
        assert_eq!(shape, [(RowKind::Item(0), 0..1), (RowKind::Steps(1), 1..4), (RowKind::Item(4), 4..5), (RowKind::Steps(5), 5..6), (RowKind::Ask("card".into()), 0..0)]);
    }

    fn said(id: &str, text: &str, from: &str, at_ms: i64) -> serde_json::Value {
        json!({"id": id, "kind": "user_message", "text": text, "from": from, "at_ms": at_ms, "state": "done"})
    }

    fn sent(id: u64, text: &str, at_ms: i64) -> Pending {
        Pending { id, thread: "t".into(), text: text.into(), files: vec![], at_ms, taken_ms: None }
    }

    #[test]
    fn a_sent_message_stays_until_the_record_has_one_with_the_same_words() {
        let items = vec![said("u0", "go on", "person", 1_000), said("u1", "hi", "lila", 9_000), said("u2", "go on", "person", 20_000)];
        let (a, b) = (sent(1, "go on", 19_990), sent(2, "go on", 20_100));
        // The older "go on" is from before; the newer one answers the first of them, and only the first.
        assert_eq!(is_in_record(&[&a, &b], &items), [true, false]);
        assert_eq!(is_in_record(&[&sent(3, "other", 19_990)], &items), [false], "other words");
        assert_eq!(is_in_record(&[&sent(4, "hi", 8_000)], &items), [false], "not from the person");
        assert_eq!(is_in_record(&[&sent(5, "go on", 19_990)], &items[..1]), [false], "older than the send");
    }

    #[test]
    fn a_spinner_shows_the_turn_runs_until_something_else_does() {
        let kinds = |c: &Conv, pending: &[&Pending], running| build_rows(Some(c), pending, running, &[]).into_iter().map(|r| r.kind).collect::<Vec<_>>();
        let mut c = Conv::new("t".into());
        c.merge(&read(json!([said("u", "hi", "person", 1)])));
        assert_eq!(kinds(&c, &[], false), [RowKind::Item(0)], "it does not work: nothing");
        assert_eq!(kinds(&c, &[], true), [RowKind::Item(0), RowKind::Busy], "works, has said nothing");
        let p = sent(7, "more", 5);
        assert_eq!(kinds(&c, &[&p], false), [RowKind::Item(0), RowKind::Pending(7), RowKind::Busy], "just sent: it works, as far as anyone knows");
        let mut step = item("tool_call", "a");
        step["state"] = json!("running");
        c.merge(&read(json!([step])));
        assert_eq!(kinds(&c, &[], true), [RowKind::Item(0), RowKind::Steps(1)], "a step under way turns in its own row");
        step["state"] = json!("done");
        c.merge(&read(json!([step, item("agent_message", "r")])));
        assert_eq!(kinds(&c, &[], true).last(), Some(&RowKind::Busy), "done steps and a word said: it still works");
        assert_eq!(kinds(&c, &[&p], true), [RowKind::Item(0), RowKind::Steps(1), RowKind::Item(2), RowKind::Pending(7), RowKind::Busy], "and after a new message");
        // A card means it waits for the person.
        assert!(build_rows(Some(&c), &[&p], true, &["card".into()]).iter().all(|r| r.kind != RowKind::Busy));
    }

    #[test]
    fn a_row_is_measured_again_when_its_content_or_count_changes() {
        let mut c = Conv::new("t".into());
        c.merge(&read(json!([item("user_message", "u"), item("tool_call", "a")])));
        let before = build_rows(Some(&c), &[], false, &[]);
        // The call finishes: the same item, replaced.
        let mut done = item("tool_call", "a");
        done["state"] = json!("failed");
        c.merge(&read(json!([done])));
        let after = build_rows(Some(&c), &[], false, &[]);
        assert_eq!(before[1].kind, after[1].kind);
        assert_ne!(before[1].ver, after[1].ver);
        assert_eq!(before[0].ver, after[0].ver, "what did not change keeps its measure");
        // A run that grows.
        c.merge(&read(json!([item("tool_call", "b")])));
        let grown = build_rows(Some(&c), &[], false, &[]);
        assert_eq!((grown[1].kind.clone(), grown[1].span.clone()), (RowKind::Steps(1), 1..3));
        assert_ne!(after[1].ver, grown[1].ver);
        // A new session starting over.
        c.merge(&json!({"thread": {"id": "t", "session": "s"}, "seq": 3, "items": []}));
        c.merge(&json!({"thread": {"id": "t", "session": "s2"}, "seq": 1, "items": [item("agent_message", "x")]}));
        assert_eq!(c.items.len(), 1);
        assert_eq!(build_rows(Some(&c), &[], false, &[]).len(), 1);
    }

    #[test]
    fn the_checkpoints_of_each_read_replace_the_last_and_a_restored_mark_is_a_row_of_its_own() {
        let mut c = Conv::new("t".into());
        let mut r = read(json!([said("u1", "go", "person", 1), {"id": "restored-1-9", "kind": "user_message", "from": "restored", "checkpoint": 1, "undo": 4, "text": "x", "at_ms": 9, "state": "done"}]));
        r["checkpoints"] = json!([{"id": 4, "kind": "restore", "item": "", "restorable": true}, {"id": 1, "kind": "turn", "item": "u1", "restorable": true}]);
        c.merge(&r);
        assert_eq!(c.checkpoints.len(), 2);
        // The mark is not a message of the person's, and not a run of steps: a row of its own, after the message.
        let rows = build_rows(Some(&c), &[], false, &[]);
        assert_eq!(rows.into_iter().map(|r| r.kind).collect::<Vec<_>>(), [RowKind::Item(0), RowKind::Item(1)]);
        // A read without checkpoints (an older lilad) leaves none.
        c.merge(&read(json!([])));
        assert!(c.checkpoints.is_empty());
        // The end of the conversation turns while the files are put back, as while the thread works.
        let busy = |running| build_rows(Some(&c), &[], running, &[]).last().map(|r| r.kind.clone());
        assert_eq!(busy(true), Some(RowKind::Busy));
        assert_eq!(busy(false), Some(RowKind::Item(1)));
    }

    #[test]
    fn the_rules_of_each_read_replace_the_last_and_an_older_lilad_has_none() {
        let mut c = Conv::new("t".into());
        let mut r = read(json!([]));
        r["rules"] = json!([{"id": 1, "kind": "command", "key": "cargo test", "label": "cargo test", "at_ms": 5}, {"id": 2, "kind": "edit", "key": "", "label": "改动此文件夹里的文件", "at_ms": 6}]);
        c.merge(&r);
        assert_eq!(c.rules.iter().map(|r| r["id"].as_i64().unwrap()).collect::<Vec<_>>(), [1, 2]);
        r["rules"] = json!([{"id": 2, "kind": "edit", "key": "", "label": "改动此文件夹里的文件", "at_ms": 6}]);
        c.merge(&r);
        assert_eq!(c.rules.len(), 1, "one was taken back");
        c.merge(&read(json!([])));
        assert!(c.rules.is_empty(), "a lilad that does not say has none");
    }

    #[test]
    fn long_messages_are_told_by_their_wrapped_lines() {
        assert!(!is_long("one line"));
        assert!(!is_long(&"a\n".repeat(8)));
        assert!(is_long(&"a\n".repeat(9)));
        assert!(is_long(&"word ".repeat(200)), "a long paragraph wraps");
        assert!(is_long(&"中".repeat(300)), "wide characters take two");
    }

    #[test]
    fn a_diffs_counts_are_its_added_and_removed_lines() {
        let d = json!([{"path": "a", "lines": ["@@ -1,2 +1,3 @@", " keep", "-old", "+new", "+more"]}, {"path": "b", "lines": ["@@ -1 +1 @@", "-x"]}]);
        assert_eq!(diff_counts(d.as_array().unwrap()), (2, 2));
    }
}

