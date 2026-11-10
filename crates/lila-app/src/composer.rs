//! The box under a member's conversation: the words, the files that go with them
//! (picked, dropped, pasted, or pointed at with `@`), the commands and skills behind `/`,
//! and the model and effort the session runs on. One per member, so a draft stays
//! with the member it was written for.
//!
//! What it offers follows what the member's runtime can do: lilad says (`conversation.commands`,
//! `conversation.files`, and `config` in `conversation.read`), and what is not there is not drawn.

use crate::app::LilaApp;
use crate::style::{self, muted, muted2};
use crate::ui::s;
use gpui::component::attachment::{Attachment, AttachmentContent, AttachmentGroup, AttachmentMedia, AttachmentTitle};
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::input::{Enter, Escape, IndentInline, MoveDown, MoveUp, Textarea, TextareaState};
use gpui::component::menu::{DropdownMenu, PopupMenuItem};
use gpui::component::{Disableable as _, Icon, IconName, Sizable, StyledExt as _, h_flex, v_flex};
use gpui::prelude::*;
use gpui::*;
use serde_json::{Value, json};
use std::ops::Range;
use std::path::PathBuf;

/// Most rows the menu shows; typing narrows it.
const SHOWN: usize = 60;
/// Pasted pictures are kept this long (then `prune` removes them).
const KEEP_PASTED_MS: u64 = 14 * 24 * 3600 * 1000;
/// The box grows with its words from this many lines to this many; then it scrolls.
const MIN_ROWS: usize = 2;
const MAX_ROWS: usize = 10;

/// What the menu in front of the box is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `@`: a file of the member's folder.
    File,
    /// `/` (Codex also `$`): a command or a skill.
    Command,
}

/// The word under the cursor that opens a menu: where it is, and what was typed after its sign.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trigger {
    pub kind: Kind,
    pub range: Range<usize>,
    pub query: String,
}

/// `@…` anywhere, `/…` (and `$…` for `dollar`) as the first word: the word the cursor ends.
pub fn detect(text: &str, cursor: usize, dollar: bool) -> Option<Trigger> {
    let cursor = cursor.min(text.len());
    if !text.is_char_boundary(cursor) {
        return None;
    }
    let head = &text[..cursor];
    // `@"` opened for a path with spaces, not closed yet: the whole of it is the word.
    if let Some(at) = head.rfind("@\"")
        && !head[at + 2..].contains('"')
        && head[..at].chars().next_back().is_none_or(char::is_whitespace)
    {
        return Some(Trigger { kind: Kind::File, range: at..cursor, query: head[at + 2..].to_owned() });
    }
    let start = head.char_indices().rev().find(|(_, c)| c.is_whitespace()).map_or(0, |(i, c)| i + c.len_utf8());
    let word = &head[start..];
    let found = |kind, query: &str| Some(Trigger { kind, range: start..cursor, query: query.trim_start_matches('"').to_owned() });
    if let Some(q) = word.strip_prefix('@') {
        return found(Kind::File, q);
    }
    if start == 0 {
        if let Some(q) = word.strip_prefix('/') {
            return found(Kind::Command, q);
        }
        if let (true, Some(q)) = (dollar, word.strip_prefix('$')) {
            return found(Kind::Command, q);
        }
    }
    None
}

/// Does a row with these words answer what was typed (case aside)?
pub fn matches(query: &str, words: &[&str]) -> bool {
    let q = query.to_lowercase();
    q.is_empty() || words.iter().any(|w| w.to_lowercase().contains(&q))
}

/// What choosing a row does.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    /// Puts these words where the trigger was (a file's `@path`, a command's name); `send`: and sends.
    Put { text: String, send: bool },
    /// Lila's own: starts the member over, stops it, shows its terminal.
    NewSession,
    Stop,
    Terminal,
    /// A request of the runtime's protocol instead of words (`conversation.run`).
    Run(String),
}

#[derive(Clone)]
pub struct Row {
    pub icon: Icon,
    pub name: String,
    pub detail: String,
    pub pick: Pick,
}

/// The menu open in front of the box, if the words under the cursor ask for one.
pub struct Menu {
    pub trigger: Trigger,
    pub selected: usize,
    pub scroll: ScrollHandle,
    /// Opened by the button, not by a typed sign: it puts its words at the start.
    pub forced: bool,
}

pub struct Composer {
    pub input: Entity<TextareaState>,
    /// The files that go with the next message.
    pub files: Vec<PathBuf>,
    pub menu: Option<Menu>,
    /// A word the person closed the menu on (Esc): not opened again until another word.
    dismissed: Option<(Kind, usize)>,
    /// Walking back through what was sent (↑): how many messages back, and the words recalled.
    back: Option<(usize, String)>,
    /// A message is on its way.
    pub busy: bool,
    /// Why the last message did not go.
    pub error: Option<String>,
    /// The words as they were at the last change: the cursor blinking is a change too.
    seen: String,
    _subs: Vec<Subscription>,
}

/// What the `/` menu of a member offers besides Lila's own actions, as lilad listed it.
pub type Commands = Vec<Value>;

impl LilaApp {
    /// The key a member's box is kept under.
    pub fn composer_key(&self, member: &str) -> String {
        format!("{}/{member}", self.selected.as_deref().unwrap_or_default())
    }

    /// The member's box, made when first looked at.
    pub fn composer(&mut self, member: &str, window: &mut Window, cx: &mut Context<Self>) -> &mut Composer {
        let key = self.composer_key(member);
        if !self.composers.contains_key(&key) {
            let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(MIN_ROWS, MAX_ROWS).placeholder("消息…").submit_on_enter(true));
            let k1 = key.clone();
            let subs = vec![
                // Any change of the words or the cursor: the menu follows the word under it.
                cx.observe_in(&input, window, move |this, _, window, cx| this.composer_changed(&k1, window, cx)),
            ];
            self.composers.insert(key.clone(), Composer { input, files: Vec::new(), menu: None, dismissed: None, back: None, busy: false, error: None, seen: String::new(), _subs: subs });
        }
        self.composers.get_mut(&key).expect("just made")
    }

    /// `--features shots`: the shown member's box has the cursor.
    #[cfg(feature = "shots")]
    pub fn conv_focus(&self, cx: &App) -> Option<FocusHandle> {
        let member = self.member.as_deref()?;
        self.composers.get(&self.composer_key(member)).map(|c| c.input.read(cx).focus_handle(cx))
    }

    /// Does the runtime take `$name` for a skill (Codex)?
    fn dollar(&self, member: &str) -> bool {
        self.members().iter().any(|m| m["id"] == member && m["runtime"] == "codex")
    }

    /// The words or the cursor moved: opens, moves or closes the menu.
    fn composer_changed(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(member) = key.split_once('/').map(|(_, m)| m.to_owned()) else { return };
        let dollar = self.dollar(&member);
        let Some(c) = self.composers.get_mut(key) else { return };
        let (text, cursor) = c.input.read_with(cx, |i, _| (i.value().to_string(), i.cursor()));
        // Typing after a recalled message makes it the person's own words again.
        if c.back.as_ref().is_some_and(|(_, recalled)| *recalled != text) {
            c.back = None;
        }
        // Typing, not the cursor blinking, takes the complaint away.
        if c.seen != text {
            c.error = None;
            c.seen = text.clone();
        }
        let raw = detect(&text, cursor, dollar);
        if raw.is_none() {
            c.dismissed = None;
        }
        let found = raw.filter(|t| c.dismissed != Some((t.kind, t.range.start)));
        let forced = c.menu.as_ref().is_some_and(|m| m.forced);
        match (found, c.menu.take()) {
            (Some(t), Some(mut m)) if m.trigger.kind == t.kind && m.trigger.range.start == t.range.start => {
                if m.trigger.query != t.query {
                    m.selected = 0;
                }
                m.trigger = t;
                m.forced = false;
                c.menu = Some(m);
            }
            (Some(t), _) => {
                // A new word: what it lists is read again (files change; commands come and go with plugins).
                let kind = t.kind;
                c.menu = Some(Menu { trigger: t, selected: 0, scroll: ScrollHandle::new(), forced: false });
                self.fetch_menu(&member, kind, window, cx);
            }
            (None, old) => c.menu = old.filter(|_| forced),
        }
        cx.notify();
    }

    /// Reads what the menu lists from lilad: the files for `@`, the commands for `/`.
    fn fetch_menu(&mut self, member: &str, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.composer_key(member);
        let method = match kind {
            Kind::File => "conversation.files",
            Kind::Command => "conversation.commands",
        };
        self.request_result(method, json!({"member": member}), window, cx, move |this, r, _, cx| {
            if let Ok(v) = r {
                match kind {
                    Kind::File => {
                        let files = v["files"].as_array().map(|a| a.iter().filter_map(|f| f.as_str().map(String::from)).collect()).unwrap_or_default();
                        this.menu_files.insert(key, files);
                    }
                    Kind::Command => {
                        this.menu_commands.insert(key, v["commands"].as_array().cloned().unwrap_or_default());
                    }
                }
            }
            cx.notify();
        });
    }

    /// The rows the menu shows for what was typed.
    pub fn menu_rows(&self, member: &str, working: bool, tui: bool, menu: &Menu) -> Vec<Row> {
        let key = self.composer_key(member);
        let q = menu.trigger.query.as_str();
        match menu.trigger.kind {
            Kind::File => {
                let files = self.menu_files.get(&key).map(Vec::as_slice).unwrap_or_default();
                // Those whose name has it before those that only have it in the folders.
                let (named, deeper): (Vec<&String>, Vec<&String>) = files.iter().filter(|f| matches(q, &[f])).partition(|f| matches(q, &[f.rsplit('/').next().unwrap_or(f)]));
                named
                    .into_iter()
                    .chain(deeper)
                    .take(SHOWN)
                    .map(|f| {
                        let (dir, name) = f.rsplit_once('/').map_or(("", f.as_str()), |(d, n)| (d, n));
                        let text = if f.contains(char::is_whitespace) { format!("@\"{f}\" ") } else { format!("@{f} ") };
                        let icon = if lila_agent::image_mime(f).is_some() { style::icon("lila/image.svg") } else { Icon::new(IconName::FileText) };
                        Row { icon, name: name.to_owned(), detail: dir.to_owned(), pick: Pick::Put { text, send: false } }
                    })
                    .collect()
            }
            Kind::Command => {
                let sigil = if self.dollar(member) { '$' } else { '/' };
                let mut rows: Vec<Row> = Vec::new();
                let own = [
                    (Icon::new(IconName::RotateCw), "新会话", "", Pick::NewSession, true),
                    (Icon::new(IconName::Square), "停止", "", Pick::Stop, working),
                    (Icon::new(IconName::SquareTerminal), "终端", "", Pick::Terminal, tui),
                ];
                rows.extend(own.into_iter().filter(|o| o.4 && matches(q, &[o.1])).map(|o| Row { icon: o.0, name: o.1.to_owned(), detail: o.2.to_owned(), pick: o.3 }));
                for c in self.menu_commands.get(&key).into_iter().flatten() {
                    let (name, detail, kind) = (s(c, "name"), s(c, "description"), s(c, "kind"));
                    if !matches(q, &[&name, &detail]) {
                        continue;
                    }
                    let icon = match kind.as_str() {
                        "skill" => style::icon("lila/sparkles.svg"),
                        "command" => style::icon("lila/square-slash.svg"),
                        _ => style::icon("lila/command.svg"),
                    };
                    let hint = c["hint"].as_str().unwrap_or_default();
                    let detail = if hint.is_empty() { detail } else { format!("{hint}  {detail}") };
                    let pick = match (c["action"].as_str(), c["insert"].as_str()) {
                        (Some(a), _) => Pick::Run(a.to_owned()),
                        // Words that take arguments (a hint, or any skill) wait for them in the box; the rest go at once.
                        (None, Some(t)) => Pick::Put { text: t.replace(['/', '$'], &sigil.to_string()), send: hint.is_empty() && kind != "skill" },
                        _ => continue,
                    };
                    rows.push(Row { icon, name: format!("{sigil}{name}"), detail, pick });
                }
                rows.truncate(SHOWN);
                rows
            }
        }
    }

    /// Moves the menu's highlight, keeping it in view.
    fn menu_move(&mut self, key: &str, rows: usize, down: bool) {
        let Some(m) = self.composers.get_mut(key).and_then(|c| c.menu.as_mut()) else { return };
        if rows == 0 {
            return;
        }
        m.selected = if down { (m.selected + 1) % rows } else { (m.selected + rows - 1) % rows };
        m.scroll.scroll_to_item(m.selected);
    }

    /// A key meant for the menu, when one is open: ↑ ↓ move, Enter and Tab choose, Esc closes.
    /// Without a menu, ↑ on an empty box brings back the last thing sent, ↓ goes forward again.
    /// Returns whether the key was used.
    fn composer_key_press(&mut self, member: &str, key: MenuKey, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let ck = self.composer_key(member);
        let (working, tui) = self.members().iter().find(|m| m["id"] == member).map(|m| (m["status"] == "working", m["caps"]["tui"] == true)).unwrap_or_default();
        let Some(c) = self.composers.get(&ck) else { return false };
        let Some(menu) = c.menu.as_ref() else {
            return self.history_key(member, key, window, cx);
        };
        let rows = self.menu_rows(member, working, tui, menu);
        match key {
            MenuKey::Up | MenuKey::Down => {
                self.menu_move(&ck, rows.len(), key == MenuKey::Down);
                cx.notify();
            }
            MenuKey::Enter | MenuKey::Tab => {
                let at = self.composers[&ck].menu.as_ref().map_or(0, |m| m.selected);
                match rows.get(at) {
                    Some(row) => self.pick(member, row.clone(), window, cx),
                    // Nothing matches: Enter is a plain Enter (sends), Tab does nothing.
                    None => return key == MenuKey::Tab,
                }
            }
            MenuKey::Esc => {
                if let Some(c) = self.composers.get_mut(&ck)
                    && let Some(m) = c.menu.take().filter(|m| !m.forced)
                {
                    c.dismissed = Some((m.trigger.kind, m.trigger.range.start));
                }
                cx.notify();
            }
        }
        true
    }

    /// Does what a chosen row says.
    fn pick(&mut self, member: &str, row: Row, window: &mut Window, cx: &mut Context<Self>) {
        let ck = self.composer_key(member);
        let Some(c) = self.composers.get_mut(&ck) else { return };
        let Some(menu) = c.menu.take() else { return };
        let input = c.input.clone();
        let range = if menu.forced { 0..0 } else { menu.trigger.range.clone() };
        let put = |text: &str, window: &mut Window, cx: &mut Context<Self>| {
            input.update(cx, |st, cx| {
                st.set_selected_range(range.clone(), cx);
                st.replace(text.to_owned(), window, cx);
            });
        };
        match row.pick {
            Pick::Put { text, send } => {
                put(&text, window, cx);
                if send {
                    self.send_to_member(member.to_owned(), window, cx);
                }
            }
            own => {
                put("", window, cx);
                let to = member.to_owned();
                match own {
                    Pick::NewSession => self.request("member.restart", json!({"member": to}), cx, |this, _, _| this.refresh_now()),
                    Pick::Stop => self.request("conversation.interrupt", json!({"member": to}), cx, |this, _, _| this.refresh_now()),
                    Pick::Terminal => self.set_view(member, true, cx),
                    Pick::Run(action) => self.request("conversation.run", json!({"member": to, "action": action}), cx, |this, _, _| this.refresh_now()),
                    Pick::Put { .. } => {}
                }
            }
        }
        cx.notify();
    }

    /// ↑ on an empty box brings back what the person sent last; again, the one before. ↓ goes forward, and past the last, empty.
    fn history_key(&mut self, member: &str, key: MenuKey, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let sent = self.sent_by_person();
        let ck = self.composer_key(member);
        let Some(c) = self.composers.get_mut(&ck) else { return false };
        let empty = c.input.read(cx).value().is_empty();
        let back = c.back.as_ref().map(|(n, _)| *n);
        let to = match (key, back) {
            (MenuKey::Up, None) if empty && !sent.is_empty() => 0,
            (MenuKey::Up, Some(n)) if n + 1 < sent.len() => n + 1,
            (MenuKey::Up, Some(_)) => return true,
            (MenuKey::Down, Some(0)) => {
                c.back = None;
                c.input.update(cx, |st, cx| st.set_value("", window, cx));
                return true;
            }
            (MenuKey::Down, Some(n)) => n - 1,
            _ => return false,
        };
        let text = sent[to].clone();
        c.back = Some((to, text.clone()));
        // A recalled `/compact` is a message, not a word being typed: no menu over it.
        c.menu = None;
        c.dismissed = detect(&text, text.len(), false).map(|t| (t.kind, t.range.start));
        c.input.update(cx, |st, cx| {
            st.set_value(text.clone(), window, cx);
            st.set_selected_range(text.len()..text.len(), cx);
        });
        true
    }

    /// What the person said in the shown conversation, the latest first.
    fn sent_by_person(&self) -> Vec<String> {
        let Some(c) = &self.conv else { return Vec::new() };
        c.items.iter().rev().filter(|i| i["kind"] == "user_message" && i["from"] == "person").map(|i| s(i, "text")).filter(|t| !t.is_empty()).collect()
    }

    /// Sends the box's words and files to the member. They are cleared once it went; if it did not,
    /// they stay and the box says why.
    pub fn send_to_member(&mut self, member: String, window: &mut Window, cx: &mut Context<Self>) {
        let ck = self.composer_key(&member);
        let Some(c) = self.composers.get_mut(&ck) else { return };
        let text = c.input.read(cx).value().trim().to_owned();
        let files: Vec<String> = c.files.iter().map(|f| f.to_string_lossy().to_string()).collect();
        if (text.is_empty() && files.is_empty()) || c.busy {
            return;
        }
        c.busy = true;
        c.error = None;
        let sent = files.len();
        self.request_result("conversation.send", json!({"member": member, "text": text, "attachments": files}), window, cx, move |this, r, window, cx| {
            let Some(c) = this.composers.get_mut(&ck) else { return };
            c.busy = false;
            match r {
                Ok(_) => {
                    // What was typed meanwhile stays.
                    let same = c.input.read(cx).value().trim() == text;
                    if same {
                        c.input.update(cx, |st, cx| st.set_value("", window, cx));
                    }
                    c.files.drain(..sent.min(c.files.len()));
                    c.back = None;
                    this.refresh_now();
                }
                Err(e) => c.error = Some(format!("{e:#}")),
            }
            cx.notify();
        });
        cx.notify();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKey {
    Up,
    Down,
    Enter,
    Tab,
    Esc,
}

impl LilaApp {
    /// Adds files to the member's next message (folders are not files).
    pub fn add_files(&mut self, member: &str, paths: impl IntoIterator<Item = PathBuf>, cx: &mut Context<Self>) {
        let key = self.composer_key(member);
        let Some(c) = self.composers.get_mut(&key) else { return };
        for p in paths {
            if !p.is_file() {
                c.error = Some("只能附加文件".into());
            } else if !c.files.contains(&p) {
                c.files.push(p);
            }
        }
        cx.notify();
    }

    fn remove_file(&mut self, member: &str, at: usize, cx: &mut Context<Self>) {
        let key = self.composer_key(member);
        let pasted = self.paths().state.join("attachments");
        if let Some(c) = self.composers.get_mut(&key)
            && at < c.files.len()
        {
            let gone = c.files.remove(at);
            // A picture pasted into the box is Lila's own copy: with nobody to send it to, it goes.
            if gone.starts_with(&pasted) {
                let _ = std::fs::remove_file(gone);
            }
        }
        cx.notify();
    }

    /// The system's open dialog; the files chosen go with the next message.
    fn choose_files(&mut self, member: String, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await {
                let _ = this.update(cx, |this, cx| this.add_files(&member, paths, cx));
            }
        })
        .detach();
    }

    /// A paste of pictures or copied files goes to the next message as files; words are left to the box.
    /// A picture is kept as a file of Lila's own, under the state folder, by member.
    fn paste_files(&mut self, member: &str, item: &ClipboardItem, cx: &mut Context<Self>) -> bool {
        if item.entries.iter().any(|e| matches!(e, ClipboardEntry::String(_))) {
            return false;
        }
        let dir = self.paths().state.join("attachments").join(self.composer_key(member));
        let mut files: Vec<PathBuf> = Vec::new();
        for e in &item.entries {
            match e {
                ClipboardEntry::ExternalPaths(p) => files.extend(p.paths().iter().cloned()),
                ClipboardEntry::Image(img) => {
                    let ext = match img.format {
                        ImageFormat::Png => "png",
                        ImageFormat::Jpeg => "jpg",
                        ImageFormat::Webp => "webp",
                        ImageFormat::Gif => "gif",
                        // The CLIs take these four.
                        _ => {
                            if let Some(c) = self.composers.get_mut(&self.composer_key(member)) {
                                c.error = Some("这种图片格式不能附加（用 PNG、JPEG、WebP 或 GIF）".into());
                            }
                            continue;
                        }
                    };
                    let name = format!("paste-{}.{ext}", crate::data::now_ms());
                    if std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join(&name), &img.bytes)).is_ok() {
                        files.push(dir.join(name));
                    }
                }
                ClipboardEntry::String(_) => {}
            }
        }
        if files.is_empty() {
            return false;
        }
        self.add_files(member, files, cx);
        true
    }

    /// Runs the model or effort the person chose from the box's menu, for this session.
    fn set_session_model(&mut self, member: &str, model: Option<String>, effort: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.request_result("conversation.model", json!({"member": member, "model": model, "effort": effort}), window, cx, |this, r, _, cx| {
            match r {
                Ok(_) => this.refresh_now(),
                Err(e) => this.notice = Some(format!("{e:#}")),
            }
            cx.notify();
        });
    }

    /// The model and effort shown in the box's tool row: a menu where the runtime lets the session change them,
    /// else the plain words. A yellow dot when they are not the role's.
    fn model_control(&mut self, member: &str, cfg: &Value, cx: &mut Context<Self>) -> Option<AnyElement> {
        let model = cfg["model"].as_str().filter(|m| !m.is_empty())?.to_owned();
        let effort = cfg["effort"].as_str().unwrap_or_default().to_owned();
        let short = |m: &str| m.rsplit('/').next().unwrap_or(m).to_owned();
        let label = if effort.is_empty() { short(&model) } else { format!("{} · {effort}", short(&model)) };
        let differs = differs(cfg);
        let dot = || div().size(px(6.)).flex_none().rounded_full().bg(style::yellow());
        if cfg["switch"] != true {
            return Some(h_flex().h(px(28.)).px_2().gap(px(6.)).items_center().text_size(px(12.)).text_color(muted2()).when(differs, |d| d.child(dot())).child(label).into_any_element());
        }
        // The choices: omp lists its own; Codex takes the models and efforts the settings offer.
        let pairs = |list: &Value| -> Vec<(String, String)> { list.as_array().into_iter().flatten().map(|c| (s(c, "id"), s(c, "name"))).collect() };
        let (mut models, mut efforts) = (pairs(&cfg["models"]), pairs(&cfg["efforts"]));
        if models.is_empty() {
            models = crate::settings::CODEX_MODELS.iter().map(|m| (m.to_string(), m.to_string())).collect();
            if !models.iter().any(|(id, _)| *id == model) {
                models.insert(0, (model.clone(), model.clone()));
            }
        }
        if efforts.is_empty() {
            efforts = crate::settings::EFFORTS.iter().map(|e| (e.to_string(), e.to_string())).collect();
        }
        let weak = cx.entity().downgrade();
        let to = member.to_owned();
        let button = Button::new("conv-model").ghost().xsmall().label(label).dropdown_caret(true).dropdown_menu_with_anchor(Anchor::BottomLeft, move |mut menu, _, _| {
            for (id, name) in &models {
                let (weak, to, id2) = (weak.clone(), to.clone(), id.clone());
                menu = menu.item(PopupMenuItem::new(name.clone()).checked(*id == model).on_click(move |_, window, cx| {
                    let _ = weak.update(cx, |this, cx| this.set_session_model(&to, Some(id2.clone()), None, window, cx));
                }));
            }
            menu = menu.separator();
            for (id, name) in &efforts {
                let (weak, to, id2) = (weak.clone(), to.clone(), id.clone());
                menu = menu.item(PopupMenuItem::new(name.clone()).checked(*id == effort).on_click(move |_, window, cx| {
                    let _ = weak.update(cx, |this, cx| this.set_session_model(&to, None, Some(id2.clone()), window, cx));
                }));
            }
            menu
        });
        Some(h_flex().gap(px(2.)).items_center().when(differs, |d| d.child(dot())).child(button).into_any_element())
    }

    /// The box under a member's conversation.
    pub fn composer_view(&mut self, m: &Value, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let member = s(m, "id");
        let working = s(m, "status") == "working";
        let tui = m["caps"]["tui"] == true;
        self.composer(&member, window, cx);
        let cfg = self.conv.as_ref().filter(|c| Some(&c.project) == self.selected.as_ref() && c.member == member).map(|c| c.config.clone()).unwrap_or(Value::Null);
        let ck = self.composer_key(&member);
        let (input, files, error, busy, has_menu) = {
            let c = &self.composers[&ck];
            (c.input.clone(), c.files.clone(), c.error.clone(), c.busy, c.menu.is_some())
        };
        let can_send = !busy && (!input.read(cx).value().trim().is_empty() || !files.is_empty());
        let menu = if has_menu {
            let menu = self.composers[&ck].menu.as_ref().expect("has one");
            let rows = self.menu_rows(&member, working, tui, menu);
            let (selected, scroll) = (menu.selected.min(rows.len().saturating_sub(1)), menu.scroll.clone());
            (!rows.is_empty()).then(|| self.menu_view(&member, rows, selected, scroll, cx))
        } else {
            None
        };
        let weak = cx.entity().downgrade();

        let cards = (!files.is_empty()).then(|| {
            AttachmentGroup::new("composer-files").gap_2().px_3().pt_3().children(files.iter().enumerate().map(|(ix, f)| {
                let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let image = lila_agent::image_mime(&f.to_string_lossy()).is_some();
                let to = member.clone();
                let app = cx.entity();
                let media = if image { AttachmentMedia::new().src(f.clone()) } else { AttachmentMedia::new().child(Icon::new(IconName::FileText).text_color(muted())) };
                Attachment::new()
                    .id(SharedString::from(format!("file-{ix}")))
                    .xsmall()
                    .tooltip(f.to_string_lossy().to_string())
                    .media(media)
                    .content(AttachmentContent::new().title(AttachmentTitle::new(name)))
                    .on_remove(move |_, _, cx| app.update(cx, |this, cx| this.remove_file(&to, ix, cx)))
            }))
        });

        let paste_to = member.clone();
        let editor = div()
            .id("composer-input")
            .px_1()
            .pt_1()
            // The menu's keys come before the box's own: ↑ ↓ move, Enter and Tab choose, Esc closes.
            .capture_action(cx.listener({
                let to = member.clone();
                move |this, _: &MoveUp, window, cx| {
                    if this.composer_key_press(&to, MenuKey::Up, window, cx) {
                        cx.stop_propagation();
                    }
                }
            }))
            .capture_action(cx.listener({
                let to = member.clone();
                move |this, _: &MoveDown, window, cx| {
                    if this.composer_key_press(&to, MenuKey::Down, window, cx) {
                        cx.stop_propagation();
                    }
                }
            }))
            // Enter chooses what the menu shows, else sends (Shift+Enter breaks the line). Taken here, so the box
            // never adds a line for it; while an input method composes, its own keys do not arrive as Enter.
            .capture_action(cx.listener({
                let to = member.clone();
                move |this, a: &Enter, window, cx| {
                    if a.shift {
                        return;
                    }
                    let menu = this.composers.get(&this.composer_key(&to)).is_some_and(|c| c.menu.is_some());
                    if !(menu && this.composer_key_press(&to, MenuKey::Enter, window, cx)) {
                        this.send_to_member(to.clone(), window, cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener({
                let to = member.clone();
                move |this, _: &IndentInline, window, cx| {
                    if this.composers.get(&this.composer_key(&to)).is_some_and(|c| c.menu.is_some()) && this.composer_key_press(&to, MenuKey::Tab, window, cx) {
                        cx.stop_propagation();
                    }
                }
            }))
            .capture_action(cx.listener({
                let to = member.clone();
                move |this, _: &Escape, window, cx| {
                    if this.composers.get(&this.composer_key(&to)).is_some_and(|c| c.menu.is_some()) && this.composer_key_press(&to, MenuKey::Esc, window, cx) {
                        cx.stop_propagation();
                    }
                }
            }))
            .child(
                Textarea::new(&input)
                    .appearance(false)
                    .bordered(false)
                    .text_size(px(13.))
                    .on_paste(move |item, _, cx| weak.update(cx, |this, cx| this.paste_files(&paste_to, item, cx)).unwrap_or(false)),
            );

        let tools = h_flex()
            .px_2()
            .pb_2()
            .gap_1()
            .items_center()
            .child({
                let to = member.clone();
                Button::new("conv-attach").ghost().size(px(style::CTL_SM)).icon(style::icon("lila/paperclip.svg").text_color(muted())).tooltip("添加附件").on_click(cx.listener(move |this, _, window, cx| this.choose_files(to.clone(), window, cx)))
            })
            .child({
                let to = member.clone();
                Button::new("conv-commands").ghost().size(px(style::CTL_SM)).icon(style::icon("lila/square-slash.svg").text_color(muted())).tooltip("命令 (/)").on_click(cx.listener(move |this, _, window, cx| this.toggle_menu(&to, window, cx)))
            })
            .children(self.model_control(&member, &cfg, cx))
            .child(div().flex_1())
            .when(working, |d| {
                let to = member.clone();
                d.child(
                    Button::new("conv-stop")
                        .secondary()
                        .size(px(style::CTL_SM))
                        .icon(IconName::Square)
                        .tooltip("停止")
                        .on_click(cx.listener(move |this, _, _, cx| this.request("conversation.interrupt", json!({"member": to}), cx, |this, _, _| this.refresh_now()))),
                )
            })
            .child({
                let to = member.clone();
                Button::new("conv-send").primary().size(px(style::CTL_SM)).icon(IconName::ArrowUp).disabled(!can_send).tooltip("发送 (↩)").on_click(cx.listener(move |this, _, window, cx| this.send_to_member(to.clone(), window, cx)))
            });

        h_flex()
            .flex_none()
            .justify_center()
            .pb_3()
            .pt_2()
            .border_t_1()
            .border_color(style::divider())
            .child(div().w_full().max_w(px(760.)).px_5().child(
                v_flex()
                    .relative()
                    .w_full()
                    .rounded(px(style::R_BLOCK))
                    .bg(style::well())
                    .border_1()
                    .border_color(style::hairline())
                    .children(menu)
                    .children(cards)
                    .child(editor)
                    .when_some(error, |d, e| d.child(div().px_3().pb_1().text_size(px(12.)).text_color(style::tint(style::red(), 0.2)).truncate().child(e)))
                    .child(tools),
            ))
            .into_any_element()
    }

    /// The command button: the menu of commands, opened over the box, whatever is in it.
    fn toggle_menu(&mut self, member: &str, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.composer_key(member);
        let Some(c) = self.composers.get_mut(&key) else { return };
        if c.menu.take().is_none() {
            c.menu = Some(Menu { trigger: Trigger { kind: Kind::Command, range: 0..0, query: String::new() }, selected: 0, scroll: ScrollHandle::new(), forced: true });
            let focus = c.input.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
            self.fetch_menu(member, Kind::Command, window, cx);
        }
        cx.notify();
    }

    /// The rows over the box.
    fn menu_view(&self, member: &str, rows: Vec<Row>, selected: usize, scroll: ScrollHandle, cx: &mut Context<Self>) -> AnyElement {
        div()
            .absolute()
            .bottom(relative(1.))
            .left_0()
            .right_0()
            .pb(px(6.))
            .child(
                v_flex()
                    .id("composer-menu")
                    .max_h(px(260.))
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .p_1()
                    .rounded(px(style::R_BLOCK))
                    .bg(style::popover())
                    .border_1()
                    .border_color(style::hairline())
                    .shadow_lg()
                    .children(rows.into_iter().enumerate().map(|(ix, row)| {
                        let (to, picked) = (member.to_owned(), row.clone());
                        h_flex()
                            .id(SharedString::from(format!("menu-row-{ix}")))
                            .h(px(28.))
                            .flex_none()
                            .px_2()
                            .gap_2()
                            .items_center()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .when(ix == selected, |d| d.bg(style::accent()))
                            .hover(|d| d.bg(style::hover()))
                            .child(row.icon.size(px(13.)).text_color(muted()))
                            .child(div().flex_none().max_w(relative(0.5)).truncate().text_size(px(13.)).font_medium().child(row.name))
                            .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(muted2()).child(row.detail))
                            .on_click(cx.listener(move |this, _, window, cx| this.pick(&to, picked.clone(), window, cx)))
                    })),
            )
            .into_any_element()
    }

    /// `--features shots`: fills the shown member's box (`fill:<words>`), adds a file (`attach:<path>`), opens the command menu (`menu`) or shows another member (`show:<id>`).
    #[cfg(feature = "shots")]
    pub fn dev_composer(&mut self, step: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(to) = step.strip_prefix("show:") {
            self.show_member(to.to_owned(), cx);
            return;
        }
        let Some(member) = self.member.clone() else { return };
        self.composer(&member, window, cx);
        let key = self.composer_key(&member);
        if let Some(words) = step.strip_prefix("fill:") {
            let input = self.composers[&key].input.clone();
            let words = words.replace('_', " ").replace("\\n", "\n");
            input.update(cx, |st, cx| {
                st.set_value(words.clone(), window, cx);
                st.set_selected_range(words.len()..words.len(), cx);
            });
        } else if let Some(path) = step.strip_prefix("attach:") {
            self.add_files(&member, [PathBuf::from(path)], cx);
        } else if step == "menu" {
            self.toggle_menu(&member, window, cx);
        }
    }
}

/// Is the model or effort not the role's? (Claude Code's model is read from its replies, as an id: only an id can differ for certain.)
pub fn differs(cfg: &Value) -> bool {
    let (m, dm) = (cfg["model"].as_str().unwrap_or_default(), cfg["default_model"].as_str().unwrap_or_default());
    let (e, de) = (cfg["effort"].as_str().unwrap_or_default(), cfg["default_effort"].as_str().unwrap_or_default());
    let model = !dm.is_empty() && m != dm && (cfg["switch"] == true || dm.starts_with("claude-"));
    let effort = !de.is_empty() && !e.is_empty() && e != de;
    model || effort
}

/// Pictures pasted into a box are kept for a while for the conversation to point at; older ones go.
pub fn prune(paths: &lila::paths::Paths) {
    let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else { return };
    for e in walkdir::WalkDir::new(paths.state.join("attachments")).into_iter().flatten().filter(|e| e.file_type().is_file()) {
        let age = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
        if age.is_some_and(|a| now.saturating_sub(a).as_millis() as u64 > KEEP_PASTED_MS) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Kind, detect, differs, matches};
    use serde_json::json;

    fn word(text: &str, dollar: bool) -> Option<(Kind, std::ops::Range<usize>, String)> {
        detect(text, text.len(), dollar).map(|t| (t.kind, t.range, t.query))
    }

    #[test]
    fn at_opens_the_files_anywhere_and_slash_the_commands_only_first() {
        assert_eq!(word("@", false), Some((Kind::File, 0..1, String::new())));
        assert_eq!(word("look at @src/ma", false), Some((Kind::File, 8..15, "src/ma".into())));
        assert_eq!(word("look at @\"my fi", false), Some((Kind::File, 8..15, "my fi".into())), "a quote opened for a path with spaces");
        assert_eq!(word("/com", false), Some((Kind::Command, 0..4, "com".into())));
        assert_eq!(word("fix /etc/hosts", false), None, "a path in the middle is a path");
        assert_eq!(word("mail me@x.com", false), None, "an @ inside a word is not a mention");
        assert_eq!(word("$tdd", false), None);
        assert_eq!(word("$tdd", true), Some((Kind::Command, 0..4, "tdd".into())), "Codex calls skills with a dollar");
        assert_eq!(word("done ", false), None);
        // The cursor decides, not the end: in the middle of the first word.
        assert_eq!(detect("/compact now", 4, false).map(|t| t.query), Some("com".into()));
        assert_eq!(detect("/compact now", 9, false), None, "past the word");
        // Wide characters: byte offsets stay on boundaries.
        assert_eq!(word("看看 @文档", false), Some((Kind::File, 7..14, "文档".into())));
        assert_eq!(detect("看看", 1, false), None, "inside a character");
    }

    #[test]
    fn rows_match_by_what_was_typed_case_aside() {
        assert!(matches("", &["x"]));
        assert!(matches("MAIN", &["src/main.rs"]));
        assert!(matches("压缩", &["compact", "压缩上下文"]));
        assert!(!matches("zzz", &["compact", "Compact the conversation"]));
    }

    #[test]
    fn the_dot_marks_a_model_or_effort_that_is_not_the_roles() {
        let cfg = |model: &str, default: &str, effort: &str, switch: bool| json!({"model": model, "default_model": default, "effort": effort, "default_effort": "high", "switch": switch});
        assert!(!differs(&cfg("gpt-6.1-sol", "gpt-6.1-sol", "high", true)));
        assert!(differs(&cfg("gpt-6-luna", "gpt-6.1-sol", "high", true)));
        assert!(differs(&cfg("gpt-6.1-sol", "gpt-6.1-sol", "low", true)));
        // Claude Code reports ids; a role that wrote an alias can't be compared.
        assert!(differs(&cfg("claude-haiku-5-5", "claude-sonnet-5-5", "high", false)));
        assert!(!differs(&cfg("claude-haiku-5-5", "haiku", "high", false)));
    }
}
