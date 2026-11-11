//! The box under a thread's conversation: the words, the files that go with them
//! (picked, dropped, pasted, or pointed at with `@`), the commands and skills behind `/`,
//! and the model and effort the session runs on. One per thread, so a draft stays
//! with the thread it was written for.
//!
//! The same box starts a session: before the first message there is no thread, only a project
//! (key `new:<project>`), and the tool row offers the engine, model, effort and permission
//! tier to start it on. Sending makes the thread.
//!
//! What it offers follows what the thread's engine can do: lilad says (`thread.commands`,
//! `thread.files`, and `config` in `thread.read`), and what is not there is not drawn.

use crate::app::LilaApp;
use crate::conversation::Pending;
use crate::style::{self, muted, muted2};
use crate::ui::s;
use gpui::component::attachment::{Attachment, AttachmentContent, AttachmentGroup, AttachmentMedia, AttachmentTitle};
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::input::{Enter, Escape, IndentInline, MoveDown, MoveUp, Textarea, TextareaState};
use gpui::component::menu::{DropdownMenu, PopupMenuItem};
use gpui::component::{Disableable as _, Icon, IconName, Sizable, StyledExt as _, h_flex, v_flex};
use gpui::prelude::*;
use gpui::*;
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::ops::Range;
use std::path::PathBuf;

/// Most rows the menu shows; typing narrows it.
const SHOWN: usize = 60;
/// Pasted pictures are kept this long (then `prune` removes them).
const KEEP_PASTED_MS: u64 = 14 * 24 * 3600 * 1000;
/// The box grows with its words from this many lines to this many; then it scrolls.
const MIN_ROWS: usize = 2;
const MAX_ROWS: usize = 10;

/// The key of the box that starts a session in `project`.
pub fn new_key(project: &str) -> String {
    format!("new:{project}")
}

/// The project a new-session box belongs to; none for a thread's box.
pub fn new_project(key: &str) -> Option<&str> {
    key.strip_prefix("new:")
}

/// What `@` puts in the box for a file: its path (in quotes when it has spaces), and a space after.
pub fn at_text(path: &str) -> String {
    if path.contains(char::is_whitespace) { format!("@\"{path}\" ") } else { format!("@{path} ") }
}

/// What the menu in front of the box is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `@`: a file of the thread's folder.
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

/// How a row's words answer what was typed: how well (higher is better), and which bytes of the words matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub score: u32,
    pub at: Vec<Range<usize>>,
}

/// What was typed after `@` or `/`, ready to be held against many rows. Its letters must appear in the words
/// in order, apart or together, case aside (`atxt` finds `abc.txt`, `ca` does not); the matching is nucleo's (helix's).
pub struct Query(Pattern);

thread_local! {
    /// nucleo's scratch memory, kept: a menu of thousands of files matches on every key.
    static SCRATCH: RefCell<(Matcher, Vec<char>, Vec<u32>)> = RefCell::new((Matcher::new(Config::DEFAULT), Vec::new(), Vec::new()));
}

impl Query {
    pub fn new(typed: &str) -> Self {
        Self(Pattern::new(typed, CaseMatching::Ignore, Normalization::Smart, AtomKind::Fuzzy))
    }

    /// Does `text` answer the query? `path`: it is a file's path, where the letters after a `/` or `.` score better.
    pub fn hit(&self, text: &str, path: bool) -> Option<Hit> {
        SCRATCH.with_borrow_mut(|(matcher, buf, chars)| {
            matcher.config = if path { Config::DEFAULT.match_paths() } else { Config::DEFAULT };
            chars.clear();
            let score = self.0.indices(Utf32Str::new(text, buf), matcher, chars)?;
            // nucleo counts characters, a highlight needs bytes: they differ in the middle of `文档.md`.
            chars.sort_unstable();
            chars.dedup();
            let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).chain([text.len()]).collect();
            let mut at: Vec<Range<usize>> = Vec::new();
            for &c in chars.iter() {
                let range = starts[c as usize]..starts[c as usize + 1];
                match at.last_mut() {
                    Some(last) if last.end == range.start => last.end = range.end,
                    _ => at.push(range),
                }
            }
            Some(Hit { score, at })
        })
    }
}

/// The words with what the typed letters matched in the accent colour and bolder.
fn marked(text: String, at: Vec<Range<usize>>) -> StyledText {
    let mark = HighlightStyle { color: Some(style::blue()), font_weight: Some(FontWeight::SEMIBOLD), ..Default::default() };
    StyledText::new(text).with_highlights(at.into_iter().map(|r| (r, mark)))
}

/// A file that answers what was typed, with what marks its name and what marks its folders.
type RankedFile<'a> = (&'a String, Vec<Range<usize>>, Vec<Range<usize>>);

/// The files that answer `typed`, best first: those whose name has it come before those that only have it in the
/// folders. With each: what marks the name, and what marks the folders.
fn rank_files<'a>(typed: &str, files: &'a [String]) -> Vec<RankedFile<'a>> {
    let query = Query::new(typed);
    let (mut named, mut deeper) = (Vec::new(), Vec::new());
    for f in files {
        let name_at = f.len() - f.rsplit('/').next().unwrap_or(f).len();
        if let Some(hit) = query.hit(&f[name_at..], false) {
            // The folders matched nothing: the name did it alone.
            named.push((hit.score, f, hit.at, Vec::new()));
        } else if let Some(hit) = query.hit(f, true) {
            let dir = name_at.saturating_sub(1);
            deeper.push((hit.score, f, within(&hit.at, name_at..f.len()), within(&hit.at, 0..dir)));
        }
    }
    // Stable: files that score alike stay in the order lilad listed them.
    named.sort_by_key(|h| std::cmp::Reverse(h.0));
    deeper.sort_by_key(|h| std::cmp::Reverse(h.0));
    named.into_iter().chain(deeper).map(|(_, f, name, dir)| (f, name, dir)).collect()
}

/// The parts of `ranges` inside `window`, moved so that `window` starts at 0.
fn within(ranges: &[Range<usize>], window: Range<usize>) -> Vec<Range<usize>> {
    ranges.iter().filter(|r| r.start < window.end && r.end > window.start).map(|r| r.start.max(window.start) - window.start..r.end.min(window.end) - window.start).collect()
}

/// Where `query` sits in `text` as it is (case aside), for the words that are found by plain inclusion.
fn included(query: &str, text: &str) -> Vec<Range<usize>> {
    let (q, t) = (query.to_lowercase(), text.to_lowercase());
    // Lowercasing that changes the length (a few letters do) would put the bytes out of place.
    if q.is_empty() || t.len() != text.len() {
        return Vec::new();
    }
    t.match_indices(&q).map(|(i, m)| i..i + m.len()).filter(|r| text.is_char_boundary(r.start) && text.is_char_boundary(r.end)).collect()
}

/// What choosing a row does.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    /// Puts these words where the trigger was (a file's `@path`, a command's name); `send`: and sends.
    Put { text: String, send: bool },
    /// Lila's own: stops the turn, shows the terminal.
    Stop,
    Terminal,
    /// A request of the runtime's protocol instead of words (`thread.run`).
    Run(String),
}

#[derive(Clone)]
pub struct Row {
    pub icon: Icon,
    pub name: String,
    pub detail: String,
    /// The bytes of `name` and `detail` that the typed words matched: drawn in colour.
    pub name_at: Vec<Range<usize>>,
    pub detail_at: Vec<Range<usize>>,
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

/// What the `/` menu of a thread offers besides Lila's own actions, as lilad listed it.
pub type Commands = Vec<Value>;

/// A thread's rules in the permission menu: what each allows, `(id, words)`, and what taking one back does.
pub struct TierRules {
    pub list: Vec<(i64, String)>,
    pub drop: DropRule,
}

/// Takes the rule with this id back.
pub type DropRule = std::rc::Rc<dyn Fn(i64, &mut App)>;

impl LilaApp {
    /// The key a thread's box is kept under (a thread's id, or `new:<project>`).
    pub fn composer_key(&self, thread: &str) -> String {
        thread.to_owned()
    }

    /// A thread's box, made when first looked at.
    pub fn composer(&mut self, target: &str, window: &mut Window, cx: &mut Context<Self>) -> &mut Composer {
        let key = self.composer_key(target);
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

    /// `--features shots`: the shown thread's (or new session's) box has the cursor.
    #[cfg(feature = "shots")]
    pub fn conv_focus(&self, cx: &App) -> Option<FocusHandle> {
        let key = self.thread.clone().or_else(|| self.new_in.as_deref().map(new_key))?;
        self.composers.get(&key).map(|c| c.input.read(cx).focus_handle(cx))
    }

    /// Does the runtime take `$name` for a skill (Codex)?
    fn dollar(&self, target: &str) -> bool {
        match new_project(target) {
            Some(p) => self.draft_of(p).is_some_and(|d| d.0 == "codex"),
            None => self.snap.threads.iter().any(|m| m["id"] == target && m["engine"] == "codex"),
        }
    }

    /// What a session started in `project` now would run: `(engine, model, effort, permission)`.
    pub fn draft_of(&self, project: &str) -> Option<(String, String, String, String)> {
        let p = self.project(project)?;
        Some(self.drafts.get(project).cloned().unwrap_or_default().effective(&p))
    }

    /// The words or the cursor moved: opens, moves or closes the menu.
    fn composer_changed(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let target = key.to_owned();
        let dollar = self.dollar(&target);
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
                self.fetch_menu(&target, kind, window, cx);
            }
            (None, old) => c.menu = old.filter(|_| forced),
        }
        cx.notify();
    }

    /// Reads what the menu lists from lilad: the files for `@`, the commands for `/`.
    fn fetch_menu(&mut self, target: &str, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.composer_key(target);
        let (method, params) = match (kind, new_project(target)) {
            (Kind::File, Some(project)) => ("thread.files", json!({"project": project})),
            // A session that is not started has no commands to list.
            (Kind::Command, Some(_)) => return,
            (Kind::File, None) => ("thread.files", json!({"thread": target})),
            (Kind::Command, None) => ("thread.commands", json!({"thread": target})),
        };
        self.request_result(method, params, window, cx, move |this, r, _, cx| {
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
    pub fn menu_rows(&self, target: &str, working: bool, tui: bool, menu: &Menu) -> Vec<Row> {
        let key = self.composer_key(target);
        let q = menu.trigger.query.as_str();
        match menu.trigger.kind {
            Kind::File => {
                let files = self.menu_files.get(&key).map(Vec::as_slice).unwrap_or_default();
                rank_files(q, files)
                    .into_iter()
                    .take(SHOWN)
                    .map(|(f, name_at, detail_at)| {
                        let (dir, name) = f.rsplit_once('/').map_or(("", f.as_str()), |(d, n)| (d, n));
                        let text = at_text(f);
                        let icon = if lila_agent::image_mime(f).is_some() { style::icon("lila/image.svg") } else { Icon::new(IconName::FileText) };
                        Row { icon, name: name.to_owned(), detail: dir.to_owned(), name_at, detail_at, pick: Pick::Put { text, send: false } }
                    })
                    .collect()
            }
            Kind::Command => {
                let sigil = if self.dollar(target) { '$' } else { '/' };
                let query = Query::new(q);
                // (score, row): those whose name matches, then those that only mention it in their description.
                let (mut named, mut described): (Vec<(u32, Row)>, Vec<Row>) = (Vec::new(), Vec::new());
                let own = [
                    (Icon::new(IconName::Square), "停止", Pick::Stop, working),
                    (Icon::new(IconName::SquareTerminal), "终端", Pick::Terminal, tui),
                ];
                for (icon, name, pick, offered) in own {
                    if let (true, Some(hit)) = (offered, query.hit(name, false)) {
                        named.push((hit.score, Row { icon, name: name.to_owned(), detail: String::new(), name_at: hit.at, detail_at: Vec::new(), pick }));
                    }
                }
                for c in self.menu_commands.get(&key).into_iter().flatten() {
                    let (name, detail, kind) = (s(c, "name"), s(c, "description"), s(c, "kind"));
                    let by_name = query.hit(&name, false);
                    let by_words = if by_name.is_some() { Vec::new() } else { included(q, &detail) };
                    if by_name.is_none() && by_words.is_empty() {
                        continue;
                    }
                    let icon = match kind.as_str() {
                        "skill" => style::icon("lila/sparkles.svg"),
                        "command" => style::icon("lila/square-slash.svg"),
                        _ => style::icon("lila/command.svg"),
                    };
                    let hint = c["hint"].as_str().unwrap_or_default();
                    // The hint goes in front of the description: what was found in the description moves with it.
                    let shift = if hint.is_empty() { 0 } else { hint.len() + 2 };
                    let detail_at = by_words.iter().map(|r| r.start + shift..r.end + shift).collect();
                    let detail = if hint.is_empty() { detail } else { format!("{hint}  {detail}") };
                    let pick = match (c["action"].as_str(), c["insert"].as_str()) {
                        (Some(a), _) => Pick::Run(a.to_owned()),
                        // Words that take arguments (a hint, or any skill) wait for them in the box; the rest go at once.
                        (None, Some(t)) => Pick::Put { text: t.replace(['/', '$'], &sigil.to_string()), send: hint.is_empty() && kind != "skill" },
                        _ => continue,
                    };
                    // The sign is part of what is drawn, so the highlight moves past it.
                    let name_at = by_name.iter().flat_map(|h| h.at.iter()).map(|r| r.start + 1..r.end + 1).collect();
                    let row = Row { icon, name: format!("{sigil}{name}"), detail, name_at, detail_at, pick };
                    match by_name {
                        Some(h) => named.push((h.score, row)),
                        None => described.push(row),
                    }
                }
                named.sort_by_key(|h| std::cmp::Reverse(h.0));
                let mut rows: Vec<Row> = named.into_iter().map(|h| h.1).chain(described).collect();
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
    fn composer_key_press(&mut self, target: &str, key: MenuKey, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let ck = self.composer_key(target);
        let (working, tui) = self.snap.threads.iter().find(|m| m["id"] == target).map(|m| (m["state"] == "running", m["caps"]["tui"] == true)).unwrap_or_default();
        let Some(c) = self.composers.get(&ck) else { return false };
        let Some(menu) = c.menu.as_ref() else {
            return self.history_key(target, key, window, cx);
        };
        let rows = self.menu_rows(target, working, tui, menu);
        match key {
            MenuKey::Up | MenuKey::Down => {
                self.menu_move(&ck, rows.len(), key == MenuKey::Down);
                cx.notify();
            }
            MenuKey::Enter | MenuKey::Tab => {
                let at = self.composers[&ck].menu.as_ref().map_or(0, |m| m.selected);
                match rows.get(at) {
                    Some(row) => self.pick(target, row.clone(), window, cx),
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
    fn pick(&mut self, target: &str, row: Row, window: &mut Window, cx: &mut Context<Self>) {
        let ck = self.composer_key(target);
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
                    self.send_message(target.to_owned(), window, cx);
                }
            }
            own => {
                put("", window, cx);
                let to = target.to_owned();
                match own {
                    Pick::Stop => self.request("thread.interrupt", json!({"thread": to}), cx, |this, _, _| this.refresh_now()),
                    Pick::Terminal => self.set_view(target, true, cx),
                    Pick::Run(action) => self.request("thread.run", json!({"thread": to, "action": action}), cx, |this, _, _| this.refresh_now()),
                    Pick::Put { .. } => {}
                }
            }
        }
        cx.notify();
    }

    /// ↑ on an empty box brings back what the person sent last; again, the one before. ↓ goes forward, and past the last, empty.
    fn history_key(&mut self, target: &str, key: MenuKey, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let sent = self.sent_by_person();
        let ck = self.composer_key(target);
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

    /// Sends the box's words and files: to the thread, or, for a new session's box, as its first message
    /// (which makes the thread, on the engine, model, effort and tier the box shows). To a thread they go
    /// at once: the box empties and the message shows at the end of the conversation (`Pending`) until the
    /// record has it, and if the send fails they come back into the box, with the reason. A new session's
    /// box keeps them until it is made.
    pub fn send_message(&mut self, target: String, window: &mut Window, cx: &mut Context<Self>) {
        let ck = self.composer_key(&target);
        // Its files are being put back: the words stay in the box.
        let restoring = self.restoring(&target);
        let Some(c) = self.composers.get_mut(&ck) else { return };
        let text = c.input.read(cx).value().trim().to_owned();
        let files: Vec<String> = c.files.iter().map(|f| f.to_string_lossy().to_string()).collect();
        if (text.is_empty() && files.is_empty()) || c.busy || restoring {
            return;
        }
        c.busy = true;
        c.error = None;
        let sent = files.len();
        let (method, params, pending) = match new_project(&target) {
            Some(project) => {
                let (engine, model, effort, permission) = self.draft_of(project).unwrap_or_default();
                let mut p = json!({"project": project, "text": text, "attachments": files, "engine": engine, "effort": effort, "permission": permission});
                if !model.is_empty() {
                    p["model"] = json!(model);
                }
                // The worktree is made before the reply: a few seconds, with the box busy.
                if self.drafts.get(project).is_some_and(|d| d.worktree) && self.project(project).is_some_and(|p| p.repo) {
                    p["worktree"] = json!(true);
                }
                ("thread.new", p, None)
            }
            None => {
                // An ended thread goes on with the words: they are the message it resumes with.
                let ended = self.snap.threads.iter().any(|t| t["id"] == target.as_str() && t["state"] == "ended");
                let id = self.next_pending;
                self.next_pending += 1;
                self.pending.push(Pending { id, thread: target.clone(), text: text.clone(), files: files.clone(), at_ms: crate::data::now_ms(), taken_ms: None });
                // Gone from the box already: what shows is the message, at the end of the conversation.
                let c = self.composers.get_mut(&ck).expect("had it");
                c.input.update(cx, |st, cx| st.set_value("", window, cx));
                c.files.clear();
                c.back = None;
                (if ended { "thread.resume" } else { "thread.send" }, json!({"thread": target, "text": text, "attachments": files}), Some(id))
            }
        };
        self.request_result(method, params, window, cx, move |this, r, window, cx| {
            let ok = r.is_ok();
            if let Some(id) = pending {
                if ok {
                    if let Some(p) = this.pending.iter_mut().find(|p| p.id == id) {
                        p.taken_ms = Some(crate::data::now_ms());
                    }
                    this.settle_pending();
                } else if let Some(at) = this.pending.iter().position(|p| p.id == id) {
                    // It did not go: the temporary row goes, the words and files are back in the box.
                    let p = this.pending.remove(at);
                    if let Some(c) = this.composers.get_mut(&ck) {
                        let now = c.input.read(cx).value().to_string();
                        let words = if now.trim().is_empty() { p.text.clone() } else { format!("{}\n{now}", p.text) };
                        // The words coming back are not typing: they must not take the complaint away (`composer_changed`).
                        c.seen = words.clone();
                        c.input.update(cx, |st, cx| {
                            st.set_value(words.clone(), window, cx);
                            st.set_selected_range(words.len()..words.len(), cx);
                        });
                        c.files.splice(0..0, p.files.iter().map(PathBuf::from).filter(|f| !c.files.contains(f)).collect::<Vec<_>>());
                    }
                }
            }
            let Some(c) = this.composers.get_mut(&ck) else { return };
            c.busy = false;
            match r {
                Ok(v) => {
                    if pending.is_none() {
                        // What was typed meanwhile stays.
                        let same = c.input.read(cx).value().trim() == text;
                        if same {
                            c.input.update(cx, |st, cx| st.set_value("", window, cx));
                        }
                        c.files.drain(..sent.min(c.files.len()));
                    }
                    c.back = None;
                    // The session is made: show it.
                    if let Some(project) = new_project(&ck) {
                        this.drafts.remove(project);
                        if let Some(id) = v["id"].as_str() {
                            this.just_made = Some(id.to_owned());
                            this.show_thread(id.to_owned(), cx);
                        }
                    }
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
    /// Adds files to a thread's next message (folders are not files).
    pub fn add_files(&mut self, target: &str, paths: impl IntoIterator<Item = PathBuf>, cx: &mut Context<Self>) {
        let key = self.composer_key(target);
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

    fn remove_file(&mut self, target: &str, at: usize, cx: &mut Context<Self>) {
        let key = self.composer_key(target);
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
    fn choose_files(&mut self, target: String, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await {
                let _ = this.update(cx, |this, cx| this.add_files(&target, paths, cx));
            }
        })
        .detach();
    }

    /// A paste of pictures or copied files goes to the next message as files; words are left to the box.
    /// A picture is kept as a file of Lila's own, under the state folder, by target.
    fn paste_files(&mut self, target: &str, item: &ClipboardItem, cx: &mut Context<Self>) -> bool {
        if item.entries.iter().any(|e| matches!(e, ClipboardEntry::String(_))) {
            return false;
        }
        let dir = self.paths().state.join("attachments").join(self.composer_key(target).replace(':', "-"));
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
                            if let Some(c) = self.composers.get_mut(&self.composer_key(target)) {
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
        self.add_files(target, files, cx);
        true
    }

    /// Runs the model or effort the person chose from the box's menu, for this session.
    fn set_session_model(&mut self, target: &str, model: Option<String>, effort: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.request_result("thread.model", json!({"thread": target, "model": model, "effort": effort}), window, cx, |this, r, _, cx| {
            match r {
                Ok(_) => this.refresh_now(),
                Err(e) => this.notice = Some(format!("{e:#}")),
            }
            cx.notify();
        });
    }

    /// The button on the right of the tool row for a session: the model, the engine and its effort. A menu of models
    /// and efforts where the session can change them (the engine never changes), else the plain words.
    /// Claude Code can't while it lives: its `/model` and `/effort` save the person's defaults (`thread.model`);
    /// ended, it can. A yellow dot when they are not the workspace's.
    fn thread_spec(&mut self, target: &str, m: &Value, cfg: &Value, cx: &mut Context<Self>) -> AnyElement {
        let quiet = style::quiet(cx);
        let engine = s(m, "engine");
        let known = |key: &str| Some(s(cfg, key)).filter(|v| !v.is_empty()).unwrap_or_else(|| s(m, key));
        let (model, effort) = (known("model"), known("effort"));
        let label = crate::engines::engine_spec(&engine, &model, &effort);
        let switch = match engine.as_str() {
            "claude" => m["state"] == "ended",
            // omp changes only what it lists, once it runs.
            "omp" => cfg["switch"] == true,
            _ => true,
        };
        if !switch {
            return spec_text(label);
        }
        // The choices: the engine lists its own (omp), else those the settings offer.
        let pairs = |list: &Value| -> Vec<(String, String)> { list.as_array().into_iter().flatten().map(|c| (s(c, "id"), s(c, "name"))).collect() };
        let (mut models, mut efforts) = (pairs(&cfg["models"]), pairs(&cfg["efforts"]));
        if models.is_empty() {
            models = crate::engines::models(&engine).iter().map(|m| (m.to_string(), crate::engines::short_model(m))).collect();
            if !model.is_empty() && !models.iter().any(|(id, _)| *id == model) {
                models.insert(0, (model.clone(), crate::engines::short_model(&model)));
            }
        }
        if efforts.is_empty() && crate::engines::has_effort(&engine) {
            efforts = crate::engines::EFFORTS.iter().map(|e| (e.to_string(), e.to_string())).collect();
        }
        let weak = cx.entity().downgrade();
        let to = target.to_owned();
        let button = Button::new("conv-model").custom(quiet).xsmall().label(label).dropdown_caret(true).tooltip("模型").dropdown_menu_with_anchor(Anchor::BottomRight, move |mut menu, _, _| {
            for (id, name) in &models {
                let (weak, to, id2) = (weak.clone(), to.clone(), id.clone());
                menu = menu.item(PopupMenuItem::new(name.clone()).checked(*id == model).on_click(move |_, window, cx| {
                    let _ = weak.update(cx, |this, cx| this.set_session_model(&to, Some(id2.clone()), None, window, cx));
                }));
            }
            if !models.is_empty() && !efforts.is_empty() {
                menu = menu.separator();
            }
            for (id, name) in &efforts {
                let (weak, to, id2) = (weak.clone(), to.clone(), id.clone());
                menu = menu.item(PopupMenuItem::new(name.clone()).checked(*id == effort).on_click(move |_, window, cx| {
                    let _ = weak.update(cx, |this, cx| this.set_session_model(&to, None, Some(id2.clone()), window, cx));
                }));
            }
            menu
        });
        h_flex().gap(px(2.)).items_center().when(differs(cfg), |d| d.child(div().size(px(6.)).flex_none().rounded_full().bg(style::yellow()))).child(button).into_any_element()
    }

    /// The box under a thread's conversation, on a bar of its own, with `ask` (the engine waits in its terminal) above it.
    pub fn composer_view(&mut self, m: &Value, ask: Option<AnyElement>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // Merged: nothing more is said to it.
        if m["merged_ms"].is_number() {
            let id = s(m, "id");
            let sha = self.panel.merged_sha.get(&id).cloned().flatten();
            let text = format!("已合并到 {}{}", s(m, "base_branch"), sha.map(|s| format!(" · {s}")).unwrap_or_default());
            return h_flex().flex_none().justify_center().py_3().border_t_1().border_color(style::divider()).child(div().w_full().max_w(px(760.)).px_5().text_size(px(13.)).text_color(muted()).child(text)).into_any_element();
        }
        let boxed = self.composer_box(&s(m, "id"), Some(m), window, cx);
        h_flex().flex_none().justify_center().pb_3().pt_2().border_t_1().border_color(style::divider()).child(v_flex().w_full().max_w(px(760.)).px_5().gap_2().children(ask).child(boxed)).into_any_element()
    }

    /// The box that starts a session in `project`: the same box, with the choices a session starts on.
    pub fn new_session_box(&mut self, project: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.composer_box(&new_key(project), None, window, cx)
    }

    /// The rounded box: files, the words, and a row of tools. `m` is the thread it writes to; none for a new session's.
    fn composer_box(&mut self, target: &str, m: Option<&Value>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let target = target.to_owned();
        let working = m.is_some_and(|m| s(m, "state") == "running");
        let waiting = m.is_some_and(|m| m["state"] == "waiting");
        let tui = m.is_some_and(|m| m["caps"]["tui"] == true);
        self.composer(&target, window, cx);
        let cfg = self.conv.as_ref().filter(|c| c.thread == target).map(|c| c.config.clone()).unwrap_or(Value::Null);
        let ck = self.composer_key(&target);
        let (input, files, error, busy, has_menu) = {
            let c = &self.composers[&ck];
            (c.input.clone(), c.files.clone(), c.error.clone(), c.busy, c.menu.is_some())
        };
        // Words to a thread that waits on a card or a dialog would answer it: lilad turns them down.
        let has_content = !input.read(cx).value().trim().is_empty() || !files.is_empty();
        // Its files are being put back: nothing is sent until they are (lilad turns the words down too).
        let restoring = m.is_some() && self.restoring(&target);
        let can_send = !busy && !waiting && !restoring && has_content;
        let menu = if has_menu {
            let menu = self.composers[&ck].menu.as_ref().expect("has one");
            let rows = self.menu_rows(&target, working, tui, menu);
            let (selected, scroll) = (menu.selected.min(rows.len().saturating_sub(1)), menu.scroll.clone());
            (!rows.is_empty()).then(|| self.menu_view(&target, rows, selected, scroll, cx))
        } else {
            None
        };
        let weak = cx.entity().downgrade();

        let cards = (!files.is_empty()).then(|| {
            AttachmentGroup::new("composer-files").gap_2().px_3().pt_3().children(files.iter().enumerate().map(|(ix, f)| {
                let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let image = lila_agent::image_mime(&f.to_string_lossy()).is_some();
                let to = target.clone();
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

        let paste_to = target.clone();
        let editor = div()
            .id("composer-input")
            .px_1()
            .pt_1()
            // The menu's keys come before the box's own: ↑ ↓ move, Enter and Tab choose, Esc closes.
            .capture_action(cx.listener({
                let to = target.clone();
                move |this, _: &MoveUp, window, cx| {
                    if this.composer_key_press(&to, MenuKey::Up, window, cx) {
                        cx.stop_propagation();
                    }
                }
            }))
            .capture_action(cx.listener({
                let to = target.clone();
                move |this, _: &MoveDown, window, cx| {
                    if this.composer_key_press(&to, MenuKey::Down, window, cx) {
                        cx.stop_propagation();
                    }
                }
            }))
            // Enter chooses what the menu shows, else sends (Shift+Enter breaks the line). Taken here, so the box
            // never adds a line for it; while an input method composes, its own keys do not arrive as Enter.
            .capture_action(cx.listener({
                let to = target.clone();
                move |this, a: &Enter, window, cx| {
                    if a.shift {
                        return;
                    }
                    let menu = this.composers.get(&this.composer_key(&to)).is_some_and(|c| c.menu.is_some());
                    if !(menu && this.composer_key_press(&to, MenuKey::Enter, window, cx)) {
                        this.send_message(to.clone(), window, cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener({
                let to = target.clone();
                move |this, _: &IndentInline, window, cx| {
                    if this.composers.get(&this.composer_key(&to)).is_some_and(|c| c.menu.is_some()) && this.composer_key_press(&to, MenuKey::Tab, window, cx) {
                        cx.stop_propagation();
                    }
                }
            }))
            .capture_action(cx.listener({
                let to = target.clone();
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

        // Running, with nothing written: the button stops the turn. Words written go as a message even now (they queue or cut in).
        let stop = working && !has_content;
        let tier: Vec<AnyElement> = match m {
            Some(m) => {
                let (weak, id) = (cx.entity().downgrade(), target.clone());
                // What was allowed "always" is listed under the three, each to be taken back by a click.
                let rules: Vec<(i64, String)> = self.conv.as_ref().filter(|c| c.thread == target).map(|c| c.rules.iter().filter_map(|r| Some((r["id"].as_i64()?, s(r, "label")))).collect()).unwrap_or_default();
                let (weak2, id2) = (weak.clone(), id.clone());
                let rules = TierRules {
                    list: rules,
                    drop: std::rc::Rc::new(move |rule, cx| {
                        let _ = weak2.update(cx, |this, cx| this.act("thread.rule.remove", json!({"thread": id2, "rule": rule}), cx));
                    }),
                };
                vec![self.tier_button("conv-tier", &s(m, "permission"), Some(rules), Anchor::BottomLeft, cx, move |key, cx| {
                    let _ = weak.update(cx, |this, cx| this.act("thread.permission", json!({"thread": id, "permission": key}), cx));
                })]
            }
            None => new_project(&target).map(String::from).map(|p| self.tier_control(&p, cx)).unwrap_or_default(),
        };
        let spec = match m {
            Some(m) => Some(self.thread_spec(&target, m, &cfg, cx)),
            None => new_project(&target).map(String::from).and_then(|p| self.new_spec(&p, cx)),
        };
        let tools = h_flex()
            .px_2()
            .pb_2()
            .gap_1()
            .items_center()
            .child(self.plus_button(&target, m.is_some(), cx))
            .children(tier)
            .child(div().flex_1())
            .children(spec)
            .child({
                let to = target.clone();
                if stop {
                    Button::new("conv-stop").primary().size(px(style::CTL_SM)).icon(IconName::Square).tooltip("停止 (⌘.)").on_click(cx.listener(move |this, _, _, cx| this.request("thread.interrupt", json!({"thread": to}), cx, |this, _, _| this.refresh_now())))
                } else {
                    Button::new("conv-send").primary().size(px(style::CTL_SM)).icon(IconName::ArrowUp).loading(busy && new_project(&to).is_some()).disabled(!can_send).tooltip("发送 (↩)").on_click(cx.listener(move |this, _, window, cx| this.send_message(to.clone(), window, cx)))
                }
            });

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
            .child(tools)
            .into_any_element()
    }

    /// The button on the right of a new session's tool row: the model, the engine and its effort, as one menu: the
    /// engines, then the models, then the efforts (the effort where the engine takes one).
    fn new_spec(&mut self, project: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        let quiet = style::quiet(cx);
        let (engine, model, effort, _) = self.draft_of(project)?;
        let weak = cx.entity().downgrade();
        let models: Vec<String> = {
            let mut m: Vec<String> = crate::engines::models(&engine).iter().map(|m| m.to_string()).collect();
            if !model.is_empty() && !m.contains(&model) {
                m.insert(0, model.clone());
            }
            m
        };
        let shown_model = if model.is_empty() { crate::engines::default_model(&engine).to_owned() } else { model.clone() };
        let (now_engine, project) = (engine.clone(), project.to_owned());
        Some(
            Button::new("new-spec")
                .custom(quiet)
                .xsmall()
                .label(crate::engines::engine_spec(&engine, &model, &effort))
                .dropdown_caret(true)
                .tooltip("模型")
                .dropdown_menu_with_anchor(Anchor::BottomRight, move |mut menu, _, _| {
                    for (id, name) in crate::engines::ENGINES {
                        let (weak, project) = (weak.clone(), project.clone());
                        menu = menu.item(PopupMenuItem::new(name).checked(id == now_engine).on_click(move |_, _, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.drafts.entry(project.clone()).or_default().set_engine(id);
                                cx.notify();
                            });
                        }));
                    }
                    if !models.is_empty() {
                        menu = menu.separator();
                    }
                    for m in &models {
                        let (weak, project, id) = (weak.clone(), project.clone(), m.clone());
                        menu = menu.item(PopupMenuItem::new(crate::engines::short_model(m)).checked(*m == shown_model).on_click(move |_, _, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.drafts.entry(project.clone()).or_default().model = Some(id.clone());
                                cx.notify();
                            });
                        }));
                    }
                    if crate::engines::has_effort(&now_engine) {
                        menu = menu.separator();
                        for e in std::iter::once("").chain(crate::engines::EFFORTS) {
                            let (weak, project) = (weak.clone(), project.clone());
                            menu = menu.item(PopupMenuItem::new(if e.is_empty() { "默认" } else { e }).checked(e == effort).on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    this.drafts.entry(project.clone()).or_default().effort = Some(e.to_owned());
                                    cx.notify();
                                });
                            }));
                        }
                    }
                    menu
                })
                .into_any_element(),
        )
    }

    /// The `+` at the start of the tool row. A session: a menu of what can be added (a file; the commands, which
    /// the engine reports, so a new session's box has none). A new session's box has only files: it opens the dialog.
    fn plus_button(&mut self, target: &str, commands: bool, cx: &mut Context<Self>) -> AnyElement {
        let quiet = style::quiet(cx);
        let to = target.to_owned();
        let button = Button::new("conv-plus").custom(quiet).size(px(style::CTL_SM)).icon(Icon::new(IconName::Plus).text_color(muted()));
        if !commands {
            return button.tooltip("添加附件").on_click(cx.listener(move |this, _, window, cx| this.choose_files(to.clone(), window, cx))).into_any_element();
        }
        let weak = cx.entity().downgrade();
        button
            .tooltip("添加")
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, _, _| {
                let (w1, to1) = (weak.clone(), to.clone());
                let (w2, to2) = (weak.clone(), to.clone());
                menu.item(PopupMenuItem::new("添加附件").icon(style::icon("lila/paperclip.svg")).on_click(move |_, window, cx| {
                    let _ = w1.update(cx, |this, cx| this.choose_files(to1.clone(), window, cx));
                }))
                .item(PopupMenuItem::new("命令").icon(style::icon("lila/square-slash.svg")).on_click(move |_, window, cx| {
                    let _ = w2.update(cx, |this, cx| this.toggle_menu(&to2, window, cx));
                }))
            })
            .into_any_element()
    }

    /// The permission tier a new session starts on, and (in a folder that is a repository) the worktree switch.
    fn tier_control(&mut self, project: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some((.., tier)) = self.draft_of(project) else { return Vec::new() };
        let mut controls = vec![self.tier_button("new-tier", &tier, None, Anchor::BottomLeft, cx, {
            let (weak, project) = (cx.entity().downgrade(), project.to_owned());
            move |key, cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.drafts.entry(project.clone()).or_default().permission = Some(key.to_owned());
                    cx.notify();
                });
            }
        })];
        if self.project(project).is_some_and(|p| p.repo) {
            controls.push(self.worktree_switch(project, cx));
        }
        controls
    }

    /// The switch for a worktree of the session's own: filled and blue while on. Not while the first message is on its way.
    fn worktree_switch(&mut self, project: &str, cx: &mut Context<Self>) -> AnyElement {
        let quiet = style::quiet(cx);
        let on = self.drafts.get(project).is_some_and(|d| d.worktree);
        let busy = self.composers.get(&new_key(project)).is_some_and(|c| c.busy);
        let project = project.to_owned();
        Button::new("new-worktree")
            .custom(quiet)
            .xsmall()
            .icon(style::icon("lila/git-branch.svg").text_color(if on { style::blue() } else { muted() }))
            .when(on, |b| b.bg(style::accent()))
            .disabled(busy)
            .tooltip("独立 worktree")
            .on_click(cx.listener(move |this, _, _, cx| {
                let d = this.drafts.entry(project.clone()).or_default();
                d.worktree = !d.worktree;
                cx.notify();
            }))
            .into_any_element()
    }

    /// A button showing a permission tier, and the three to choose from; `pick` gets the one chosen. Under them, the thread's `rules`.
    pub fn tier_button(&self, id: &'static str, tier: &str, rules: Option<TierRules>, anchor: Anchor, cx: &App, pick: impl Fn(&'static str, &mut App) + Clone + 'static) -> AnyElement {
        let quiet = style::quiet(cx);
        let now = tier.to_owned();
        Button::new(id)
            .custom(quiet)
            .xsmall()
            .icon(style::icon(crate::engines::tier_icon(tier)))
            .label(crate::engines::tier_name(tier))
            .dropdown_caret(true)
            .tooltip("权限")
            .dropdown_menu_with_anchor(anchor, move |mut menu, _, _| {
                for (key, name, _) in crate::engines::TIERS {
                    let pick = pick.clone();
                    menu = menu.item(PopupMenuItem::new(name).checked(key == now).on_click(move |_, _, cx| pick(key, cx)));
                }
                if let Some(rules) = rules.as_ref().filter(|r| !r.list.is_empty()) {
                    menu = menu.separator();
                    for (rule, label) in &rules.list {
                        let (drop, rule) = (rules.drop.clone(), *rule);
                        menu = menu.item(PopupMenuItem::new(format!("不再总是允许{}{}", if label.starts_with(|c: char| c.is_ascii()) { " " } else { "" }, crate::engines::clip(label, 36))).icon(IconName::Close).on_click(move |_, _, cx| drop(rule, cx)));
                    }
                }
                menu
            })
            .into_any_element()
    }

    /// The command button: the menu of commands, opened over the box, whatever is in it.
    fn toggle_menu(&mut self, target: &str, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.composer_key(target);
        let Some(c) = self.composers.get_mut(&key) else { return };
        if c.menu.take().is_none() {
            c.menu = Some(Menu { trigger: Trigger { kind: Kind::Command, range: 0..0, query: String::new() }, selected: 0, scroll: ScrollHandle::new(), forced: true });
            let focus = c.input.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
            self.fetch_menu(target, Kind::Command, window, cx);
        }
        cx.notify();
    }

    /// The rows over the box.
    fn menu_view(&self, target: &str, rows: Vec<Row>, selected: usize, scroll: ScrollHandle, cx: &mut Context<Self>) -> AnyElement {
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
                        let (to, picked) = (target.to_owned(), row.clone());
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
                            .child(div().flex_none().max_w(relative(0.5)).truncate().text_size(px(13.)).font_medium().child(marked(row.name, row.name_at)))
                            .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(muted2()).child(marked(row.detail, row.detail_at)))
                            .on_click(cx.listener(move |this, _, window, cx| this.pick(&to, picked.clone(), window, cx)))
                    })),
            )
            .into_any_element()
    }

    /// `--features shots`: fills the shown thread's box (`fill:<words>`), adds a file (`attach:<path>`), opens the command menu (`menu`) or shows another thread (`show:<id>`).
    #[cfg(feature = "shots")]
    pub fn dev_composer(&mut self, step: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(to) = step.strip_prefix("show:") {
            self.show_thread(to.to_owned(), cx);
            return;
        }
        let Some(target) = self.thread.clone().or_else(|| self.new_in.as_deref().map(new_key)) else { return };
        self.composer(&target, window, cx);
        let key = self.composer_key(&target);
        if let Some(words) = step.strip_prefix("fill:") {
            let input = self.composers[&key].input.clone();
            let words = words.replace('_', " ").replace("\\n", "\n");
            input.update(cx, |st, cx| {
                st.set_value(words.clone(), window, cx);
                st.set_selected_range(words.len()..words.len(), cx);
            });
        } else if let Some(path) = step.strip_prefix("attach:") {
            self.add_files(&target, [PathBuf::from(path)], cx);
        } else if step == "menu" {
            self.toggle_menu(&target, window, cx);
        }
    }
}

/// The model, engine and effort as plain words, where the session can't change them.
fn spec_text(label: String) -> AnyElement {
    h_flex().h(px(style::CTL_SM)).px_2().min_w_0().overflow_hidden().whitespace_nowrap().items_center().text_size(px(12.)).text_color(muted2()).child(label).into_any_element()
}

/// Is the model or effort not the project's? (Claude Code's model is read from its replies, as an id: only an id can differ for certain.)
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
    use super::{Kind, Query, detect, differs, included, rank_files, within};
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

    /// The matched pieces of `text` for `query`, as text.
    fn found(query: &str, text: &str) -> Option<Vec<String>> {
        Query::new(query).hit(text, false).map(|h| h.at.into_iter().map(|r| text[r].to_owned()).collect())
    }

    #[test]
    fn letters_in_order_match_apart_or_together_case_aside() {
        for q in ["ab", "bc", "ac", "atxt", "AB", "abc.txt", ""] {
            assert!(found(q, "abc.txt").is_some(), "{q} should find abc.txt");
        }
        for q in ["ca", "txta", "zzz", "abcd"] {
            assert!(found(q, "abc.txt").is_none(), "{q} should not find abc.txt");
        }
        assert!(Query::new("MAIN").hit("src/main.rs", true).is_some());
    }

    #[test]
    fn what_matched_is_given_as_byte_ranges_and_neighbours_join() {
        assert_eq!(found("ab", "abc.txt"), Some(vec!["ab".into()]));
        assert_eq!(found("ac", "abc.txt"), Some(vec!["a".into(), "c".into()]));
        assert_eq!(found("atxt", "abc.txt").map(|v| v.concat()), Some("atxt".into()));
        assert_eq!(found("", "abc.txt"), Some(vec![]), "nothing typed: everything, nothing marked");
        let hit = Query::new("ac").hit("abc.txt", false).unwrap();
        assert_eq!(hit.at, vec![0..1, 2..3]);
    }

    #[test]
    fn wide_characters_are_counted_in_bytes_not_characters() {
        let hit = Query::new("档md").hit("文档.md", false).unwrap();
        // 文 is bytes 0..3, 档 3..6, '.' 6..7, m 7..8, d 8..9.
        assert_eq!(hit.at, vec![3..6, 7..9]);
        assert_eq!(found("文md", "文档.md"), Some(vec!["文".into(), "md".into()]));
        assert_eq!(found("压缩", "压缩上下文"), Some(vec!["压缩".into()]));
        assert!(found("缩压", "压缩上下文").is_none());
        // Mixed with letters before them: the offsets of the letters are right too.
        assert_eq!(found("a文", "ab文档"), Some(vec!["a".into(), "文".into()]));
    }

    #[test]
    fn a_better_match_scores_higher() {
        let q = Query::new("main");
        let (whole, loose) = (q.hit("main.rs", false).unwrap(), q.hit("my_amazing_index.rs", false).unwrap());
        assert!(whole.score > loose.score);
    }

    #[test]
    fn a_path_is_split_into_what_marks_the_folders_and_the_name() {
        // `src/main.rs`: the folders are bytes 0..3, the name starts at 4.
        let hit = Query::new("sm").hit("src/main.rs", true).unwrap();
        assert_eq!(hit.at, vec![0..1, 4..5]);
        assert_eq!(within(&hit.at, 4..11), vec![0..1]);
        assert_eq!(within(&hit.at, 0..3), vec![0..1]);
        // A match that runs across the slash is cut at it.
        let one: Vec<std::ops::Range<usize>> = std::iter::once(2..6).collect();
        assert_eq!(within(&one, 0..3), vec![2..3]);
        assert_eq!(within(&one, 4..11), vec![0..2]);
    }

    #[test]
    fn files_named_for_it_come_before_files_in_folders_named_for_it() {
        let files: Vec<String> = ["docs/readme.md", "src/app/main.rs", "src/ui/app.rs", "app.txt", "x/y/z.rs"].map(String::from).to_vec();
        let ranked = rank_files("app", &files);
        let names: Vec<&str> = ranked.iter().map(|r| r.0.as_str()).collect();
        // Named for it: app.txt and app.rs (best first, the shorter whole match wins); only in a folder: src/app/main.rs.
        assert_eq!(names.len(), 3);
        assert_eq!(names[2], "src/app/main.rs");
        assert!(names[..2].contains(&"app.txt") && names[..2].contains(&"src/ui/app.rs"));
        assert_eq!(ranked[2].1, Vec::<std::ops::Range<usize>>::new());
        assert_eq!(ranked[2].2, vec![4..7], "the folder part `src/app` marks `app` at bytes 4..7");
        assert!(rank_files("zq", &files).is_empty());
        assert_eq!(rank_files("", &files).len(), 5, "nothing typed: all, in the order given");
        assert_eq!(rank_files("", &files)[0].0, "docs/readme.md");
    }

    #[test]
    fn a_description_is_searched_by_plain_inclusion() {
        assert_eq!(included("Compact", "compact the conversation"), vec![0..7]);
        assert_eq!(included("the", "compact the conversation"), vec![8..11]);
        assert!(included("cmt", "compact the conversation").is_empty(), "no skipping letters here");
        assert_eq!(included("压缩", "压缩上下文"), vec![0..6]);
        assert!(included("", "x").is_empty());
    }

    #[test]
    fn the_dot_marks_a_model_or_effort_that_is_not_the_projects() {
        let cfg = |model: &str, default: &str, effort: &str, switch: bool| json!({"model": model, "default_model": default, "effort": effort, "default_effort": "high", "switch": switch});
        assert!(!differs(&cfg("gpt-6.1-sol", "gpt-6.1-sol", "high", true)));
        assert!(differs(&cfg("gpt-6-luna", "gpt-6.1-sol", "high", true)));
        assert!(differs(&cfg("gpt-6.1-sol", "gpt-6.1-sol", "low", true)));
        // Claude Code reports ids; a project that wrote an alias can't be compared.
        assert!(differs(&cfg("claude-haiku-5-5", "claude-sonnet-5-5", "high", false)));
        assert!(!differs(&cfg("claude-haiku-5-5", "haiku", "high", false)));
    }
}
