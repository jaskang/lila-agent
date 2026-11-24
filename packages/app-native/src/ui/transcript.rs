//! Virtualized transcript powered by `gpui-component`'s `MessageScroller` and
//! `TextView::markdown`: selectable user bubbles with hover copy/rewind, clean
//! borderless assistant prose with code-block copy and hover reply copy, live
//! multi-subagent progress cards, spinning running tools, semantic spacing,
//! and tinted line-by-line diff blocks.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::shimmer::ShimmerText;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::{SelectionFormat, TextView};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    App, Entity, Hsla, Image, ImageFormat, ImageSource, InteractiveElement as _, IntoElement,
    ObjectFit, ParentElement as _, SharedString, SharedUri, StatefulInteractiveElement as _,
    StyleRefinement, Styled as _, StyledImage as _, div, img, prelude::FluentBuilder as _, px,
};
use serde_json::Value;

use crate::format::{
    decode_base64, format_compact_tokens, format_duration, format_elapsed, now_millis,
    percent_decode,
};
use crate::protocol::{
    SubagentNotification, SubagentStatus, SubagentToolInfo, ToolStatus, TranscriptItem,
};
use crate::shell_bridge;
use crate::store::{AppStore, ToastTone};
use crate::transcript_layout::{FileChange, FileChangeKind, TranscriptBlock};
use crate::ui::{BadgeTone, pill_badge, theme_setup};

/// Convert arbitrary plain text (user messages, thinking traces, raw tool logs)
/// into Markdown that renders verbatim while preserving line breaks and enabling
/// native `TextView` text selection and plain-text clipboard copy.
pub(crate) fn plain_text_to_markdown(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mut out = String::with_capacity(normalized.len() + lines.len() * 4);

    for (idx, line) in lines.iter().enumerate() {
        for ch in line.chars() {
            if matches!(
                ch,
                '\\' | '`'
                    | '*'
                    | '_'
                    | '{'
                    | '}'
                    | '['
                    | ']'
                    | '('
                    | ')'
                    | '#'
                    | '+'
                    | '-'
                    | '.'
                    | '!'
                    | '>'
                    | '|'
                    | '~'
                    | '<'
                    | '$'
            ) {
                out.push('\\');
            }
            out.push(ch);
        }
        if idx + 1 < lines.len() {
            if line.is_empty() {
                out.push('\n');
            } else {
                out.push_str("  \n");
            }
        }
    }
    out
}

fn stable_hash(seed: &str, text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    seed.hash(&mut hasher);
    text.hash(&mut hasher);
    hasher.finish()
}

fn running_accent(cx: &App) -> Hsla {
    theme_setup::running_fg(cx)
}

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

fn decode_data_url_image(url: &str) -> Option<Arc<Image>> {
    let rest = url.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let mut parts = meta.split(';');
    let mime = parts.next()?.trim();
    let format = ImageFormat::from_mime_type(mime)?;
    let is_base64 = parts.any(|p| p.trim().eq_ignore_ascii_case("base64"));
    let bytes = if is_base64 {
        decode_base64(payload)?
    } else {
        percent_decode(payload).into_bytes()
    };
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

fn resolve_local_path(raw: &str, project_path: Option<&str>) -> Option<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let without_scheme = if let Some(rest) = trimmed.strip_prefix("file://localhost") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("file://") {
        rest
    } else {
        trimmed
    };
    let clean = without_scheme
        .split(['?', '#'])
        .next()
        .unwrap_or(without_scheme);
    let decoded = percent_decode(clean);
    if let Some(after_tilde) = decoded.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME")
            && !home.is_empty()
        {
            return Some(PathBuf::from(home).join(after_tilde));
        }
    }
    let path = PathBuf::from(&decoded);
    if path.is_absolute() {
        return Some(path);
    }
    if let Some(pp) = project_path {
        return Some(Path::new(pp).join(path));
    }
    Some(path)
}

fn resolve_markdown_image_source(uri: &SharedUri, project_path: Option<&str>) -> ImageSource {
    let raw = uri.as_ref().trim();
    if raw.starts_with("https://") || raw.starts_with("http://") {
        return ImageSource::from(uri.clone());
    }
    if raw.starts_with("data:") {
        if let Some(image) = decode_data_url_image(raw) {
            return ImageSource::Image(image);
        }
        return ImageSource::from(uri.clone());
    }
    if let Some(path) = resolve_local_path(raw, project_path) {
        return ImageSource::from(path);
    }
    ImageSource::from(uri.clone())
}

/// Build a `TextView::markdown` with syntax highlighting, local/inline image
/// resolution against the session's project directory, click-to-open links,
/// and per-fenced-block clipboard copy buttons.
fn render_rich_markdown(
    id: String,
    body: String,
    streaming: bool,
    store_entity: &Entity<AppStore>,
    project_path: Option<&str>,
) -> TextView {
    let st_link = store_entity.clone();
    let pp_owned = project_path.map(str::to_owned);
    let pp_img = pp_owned.clone();
    let block_seed = id.clone();

    TextView::markdown(SharedString::from(id), body)
        .selectable(true)
        .stream_fade(streaming)
        .image_source(move |uri| resolve_markdown_image_source(uri, pp_img.as_deref()))
        .code_block_actions(move |block, _, _| {
            let code = block.code().clone();
            let h = stable_hash(&block_seed, code.as_ref());
            Clipboard::new(SharedString::from(format!("copy-cb-{h:x}")))
                .value(code)
                .tooltip("复制代码")
        })
        .on_link_click(move |url, _, _, cx| {
            handle_markdown_link(&st_link, url.as_ref(), pp_owned.as_deref(), cx);
        })
}

/// Determine whether a quiet "thinking / waiting" indicator should sit below the
/// transcript while a run is active without a visible streaming reply or active tool.
fn should_show_thinking_footer(store: &AppStore) -> bool {
    if !store.is_busy()
        || store.main_slot.state.as_ref().is_some_and(|state| !state.pending.is_empty())
    {
        return false;
    }
    let Some(last) = store.main_slot.blocks.iter().rev()
        .find(|block| !matches!(block, TranscriptBlock::Changes { .. }))
    else {
        return true;
    };
    match last {
        TranscriptBlock::Item { item_id, .. } => {
            match store.main_slot.item_by_id(item_id) {
                Some(TranscriptItem::Assistant {
                    streaming: true,
                    text,
                    thinking,
                    ..
                }) => text.is_empty() && thinking.is_empty(),
                Some(TranscriptItem::Tool {
                    status: ToolStatus::Running,
                    ..
                }) => false,
                Some(TranscriptItem::Tool {
                    subagent: Some(sub),
                    ..
                }) if matches!(sub.status, SubagentStatus::Running | SubagentStatus::Queued) => {
                    false
                }
                _ => true,
            }
        }
        _ => true,
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
    let show_thinking_footer = should_show_thinking_footer(store);

    if loading && !has_items {
        return v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .child(Spinner::new().small().color(cx.theme().muted_foreground))
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
                    .rounded(px(7.0))
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
                    .text_color(cx.theme().muted_foreground.opacity(0.75))
                    .child("发一句话开始对话"),
            )
            .into_any_element();
    }

    let st_earlier = store_entity.clone();
    let st_row = store_entity.clone();

    // Override `MessageScroller`'s default 32px (`pb_8`) per-row gap so spacing
    // is controlled semantically by block type rather than uniform huge voids.
    let mut row_style = StyleRefinement::default();
    row_style.padding.left = Some(px(0.0).into());
    row_style.padding.right = Some(px(0.0).into());
    row_style.padding.bottom = Some(px(0.0).into());

    v_flex()
        .flex_1()
        .min_h_0()
        .when(has_earlier, |el| {
            el.child(
                h_flex().w_full().justify_center().py_1().child(
                    Button::new("load-earlier-btn")
                        .ghost()
                        .xsmall()
                        .rounded(px(6.0))
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

                let (pt, pb, inner) = match block {
                    TranscriptBlock::Activity { id, item_ids } => (
                        3.0,
                        3.0,
                        render_activity_group(
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
                    ),
                    TranscriptBlock::Changes { id, changes } => (
                        4.0,
                        4.0,
                        render_changes_block(&st_row, id, changes, cx).into_any_element(),
                    ),
                    TranscriptBlock::Item { item_id, .. } => {
                        if let Some(item) = store.main_slot.item_by_id(&item_id).cloned() {
                            let (top, bottom) = match &item {
                                TranscriptItem::User { .. } => {
                                    (if ix == 0 { 10.0 } else { 20.0 }, 6.0)
                                }
                                TranscriptItem::Assistant { .. } => (4.0, 4.0),
                                TranscriptItem::Tool { .. } => (3.0, 3.0),
                                TranscriptItem::Notice { .. }
                                | TranscriptItem::Custom { .. }
                                | TranscriptItem::Run { .. } => (4.0, 4.0),
                            };
                            (
                                top,
                                bottom,
                                render_item(
                                    &st_row,
                                    item,
                                    &sid,
                                    pp.as_deref(),
                                    running,
                                    read_only,
                                    cx,
                                )
                                .into_any_element(),
                            )
                        } else {
                            (0.0, 0.0, div().into_any_element())
                        }
                    }
                };

                h_flex()
                    .w_full()
                    .justify_center()
                    .px_6()
                    .pt(px(pt))
                    .pb(px(pb))
                    .child(div().w_full().max_w(px(theme_setup::CONTENT_MAX_W)).child(inner))
                    .into_any_element()
            })
            .flex_1()
            .min_h_0()
            .with_row_style(row_style)
            .with_bottom_fade(theme_setup::workspace_bg(cx))
            .with_jump_button_label("回到底部"),
        )
        .when(show_thinking_footer, |el| {
            el.child(
                h_flex().w_full().justify_center().px_6().py(px(4.0)).child(
                    h_flex()
                        .w_full()
                        .max_w(px(theme_setup::CONTENT_MAX_W))
                        .items_center()
                        .gap(px(7.0))
                        .child(Spinner::new().xsmall().color(running_accent(cx)))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(ShimmerText::new("思考中…")),
                        ),
                ),
            )
        })
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
                .min_w_0()
                .max_w(gpui_kit::relative(0.78))
                .px(px(14.0))
                .py(px(9.0))
                .rounded(px(theme_setup::RADIUS_MD))
                .bg(theme_setup::user_bubble_bg(cx))
                .gap(px(6.0));

            if !body.is_empty() {
                bubble = bubble.child(
                    div()
                        .text_size(px(14.5))
                        .text_color(cx.theme().foreground)
                        .child(
                            TextView::markdown(
                                SharedString::from(format!("user-md-{id}")),
                                plain_text_to_markdown(&body),
                            )
                            .selectable(true)
                            .selection_format(SelectionFormat::Plain),
                        ),
                );
            }
            if !images.is_empty() {
                let mut img_row = h_flex().flex_wrap().gap(px(6.0));
                let mut rendered_any = false;
                for image_input in &images {
                    if let Some(format) = ImageFormat::from_mime_type(&image_input.mime_type)
                        && let Some(bytes) = decode_base64(&image_input.data)
                    {
                        let gpui_img = Arc::new(Image::from_bytes(format, bytes));
                        img_row = img_row.child(
                            div()
                                .rounded(px(8.0))
                                .border_1()
                                .border_color(theme_setup::card_border(cx))
                                .overflow_hidden()
                                .child(
                                    img(ImageSource::Image(gpui_img))
                                        .max_w(px(280.0))
                                        .max_h(px(200.0))
                                        .object_fit(ObjectFit::Contain),
                                ),
                        );
                        rendered_any = true;
                    }
                }
                if rendered_any {
                    bubble = bubble.child(img_row);
                } else {
                    bubble = bubble.child(
                        h_flex()
                            .items_center()
                            .gap(px(5.0))
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(12.5))
                            .text_color(cx.theme().muted_foreground)
                            .child(Icon::new(IconName::File).size(px(11.5)))
                            .child(format!("附带 {} 张图片", images.len())),
                    );
                }
            }

            let mut actions_row = h_flex()
                .items_center()
                .gap(px(2.0))
                .opacity(0.0)
                .group_hover(group_name.clone(), |s| s.opacity(1.0));

            if !body.is_empty() {
                actions_row = actions_row.child(
                    Clipboard::new(SharedString::from(format!("copy-user-{id}")))
                        .value(body)
                        .tooltip("复制消息"),
                );
            }

            if can_rewind && !running && !read_only {
                let st_rw = store_entity.clone();
                let sid_rw = session_id.to_owned();
                let id_rw = id.clone();
                let imgs_rw = images;
                actions_row = actions_row.child(
                    Button::new(SharedString::from(format!("rewind-{id}")))
                        .ghost()
                        .xsmall()
                        .rounded(px(6.0))
                        .icon(
                            Icon::new(IconName::Undo2)
                                .size(px(11.0))
                                .text_color(cx.theme().muted_foreground),
                        )
                        .label("编辑重发")
                        .text_color(cx.theme().muted_foreground)
                        .on_click(move |_, _, cx| {
                            st_rw.update(cx, |s, cx| {
                                s.rewind(sid_rw.clone(), id_rw.clone(), imgs_rw.clone(), cx);
                            });
                        }),
                );
            }

            v_flex()
                .group(group_name)
                .w_full()
                .items_end()
                .gap(px(2.0))
                .child(bubble)
                .child(actions_row)
                .into_any_element()
        }

        // Borderless editorial prose directly on the elevated workspace card.
        TranscriptItem::Assistant {
            id,
            text: body,
            thinking,
            streaming,
            error,
            ..
        } => {
            let group_name: SharedString = format!("asst-msg-{id}").into();
            let mut col = v_flex().group(group_name.clone()).w_full().gap(px(6.0));

            if !thinking.is_empty() {
                let think_key = format!("think:{id}");
                let expanded = store_entity.read(cx).is_expanded(&think_key);
                let st_tog = store_entity.clone();
                let k_tog = think_key.clone();
                let char_count = thinking.chars().count();
                let active_thinking = streaming && body.is_empty();
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
                                .child(if active_thinking {
                                    Spinner::new()
                                        .xsmall()
                                        .color(running_accent(cx))
                                        .into_any_element()
                                } else {
                                    Icon::new(chevron)
                                        .size(px(12.0))
                                        .text_color(cx.theme().muted_foreground.opacity(0.7))
                                        .into_any_element()
                                })
                                .child(if active_thinking {
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(ShimmerText::new("思考中…"))
                                        .into_any_element()
                                } else {
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("思考过程")
                                        .into_any_element()
                                })
                                .child(
                                    div()
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_size(px(12.0))
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
                                .child(
                                    TextView::markdown(
                                        SharedString::from(format!("think-md-{id}")),
                                        plain_text_to_markdown(&thinking),
                                    )
                                    .selectable(true)
                                    .selection_format(SelectionFormat::Plain),
                                ),
                        ),
                );
            }

            if !body.is_empty() {
                col = col.child(render_rich_markdown(
                    format!("md-{id}"),
                    body.clone(),
                    streaming,
                    store_entity,
                    project_path,
                ));
            }

            if let Some(err) = error {
                col = col.child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(cx.theme().danger.opacity(0.18))
                        .bg(cx.theme().danger.opacity(0.08))
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(Icon::new(IconName::CircleAlert).size(px(13.0)))
                        .child(err),
                );
            }

            // Quiet hover-only copy action on completed assistant replies (no noisy model tag).
            if !streaming && !body.is_empty() {
                col = col.child(
                    h_flex()
                        .h(px(22.0))
                        .items_center()
                        .gap(px(2.0))
                        .opacity(0.0)
                        .group_hover(group_name, |s| s.opacity(1.0))
                        .child(
                            Clipboard::new(SharedString::from(format!("copy-asst-{id}")))
                                .value(body)
                                .tooltip("复制回复"),
                        ),
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
                return render_subagent_tool_card(store_entity, id, sub, output, project_path, cx)
                    .into_any_element();
            }
            let summary = summarize_tool_args(&args);
            render_tool_row(store_entity, id, name, summary, status, output, diff, cx)
                .into_any_element()
        }

        TranscriptItem::Notice { tone, text: msg, .. } => {
            let (icon, color) = match tone.as_str() {
                "error" => (IconName::CircleAlert, theme_setup::danger_fg(cx)),
                "warning" => (IconName::TriangleAlert, theme_setup::warning_fg(cx)),
                _ => (IconName::Info, cx.theme().muted_foreground),
            };
            h_flex()
                .items_center()
                .gap_2()
                .px_3()
                .py(px(6.0))
                .rounded(px(8.0))
                .border_1()
                .border_color(theme_setup::card_border(cx))
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
                .rounded(px(8.0))
                .border_1()
                .border_color(theme_setup::card_border(cx))
                .bg(theme_setup::subtle_surface_bg(cx))
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(body)
                .into_any_element()
        }

        // Only non-normal Run statuses reach here (Completed is omitted in layout).
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
            let color = match status {
                crate::protocol::RunStatus::Error => theme_setup::danger_fg(cx),
                crate::protocol::RunStatus::Aborted | crate::protocol::RunStatus::Incomplete => {
                    theme_setup::warning_fg(cx)
                }
                crate::protocol::RunStatus::Completed => cx.theme().muted_foreground,
            };
            h_flex()
                .w_full()
                .items_center()
                .gap(px(6.0))
                .py(px(2.0))
                .child(div().w(px(12.0)).h(px(1.0)).bg(color.opacity(0.45)))
                .child(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(12.0))
                        .text_color(color.opacity(0.85))
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
        .rounded(px(10.0))
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
                            .rounded(px(6.0))
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
    project_path: Option<&str>,
    cx: &App,
) -> impl IntoElement {
    let key = format!("subtool:{id}");
    let store = store_entity.read(cx);
    let expanded = store.is_expanded(&key);

    // Overlay real-time `SessionState.subagents` telemetry when available so
    // background and parallel sub-agents stream live activity directly in the card.
    let live = sub.id.as_deref().and_then(|sid| {
        store
            .main_slot
            .state
            .as_ref()
            .and_then(|st| st.subagents.as_ref())
            .and_then(|list| list.iter().find(|p| p.id == sid))
    });

    let status = live.map(|l| l.status).unwrap_or(sub.status);
    let activity = live
        .and_then(|l| l.activity.clone())
        .or_else(|| sub.activity.clone())
        .filter(|a| !a.trim().is_empty());
    let turns = live.map(|l| l.turns).filter(|&t| t > 0).or(sub.turns);
    let tool_uses = live
        .map(|l| l.tool_uses)
        .filter(|&u| u > 0)
        .or(sub.tool_uses);
    let tokens = live.map(|l| l.tokens).filter(|&t| t > 0).or(sub.tokens);
    let duration_str = live
        .and_then(|l| format_duration(l.duration_ms))
        .or_else(|| format_duration(sub.duration_ms))
        .or_else(|| {
            live.filter(|l| matches!(l.status, SubagentStatus::Running | SubagentStatus::Queued))
                .map(|l| format_elapsed(l.started_at, now_millis()))
        });
    let err_text = live
        .and_then(|l| l.error.clone())
        .or_else(|| sub.error.clone());
    let body_md = live
        .and_then(|l| l.output.clone())
        .or_else(|| sub.output.clone())
        .unwrap_or(output);

    let is_active = matches!(status, SubagentStatus::Running | SubagentStatus::Queued);
    let is_err = status.is_error();

    let mut meta_parts = Vec::new();
    if let Some(t) = turns {
        if let Some(max) = sub.max_turns {
            meta_parts.push(format!("{t}/{max} 轮"));
        } else {
            meta_parts.push(format!("{t} 轮"));
        }
    }
    if let Some(u) = tool_uses {
        meta_parts.push(format!("{u} 工具"));
    }
    if let Some(tok) = tokens {
        meta_parts.push(format_compact_tokens(tok));
    }
    if let Some(dur) = duration_str {
        meta_parts.push(dur);
    }
    let meta = meta_parts.join(" · ");

    let st_tog = store_entity.clone();
    let k_tog = key.clone();
    let st_jump = store_entity.clone();
    let sub_sid = sub.id.clone();

    let leading_status = if is_active {
        Spinner::new()
            .xsmall()
            .color(running_accent(cx))
            .into_any_element()
    } else if is_err {
        Icon::new(IconName::CircleAlert)
            .size(px(13.0))
            .text_color(cx.theme().danger)
            .into_any_element()
    } else {
        Icon::new(IconName::Check)
            .size(px(13.0))
            .text_color(theme_setup::success_fg(cx))
            .into_any_element()
    };

    let header = v_flex()
        .w_full()
        .gap(px(4.0))
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
                        .gap(px(7.0))
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
                            .size(px(11.5))
                            .text_color(cx.theme().muted_foreground.opacity(0.65)),
                        )
                        .child(leading_status)
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
                        .when(sub.background && is_active, |el| {
                            el.child(pill_badge("后台", BadgeTone::Muted, cx))
                        })
                        .when(is_err, |el| {
                            el.child(pill_badge(status.label(), BadgeTone::Red, cx))
                        })
                        .when(!meta.is_empty(), |el| {
                            el.child(
                                div()
                                    .flex_shrink_0()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_size(px(12.0))
                                    .text_color(cx.theme().muted_foreground.opacity(0.8))
                                    .child(meta),
                            )
                        }),
                )
                .when_some(sub_sid, |el, tid| {
                    el.child(
                        Button::new(SharedString::from(format!("jump-sub-{id}")))
                            .ghost()
                            .xsmall()
                            .rounded(px(6.0))
                            .icon(IconName::ArrowRight)
                            .label("查看")
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
        .when(is_active, |el| {
            el.when_some(activity, |el, act| {
                el.child(
                    div()
                        .pl(px(32.0))
                        .truncate()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(12.5))
                        .text_color(cx.theme().muted_foreground.opacity(0.85))
                        .child(act),
                )
            })
        });

    Collapsible::new()
        .open(expanded)
        .px_3()
        .py(px(8.0))
        .rounded(px(10.0))
        .border_1()
        .border_color(if is_active {
            running_accent(cx).opacity(0.18)
        } else if is_err {
            cx.theme().danger.opacity(0.18)
        } else {
            theme_setup::card_border(cx)
        })
        .bg(theme_setup::subtle_surface_bg(cx))
        .child(header)
        .content({
            let mut inner = v_flex()
                .mt_2()
                .pt_2()
                .gap_2()
                .border_t_1()
                .border_color(theme_setup::hairline(cx));
            if let Some(err) = err_text {
                inner = inner.child(
                    div()
                        .p_2()
                        .rounded(px(6.0))
                        .bg(cx.theme().danger.opacity(0.1))
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(err),
                );
            }
            if !body_md.is_empty() {
                inner = inner.child(render_rich_markdown(
                    format!("sub-md-{id}"),
                    body_md,
                    false,
                    store_entity,
                    project_path,
                ));
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
    let is_running = status == ToolStatus::Running;
    let expanded = store_entity.read(cx).is_expanded(&id);
    let st_tog = store_entity.clone();
    let id_tog = id.clone();

    let status_indicator = match status {
        ToolStatus::Running => Spinner::new()
            .xsmall()
            .color(running_accent(cx))
            .into_any_element(),
        ToolStatus::Done => Icon::new(IconName::Check)
            .size(px(12.0))
            .text_color(cx.theme().muted_foreground.opacity(0.55))
            .into_any_element(),
        ToolStatus::Error => Icon::new(IconName::CircleAlert)
            .size(px(12.0))
            .text_color(cx.theme().danger)
            .into_any_element(),
    };

    Collapsible::new()
        .open(expanded)
        .rounded(px(8.0))
        .when(expanded || is_running || is_error, |el| {
            el.px(px(10.0))
                .py(px(5.5))
                .bg(theme_setup::subtle_surface_bg(cx))
                .border_1()
                .border_color(if is_running {
                    running_accent(cx).opacity(0.18)
                } else if is_error {
                    cx.theme().danger.opacity(0.18)
                } else {
                    theme_setup::card_border(cx)
                })
        })
        .child(
            h_flex()
                .id(SharedString::from(format!("tool-hdr-{id}")))
                .w_full()
                .items_center()
                .justify_between()
                .gap_2()
                .rounded(px(7.0))
                .when(!expanded && !is_running && !is_error, |el| {
                    el.px(px(8.0))
                        .py(px(4.5))
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
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(px(13.0))
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .text_color(if is_error {
                                    cx.theme().danger
                                } else {
                                    cx.theme().foreground.opacity(0.9)
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
                                    .text_size(px(12.5))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(summary),
                            )
                        }),
                )
                .child(status_indicator),
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
                let full_output = output.clone();
                let clipped = if output.chars().count() > 4000 {
                    let prefix: String = output.chars().take(4000).collect();
                    format!("{prefix}\n…（已截断，共 {} 字）", output.chars().count())
                } else {
                    output
                };
                inner = inner.child(
                    v_flex()
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(theme_setup::card_border(cx))
                        .bg(theme_setup::code_surface_bg(cx))
                        .overflow_hidden()
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .justify_between()
                                .px(px(10.0))
                                .py(px(4.0))
                                .border_b_1()
                                .border_color(theme_setup::hairline(cx))
                                .child(
                                    div()
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_size(px(11.5))
                                        .text_color(cx.theme().muted_foreground.opacity(0.75))
                                        .child("输出"),
                                )
                                .child(
                                    Clipboard::new(SharedString::from(format!("copy-out-{id}")))
                                        .value(full_output)
                                        .tooltip("复制完整输出"),
                                ),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("tool-out-scroll-{id}")))
                                .max_h(px(320.0))
                                .overflow_y_scrollbar()
                                .px(px(10.0))
                                .py(px(8.0))
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(px(12.5))
                                .text_color(cx.theme().foreground.opacity(0.88))
                                .child(
                                    TextView::markdown(
                                        SharedString::from(format!("tool-out-md-{id}")),
                                        plain_text_to_markdown(&clipped),
                                    )
                                    .selectable(true)
                                    .selection_format(SelectionFormat::Plain),
                                ),
                        ),
                );
            }
            inner
        })
}

fn render_diff_code_block(patch: String, cx: &App) -> impl IntoElement {
    let mut lines_col = v_flex()
        .py(px(6.0))
        .rounded(px(theme_setup::RADIUS_SM))
        .bg(theme_setup::code_surface_bg(cx))
        .border_1()
        .border_color(theme_setup::card_border(cx))
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(px(12.5))
        .overflow_hidden();

    for line in patch.lines().take(180) {
        let (color, bg) = if line.starts_with('+') && !line.starts_with("+++") {
            (
                theme_setup::success_fg(cx),
                Some(theme_setup::diff_add_bg(cx)),
            )
        } else if line.starts_with('-') && !line.starts_with("---") {
            (theme_setup::danger_fg(cx), Some(theme_setup::diff_del_bg(cx)))
        } else if line.starts_with("@@") {
            (
                theme_setup::running_fg(cx),
                Some(theme_setup::diff_hunk_bg(cx)),
            )
        } else {
            (cx.theme().muted_foreground, None)
        };
        lines_col = lines_col.child(
            div()
                .w_full()
                .px(px(10.0))
                .py(px(0.5))
                .when_some(bg, |el, b| el.bg(b))
                .text_color(color)
                .child(line.to_owned()),
        );
    }
    lines_col
}

fn summarize_activity_group(store: &AppStore, item_ids: &[String]) -> String {
    let mut tool_count = 0usize;
    let mut think_count = 0usize;
    for id in item_ids {
        match store.main_slot.item_by_id(id) {
            Some(TranscriptItem::Tool { .. }) => tool_count += 1,
            Some(TranscriptItem::Assistant { .. }) => think_count += 1,
            _ => {}
        }
    }
    match (think_count > 0, tool_count) {
        (true, 0) => "思考过程".to_owned(),
        (false, 1) => {
            if let Some(TranscriptItem::Tool { name, args, .. }) =
                item_ids.first().and_then(|id| store.main_slot.item_by_id(id))
            {
                let summary = summarize_tool_args(args);
                if summary.is_empty() {
                    format!("执行了 {name}")
                } else {
                    format!("{name} · {summary}")
                }
            } else {
                "执行了 1 次工具".to_owned()
            }
        }
        (false, n) if n > 1 => format!("执行了 {n} 次工具"),
        (true, n) if n > 0 => format!("思考并执行了 {n} 次工具"),
        _ => format!("已完成 {} 项步骤", item_ids.len()),
    }
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
    let store = store_entity.read(cx);
    let expanded = store.is_expanded(&block_id);
    let summary_label = summarize_activity_group(store, &item_ids);
    let st_tog = store_entity.clone();
    let k_tog = block_id.clone();

    let mut content = v_flex()
        .mt_1()
        .ml(px(5.0))
        .pl_3()
        .gap(px(4.0))
        .border_l_1()
        .border_color(theme_setup::hairline(cx));
    if expanded {
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
                .py(px(2.5))
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
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(summary_label),
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
        for (idx, c) in changes.into_iter().enumerate() {
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
                            .text_size(px(12.5))
                            .text_color(cx.theme().foreground.opacity(0.9))
                            .child(c.path),
                    ),
            );
            match c.kind {
                FileChangeKind::Patch(patch) => {
                    entry = entry.child(render_diff_code_block(patch, cx));
                }
                FileChangeKind::Write(text) => {
                    let full_text = text.clone();
                    let clipped = if text.chars().count() > 2000 {
                        let prefix: String = text.chars().take(2000).collect();
                        format!("{prefix}\n…")
                    } else {
                        text
                    };
                    entry = entry.child(
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
                                            .id(SharedString::from(format!(
                                                "chg-write-scroll-{block_id}-{idx}"
                                            )))
                                            .flex_1()
                                            .min_w_0()
                                            .max_h(px(280.0))
                                            .overflow_y_scrollbar()
                                            .px(px(10.0))
                                            .py(px(8.0))
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .text_size(px(12.5))
                                            .text_color(cx.theme().foreground.opacity(0.88))
                                            .child(
                                                TextView::markdown(
                                                    SharedString::from(format!(
                                                        "chg-write-md-{block_id}-{idx}"
                                                    )),
                                                    plain_text_to_markdown(&clipped),
                                                )
                                                .selectable(true)
                                                .selection_format(SelectionFormat::Plain),
                                            ),
                                    )
                                    .child(
                                        div().p(px(4.0)).flex_shrink_0().child(
                                            Clipboard::new(SharedString::from(format!(
                                                "copy-chg-write-{block_id}-{idx}"
                                            )))
                                            .value(full_text)
                                            .tooltip("复制文件内容"),
                                        ),
                                    ),
                            ),
                    );
                }
            }
            content = content.child(entry);
        }
    }

    Collapsible::new()
        .open(expanded)
        .px_3()
        .py(px(7.5))
        .rounded(px(10.0))
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
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(12.0))
                        .text_color(cx.theme().muted_foreground.opacity(0.75))
                        .child(if expanded { "收起" } else { "查看改动" }),
                ),
        )
        .content(content)
}

#[cfg(test)]
mod tests {
    use gpui_kit::{ImageSource, Resource, SharedUri};

    use super::{plain_text_to_markdown, resolve_markdown_image_source};

    #[test]
    fn plain_text_to_markdown_escapes_syntax_and_preserves_newlines() {
        let input = "检查 foo_bar_baz\n1. #标题 *强调* `code`\n\n第三段";
        let escaped = plain_text_to_markdown(input);
        assert!(escaped.contains(r"foo\_bar\_baz"));
        assert!(escaped.contains(r"1\. \#标题 \*强调\* \`code\`"));
        assert!(escaped.contains("  \n"));
        assert!(escaped.contains("\n第三段"));
    }

    #[test]
    fn markdown_image_source_resolves_project_relative_file_and_data_urls() {
        let rel = SharedUri::from("public/image-display-test.png");
        match resolve_markdown_image_source(&rel, Some("/tmp/demo-proj")) {
            ImageSource::Resource(Resource::Path(p)) => {
                assert_eq!(
                    p.to_string_lossy(),
                    "/tmp/demo-proj/public/image-display-test.png"
                );
            }
            _ => panic!("expected Resource::Path for relative image"),
        }

        let file_uri = SharedUri::from("file:///tmp/my%20dir/test.png?v=1");
        match resolve_markdown_image_source(&file_uri, None) {
            ImageSource::Resource(Resource::Path(p)) => {
                assert_eq!(p.to_string_lossy(), "/tmp/my dir/test.png");
            }
            _ => panic!("expected Resource::Path for file:// URL"),
        }

        let data_uri = SharedUri::from("data:image/png;base64,iVBORw0KGgo=");
        assert!(matches!(
            resolve_markdown_image_source(&data_uri, None),
            ImageSource::Image(_)
        ));

        let http_uri = SharedUri::from("https://example.com/a.png");
        assert!(matches!(
            resolve_markdown_image_source(&http_uri, None),
            ImageSource::Resource(Resource::Uri(_))
        ));
    }
}
