//! Virtualized transcript powered by `gpui-component`'s `MessageScroller` and
//! `TextView::markdown` (with Claude-style `stream_fade`, syntax highlighting,
//! frosted content cards, collapsible activity/changes, subagent cards, and rewind).

use std::collections::HashMap;
use std::path::Path;

use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    App, Entity, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _, px, rgb,
};
use serde_json::Value;

use crate::format::{format_compact_tokens, format_duration};
use crate::protocol::{SubagentNotification, SubagentToolInfo, ToolStatus, TranscriptItem};
use crate::shell_bridge;
use crate::store::{AppStore, ToastTone};
use crate::transcript_layout::{FileChange, FileChangeKind, TranscriptBlock};
use crate::ui::{BadgeTone, pill_badge, theme_setup};

fn summarize_tool_args(args: &HashMap<String, Value>) -> String {
    for key in ["command", "path", "file_path", "pattern", "query", "url"] {
        if let Some(s) = args.get(key).and_then(|v| v.as_str()) {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                let single_line = trimmed.lines().next().unwrap_or(trimmed);
                if single_line.chars().count() > 80 {
                    let prefix: String = single_line.chars().take(80).collect();
                    return format!("{prefix}…");
                }
                return single_line.to_owned();
            }
        }
    }
    String::new()
}

fn handle_markdown_link(
    store: &Entity<AppStore>,
    url: &str,
    project_path: Option<&str>,
    cx: &mut App,
) {
    let trimmed = url.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return;
    }
    if trimmed.starts_with("https://")
        || trimmed.starts_with("http://")
        || trimmed.starts_with("mailto:")
    {
        cx.open_url(trimmed);
        return;
    }
    let clean = trimmed.strip_prefix("file://").unwrap_or(trimmed);
    let target = if Path::new(clean).is_absolute() || clean.starts_with('~') {
        clean.to_owned()
    } else if let Some(pp) = project_path {
        Path::new(pp).join(clean).to_string_lossy().into_owned()
    } else {
        clean.to_owned()
    };
    let res = shell_bridge::open_whitelisted_path(&target);
    if let Err(err) = res {
        store.update(cx, |s, cx| s.emit_toast(ToastTone::Error, err, cx));
    }
}

pub fn render_transcript(
    store_entity: Entity<AppStore>,
    scroller_state: Entity<MessageScrollerState>,
    cx: &App,
) -> impl IntoElement {
    let store = store_entity.read(cx);
    let slot = &store.main_slot;
    let has_items = !slot.blocks.is_empty();
    let loading = slot.loading;
    let error = slot.error.clone();
    let has_earlier = slot.has_earlier;
    let loading_earlier = slot.loading_earlier;
    let session_id = slot.session_id.clone().unwrap_or_default();

    if loading && !has_items {
        return v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("正在加载会话…"),
            )
            .into_any_element();
    }

    if let Some(err) = error
        && !has_items
    {
        let st_retry = store_entity.clone();
        let sid_retry = session_id.clone();
        return v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .p_6()
            .child(div().text_xs().text_color(cx.theme().danger).child(err))
            .child(
                Button::new("transcript-retry")
                    .xsmall()
                    .label("重试")
                    .on_click(move |_, _, cx| {
                        st_retry.update(cx, |s, cx| s.open_main_slot(sid_retry.clone(), cx));
                    }),
            )
            .into_any_element();
    }

    if !has_items {
        return v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .p_8()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("还没有消息，发一句话开始吧。"),
            )
            .into_any_element();
    }

    let st_earlier = store_entity.clone();
    let st_row = store_entity.clone();

    v_flex()
        .flex_1()
        .min_h_0()
        .when(has_earlier, |el| {
            el.child(
                h_flex().w_full().justify_center().py_1().child(
                    Button::new("load-earlier-btn")
                        .ghost()
                        .xsmall()
                        .disabled(loading_earlier)
                        .label(if loading_earlier {
                            "正在加载更早消息…"
                        } else {
                            "加载更早消息"
                        })
                        .on_click(move |_, _, cx| {
                            st_earlier.update(cx, |s, cx| s.load_earlier(cx));
                        }),
                ),
            )
        })
        .child(
            MessageScroller::new("main-transcript", scroller_state, move |ix, _window, cx| {
                let store = st_row.read(cx);
                let Some(block) = store.main_slot.blocks.get(ix).cloned() else {
                    return div().into_any_element();
                };
                let sid = store.main_slot.session_id.clone().unwrap_or_default();
                let pp = store.project_path_for_session(&sid);
                let running = store.is_busy();
                let read_only = store
                    .sessions
                    .get(&sid)
                    .is_some_and(|s| s.parent_id.is_some());

                let inner = match block {
                    TranscriptBlock::Activity { id, item_ids } => {
                        render_activity_group(&st_row, id, item_ids, &sid, pp.as_deref(), running, read_only, cx)
                            .into_any_element()
                    }
                    TranscriptBlock::Changes { id, changes } => {
                        render_changes_block(&st_row, id, changes, cx).into_any_element()
                    }
                    TranscriptBlock::Item { item_id, .. } => {
                        if let Some(item) = store.main_slot.item_by_id(&item_id).cloned() {
                            render_item(&st_row, item, &sid, pp.as_deref(), running, read_only, cx)
                                .into_any_element()
                        } else {
                            div().into_any_element()
                        }
                    }
                };

                h_flex()
                    .w_full()
                    .justify_center()
                    .px_4()
                    .py(px(5.0))
                    .child(div().w_full().max_w(px(820.0)).child(inner))
                    .into_any_element()
            })
            .flex_1()
            .min_h_0()
            .with_jump_button_label("回到底部"),
        )
        .into_any_element()
}

fn render_item(
    store_entity: &Entity<AppStore>,
    item: TranscriptItem,
    session_id: &str,
    project_path: Option<&str>,
    running: bool,
    read_only: bool,
    cx: &App,
) -> impl IntoElement {
    match item {
        TranscriptItem::User {
            id,
            text: body,
            images,
            can_rewind,
        } => {
            let mut bubble = v_flex()
                .max_w(gpui_kit::relative(0.82))
                .px_4()
                .py(px(10.0))
                .rounded_2xl()
                .border_1()
                .border_color(theme_setup::user_bubble_border(cx))
                .bg(theme_setup::user_bubble_bg(cx))
                .shadow_xs()
                .gap_2();

            if !body.is_empty() {
                bubble = bubble.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().foreground)
                        .child(body),
                );
            }
            if !images.is_empty() {
                bubble = bubble.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("📎 附带 {} 张图片", images.len())),
                );
            }

            let mut outer = v_flex().w_full().items_end().gap_1().child(bubble);
            if can_rewind && !running && !read_only {
                let st_rw = store_entity.clone();
                let sid_rw = session_id.to_owned();
                let id_rw = id.clone();
                let imgs_rw = images;
                outer = outer.child(
                    Button::new(SharedString::from(format!("rewind-{id}")))
                        .ghost()
                        .xsmall()
                        .label("编辑重发")
                        .on_click(move |_, _, cx| {
                            st_rw.update(cx, |s, cx| {
                                s.rewind(sid_rw.clone(), id_rw.clone(), imgs_rw.clone(), cx);
                            });
                        }),
                );
            }
            outer.into_any_element()
        }

        TranscriptItem::Assistant {
            id,
            text: body,
            thinking,
            model,
            streaming,
            error,
        } => {
            let mut card = v_flex()
                .w_full()
                .px_4()
                .py_3()
                .gap_2()
                .rounded_2xl()
                .border_1()
                .border_color(theme_setup::card_border(cx))
                .bg(theme_setup::card_bg(cx))
                .shadow_xs();

            if let Some(m) = model.filter(|m| !m.is_empty()) {
                card = card.child(
                    div()
                        .text_size(px(10.0))
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .text_color(cx.theme().muted_foreground)
                        .child(m),
                );
            }

            if !thinking.is_empty() {
                let think_key = format!("think:{id}");
                let expanded = store_entity.read(cx).is_expanded(&think_key);
                let st_tog = store_entity.clone();
                let k_tog = think_key.clone();
                let char_count = thinking.chars().count();

                card = card.child(
                    Collapsible::new()
                        .open(expanded)
                        .px_3()
                        .py(px(6.0))
                        .rounded_xl()
                        .border_1()
                        .border_color(theme_setup::card_border(cx))
                        .bg(theme_setup::inset_card_bg(cx))
                        .child(
                            h_flex()
                                .id(SharedString::from(format!("think-hdr-{id}")))
                                .w_full()
                                .items_center()
                                .justify_between()
                                .cursor_pointer()
                                .on_click(move |_, _, cx| {
                                    st_tog.update(cx, |s, cx| s.toggle_expanded(k_tog.clone(), cx));
                                })
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if expanded {
                                            "▾ 思考过程"
                                        } else {
                                            "▸ 思考过程"
                                        }),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(cx.theme().muted_foreground.opacity(0.75))
                                        .child(format!("{char_count} 字")),
                                ),
                        )
                        .content(
                            div()
                                .mt(px(6.0))
                                .pt(px(6.0))
                                .border_t_1()
                                .border_color(theme_setup::card_border(cx))
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(thinking),
                        ),
                );
            }

            if !body.is_empty() {
                let st_link = store_entity.clone();
                let pp_owned = project_path.map(str::to_owned);
                card = card.child(
                    TextView::markdown(SharedString::from(format!("md-{id}")), body)
                        .selectable(true)
                        .stream_fade(streaming)
                        .on_link_click(move |url, _, _, cx| {
                            handle_markdown_link(&st_link, url.as_ref(), pp_owned.as_deref(), cx);
                        }),
                );
            }

            if let Some(err) = error {
                card = card.child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .border_1()
                        .border_color(cx.theme().danger.opacity(0.4))
                        .bg(cx.theme().danger.opacity(0.1))
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(err),
                );
            }

            card.into_any_element()
        }

        TranscriptItem::Tool {
            id,
            name,
            args,
            status,
            output,
            diff,
            subagent,
            ..
        } => {
            if let Some(sub) = subagent {
                return render_subagent_tool_card(store_entity, id, sub, output, cx)
                    .into_any_element();
            }
            let summary = summarize_tool_args(&args);
            render_tool_row(store_entity, id, name, summary, status, output, diff, cx)
                .into_any_element()
        }

        TranscriptItem::Notice { tone, text: msg, .. } => {
            let color = match tone.as_str() {
                "error" => cx.theme().danger,
                "warning" => rgb(0xd97706).into(),
                _ => cx.theme().muted_foreground,
            };
            div()
                .px_3()
                .py_2()
                .rounded_xl()
                .border_1()
                .border_color(theme_setup::card_border(cx))
                .bg(theme_setup::inset_card_bg(cx))
                .text_xs()
                .text_color(color)
                .child(msg)
                .into_any_element()
        }

        TranscriptItem::Custom {
            id,
            text: body,
            notifications,
            ..
        } => {
            if let Some(list) = notifications
                && !list.is_empty()
            {
                let mut col = v_flex().w_full().gap_2();
                for (idx, notification) in list.into_iter().enumerate() {
                    col = col.child(render_subagent_notification_card(
                        store_entity,
                        format!("{id}-{idx}"),
                        notification,
                        cx,
                    ));
                }
                return col.into_any_element();
            }
            if body.is_empty() {
                return div().into_any_element();
            }
            div()
                .px_3()
                .py_2()
                .rounded_xl()
                .border_1()
                .border_color(theme_setup::card_border(cx))
                .bg(theme_setup::inset_card_bg(cx))
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(body)
                .into_any_element()
        }

        TranscriptItem::Run {
            status,
            duration_ms,
            ..
        } => {
            let mut label = status.label().to_owned();
            if let Some(dur) = format_duration(duration_ms) {
                label.push_str(" · ");
                label.push_str(&dur);
            }
            h_flex()
                .w_full()
                .items_center()
                .justify_center()
                .py_1()
                .child(
                    div()
                        .px_2()
                        .py(px(2.0))
                        .rounded_full()
                        .bg(theme_setup::inset_card_bg(cx))
                        .text_size(px(10.0))
                        .text_color(cx.theme().muted_foreground)
                        .child(label),
                )
                .into_any_element()
        }
    }
}

fn render_subagent_notification_card(
    store_entity: &Entity<AppStore>,
    id: String,
    notification: SubagentNotification,
    cx: &App,
) -> impl IntoElement {
    let tone = if notification.status.is_error() {
        BadgeTone::Red
    } else {
        BadgeTone::Green
    };
    let st_open = store_entity.clone();
    let target_id = notification.id.clone();

    let mut card = v_flex()
        .w_full()
        .px_3()
        .py_2()
        .gap_1()
        .rounded_2xl()
        .border_1()
        .border_color(theme_setup::card_border(cx))
        .bg(theme_setup::card_bg(cx))
        .shadow_xs()
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
                        .gap_2()
                        .child(pill_badge(notification.status.label(), tone, cx))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .text_color(cx.theme().foreground)
                                .child(notification.description),
                        ),
                )
                .when_some(target_id, |el, tid| {
                    el.child(
                        Button::new(SharedString::from(format!("open-sub-notif-{id}")))
                            .ghost()
                            .xsmall()
                            .label("查看子会话 →")
                            .on_click(move |_, _, cx| {
                                st_open.update(cx, |s, cx| {
                                    s.select_session(Some(tid.clone()), cx);
                                });
                            }),
                    )
                }),
        );

    if let Some(preview) = notification.preview.filter(|p| !p.is_empty()) {
        card = card.child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(preview),
        );
    }
    if let Some(err) = notification.error {
        card = card.child(div().text_xs().text_color(cx.theme().danger).child(err));
    }
    card
}

fn render_subagent_tool_card(
    store_entity: &Entity<AppStore>,
    id: String,
    sub: SubagentToolInfo,
    output: String,
    cx: &App,
) -> impl IntoElement {
    let key = format!("subtool:{id}");
    let expanded = store_entity.read(cx).is_expanded(&key);
    let st_tog = store_entity.clone();
    let k_tog = key.clone();

    let tone = if sub.status.is_error() {
        BadgeTone::Red
    } else if matches!(sub.status, crate::protocol::SubagentStatus::Running) {
        BadgeTone::Blue
    } else {
        BadgeTone::Green
    };

    let mut meta_parts = Vec::new();
    if let Some(u) = sub.tool_uses {
        meta_parts.push(format!("{u} 次工具"));
    }
    if let Some(t) = sub.tokens {
        meta_parts.push(format_compact_tokens(t));
    }
    if let Some(dur) = format_duration(sub.duration_ms) {
        meta_parts.push(dur);
    }
    let meta = meta_parts.join(" · ");

    let body_md = sub.output.clone().unwrap_or(output);
    let st_jump = store_entity.clone();
    let sub_sid = sub.id.clone();

    Collapsible::new()
        .open(expanded)
        .px_3()
        .py_2()
        .rounded_2xl()
        .border_1()
        .border_color(theme_setup::card_border(cx))
        .bg(theme_setup::card_bg(cx))
        .shadow_xs()
        .child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    h_flex()
                        .id(SharedString::from(format!("subtool-hdr-{id}")))
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            st_tog.update(cx, |s, cx| s.toggle_expanded(k_tog.clone(), cx));
                        })
                        .child(pill_badge(sub.status.label(), tone, cx))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .text_color(cx.theme().foreground)
                                .child(sub.description),
                        )
                        .when(!meta.is_empty(), |el| {
                            el.child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(meta),
                            )
                        }),
                )
                .when_some(sub_sid, |el, tid| {
                    el.child(
                        Button::new(SharedString::from(format!("jump-sub-{id}")))
                            .ghost()
                            .xsmall()
                            .label("打开 →")
                            .on_click(move |_, _, cx| {
                                st_jump.update(cx, |s, cx| {
                                    s.select_session(Some(tid.clone()), cx);
                                });
                            }),
                    )
                }),
        )
        .content({
            let mut inner = v_flex()
                .mt_2()
                .pt_2()
                .gap_2()
                .border_t_1()
                .border_color(theme_setup::card_border(cx));
            if let Some(err) = sub.error {
                inner = inner.child(
                    div()
                        .p_2()
                        .rounded_lg()
                        .bg(cx.theme().danger.opacity(0.12))
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(err),
                );
            }
            if !body_md.is_empty() {
                inner = inner.child(
                    TextView::markdown(SharedString::from(format!("sub-md-{id}")), body_md)
                        .selectable(true),
                );
            }
            inner
        })
}

fn render_tool_row(
    store_entity: &Entity<AppStore>,
    id: String,
    name: String,
    summary: String,
    status: ToolStatus,
    output: String,
    diff: Option<String>,
    cx: &App,
) -> impl IntoElement {
    let is_error = status == ToolStatus::Error;
    let expanded = is_error || store_entity.read(cx).is_expanded(&id);
    let st_tog = store_entity.clone();
    let id_tog = id.clone();

    let (status_label, tone) = match status {
        ToolStatus::Running => ("运行中", BadgeTone::Blue),
        ToolStatus::Done => ("完成", BadgeTone::Green),
        ToolStatus::Error => ("失败", BadgeTone::Red),
    };

    Collapsible::new()
        .open(expanded)
        .px_3()
        .py(px(7.0))
        .rounded_xl()
        .border_1()
        .border_color(theme_setup::card_border(cx))
        .bg(theme_setup::inset_card_bg(cx))
        .child(
            h_flex()
                .id(SharedString::from(format!("tool-hdr-{id}")))
                .w_full()
                .items_center()
                .justify_between()
                .gap_2()
                .cursor_pointer()
                .on_click(move |_, _, cx| {
                    st_tog.update(cx, |s, cx| s.toggle_expanded(id_tog.clone(), cx));
                })
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .text_color(cx.theme().foreground)
                                .child(format!("{} {name}", if expanded { "▾" } else { "▸" })),
                        )
                        .when(!summary.is_empty(), |el| {
                            el.child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(summary),
                            )
                        }),
                )
                .child(pill_badge(status_label, tone, cx)),
        )
        .content({
            let mut inner = v_flex()
                .mt_2()
                .pt_2()
                .gap_2()
                .border_t_1()
                .border_color(theme_setup::card_border(cx));
            if let Some(patch) = diff {
                inner = inner.child(render_diff_code_block(patch, cx));
            }
            if !output.is_empty() {
                let clipped = if output.len() > 4000 {
                    let prefix: String = output.chars().take(4000).collect();
                    format!("{prefix}\n…（已截断，共 {} 字）", output.chars().count())
                } else {
                    output
                };
                inner = inner.child(
                    div()
                        .p_2()
                        .rounded_lg()
                        .bg(theme_setup::card_bg(cx))
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(11.0))
                        .text_color(cx.theme().foreground.opacity(0.9))
                        .child(clipped),
                );
            }
            inner
        })
}

fn render_diff_code_block(patch: String, cx: &App) -> impl IntoElement {
    let mut lines_col = v_flex()
        .p_2()
        .rounded_lg()
        .bg(theme_setup::card_bg(cx))
        .border_1()
        .border_color(theme_setup::card_border(cx))
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(px(11.0))
        .gap(px(1.0));

    for line in patch.lines().take(180) {
        let color = if line.starts_with('+') && !line.starts_with("+++") {
            rgb(0x16a34a).into()
        } else if line.starts_with('-') && !line.starts_with("---") {
            cx.theme().danger
        } else if line.starts_with("@@") {
            rgb(0x2563eb).into()
        } else {
            cx.theme().muted_foreground
        };
        lines_col = lines_col.child(div().text_color(color).child(line.to_owned()));
    }
    lines_col
}

fn render_activity_group(
    store_entity: &Entity<AppStore>,
    block_id: String,
    item_ids: Vec<String>,
    session_id: &str,
    project_path: Option<&str>,
    running: bool,
    read_only: bool,
    cx: &App,
) -> impl IntoElement {
    let expanded = store_entity.read(cx).is_expanded(&block_id);
    let st_tog = store_entity.clone();
    let k_tog = block_id.clone();
    let count = item_ids.len();

    let mut content = v_flex()
        .mt_2()
        .pt_2()
        .gap(px(6.0))
        .border_t_1()
        .border_color(theme_setup::card_border(cx));
    if expanded {
        let store = store_entity.read(cx);
        for id in item_ids {
            if let Some(item) = store.main_slot.item_by_id(&id).cloned() {
                content = content.child(render_item(
                    store_entity,
                    item,
                    session_id,
                    project_path,
                    running,
                    read_only,
                    cx,
                ));
            }
        }
    }

    Collapsible::new()
        .open(expanded)
        .px_3()
        .py(px(7.0))
        .rounded_xl()
        .border_1()
        .border_color(theme_setup::card_border(cx))
        .bg(theme_setup::inset_card_bg(cx))
        .child(
            h_flex()
                .id(SharedString::from(format!("act-hdr-{block_id}")))
                .w_full()
                .items_center()
                .justify_between()
                .cursor_pointer()
                .on_click(move |_, _, cx| {
                    st_tog.update(cx, |s, cx| s.toggle_expanded(k_tog.clone(), cx));
                })
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "{} 已完成 {count} 项中间步骤",
                            if expanded { "▾" } else { "▸" }
                        )),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(cx.theme().muted_foreground.opacity(0.75))
                        .child(if expanded { "收起" } else { "展开" }),
                ),
        )
        .content(content)
}

fn render_changes_block(
    store_entity: &Entity<AppStore>,
    block_id: String,
    changes: Vec<FileChange>,
    cx: &App,
) -> impl IntoElement {
    let expanded = store_entity.read(cx).is_expanded(&block_id);
    let st_tog = store_entity.clone();
    let k_tog = block_id.clone();
    let count = changes.len();

    let mut content = v_flex()
        .mt_2()
        .pt_2()
        .gap_2()
        .border_t_1()
        .border_color(theme_setup::card_border(cx));
    if expanded {
        for c in changes {
            let mut entry = v_flex().gap_1().child(
                div()
                    .text_size(px(11.0))
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(cx.theme().foreground)
                    .child(c.path),
            );
            match c.kind {
                FileChangeKind::Patch(patch) => {
                    entry = entry.child(render_diff_code_block(patch, cx));
                }
                FileChangeKind::Write(text) => {
                    let clipped = if text.len() > 2000 {
                        let prefix: String = text.chars().take(2000).collect();
                        format!("{prefix}\n…")
                    } else {
                        text
                    };
                    entry = entry.child(
                        div()
                            .p_2()
                            .rounded_lg()
                            .bg(theme_setup::inset_card_bg(cx))
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(11.0))
                            .text_color(cx.theme().foreground.opacity(0.9))
                            .child(clipped),
                    );
                }
            }
            content = content.child(entry);
        }
    }

    Collapsible::new()
        .open(expanded)
        .px_3()
        .py_2()
        .rounded_2xl()
        .border_1()
        .border_color(theme_setup::card_border(cx))
        .bg(theme_setup::card_bg(cx))
        .shadow_xs()
        .child(
            h_flex()
                .id(SharedString::from(format!("chg-hdr-{block_id}")))
                .w_full()
                .items_center()
                .justify_between()
                .cursor_pointer()
                .on_click(move |_, _, cx| {
                    st_tog.update(cx, |s, cx| s.toggle_expanded(k_tog.clone(), cx));
                })
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .text_color(cx.theme().foreground)
                        .child(format!(
                            "{} 修改了 {count} 个文件",
                            if expanded { "▾" } else { "▸" }
                        )),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(cx.theme().muted_foreground)
                        .child(if expanded { "收起 diff" } else { "查看 diff" }),
                ),
        )
        .content(content)
}
