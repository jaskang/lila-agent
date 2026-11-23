//! Virtualized transcript powered by `gpui-component`'s `MessageScroller` and
//! `TextView::markdown`: borderless editorial assistant prose, neutral user
//! bubbles with hover-only rewind, whisper-quiet collapsible traces, vector
//! icons, and clean diff blocks.

use std::collections::HashMap;
use std::path::Path;

use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
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

fn short_model_name(model_id: &str) -> &str {
    model_id.split_once('/').map(|(_, rest)| rest).unwrap_or(model_id)
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
                    .text_color(cx.theme().muted_foreground.opacity(0.8))
                    .child("发一句话开始对话"),
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
                        .icon(IconName::ChevronUp)
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
                    TranscriptBlock::Activity { id, item_ids } => render_activity_group(
                        &st_row,
                        id,
                        item_ids,
                        &sid,
                        pp.as_deref(),
                        running,
                        read_only,
                        cx,
                    )
                    .into_any_element(),
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
                    .px_6()
                    .py(px(6.0))
                    .child(div().w_full().max_w(px(760.0)).child(inner))
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
            let group_name: SharedString = format!("user-msg-{id}").into();

            let mut bubble = v_flex()
                .max_w(gpui_kit::relative(0.78))
                .px(px(14.0))
                .py(px(9.0))
                .rounded_2xl()
                .border_1()
                .border_color(theme_setup::user_bubble_border(cx))
                .bg(theme_setup::user_bubble_bg(cx))
                .gap(px(6.0));

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
                    h_flex()
                        .items_center()
                        .gap(px(5.0))
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(Icon::new(IconName::File).size(px(12.0)))
                        .child(format!("附带 {} 张图片", images.len())),
                );
            }

            let mut outer = v_flex()
                .group(group_name.clone())
                .w_full()
                .items_end()
                .gap(px(2.0))
                .pt_2()
                .child(bubble);

            if can_rewind && !running && !read_only {
                let st_rw = store_entity.clone();
                let sid_rw = session_id.to_owned();
                let id_rw = id.clone();
                let imgs_rw = images;
                outer = outer.child(
                    div()
                        .opacity(0.0)
                        .group_hover(group_name, |s| s.opacity(1.0))
                        .child(
                            Button::new(SharedString::from(format!("rewind-{id}")))
                                .ghost()
                                .xsmall()
                                .icon(
                                    Icon::new(IconName::Undo2)
                                        .size(px(11.0))
                                        .text_color(cx.theme().muted_foreground),
                                )
                                .label("编辑重发")
                                .text_color(cx.theme().muted_foreground)
                                .on_click(move |_, _, cx| {
                                    st_rw.update(cx, |s, cx| {
                                        s.rewind(
                                            sid_rw.clone(),
                                            id_rw.clone(),
                                            imgs_rw.clone(),
                                            cx,
                                        );
                                    });
                                }),
                        ),
                );
            }
            outer.into_any_element()
        }

        // Borderless editorial prose directly on the workspace canvas (no box-in-box).
        TranscriptItem::Assistant {
            id,
            text: body,
            thinking,
            model,
            streaming,
            error,
        } => {
            let mut col = v_flex().w_full().py_1().gap(px(8.0));

            if !thinking.is_empty() {
                let think_key = format!("think:{id}");
                let expanded = store_entity.read(cx).is_expanded(&think_key);
                let st_tog = store_entity.clone();
                let k_tog = think_key.clone();
                let char_count = thinking.chars().count();
                let chevron = if expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                };

                col = col.child(
                    Collapsible::new()
                        .open(expanded)
                        .child(
                            h_flex()
                                .id(SharedString::from(format!("think-hdr-{id}")))
                                .items_center()
                                .gap(px(6.0))
                                .py(px(2.0))
                                .cursor_pointer()
                                .hover(|s| s.text_color(cx.theme().foreground))
                                .on_click(move |_, _, cx| {
                                    st_tog.update(cx, |s, cx| s.toggle_expanded(k_tog.clone(), cx));
                                })
                                .child(
                                    Icon::new(chevron)
                                        .size(px(12.0))
                                        .text_color(cx.theme().muted_foreground.opacity(0.7)),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if streaming && body.is_empty() {
                                            "思考中…"
                                        } else {
                                            "思考过程"
                                        }),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.5))
                                        .text_color(cx.theme().muted_foreground.opacity(0.55))
                                        .child(format!("{char_count} 字")),
                                ),
                        )
                        .content(
                            div()
                                .mt(px(4.0))
                                .ml(px(5.0))
                                .pl_3()
                                .py(px(2.0))
                                .border_l_2()
                                .border_color(theme_setup::hairline(cx))
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(thinking),
                        ),
                );
            }

            if !body.is_empty() {
                let st_link = store_entity.clone();
                let pp_owned = project_path.map(str::to_owned);
                col = col.child(
                    TextView::markdown(SharedString::from(format!("md-{id}")), body)
                        .selectable(true)
                        .stream_fade(streaming)
                        .on_link_click(move |url, _, _, cx| {
                            handle_markdown_link(&st_link, url.as_ref(), pp_owned.as_deref(), cx);
                        }),
                );
            }

            if let Some(err) = error {
                col = col.child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .border_1()
                        .border_color(cx.theme().danger.opacity(0.32))
                        .bg(cx.theme().danger.opacity(0.08))
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(Icon::new(IconName::CircleAlert).size(px(13.0)))
                        .child(err),
                );
            }

            if !streaming
                && let Some(m) = model.filter(|m| !m.is_empty())
            {
                col = col.child(
                    div()
                        .text_size(px(10.5))
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_color(cx.theme().muted_foreground.opacity(0.5))
                        .child(short_model_name(&m).to_owned()),
                );
            }

            col.into_any_element()
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
            let (icon, color) = match tone.as_str() {
                "error" => (IconName::CircleAlert, cx.theme().danger),
                "warning" => (IconName::TriangleAlert, rgb(0xf59e0b).into()),
                _ => (IconName::Info, cx.theme().muted_foreground),
            };
            h_flex()
                .items_center()
                .gap_2()
                .px_3()
                .py(px(6.0))
                .rounded_lg()
                .bg(theme_setup::subtle_surface_bg(cx))
                .text_xs()
                .text_color(color)
                .child(Icon::new(icon).size(px(13.0)).text_color(color))
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
                let mut col = v_flex().w_full().gap(px(6.0));
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
                .rounded_lg()
                .bg(theme_setup::subtle_surface_bg(cx))
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
                .gap(px(6.0))
                .py(px(2.0))
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(cx.theme().muted_foreground.opacity(0.55))
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
    let is_err = notification.status.is_error();
    let st_open = store_entity.clone();
    let target_id = notification.id.clone();

    let mut card = v_flex()
        .w_full()
        .px_3()
        .py(px(8.0))
        .gap_1()
        .rounded_xl()
        .border_1()
        .border_color(theme_setup::card_border(cx))
        .bg(theme_setup::subtle_surface_bg(cx))
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
                        .child(
                            Icon::new(if is_err {
                                IconName::CircleAlert
                            } else {
                                IconName::Bot
                            })
                            .size(px(13.0))
                            .text_color(if is_err {
                                cx.theme().danger
                            } else {
                                cx.theme().muted_foreground
                            }),
                        )
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
                            .icon(IconName::ArrowRight)
                            .label("子会话")
                            .text_color(cx.theme().muted_foreground)
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

    let is_running = matches!(sub.status, crate::protocol::SubagentStatus::Running);
    let is_err = sub.status.is_error();

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
        .py(px(7.0))
        .rounded_xl()
        .border_1()
        .border_color(theme_setup::card_border(cx))
        .bg(theme_setup::subtle_surface_bg(cx))
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
                        .child(
                            Icon::new(if expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size(px(12.0))
                            .text_color(cx.theme().muted_foreground.opacity(0.7)),
                        )
                        .child(
                            Icon::new(IconName::Bot)
                                .size(px(13.0))
                                .text_color(if is_err {
                                    cx.theme().danger
                                } else if is_running {
                                    rgb(0x64a8ff).into()
                                } else {
                                    cx.theme().muted_foreground
                                }),
                        )
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
                        .when(is_running || is_err, |el| {
                            el.child(pill_badge(
                                sub.status.label(),
                                if is_err {
                                    BadgeTone::Red
                                } else {
                                    BadgeTone::Blue
                                },
                                cx,
                            ))
                        })
                        .when(!meta.is_empty(), |el| {
                            el.child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(cx.theme().muted_foreground.opacity(0.75))
                                    .child(meta),
                            )
                        }),
                )
                .when_some(sub_sid, |el, tid| {
                    el.child(
                        Button::new(SharedString::from(format!("jump-sub-{id}")))
                            .ghost()
                            .xsmall()
                            .icon(IconName::ArrowRight)
                            .tooltip("打开子会话")
                            .text_color(cx.theme().muted_foreground)
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
                .border_color(theme_setup::hairline(cx));
            if let Some(err) = sub.error {
                inner = inner.child(
                    div()
                        .p_2()
                        .rounded_lg()
                        .bg(cx.theme().danger.opacity(0.1))
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

    let status_icon = match status {
        ToolStatus::Running => Icon::new(IconName::LoaderCircle)
            .size(px(12.0))
            .text_color(if cx.theme().is_dark() {
                rgb(0x64a8ff)
            } else {
                rgb(0x0071e3)
            }),
        ToolStatus::Done => Icon::new(IconName::Check)
            .size(px(12.0))
            .text_color(cx.theme().muted_foreground.opacity(0.55)),
        ToolStatus::Error => Icon::new(IconName::CircleAlert)
            .size(px(12.0))
            .text_color(cx.theme().danger),
    };

    Collapsible::new()
        .open(expanded)
        .rounded_lg()
        .when(expanded, |el| {
            el.px(px(8.0))
                .py(px(6.0))
                .bg(theme_setup::subtle_surface_bg(cx))
                .border_1()
                .border_color(theme_setup::card_border(cx))
        })
        .child(
            h_flex()
                .id(SharedString::from(format!("tool-hdr-{id}")))
                .w_full()
                .items_center()
                .justify_between()
                .gap_2()
                .rounded_lg()
                .when(!expanded, |el| {
                    el.px(px(8.0))
                        .py(px(5.0))
                        .hover(|s| s.bg(cx.theme().list_hover))
                })
                .cursor_pointer()
                .on_click(move |_, _, cx| {
                    st_tog.update(cx, |s, cx| s.toggle_expanded(id_tog.clone(), cx));
                })
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            Icon::new(if expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size(px(11.0))
                            .text_color(cx.theme().muted_foreground.opacity(0.6)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .text_color(if is_error {
                                    cx.theme().danger
                                } else {
                                    cx.theme().foreground.opacity(0.88)
                                })
                                .child(name),
                        )
                        .when(!summary.is_empty(), |el| {
                            el.child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_size(px(11.0))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(summary),
                            )
                        }),
                )
                .child(status_icon),
        )
        .content({
            let mut inner = v_flex()
                .mt_2()
                .pt_2()
                .gap_2()
                .border_t_1()
                .border_color(theme_setup::hairline(cx));
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
                        .rounded_md()
                        .bg(theme_setup::card_bg(cx))
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(11.0))
                        .text_color(cx.theme().foreground.opacity(0.88))
                        .child(clipped),
                );
            }
            inner
        })
}

fn render_diff_code_block(patch: String, cx: &App) -> impl IntoElement {
    let is_dark = cx.theme().is_dark();
    let mut lines_col = v_flex()
        .p_2()
        .rounded_md()
        .bg(theme_setup::card_bg(cx))
        .border_1()
        .border_color(theme_setup::card_border(cx))
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(px(11.0))
        .gap(px(1.0));

    for line in patch.lines().take(180) {
        let color = if line.starts_with('+') && !line.starts_with("+++") {
            if is_dark {
                rgb(0x4ade80).into()
            } else {
                rgb(0x15803d).into()
            }
        } else if line.starts_with('-') && !line.starts_with("---") {
            cx.theme().danger
        } else if line.starts_with("@@") {
            if is_dark {
                rgb(0x64a8ff).into()
            } else {
                rgb(0x0066cc).into()
            }
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
        .mt_1()
        .ml(px(5.0))
        .pl_3()
        .gap(px(4.0))
        .border_l_1()
        .border_color(theme_setup::hairline(cx));
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
        .child(
            h_flex()
                .id(SharedString::from(format!("act-hdr-{block_id}")))
                .items_center()
                .gap(px(6.0))
                .py(px(3.0))
                .cursor_pointer()
                .on_click(move |_, _, cx| {
                    st_tog.update(cx, |s, cx| s.toggle_expanded(k_tog.clone(), cx));
                })
                .child(
                    Icon::new(if expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .size(px(12.0))
                    .text_color(cx.theme().muted_foreground.opacity(0.65)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("已完成 {count} 项步骤")),
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
        .border_color(theme_setup::hairline(cx));
    if expanded {
        for c in changes {
            let mut entry = v_flex().gap_1().child(
                h_flex()
                    .items_center()
                    .gap(px(5.0))
                    .child(
                        Icon::new(IconName::FileText)
                            .size(px(11.0))
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(11.0))
                            .text_color(cx.theme().foreground.opacity(0.9))
                            .child(c.path),
                    ),
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
                            .rounded_md()
                            .bg(theme_setup::card_bg(cx))
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(11.0))
                            .text_color(cx.theme().foreground.opacity(0.88))
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
        .py(px(7.0))
        .rounded_xl()
        .border_1()
        .border_color(theme_setup::card_border(cx))
        .bg(theme_setup::subtle_surface_bg(cx))
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
                    h_flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            Icon::new(if expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size(px(12.0))
                            .text_color(cx.theme().muted_foreground.opacity(0.7)),
                        )
                        .child(
                            Icon::new(IconName::FileText)
                                .size(px(12.0))
                                .text_color(cx.theme().muted_foreground),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .text_color(cx.theme().foreground.opacity(0.9))
                                .child(format!("修改了 {count} 个文件")),
                        ),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(cx.theme().muted_foreground.opacity(0.7))
                        .child(if expanded { "收起" } else { "查看改动" }),
                ),
        )
        .content(content)
}
