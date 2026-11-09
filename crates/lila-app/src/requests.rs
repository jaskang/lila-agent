//! The requests: the middle card lists them by whose move it is (`lila_core::request`),
//! the right card shows the selected one with what it needs from the person.

use crate::app::LilaApp;
use crate::data;
use crate::style::{self, card, muted, muted2};
use crate::style::Control as _;
use crate::ui::{dot, s};
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::scroll::ScrollableElement;
use gpui::component::spinner::Spinner;
use gpui::component::{IconName, Sizable, StyledExt, h_flex, v_flex};
use gpui::prelude::*;
use gpui::*;
use serde_json::Value;

/// The groups, in order: title, color of their marks, how many finished ones show.
const GROUPS: [(&str, &str, usize); 6] = [
    ("needs_you", "需要你", usize::MAX),
    ("working", "进行中", usize::MAX),
    ("waiting", "等别人", usize::MAX),
    ("queued", "排队", usize::MAX),
    ("delivered", "已交付", 8),
    ("dropped", "放弃", 5),
];

fn group_color(group: &str) -> Option<Hsla> {
    match group {
        "needs_you" => Some(style::red()),
        "working" => Some(style::blue()),
        "waiting" => Some(style::yellow()),
        "delivered" => Some(style::green()),
        _ => None,
    }
}

/// An id as a tag in its group's color (neutral for queued and dropped).
fn id_tag(group: &str, id: String) -> Div {
    match group_color(group) {
        Some(c) => style::tag(c, id),
        None => style::tag_neutral(id),
    }
}

/// The PM is the person's assistant in the window; members keep their ids.
pub fn who(member: &str, owner: &str) -> String {
    if member == owner { "助理".to_owned() } else { member.to_owned() }
}

/// What an inbox item asks, in a few words.
pub fn inbox_words(kind: &str) -> &'static str {
    match kind {
        "review" => "等你验收",
        "triage" => "要修吗？",
        "draft" => "评论草稿",
        "question" => "助理在问你",
        "merge" => "合并没成功",
        "budget" => "超出预算",
        "terminal" => "终端在等你",
        "startup" => "启动卡住",
        "stalled" => "没有回应",
        "runaway" => "可能在打转",
        "lost" => "成员退出",
        "quota" => "额度用完",
        "report" => "报告",
        _ => "提醒",
    }
}

/// What is happening to a request now, in a few words.
fn doing_words(r: &Value, owner: &str) -> String {
    let d = &r["doing"];
    match d["kind"].as_str().unwrap_or("") {
        "you" => inbox_words(d["inbox"].as_str().unwrap_or("")).to_owned(),
        // An issue the PM has not sorted yet.
        "member" if r["external"].is_object() && r["external"]["triage"].is_null() && d["member"] == owner => "助理在分拣".to_owned(),
        "member" => {
            let m = who(d["member"].as_str().unwrap_or(""), owner);
            if d["state"] == "blocked" { format!("{m} 卡住了") } else { format!("{m} 在做") }
        }
        "check" => "检查中".to_owned(),
        "commit" => "合并中".to_owned(),
        "wait" => match d["hold"].as_str().unwrap_or("") {
            "worktree" => "等空位".to_owned(),
            "folder" => "等目录空出".to_owned(),
            "review" => "等你先验收别的".to_owned(),
            "quota" => "等额度恢复".to_owned(),
            "starting" => "成员启动中".to_owned(),
            "stopped" => "团队没启动".to_owned(),
            _ => "等成员有空".to_owned(),
        },
        "reply" => format!("等 @{} 回复", r["external"]["author"].as_str().unwrap_or("")),
        "dropped" => "已放弃".to_owned(),
        // Handed over as a pull request; people merge it on GitHub.
        _ if r["external"]["pr"].is_string() => "已开 PR".to_owned(),
        _ => "已交付".to_owned(),
    }
}

/// How long, in the largest unit: 刚刚, 12 分钟, 3 小时, 2 天.
pub(crate) fn ago(ms: i64) -> String {
    let m = (data::now_ms() - ms).max(0) / 60_000;
    match m {
        0 => "刚刚".to_owned(),
        1..=59 => format!("{m} 分钟"),
        60..=1439 => format!("{} 小时", m / 60),
        _ => format!("{} 天", m / 1440),
    }
}

/// When a finished request ended: the time today, else the date.
fn ended(ms: i64) -> String {
    let t = data::local(ms);
    if t.date_naive() == chrono::Local::now().date_naive() { t.format("%H:%M").to_string() } else { t.format("%-m月%-d日").to_string() }
}

pub(crate) fn sublabel(title: &str, count: usize) -> Div {
    h_flex()
        .px_2()
        .pt(px(2.))
        .pb_1()
        .gap(px(6.))
        .text_size(px(11.))
        .font_semibold()
        .text_color(muted2())
        .child(title.to_owned())
        .child(count.to_string())
}

fn bullets(items: &[String], color: Hsla) -> Div {
    v_flex().gap_1().text_size(px(13.)).children(items.iter().map(|i| {
        h_flex().gap_2().items_start().child(div().flex_none().text_color(muted2()).child("•")).child(div().flex_1().min_w_0().text_color(color).child(i.clone()))
    }))
}

fn section(title: &str, color: Hsla) -> Div {
    v_flex().gap_2().child(div().text_xs().font_semibold().text_color(color).child(title.to_owned()))
}

impl LilaApp {
    fn owner_id(&self) -> String {
        self.members().iter().find(|m| m["owner"] == true).map(|m| s(m, "id")).unwrap_or_default()
    }

    /// One row: the id in its group's color, the title, what is happening, how long.
    fn request_row(&self, r: &Value, owner: &str, cx: &mut Context<Self>) -> AnyElement {
        let id = s(r, "id");
        let group = s(r, "group");
        let finished = matches!(group.as_str(), "delivered" | "dropped");
        let when = if finished { ended(r["updated_ms"].as_i64().unwrap_or(0)) } else { ago(r["since_ms"].as_i64().unwrap_or(0)) };
        let active = self.request.as_deref() == Some(id.as_str());
        let open = id.clone();
        let working = group == "working";
        h_flex()
            .id(SharedString::from(format!("request-{id}")))
            .h(px(34.))
            .px_2()
            .gap(px(10.))
            .rounded(px(style::R_CTL))
            .cursor_pointer()
            .when(active, |d| d.bg(style::accent()))
            .when(!active, |d| d.hover(|d| d.bg(style::hover())))
            .on_click(cx.listener(move |this, _, _, cx| this.select_request(open.clone(), cx)))
            .child(id_tag(&group, id))
            .child(div().flex_1().min_w_0().text_size(px(13.)).truncate().when(finished, |d| d.text_color(muted())).child(s(r, "title")))
            .when(working, |d| d.child(Spinner::new().xsmall().color(style::blue())))
            .child(div().flex_none().max_w(px(150.)).truncate().text_xs().text_color(if group == "needs_you" { style::tint(style::red(), 0.35) } else { muted2() }).child(doing_words(r, owner)))
            .child(div().flex_none().w(px(46.)).text_xs().text_color(muted2()).flex().justify_end().child(when))
            .into_any_element()
    }

    /// An inbox item about no request, listed with what needs the person.
    pub(crate) fn loose_row(&self, i: &Value, cx: &mut Context<Self>) -> AnyElement {
        let id = s(i, "id");
        let active = self.request.as_deref() == Some(id.as_str());
        let open = id.clone();
        let first = s(i, "text").lines().next().unwrap_or_default().to_owned();
        h_flex()
            .id(SharedString::from(format!("loose-{id}")))
            .h(px(34.))
            .px_2()
            .gap(px(10.))
            .rounded(px(style::R_CTL))
            .cursor_pointer()
            .when(active, |d| d.bg(style::accent()))
            .when(!active, |d| d.hover(|d| d.bg(style::hover())))
            .on_click(cx.listener(move |this, _, _, cx| this.select_request(open.clone(), cx)))
            .child(style::tag(style::red(), inbox_words(&s(i, "kind"))))
            .child(div().flex_1().min_w_0().text_size(px(13.)).truncate().text_color(muted()).child(first))
            .child(div().flex_none().w(px(46.)).text_xs().text_color(muted2()).flex().justify_end().child(ago(i["created_ms"].as_i64().unwrap_or(0))))
            .into_any_element()
    }

    /// Back after a while: one line on what happened since (delivered, waiting
    /// for the person, dropped). Click: only those; ×: gone.
    fn recap_line(&self, since: i64, cx: &mut Context<Self>) -> Option<AnyElement> {
        let count = |g: &str| self.snap.requests.iter().filter(|r| r["group"] == g && (g == "needs_you" || r["updated_ms"].as_i64().unwrap_or(0) >= since)).count();
        let needs = count("needs_you") + self.loose_inbox().len();
        let parts: Vec<String> = [(count("delivered"), "交付 {}"), (needs, "{} 件等你"), (count("dropped"), "放弃 {}")]
            .into_iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, f)| f.replace("{}", &n.to_string()))
            .collect();
        if parts.is_empty() {
            return None;
        }
        let on = self.recap_only;
        Some(
            h_flex()
                .id("recap")
                .mx_2()
                .mt(px(10.))
                .px_3()
                .h(px(34.))
                .gap_2()
                .rounded(px(style::R_CTL))
                .cursor_pointer()
                .bg(if on { style::accent() } else { style::fill() })
                .hover(|d| d.bg(style::accent()))
                .text_size(px(13.))
                .child(div().text_color(muted()).child(format!("离开的 {}", ago(since))))
                .child(div().child(parts.join(" · ")))
                .child(div().flex_1())
                .child(Button::new("recap-close").ghost().xsmall().icon(IconName::Close).on_click(cx.listener(|this, _, _, cx| {
                    this.recap = None;
                    this.recap_only = false;
                    cx.stop_propagation();
                    cx.notify();
                })))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.recap_only = !this.recap_only;
                    cx.notify();
                }))
                .into_any_element(),
        )
    }

    /// The middle card's body: every request, by whose move it is.
    pub fn request_list(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let owner = self.owner_id();
        let recap = self.recap.and_then(|since| self.recap_line(since, cx).map(|line| (since, line)));
        let only = recap.as_ref().filter(|_| self.recap_only).map(|(since, _)| *since);
        let requests: Vec<Value> = self.snap.requests.iter().filter(|r| only.is_none_or(|t| r["group"] == "needs_you" || r["updated_ms"].as_i64().unwrap_or(0) >= t)).cloned().collect();
        let loose = self.loose_inbox();
        let mut groups: Vec<AnyElement> = Vec::new();
        for (key, title, limit) in GROUPS {
            let rows: Vec<&Value> = requests.iter().filter(|r| r["group"] == key).collect();
            let extra = if key == "needs_you" { loose.len() } else { 0 };
            if rows.is_empty() && extra == 0 {
                continue;
            }
            let mut body: Vec<AnyElement> = Vec::new();
            if key == "needs_you" {
                body.extend(loose.iter().map(|i| self.loose_row(i, cx)));
            }
            body.extend(rows.iter().take(limit).map(|r| self.request_row(r, &owner, cx)));
            groups.push(v_flex().gap(px(2.)).child(sublabel(title, rows.len() + extra)).children(body).into_any_element());
        }
        v_flex()
            .flex_1()
            .min_h_0()
            .children(recap.map(|(_, line)| line))
            .child(v_flex().id("requests").flex_1().min_h_0().overflow_y_scrollbar().px_2().pt(px(10.)).pb(px(10.)).gap_4().children(groups))
            .into_any_element()
    }

    /// The right card: the selected request, or the loose inbox item.
    pub fn request_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.request.clone()?;
        if let Some(item) = self.loose_inbox().into_iter().find(|i| i["id"] == id.as_str()) {
            let card_el = self.inbox_card(item, false, cx).into_any_element();
            return Some(card().flex_none().flex().flex_col().pt_3().child(card_el).into_any_element());
        }
        let Some(v) = self.snap.request.clone() else {
            return Some(card().flex_1().flex().items_center().justify_center().child(Spinner::new().color(muted())).into_any_element());
        };
        let owner = self.owner_id();
        let r = v["request"].clone();
        let group = s(&r, "group");
        let tasks: Vec<Value> = v["tasks"].as_array().cloned().unwrap_or_default();
        let items: Vec<Value> = v["inbox"].as_array().cloned().unwrap_or_default();
        let ext = r["external"].clone();
        // Who did what and said what; Lila's own bookkeeping (deliveries, sessions,
        // worktrees, the tasks it opens for a flow's steps) stays in `lila log`.
        let all: Vec<Value> = v["events"].as_array().cloned().unwrap_or_default();
        // An answer that the next line already tells (通过, 打回, 发出评论).
        let told = |e: &Value| {
            let at = e["at_ms"].as_i64().unwrap_or(0);
            all.iter().any(|x| x["task"] == e["task"] && matches!(x["kind"].as_str(), Some("accepted" | "rejected" | "commented")) && (x["at_ms"].as_i64().unwrap_or(0) - at).abs() < 10_000)
        };
        let events: Vec<Value> = all
            .iter()
            .filter(|e| {
                let (actor, kind, detail) = (e["actor"].as_str().unwrap_or(""), e["kind"].as_str().unwrap_or(""), e["detail"].as_str().unwrap_or(""));
                let bookkeeping = matches!(kind, "delivered" | "fresh_session" | "resumed_session" | "worktree" | "flow" | "member_started" | "member_gone" | "edited")
                    || (kind == "answered" && told(e))
                    || (kind == "created" && actor == "lilad")
                    // An issue: "Lila 从 GitHub 收到" says it, not "你 创建".
                    || (kind == "created" && ext.is_object() && e["task"] == r["id"])
                    || (kind == "check" && detail.starts_with("running"));
                !bookkeeping
            })
            .cloned()
            .collect();
        let collect = |key: &str| -> Vec<String> { tasks.iter().flat_map(|t| t[key].as_array().cloned().unwrap_or_default()).filter_map(|x| x.as_str().map(String::from)).collect() };
        let (decisions, unverified) = (collect("decisions"), collect("unverified"));
        let from = if ext.is_object() {
            format!("@{} 在 GitHub 提的", s(&ext, "author"))
        } else if r["from"] == "human" {
            "你提的".to_owned()
        } else {
            format!("{}开的", who(r["from"].as_str().unwrap_or(""), &owner))
        };
        // The issue and its pull request, opened in the browser.
        let links: Vec<(String, String)> = [(format!("#{}", s(&ext, "ref")), s(&ext, "url")), ("PR".to_owned(), s(&ext, "pr"))].into_iter().filter(|(_, u)| !u.is_empty()).collect();
        let member = (r["doing"]["kind"] == "member").then(|| s(&r["doing"], "member"));
        let mark: AnyElement = if group == "working" {
            Spinner::new().xsmall().color(style::blue()).into_any_element()
        } else {
            dot(group_color(&group).unwrap_or_else(muted)).into_any_element()
        };
        let now = h_flex()
            .gap(px(8.))
            .text_size(px(13.))
            .child(mark)
            .child(div().child(doing_words(&r, &owner)))
            .child(div().text_xs().text_color(muted2()).child(ago(r["since_ms"].as_i64().unwrap_or(0))))
            .child(div().flex_1())
            .when_some(member, |d, m| {
                d.child(Button::new("request-look").small().small_button().outline().icon(IconName::SquareTerminal).label("看现场").on_click(cx.listener(move |this, _, _, cx| {
                    this.member = Some(m.clone());
                    cx.notify();
                })))
            });
        let cards: Vec<AnyElement> = items.into_iter().enumerate().map(|(n, item)| self.inbox_card(item, n > 0, cx).into_any_element()).collect();
        let steps: Vec<AnyElement> = tasks
            .iter()
            .map(|t| {
                let tid = s(t, "id");
                let open = tid.clone();
                h_flex()
                    .id(SharedString::from(format!("step-{tid}")))
                    .px_2()
                    .py(px(6.))
                    .gap_2()
                    .rounded(px(style::R_CTL))
                    .cursor_pointer()
                    .hover(|d| d.bg(style::hover()))
                    .text_size(px(13.))
                    .child(crate::detail::state_tag(&s(t, "state"), tid))
                    .child(div().flex_1().min_w_0().truncate().child(s(t, "title")))
                    .child(div().text_xs().text_color(muted()).flex_none().child(who(&s(t, "to"), &owner)))
                    .on_click(cx.listener(move |this, _, _, cx| this.show_task(open.clone(), cx)))
                    .into_any_element()
            })
            .collect();
        let n = events.len();
        let timeline = v_flex().ml_1().pl_4().gap(px(10.)).border_l_1().border_color(gpui::Hsla::from(rgba(0xffffff1a))).children(events.iter().enumerate().map(|(i, e)| {
            let (line, detail) = crate::detail::event_line(&who(&s(e, "actor"), &owner), &s(e, "kind"), &s(e, "detail"));
            h_flex()
                .relative()
                .gap_2()
                .items_start()
                .text_xs()
                .child(div().absolute().left(px(-20.)).top(px(4.)).size(px(7.)).rounded_full().bg(if i + 1 == n { style::blue() } else { gpui::Hsla::from(rgba(0x48484eff)) }))
                .child(div().flex_none().text_color(muted()).child(data::clock(e["at_ms"].as_i64().unwrap_or(0))))
                .child(div().flex_none().child(line))
                .when_some(detail, |d, detail| d.child(div().flex_1().min_w_0().text_color(muted2()).child(detail)))
        }));
        Some(
            card()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(
                    v_flex()
                        .id("request-panel")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scrollbar()
                        .pb_5()
                        .gap_4()
                        .child(
                            v_flex()
                                .px_4()
                                .pt(px(14.))
                                .gap(px(6.))
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(id_tag(&group, s(&r, "id")))
                                        .child(div().text_xs().text_color(muted2()).child(format!("{from} · {}", data::clock(r["created_ms"].as_i64().unwrap_or(0)))))
                                        .child(div().flex_1())
                                        .children(links.into_iter().enumerate().map(|(n, (label, url))| {
                                            style::link(SharedString::from(format!("request-link-{n}")), label, false).text_xs().on_click(move |_, _, _| {
                                                let _ = std::process::Command::new("open").arg(&url).spawn();
                                            })
                                        })),
                                )
                                .child(div().text_size(px(15.)).font_semibold().line_height(relative(1.35)).child(s(&r, "title")))
                                // An inbox card below says what waits for the person.
                                .when(cards.is_empty(), |d| d.child(div().pt_1().child(now))),
                        )
                        .when(!cards.is_empty(), |d| d.child(v_flex().border_t_1().border_color(style::divider()).pt_3().children(cards)))
                        .child(
                            v_flex()
                                .px_4()
                                .gap_4()
                                .when(!decisions.is_empty(), |d| d.child(section("替你做的决定", muted()).child(bullets(&decisions, style::fg()))))
                                .when(!unverified.is_empty(), |d| d.child(section("没验证", style::yellow()).child(bullets(&unverified, style::fg()))))
                                .when(steps.len() > 1, |d| d.child(section("步骤", muted()).child(v_flex().children(steps))))
                                .when(n > 0, |d| d.child(section("经过", muted()).child(timeline))),
                        ),
                )
                .into_any_element(),
        )
    }
}
