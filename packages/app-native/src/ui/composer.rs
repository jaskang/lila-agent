//! Multi-line floating composer built on `gpui-component`'s `Textarea` with
//! zero-layout-shift `/` & `@` autocomplete, paste/drop/picker image attachments,
//! borderless ghost controls, and monochrome primary send/stop actions.

use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{
    Enter, Escape, IndentInline, InputEvent, InputState, MoveDown, MoveUp, Textarea, TextareaState,
};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, ClipboardEntry, ClipboardItem, Context, Entity, ExternalPaths,
    Focusable as _, ImageFormat, InteractiveElement as _, IntoElement, KeyBinding, KeyDownEvent,
    ParentElement as _, PathPromptOptions, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, deferred, div,
    prelude::FluentBuilder as _, px, relative,
};

use crate::format::{bytes_to_base64, format_elapsed, now_millis};
use crate::protocol::{
    ApprovalMode, ConnectionState, ModelInfo, SlashCommand, SubagentStatus, ThinkingLevel,
};
use crate::store::{AppStore, DraftImage, NEW_DRAFT, ToastTone};
use crate::ui::controls::{
    render_approval_picker, render_context_meter, render_model_picker, render_project_picker,
    render_thinking_picker,
};
use crate::ui::theme_setup;

const COMPOSER_KEY_CONTEXT: &str = "LilaComposer";
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
static NEXT_IMAGE_ID: AtomicU64 = AtomicU64::new(1);

fn next_image_id() -> String {
    format!("img-{}", NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed))
}

fn mime_for_image_ext(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        _ => None,
    }
}

fn mime_for_gpui_format(fmt: ImageFormat) -> Option<(&'static str, &'static str)> {
    match fmt {
        ImageFormat::Png => Some(("image/png", "png")),
        ImageFormat::Jpeg => Some(("image/jpeg", "jpg")),
        ImageFormat::Webp => Some(("image/webp", "webp")),
        ImageFormat::Gif => Some(("image/gif", "gif")),
        _ => None,
    }
}

fn load_draft_image_from_path(path: &Path) -> Result<Option<DraftImage>, String> {
    let Some(mime) = mime_for_image_ext(path) else {
        return Ok(None);
    };
    let meta = std::fs::metadata(path).map_err(|e| format!("读取图片失败：{e}"))?;
    if meta.len() > MAX_IMAGE_BYTES {
        return Err(format!("图片超过 20MB 限制：{}", path.display()));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("读取图片失败：{e}"))?;
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("image")
        .to_owned();
    Ok(Some(DraftImage {
        id: next_image_id(),
        name,
        mime_type: mime.to_owned(),
        data: bytes_to_base64(&bytes),
    }))
}

fn extract_clipboard_images(item: &ClipboardItem) -> Result<Vec<DraftImage>, String> {
    let mut images = Vec::new();
    for entry in item.entries() {
        match entry {
            ClipboardEntry::Image(img) => {
                if (img.bytes.len() as u64) > MAX_IMAGE_BYTES {
                    return Err("剪贴板图片超过 20MB 限制".to_owned());
                }
                if let Some((mime, ext)) = mime_for_gpui_format(img.format) {
                    images.push(DraftImage {
                        id: next_image_id(),
                        name: format!("pasted.{ext}"),
                        mime_type: mime.to_owned(),
                        data: bytes_to_base64(&img.bytes),
                    });
                }
            }
            ClipboardEntry::ExternalPaths(paths) => {
                for path in paths.paths() {
                    if let Some(img) = load_draft_image_from_path(path)? {
                        images.push(img);
                    }
                }
            }
            ClipboardEntry::String(_) => {}
        }
    }
    Ok(images)
}

pub(super) fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "alt-enter",
        Enter {
            secondary: true,
            shift: false,
        },
        Some(COMPOSER_KEY_CONTEXT),
    )]);
}

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

fn step_selection(current: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let cur = current.min(len - 1) as isize;
    (cur + delta).rem_euclid(len as isize) as usize
}

pub struct ComposerView {
    store: Entity<AppStore>,
    textarea: Entity<TextareaState>,
    model_search: Entity<InputState>,
    commands_cache: Vec<SlashCommand>,
    loaded_commands_project: Option<String>,
    file_matches: Vec<String>,
    active_trigger: Option<ActiveTrigger>,
    selected_index: usize,
    completion_scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl ComposerView {
    pub fn new(store: Entity<AppStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let initial_key = store.read(cx).active_draft_key();
        let initial_text = store.read(cx).draft_text(&initial_key);

        let textarea = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("描述任务，/ 调命令，@ 引文件…")
                .submit_on_enter(true)
                .auto_grow(2, 8)
                .default_value(initial_text)
        });

        let model_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("搜索模型或提供商…"));

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
                    if let Some(opt) = this.selected_option() {
                        this.apply_completion(opt, window, cx);
                        return;
                    }
                    let mode = if *secondary { Some("followUp") } else { None };
                    this.submit(mode, cx);
                }
                _ => {}
            },
        );

        let focus = textarea.read(cx).focus_handle(cx);
        let focus_in = cx.on_focus_in(&focus, window, |_, _, cx| cx.notify());
        let focus_out = cx.on_focus_out(&focus, window, |_, _, _, cx| cx.notify());

        Self {
            store,
            textarea,
            model_search,
            commands_cache: Vec::new(),
            loaded_commands_project: None,
            file_matches: Vec::new(),
            active_trigger: None,
            selected_index: 0,
            completion_scroll: ScrollHandle::new(),
            _subscriptions: vec![sub, focus_in, focus_out],
        }
    }

    pub fn focus_handle(&self, cx: &App) -> gpui_kit::FocusHandle {
        self.textarea.read(cx).focus_handle(cx)
    }

    pub fn sync_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.textarea.read(cx).value().as_ref() != text.as_str() {
            let end_pos = text.len();
            self.textarea.update(cx, |ta, cx| {
                ta.set_value(text, window, cx);
                ta.set_selected_range(end_pos..end_pos, cx);
            });
        }
        self.active_trigger = None;
        self.selected_index = 0;
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
        if self.active_trigger != trig {
            self.selected_index = 0;
        }
        self.active_trigger = trig.clone();
        if let Some(ActiveTrigger::At { query, .. }) = trig
            && let Some(pid) = self.current_project_id(cx)
        {
            let fut = self.store.read(cx).search_files_rpc(pid, query);
            cx.spawn(async move |this, cx| {
                let matches = fut.await;
                let _ = this.update(cx, |view, cx| {
                    view.file_matches = matches;
                    view.clamp_selection();
                    cx.notify();
                });
            })
            .detach();
        }
        self.clamp_selection();
        cx.notify();
    }

    fn clamp_selection(&mut self) {
        let len = self.current_options().len();
        if len == 0 {
            self.selected_index = 0;
        } else if self.selected_index >= len {
            self.selected_index = len - 1;
        }
    }

    fn selected_option(&self) -> Option<CompletionOption> {
        let opts = self.current_options();
        if opts.is_empty() {
            return None;
        }
        let idx = self.selected_index.min(opts.len() - 1);
        opts.into_iter().nth(idx)
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let len = self.current_options().len();
        if len == 0 {
            return false;
        }
        self.selected_index = step_selection(self.selected_index, delta, len);
        self.completion_scroll.scroll_to_item(self.selected_index);
        cx.notify();
        true
    }

    fn handle_completion_move_down(
        &mut self,
        _: &MoveDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.move_selection(1, cx) {
            cx.stop_propagation();
        }
    }

    fn handle_completion_move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        if self.move_selection(-1, cx) {
            cx.stop_propagation();
        }
    }

    fn handle_completion_tab(
        &mut self,
        _: &IndentInline,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(opt) = self.selected_option() {
            self.apply_completion(opt, window, cx);
            cx.stop_propagation();
        }
    }

    fn handle_completion_enter(
        &mut self,
        action: &Enter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if action.shift || action.secondary {
            return;
        }
        if let Some(opt) = self.selected_option() {
            self.apply_completion(opt, window, cx);
            cx.stop_propagation();
        }
    }

    fn handle_completion_escape(&mut self, _: &Escape, _: &mut Window, cx: &mut Context<Self>) {
        if !self.current_options().is_empty() {
            self.active_trigger = None;
            self.selected_index = 0;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let (busy, selected) = {
            let store = self.store.read(cx);
            (store.is_busy(), store.selected_id.clone())
        };
        if busy && let Some(sid) = selected {
            self.store.update(cx, |s, cx| s.abort(sid, cx));
            cx.stop_propagation();
        }
    }

    fn handle_completion_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let options = self.current_options();
        if options.is_empty() {
            return;
        }

        let mods = &event.keystroke.modifiers;
        let key = event.keystroke.key.as_str();
        let ctrl_only = mods.control && !mods.platform && !mods.alt && !mods.shift;

        match key {
            "n" if ctrl_only => {
                self.move_selection(1, cx);
                cx.stop_propagation();
                window.prevent_default();
            }
            "p" if ctrl_only => {
                self.move_selection(-1, cx);
                cx.stop_propagation();
                window.prevent_default();
            }
            _ => {}
        }
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
        self.selected_index = 0;
        let (start, end) = match trig {
            ActiveTrigger::Slash { start, end, .. } | ActiveTrigger::At { start, end, .. } => {
                (start, end)
            }
        };
        let current = self.textarea.read(cx).value().to_string();
        if start <= end && end <= current.len() {
            let cursor_pos = start + opt.insert_text.len();
            let next = format!(
                "{}{}{}",
                &current[..start],
                opt.insert_text,
                &current[end..]
            );
            let key = self.store.read(cx).active_draft_key();
            self.store
                .update(cx, |s, cx| s.set_draft_text(&key, next.clone(), cx));
            self.textarea.update(cx, |ta, cx| {
                ta.set_value(next, window, cx);
                ta.set_selected_range(cursor_pos..cursor_pos, cx);
                ta.focus(window, cx);
            });
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
            self.store.update(cx, |s, cx| s.send_draft(sid, mode, cx));
        } else {
            self.store.update(cx, |s, cx| s.create_and_prompt(cx));
        }
    }
}

impl Render for ComposerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_commands_loaded(cx);

        let focused = self.textarea.read(cx).focus_handle(cx).is_focused(window);
        let store = self.store.read(cx);
        let selected_id = store.selected_id.clone();
        let draft_key = selected_id.clone().unwrap_or_else(|| NEW_DRAFT.to_owned());
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
                store
                    .new_session
                    .thinking_level
                    .unwrap_or(ThinkingLevel::Off),
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
                st_ref
                    .map(|s| s.thinking_levels.clone())
                    .unwrap_or_default(),
                st_ref
                    .map(|s| s.approval_mode)
                    .unwrap_or(ApprovalMode::Auto),
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
            window,
            cx,
        );

        let project_el = is_new
            .then(|| render_project_picker("composer-project-picker", self.store.clone(), !connected, cx))
            .flatten();

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
        let active_idx = if options.is_empty() {
            0
        } else {
            self.selected_index.min(options.len() - 1)
        };
        let autocomplete = (!options.is_empty()).then(|| {
            let mut list = v_flex().p_1().gap(px(2.0));
            for (i, opt) in options.into_iter().enumerate() {
                let opt_click = opt.clone();
                let is_selected = i == active_idx;
                list = list.child(
                    h_flex()
                        .id(SharedString::from(format!("comp-opt-{i}")))
                        .w_full()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .px_2()
                        .py(px(5.0))
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .when(is_selected, |el| el.bg(cx.theme().accent))
                        .hover(|s| s.bg(cx.theme().accent))
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered && this.selected_index != i {
                                this.selected_index = i;
                                cx.notify();
                            }
                        }))
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
            deferred(
                div()
                    .absolute()
                    .bottom(relative(1.0))
                    .left_0()
                    .right_0()
                    .mb(px(6.0))
                    .child(
                        v_flex()
                            .w_full()
                            .rounded(px(theme_setup::RADIUS_MD))
                            .border_1()
                            .border_color(theme_setup::composer_border(cx))
                            .bg(cx.theme().popover)
                            .shadow(vec![theme_setup::overlay_shadow(cx)])
                            .overflow_hidden()
                            .child(
                                list.id("comp-opt-scroll")
                                    .track_scroll(&self.completion_scroll)
                                    .max_h(px(200.0))
                                    .overflow_y_scrollbar(),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .items_center()
                                    .justify_end()
                                    .gap(px(10.0))
                                    .px(px(10.0))
                                    .py(px(4.0))
                                    .border_t_1()
                                    .border_color(theme_setup::hairline(cx))
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_size(px(11.5))
                                    .text_color(cx.theme().muted_foreground.opacity(0.75))
                                    .child("↑↓ 选择")
                                    .child("↵ / Tab 确认")
                                    .child("Esc 关闭"),
                            ),
                    ),
            )
        });

        let st_abort = self.store.clone();
        let sid_abort = selected_id.clone();
        let st_paste = self.store.clone();
        let dk_paste = draft_key.clone();
        let st_drop = self.store.clone();
        let dk_drop = draft_key.clone();
        let st_pick = self.store.clone();
        let dk_pick = draft_key.clone();

        v_flex()
            .key_context(COMPOSER_KEY_CONTEXT)
            .w_full()
            .gap(px(6.0))
            .capture_action(cx.listener(Self::handle_completion_move_down))
            .capture_action(cx.listener(Self::handle_completion_move_up))
            .capture_action(cx.listener(Self::handle_completion_tab))
            .capture_action(cx.listener(Self::handle_completion_enter))
            .capture_action(cx.listener(Self::handle_completion_escape))
            .capture_key_down(cx.listener(Self::handle_completion_key_down))
            .when_some(status_bar, |el, bar| el.child(bar))
            .child(
                div()
                    .relative()
                    .w_full()
                    .when_some(autocomplete, |el, pop| el.child(pop))
                    .child(
                        v_flex()
                            .w_full()
                            .px_2()
                            .pt_2()
                            .pb_2()
                            .gap(px(6.0))
                            .rounded(px(theme_setup::RADIUS_MD))
                            .border_1()
                            .border_color(if focused {
                                cx.theme().ring
                            } else {
                                theme_setup::composer_border(cx)
                            })
                            .bg(theme_setup::composer_bg(cx))
                            .shadow(vec![theme_setup::composer_shadow(cx)])
                            .drag_over::<ExternalPaths>(|style, _, _, cx| {
                                style
                                    .border_color(cx.theme().drag_border)
                                    .bg(cx.theme().drop_target)
                            })
                            .on_drop(move |paths: &ExternalPaths, _, cx| {
                                let mut added = Vec::new();
                                for path in paths.paths() {
                                    match load_draft_image_from_path(path) {
                                        Ok(Some(img)) => added.push(img),
                                        Ok(None) => {}
                                        Err(err) => {
                                            st_drop.update(cx, |s, cx| {
                                                s.emit_toast(ToastTone::Warning, err, cx);
                                            });
                                        }
                                    }
                                }
                                if !added.is_empty() {
                                    st_drop.update(cx, |s, cx| {
                                        s.add_draft_images(&dk_drop, added, cx);
                                    });
                                }
                            })
                            .when(!images.is_empty(), |el| {
                                let mut row =
                                    h_flex().flex_wrap().px(px(6.0)).pb(px(2.0)).gap(px(6.0));
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
                                            .py(px(2.5))
                                            .rounded(px(theme_setup::RADIUS_SM))
                                            .border_1()
                                            .border_color(theme_setup::chip_border(cx))
                                            .bg(theme_setup::chip_bg(cx))
                                            .child(
                                                Icon::new(IconName::Frame)
                                                    .size(px(11.5))
                                                    .text_color(cx.theme().muted_foreground),
                                            )
                                            .child(
                                                div()
                                                    .max_w(px(140.0))
                                                    .truncate()
                                                    .font_family(
                                                        cx.theme().mono_font_family.clone(),
                                                    )
                                                    .text_size(px(12.0))
                                                    .text_color(cx.theme().foreground)
                                                    .child(img.name),
                                            )
                                            .child(
                                                Button::new(SharedString::from(format!(
                                                    "rm-img-{}",
                                                    img.id
                                                )))
                                                .ghost()
                                                .xsmall()
                                                .compact()
                                                .rounded(px(theme_setup::RADIUS_SM))
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
                                    .bordered(false)
                                    .on_paste(move |clip, _, cx| match extract_clipboard_images(clip)
                                    {
                                        Ok(imgs) if !imgs.is_empty() => {
                                            st_paste.update(cx, |s, cx| {
                                                s.add_draft_images(&dk_paste, imgs, cx);
                                            });
                                            true
                                        }
                                        Ok(_) => false,
                                        Err(err) => {
                                            st_paste.update(cx, |s, cx| {
                                                s.emit_toast(ToastTone::Warning, err, cx);
                                            });
                                            true
                                        }
                                    }),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .px(px(2.0))
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child(
                                        h_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .items_center()
                                            .flex_wrap()
                                            .gap(px(2.0))
                                            .when_some(project_el, |el, p| el.child(p))
                                            .child(model_el)
                                            .when_some(thinking_el, |el, t| el.child(t))
                                            .child(approval_el),
                                    )
                                    .child(
                                        h_flex()
                                            .flex_shrink_0()
                                            .items_center()
                                            .gap(px(4.0))
                                            .when_some(ctx_el, |el, c| el.child(c))
                                            .child(
                                                Button::new("composer-attach-img")
                                                    .ghost()
                                                    .xsmall()
                                                    .rounded(px(theme_setup::RADIUS_SM))
                                                    .disabled(!connected)
                                                    .icon(
                                                        Icon::new(IconName::Plus)
                                                            .size(px(13.0))
                                                            .text_color(
                                                                cx.theme().muted_foreground,
                                                            ),
                                                    )
                                                    .tooltip("添加图片（支持粘贴或拖入）")
                                                    .on_click(move |_, _, cx| {
                                                        let rx = cx.prompt_for_paths(
                                                            PathPromptOptions {
                                                                files: true,
                                                                directories: false,
                                                                multiple: true,
                                                                prompt: Some("选择图片".into()),
                                                            },
                                                        );
                                                        let st = st_pick.clone();
                                                        let dk = dk_pick.clone();
                                                        cx.spawn(async move |cx| {
                                                            let Ok(Ok(Some(paths))) = rx.await
                                                            else {
                                                                return;
                                                            };
                                                            let mut added = Vec::new();
                                                            let mut first_err = None;
                                                            for path in paths {
                                                                match load_draft_image_from_path(
                                                                    &path,
                                                                ) {
                                                                    Ok(Some(img)) => {
                                                                        added.push(img)
                                                                    }
                                                                    Ok(None) => {
                                                                        if first_err.is_none() {
                                                                            first_err = Some(
                                                                                "仅支持 PNG / JPG / WEBP / GIF 图片".to_owned(),
                                                                            );
                                                                        }
                                                                    }
                                                                    Err(err) => {
                                                                        first_err = Some(err);
                                                                    }
                                                                }
                                                            }
                                                            cx.update(|cx| {
                                                                if let Some(err) = first_err {
                                                                    st.update(cx, |s, cx| {
                                                                        s.emit_toast(
                                                                            ToastTone::Warning,
                                                                            err,
                                                                            cx,
                                                                        );
                                                                    });
                                                                }
                                                                if !added.is_empty() {
                                                                    st.update(cx, |s, cx| {
                                                                        s.add_draft_images(
                                                                            &dk, added, cx,
                                                                        );
                                                                    });
                                                                }
                                                            });
                                                        })
                                                        .detach();
                                                    }),
                                            )
                                            .when(busy, |el| {
                                                el.child(
                                                    Button::new("composer-stop")
                                                        .outline()
                                                        .small()
                                                        .rounded(px(theme_setup::RADIUS_SM))
                                                        .disabled(!connected)
                                                        .icon(
                                                            Icon::new(IconName::Square)
                                                                .size(px(10.5)),
                                                        )
                                                        .tooltip("停止运行 (Esc / ⌘.)")
                                                        .on_click(move |_, _, cx| {
                                                            if let Some(ref sid) = sid_abort {
                                                                st_abort.update(cx, |s, cx| {
                                                                    s.abort(sid.clone(), cx);
                                                                });
                                                            }
                                                        }),
                                                )
                                            })
                                            .when(busy && can_send, |el| {
                                                el.child(
                                                    Button::new("composer-follow-up")
                                                        .ghost()
                                                        .small()
                                                        .rounded(px(theme_setup::RADIUS_SM))
                                                        .label("排队")
                                                        .tooltip("当前运行结束后发送 (⌥↵)")
                                                        .on_click(cx.listener(
                                                            |this, _, _, cx| {
                                                                this.submit(Some("followUp"), cx);
                                                            },
                                                        )),
                                                )
                                            })
                                            .child(
                                                Button::new("composer-send")
                                                    .primary()
                                                    .small()
                                                    .rounded(px(theme_setup::RADIUS_SM))
                                                    .disabled(!can_send)
                                                    .icon(
                                                        Icon::new(IconName::ArrowUp)
                                                            .size(px(14.0)),
                                                    )
                                                    .tooltip(if busy {
                                                        "插话 (↵)"
                                                    } else {
                                                        "发送 (↵)"
                                                    })
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.submit(None, cx);
                                                    })),
                                            ),
                                    ),
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
        .gap(px(5.0))
        .px_3()
        .py(px(7.0))
        .rounded(px(10.0))
        .border_1()
        .border_color(theme_setup::card_border(cx))
        .bg(theme_setup::subtle_surface_bg(cx));

    if !running_subs.is_empty() {
        let running_color = theme_setup::running_fg(cx);
        let mut sub_col = v_flex().w_full().gap(px(4.0)).child(
            h_flex()
                .items_center()
                .gap(px(5.0))
                .text_size(px(12.5))
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .text_color(cx.theme().muted_foreground)
                .child(Icon::new(IconName::Bot).size(px(12.0)))
                .child(format!("{} 个子任务运行中", running_subs.len())),
        );
        for sub in running_subs {
            let elapsed = format_elapsed(sub.started_at, now);
            let st_c = store_entity.clone();
            let sid = sub.id.clone();
            let mut meta = Vec::new();
            if sub.tool_uses > 0 {
                meta.push(format!("{} 工具", sub.tool_uses));
            }
            meta.push(elapsed);
            let meta_str = meta.join(" · ");
            let act = sub.activity.filter(|a| !a.trim().is_empty());

            sub_col = sub_col.child(
                h_flex()
                    .id(SharedString::from(format!("run-sub-{}", sub.id)))
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(theme_setup::chip_border(cx))
                    .bg(theme_setup::chip_bg(cx))
                    .cursor_pointer()
                    .hover(|s| s.bg(cx.theme().accent))
                    .on_click(move |_, _, cx| {
                        st_c.update(cx, |s, cx| s.select_session(Some(sid.clone()), cx));
                    })
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(px(6.0))
                            .child(Spinner::new().xsmall().color(running_color))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .text_color(cx.theme().foreground)
                                    .child(sub.description),
                            )
                            .when_some(act, |el, a| {
                                el.child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_size(px(12.0))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(a),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap(px(5.0))
                            .child(
                                div()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_size(px(12.0))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(meta_str),
                            )
                            .child(
                                Icon::new(IconName::ArrowRight)
                                    .size(px(11.0))
                                    .text_color(cx.theme().muted_foreground.opacity(0.75)),
                            ),
                    ),
            );
        }
        bar = bar.child(sub_col);
    }

    if !s.queue.steering.is_empty() || !s.queue.follow_up.is_empty() {
        let mut row = h_flex().flex_wrap().gap(px(5.0));
        for msg in &s.queue.steering {
            row = row.child(
                div()
                    .px(px(7.0))
                    .py(px(2.0))
                    .rounded(px(5.0))
                    .border_1()
                    .border_color(theme_setup::chip_border(cx))
                    .bg(cx.theme().secondary)
                    .text_xs()
                    .text_color(cx.theme().secondary_foreground)
                    .child(format!("引导: {msg}")),
            );
        }
        for msg in &s.queue.follow_up {
            row = row.child(
                div()
                    .px(px(7.0))
                    .py(px(2.0))
                    .rounded(px(5.0))
                    .border_1()
                    .border_color(theme_setup::chip_border(cx))
                    .bg(theme_setup::chip_bg(cx))
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
                .font_family(cx.theme().mono_font_family.clone())
                .text_size(px(12.5))
                .text_color(theme_setup::warning_fg(cx))
                .child(format!(
                    "正在重试 ({}/{}){detail}",
                    retry.attempt, retry.max
                )),
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
                .font_family(cx.theme().mono_font_family.clone())
                .text_size(px(12.5))
                .text_color(cx.theme().muted_foreground)
                .truncate()
                .child(line),
        );
    }

    Some(bar)
}
