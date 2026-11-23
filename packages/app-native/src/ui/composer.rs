//! Multi-line floating glass composer built on `gpui-component`'s `Textarea` with
//! `/` slash commands, `@` file mentions, image attachments, live status strip,
//! quiet controls, and circular primary send/stop buttons.

use std::rc::Rc;

use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, px, rgb,
};

use crate::format::{format_elapsed, now_millis};
use crate::protocol::{
    ApprovalMode, ConnectionState, ModelInfo, SlashCommand, SubagentStatus, ThinkingLevel,
};
use crate::store::{AppStore, NEW_DRAFT};
use crate::ui::controls::{
    render_approval_picker, render_context_meter, render_model_picker, render_thinking_picker,
};
use crate::ui::theme_setup;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompletionOption {
    label: String,
    detail: Option<String>,
    insert_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ActiveTrigger {
    Slash {
        start: usize,
        end: usize,
        query: String,
    },
    At {
        start: usize,
        end: usize,
        query: String,
    },
}

fn detect_trigger(text: &str) -> Option<ActiveTrigger> {
    let byte_idx = text.len();
    let line_start = text.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line = &text[line_start..];

    if line_start == 0 && line.starts_with('/') && !line.contains(' ') {
        return Some(ActiveTrigger::Slash {
            start: 0,
            end: byte_idx,
            query: line[1..].to_lowercase(),
        });
    }

    if let Some(at_rel) = line.rfind('@') {
        let valid_prefix = at_rel == 0
            || line[..at_rel]
                .chars()
                .last()
                .is_some_and(|c| c.is_whitespace());
        let after_at = &line[at_rel + 1..];
        if valid_prefix && !after_at.contains(char::is_whitespace) {
            return Some(ActiveTrigger::At {
                start: line_start + at_rel,
                end: byte_idx,
                query: after_at.to_owned(),
            });
        }
    }

    None
}

pub struct ComposerView {
    store: Entity<AppStore>,
    textarea: Entity<TextareaState>,
    model_search: Entity<InputState>,
    commands_cache: Vec<SlashCommand>,
    loaded_commands_project: Option<String>,
    file_matches: Vec<String>,
    active_trigger: Option<ActiveTrigger>,
    _subscriptions: Vec<Subscription>,
}

impl ComposerView {
    pub fn new(store: Entity<AppStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let initial_key = store.read(cx).active_draft_key();
        let initial_text = store.read(cx).draft_text(&initial_key);

        let textarea = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("发送消息…  / 命令 · @ 文件")
                .auto_grow(2, 8)
                .default_value(initial_text)
        });

        let model_search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索模型或提供商…"));

        let sub = cx.subscribe_in(
            &textarea,
            window,
            |this, textarea, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let val = textarea.read(cx).value().to_string();
                    let key = this.store.read(cx).active_draft_key();
                    this.store
                        .update(cx, |s, cx| s.set_draft_text(&key, val.clone(), cx));
                    this.refresh_trigger(&val, cx);
                }
                InputEvent::PressEnter { secondary, shift } => {
                    if *shift {
                        return;
                    }
                    if let Some(first) = this.current_options().into_iter().next() {
                        this.apply_completion(first, window, cx);
                        return;
                    }
                    let mode = if *secondary { Some("followUp") } else { None };
                    this.submit(mode, cx);
                }
                _ => {}
            },
        );

        Self {
            store,
            textarea,
            model_search,
            commands_cache: Vec::new(),
            loaded_commands_project: None,
            file_matches: Vec::new(),
            active_trigger: None,
            _subscriptions: vec![sub],
        }
    }

    pub fn sync_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.textarea.read(cx).value().as_ref() != text.as_str() {
            self.textarea.update(cx, |ta, cx| {
                ta.set_value(text, window, cx);
            });
        }
        self.active_trigger = None;
        cx.notify();
    }

    fn current_project_id(&self, cx: &App) -> Option<String> {
        let store = self.store.read(cx);
        if let Some(ref sid) = store.selected_id {
            store.sessions.get(sid).map(|s| s.project_id.clone())
        } else {
            store.active_project().map(|p| p.id.clone())
        }
    }

    fn ensure_commands_loaded(&mut self, cx: &mut Context<Self>) {
        let pid = self.current_project_id(cx);
        if pid == self.loaded_commands_project {
            return;
        }
        self.loaded_commands_project = pid.clone();
        let Some(project_id) = pid else {
            self.commands_cache.clear();
            return;
        };
        let sid = self.store.read(cx).selected_id.clone();
        let fut = self.store.read(cx).list_commands_rpc(project_id, sid);
        cx.spawn(async move |this, cx| {
            let list = fut.await;
            let _ = this.update(cx, |view, cx| {
                view.commands_cache = list;
                cx.notify();
            });
        })
        .detach();
    }

    fn refresh_trigger(&mut self, text: &str, cx: &mut Context<Self>) {
        let trig = detect_trigger(text);
        self.active_trigger = trig.clone();
        if let Some(ActiveTrigger::At { query, .. }) = trig
            && let Some(pid) = self.current_project_id(cx)
        {
            let fut = self.store.read(cx).search_files_rpc(pid, query);
            cx.spawn(async move |this, cx| {
                let matches = fut.await;
                let _ = this.update(cx, |view, cx| {
                    view.file_matches = matches;
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    fn current_options(&self) -> Vec<CompletionOption> {
        let Some(ref trig) = self.active_trigger else {
            return Vec::new();
        };
        match trig {
            ActiveTrigger::Slash { query, .. } => {
                let mut out = Vec::new();
                for (name, desc) in [
                    ("compact", "压缩当前会话上下文"),
                    ("reload", "重新加载技能、扩展和 MCP"),
                ] {
                    if query.is_empty() || name.contains(query) {
                        out.push(CompletionOption {
                            label: format!("/{name}"),
                            detail: Some(desc.to_owned()),
                            insert_text: format!("/{name} "),
                        });
                    }
                }
                for cmd in &self.commands_cache {
                    if cmd.name == "compact" || cmd.name == "reload" {
                        continue;
                    }
                    if query.is_empty() || cmd.name.to_lowercase().contains(query) {
                        let detail = if cmd.description.is_empty() {
                            Some(cmd.source_label().to_owned())
                        } else {
                            Some(cmd.description.clone())
                        };
                        out.push(CompletionOption {
                            label: format!("/{}", cmd.name),
                            detail,
                            insert_text: format!("/{} ", cmd.name),
                        });
                    }
                }
                out.truncate(10);
                out
            }
            ActiveTrigger::At { .. } => self
                .file_matches
                .iter()
                .take(10)
                .map(|path| CompletionOption {
                    label: format!("@{path}"),
                    detail: None,
                    insert_text: format!("@{path} "),
                })
                .collect(),
        }
    }

    fn apply_completion(
        &mut self,
        opt: CompletionOption,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(trig) = self.active_trigger.take() else {
            return;
        };
        let (start, end) = match trig {
            ActiveTrigger::Slash { start, end, .. } | ActiveTrigger::At { start, end, .. } => {
                (start, end)
            }
        };
        let current = self.textarea.read(cx).value().to_string();
        if start <= end && end <= current.len() {
            let next = format!("{}{}{}", &current[..start], opt.insert_text, &current[end..]);
            let key = self.store.read(cx).active_draft_key();
            self.store
                .update(cx, |s, cx| s.set_draft_text(&key, next.clone(), cx));
            self.textarea
                .update(cx, |ta, cx| ta.set_value(next, window, cx));
        }
        cx.notify();
    }

    fn submit(&mut self, mode: Option<&'static str>, cx: &mut Context<Self>) {
        let connected = self.store.read(cx).connection == ConnectionState::Connected;
        if !connected {
            return;
        }
        let selected = self.store.read(cx).selected_id.clone();
        if let Some(sid) = selected {
            self.store
                .update(cx, |s, cx| s.send_draft(sid, mode, cx));
        } else {
            self.store.update(cx, |s, cx| s.create_and_prompt(cx));
        }
    }
}

impl Render for ComposerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_commands_loaded(cx);

        let store = self.store.read(cx);
        let selected_id = store.selected_id.clone();
        let draft_key = selected_id
            .clone()
            .unwrap_or_else(|| NEW_DRAFT.to_owned());
        let connected = store.connection == ConnectionState::Connected;
        let busy = selected_id.is_some() && store.is_busy();
        let sending = store.sending_drafts.contains(&draft_key);
        let text_val = self.textarea.read(cx).value().to_string();
        let images = store.draft_images_for(&draft_key);
        let can_send = connected && !sending && (!text_val.trim().is_empty() || !images.is_empty());

        let is_new = selected_id.is_none();
        let session_state = store.main_slot.state.clone();

        let (model_id, thinking, thinking_levels, approval) = if is_new {
            let mid = store.effective_new_model_id();
            let reasoning = mid
                .as_deref()
                .and_then(|id| store.models.iter().find(|m| m.id == id))
                .is_some_and(|m| m.reasoning);
            let levels = if reasoning {
                vec![
                    ThinkingLevel::Off,
                    ThinkingLevel::Minimal,
                    ThinkingLevel::Low,
                    ThinkingLevel::Medium,
                    ThinkingLevel::High,
                    ThinkingLevel::Xhigh,
                ]
            } else {
                vec![ThinkingLevel::Off]
            };
            (
                mid,
                store.new_session.thinking_level.unwrap_or(ThinkingLevel::Off),
                levels,
                store.new_session.approval_mode,
            )
        } else {
            let st_ref = session_state.as_ref();
            (
                st_ref.and_then(|s| s.model_id.clone()),
                st_ref
                    .map(|s| s.thinking_level)
                    .unwrap_or(ThinkingLevel::Off),
                st_ref.map(|s| s.thinking_levels.clone()).unwrap_or_default(),
                st_ref.map(|s| s.approval_mode).unwrap_or(ApprovalMode::Auto),
            )
        };

        let st_m = self.store.clone();
        let sid_m = selected_id.clone();
        let model_el = render_model_picker(
            "composer-model-picker",
            self.store.clone(),
            self.model_search.clone(),
            model_id,
            !connected,
            Rc::new(move |m: ModelInfo, cx| {
                st_m.update(cx, |s, cx| {
                    if let Some(ref sid) = sid_m {
                        s.configure(sid.clone(), Some(m.id), None, None, cx);
                    } else {
                        s.update_new_session(|ns| ns.model_id = Some(m.id), cx);
                    }
                });
            }),
            cx,
        );

        let st_t = self.store.clone();
        let sid_t = selected_id.clone();
        let thinking_el = render_thinking_picker(
            "composer-thinking-picker",
            thinking,
            thinking_levels,
            !connected,
            Rc::new(move |lvl: ThinkingLevel, cx| {
                st_t.update(cx, |s, cx| {
                    if let Some(ref sid) = sid_t {
                        s.configure(sid.clone(), None, Some(lvl), None, cx);
                    } else {
                        s.update_new_session(|ns| ns.thinking_level = Some(lvl), cx);
                    }
                });
            }),
            cx,
        );

        let st_a = self.store.clone();
        let sid_a = selected_id.clone();
        let approval_el = render_approval_picker(
            "composer-approval-picker",
            approval,
            !connected,
            Rc::new(move |mode: ApprovalMode, cx| {
                st_a.update(cx, |s, cx| {
                    if let Some(ref sid) = sid_a {
                        s.configure(sid.clone(), None, None, Some(mode), cx);
                    } else {
                        s.update_new_session(|ns| ns.approval_mode = mode, cx);
                    }
                });
            }),
            cx,
        );

        let ctx_el = selected_id
            .clone()
            .and_then(|sid| render_context_meter(self.store.clone(), sid, cx));

        let status_bar = render_status_bar(&self.store, cx);

        let options = self.current_options();
        let autocomplete = (!options.is_empty()).then(|| {
            let mut list = v_flex().p_1().gap(px(2.0));
            for (i, opt) in options.into_iter().enumerate() {
                let opt_click = opt.clone();
                list = list.child(
                    h_flex()
                        .id(SharedString::from(format!("comp-opt-{i}")))
                        .w_full()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .px_2()
                        .py(px(5.0))
                        .rounded_md()
                        .cursor_pointer()
                        .when(i == 0, |el| el.bg(cx.theme().accent))
                        .hover(|s| s.bg(cx.theme().accent))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.apply_completion(opt_click.clone(), window, cx);
                        }))
                        .child(
                            div()
                                .text_xs()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_color(cx.theme().foreground)
                                .child(opt.label),
                        )
                        .when_some(opt.detail, |el, d| {
                            el.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .truncate()
                                    .child(d),
                            )
                        }),
                );
            }
            div()
                .w_full()
                .rounded_xl()
                .border_1()
                .border_color(theme_setup::composer_border(cx))
                .bg(cx.theme().popover)
                .shadow_md()
                .child(
                    div()
                        .id("comp-opt-scroll")
                        .max_h(px(200.0))
                        .overflow_y_scrollbar()
                        .child(list),
                )
        });

        let st_abort = self.store.clone();
        let sid_abort = selected_id.clone();

        v_flex()
            .w_full()
            .gap(px(6.0))
            .when_some(status_bar, |el, bar| el.child(bar))
            .when_some(autocomplete, |el, pop| el.child(pop))
            .child(
                v_flex()
                    .w_full()
                    .px_3()
                    .pt(px(10.0))
                    .pb_2()
                    .gap(px(6.0))
                    .rounded_2xl()
                    .border_1()
                    .border_color(theme_setup::composer_border(cx))
                    .bg(theme_setup::composer_bg(cx))
                    .shadow_sm()
                    .when(!images.is_empty(), |el| {
                        let mut row = h_flex().flex_wrap().gap_2();
                        for img in images.clone() {
                            let st_rm = self.store.clone();
                            let dk_rm = draft_key.clone();
                            let id_rm = img.id.clone();
                            row = row.child(
                                h_flex()
                                    .items_center()
                                    .gap(px(5.0))
                                    .pl_2()
                                    .pr_1()
                                    .py(px(3.0))
                                    .rounded_lg()
                                    .border_1()
                                    .border_color(theme_setup::card_border(cx))
                                    .bg(theme_setup::subtle_surface_bg(cx))
                                    .child(
                                        Icon::new(IconName::File)
                                            .size(px(12.0))
                                            .text_color(cx.theme().muted_foreground),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().foreground)
                                            .child(img.name),
                                    )
                                    .child(
                                        Button::new(SharedString::from(format!("rm-img-{}", img.id)))
                                            .ghost()
                                            .xsmall()
                                            .compact()
                                            .rounded(px(999.0))
                                            .icon(Icon::new(IconName::Close).size(px(11.0)))
                                            .on_click(move |_, _, cx| {
                                                st_rm.update(cx, |s, cx| {
                                                    s.remove_draft_image(&dk_rm, &id_rm, cx);
                                                });
                                            }),
                                    ),
                            );
                        }
                        el.child(row)
                    })
                    .child(
                        Textarea::new(&self.textarea)
                            .appearance(false)
                            .bordered(false),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                h_flex()
                                    .items_center()
                                    .flex_wrap()
                                    .gap(px(2.0))
                                    .child(model_el)
                                    .when_some(thinking_el, |el, t| el.child(t))
                                    .child(approval_el),
                            )
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .when_some(ctx_el, |el, c| el.child(c))
                                    .when(busy && !can_send, |el| {
                                        let st_ab = st_abort.clone();
                                        let sid_ab = sid_abort.clone();
                                        el.child(
                                            Button::new("composer-stop")
                                                .primary()
                                                .xsmall()
                                                .rounded(px(999.0))
                                                .icon(Icon::new(IconName::Square).size(px(11.0)))
                                                .tooltip("停止运行")
                                                .on_click(move |_, _, cx| {
                                                    if let Some(ref sid) = sid_ab {
                                                        st_ab.update(cx, |s, cx| {
                                                            s.abort(sid.clone(), cx);
                                                        });
                                                    }
                                                }),
                                        )
                                    })
                                    .when(!busy || can_send, |el| {
                                        el.child(
                                            Button::new("composer-send")
                                                .primary()
                                                .xsmall()
                                                .rounded(px(999.0))
                                                .disabled(!can_send)
                                                .icon(Icon::new(IconName::ArrowUp).size(px(14.0)))
                                                .tooltip(if busy {
                                                    "引导发送 (↵)"
                                                } else {
                                                    "发送 (↵)"
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.submit(None, cx);
                                                })),
                                        )
                                    }),
                            ),
                    ),
            )
    }
}

fn render_status_bar(store_entity: &Entity<AppStore>, cx: &App) -> Option<impl IntoElement> {
    let store = store_entity.read(cx);
    let s = store.main_slot.state.as_ref()?;
    let running_subs: Vec<_> = s
        .subagents
        .as_ref()
        .map(|list| {
            list.iter()
                .filter(|sub| {
                    matches!(sub.status, SubagentStatus::Running | SubagentStatus::Queued)
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();

    if running_subs.is_empty()
        && s.queue.steering.is_empty()
        && s.queue.follow_up.is_empty()
        && s.retry.is_none()
        && s.status_lines.is_empty()
    {
        return None;
    }

    let now = now_millis();
    let mut bar = v_flex()
        .w_full()
        .gap_1()
        .px_3()
        .py(px(6.0))
        .rounded_xl()
        .border_1()
        .border_color(theme_setup::card_border(cx))
        .bg(theme_setup::subtle_surface_bg(cx));

    if !running_subs.is_empty() {
        let mut row = h_flex().flex_wrap().items_center().gap(px(6.0)).child(
            h_flex()
                .items_center()
                .gap(px(4.0))
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(Icon::new(IconName::Bot).size(px(12.0)))
                .child("子任务运行中"),
        );
        for sub in running_subs {
            let elapsed = format_elapsed(sub.started_at, now);
            let st_c = store_entity.clone();
            let sid = sub.id.clone();
            row = row.child(
                Button::new(SharedString::from(format!("run-sub-{}", sub.id)))
                    .ghost()
                    .xsmall()
                    .label(format!("{} · {elapsed}", sub.description))
                    .on_click(move |_, _, cx| {
                        st_c.update(cx, |s, cx| s.select_session(Some(sid.clone()), cx));
                    }),
            );
        }
        bar = bar.child(row);
    }

    if !s.queue.steering.is_empty() || !s.queue.follow_up.is_empty() {
        let mut row = h_flex().flex_wrap().gap_1();
        for msg in &s.queue.steering {
            row = row.child(
                div()
                    .px_2()
                    .py(px(2.0))
                    .rounded_md()
                    .bg(cx.theme().secondary)
                    .text_xs()
                    .text_color(cx.theme().secondary_foreground)
                    .child(format!("引导: {msg}")),
            );
        }
        for msg in &s.queue.follow_up {
            row = row.child(
                div()
                    .px_2()
                    .py(px(2.0))
                    .rounded_md()
                    .bg(cx.theme().muted)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("排队: {msg}")),
            );
        }
        bar = bar.child(row);
    }

    if let Some(retry) = &s.retry {
        let detail = if retry.message.is_empty() {
            String::new()
        } else {
            format!(" · {}", retry.message)
        };
        bar = bar.child(
            div()
                .text_xs()
                .text_color(rgb(0xf59e0b))
                .child(format!("正在重试 ({}/{}){detail}", retry.attempt, retry.max)),
        );
    }

    if !s.status_lines.is_empty() {
        let mut entries: Vec<_> = s.status_lines.iter().collect();
        entries.sort_by(|(a, _), (b, _)| a.cmp(b));
        let line = entries
            .into_iter()
            .map(|(_, v)| v.as_str())
            .collect::<Vec<_>>()
            .join(" · ");
        bar = bar.child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .truncate()
                .child(line),
        );
    }

    Some(bar)
}
