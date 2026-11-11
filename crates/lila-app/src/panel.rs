//! The right panel: a third card beside the sidebar and the shown thread, for what the thread did in
//! its folder. Two views, opened from the header: the files (a tree, and a read-only preview with
//! syntax colors) and the changes (what differs from HEAD, with each file's diff, and the buttons that
//! commit them and, for a thread in a worktree of its own, merge it back). Everything here is read from
//! lilad (`thread.files/file/changes/diff`) and done through it (`thread.commit/merge`); the
//! panel only draws (docs/architecture-v4.md §4, §10).

use crate::app::LilaApp;
use crate::style::{self, Control as _, card, muted, muted2};
use crate::ui::{BAR, drag_bar, s};
use gpui::component::button::{Button, ButtonVariants};
use gpui::component::input::{Editor, EditorState};
use gpui::component::list::ListItem;
use gpui::component::scroll::ScrollableElement;
use gpui::component::spinner::Spinner;
use gpui::component::tree::{TreeEvent, TreeItem, TreeState, tree};
use gpui::component::{Disableable, Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
use gpui::prelude::*;
use gpui::*;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

/// The panel's width when it was never dragged, and how far it may be dragged.
pub const DEFAULT_WIDTH: f32 = 380.;
pub const MIN_WIDTH: f32 = 280.;
pub const MAX_WIDTH: f32 = 720.;
/// The shown thread keeps this much when the window is narrow: the panel gives way first, down to `MIN_WIDTH`,
/// and only then the thread, down to `HARD_CENTER`.
pub const MIN_CENTER: f32 = 360.;
const HARD_CENTER: f32 = 240.;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Files,
    Changes,
}

impl View {
    fn key(self) -> &'static str {
        match self {
            View::Files => "files",
            View::Changes => "changes",
        }
    }

    fn of(key: &str) -> Option<View> {
        match key {
            "files" => Some(View::Files),
            "changes" => Some(View::Changes),
            _ => None,
        }
    }
}

/// One file's diff, as lilad gave it, while it is read or after.
pub enum Diff {
    Loading,
    /// A `thread.diff` item (`Null`: the file has no diff any more).
    Ready(Value),
    Failed(String),
}

/// What a thread's changes view has: the last `thread.changes`, the files opened to show their diff, and
/// what the buttons are doing.
#[derive(Default)]
pub struct Changes {
    pub data: Option<Rc<Value>>,
    loading: bool,
    /// Asked again while reading: read once more when this one is in.
    again: bool,
    pub error: Option<String>,
    /// The paths whose diff is open.
    pub open: HashSet<String>,
    pub diffs: HashMap<String, Diff>,
    /// The commits of a worktree thread are listed (else only their count).
    pub commits_open: bool,
    pub committing: bool,
    pub merging: bool,
    /// Why the last commit or merge did not go.
    pub failed: Option<String>,
}

/// The preview of the file open in the files view.
pub struct Preview {
    pub path: String,
    pub body: PreviewBody,
}

pub enum PreviewBody {
    Loading,
    Text(Entity<EditorState>),
    Binary,
    TooLarge,
    Failed(String),
}

/// A thread's files: the tree (made from the paths lilad lists), and the file looked at.
pub struct FileTree {
    pub files: Vec<String>,
    pub state: Entity<TreeState>,
    /// The folders opened, kept when the list is read again.
    expanded: Rc<RefCell<HashSet<SharedString>>>,
    loading: bool,
    pub error: Option<String>,
    pub preview: Option<Preview>,
    _sub: Subscription,
}

pub struct Panel {
    pub open: bool,
    pub view: View,
    /// The width it opens at (the last one saved); the live one is in `size`.
    pub width: f32,
    pub size: Entity<gpui::base::ResizableState>,
    pub changes: HashMap<String, Changes>,
    pub files: HashMap<String, FileTree>,
    /// The thread whose merge the dialog asks about.
    pub merge_confirm: Option<String>,
    /// The commit a merged thread went in as (`None`: lilad's log does not say), by thread; read once.
    pub merged_sha: HashMap<String, Option<String>>,
    /// What the panel was last made current for: the thread, its changes summary and state. A snapshot that says
    /// something else about the shown thread reads again.
    followed: Option<(String, Value, String)>,
    _resized: Subscription,
}

impl Panel {
    pub fn new(paths: &lila::paths::Paths, cx: &mut Context<LilaApp>) -> Panel {
        let (open, view, width) = crate::data::load_panel(paths);
        let size: Entity<gpui::base::ResizableState> = cx.new(|_| Default::default());
        let sub = cx.subscribe(&size, |this, state, _: &gpui::base::ResizablePanelEvent, cx| {
            // The panel is the second of the two: its size is what the drag left it at.
            if let Some(w) = state.read(cx).sizes().get(1).map(|w| f32::from(*w)).filter(|w| *w >= MIN_WIDTH) {
                this.panel.width = w;
                this.save_panel();
            }
        });
        Panel {
            open,
            view: view.as_deref().and_then(View::of).unwrap_or(View::Changes),
            width: width.unwrap_or(DEFAULT_WIDTH).clamp(MIN_WIDTH, MAX_WIDTH),
            size,
            changes: HashMap::new(),
            files: HashMap::new(),
            merge_confirm: None,
            merged_sha: HashMap::new(),
            followed: None,
            _resized: sub,
        }
    }
}

/// `src/a/b.rs` as (`src/a`, `b.rs`).
fn split(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

/// The language a file is highlighted as: its extension, else its name (`Makefile`, `Dockerfile`). The
/// highlighter knows extensions and names alike.
pub fn language_of(path: &str) -> String {
    let name = split(path).1;
    if name == "Cargo.lock" {
        return "toml".into();
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_lowercase(),
        _ => name.to_owned(),
    }
}

/// The folders and files under one folder of the tree.
#[derive(Default)]
struct Node {
    dirs: BTreeMap<String, Node>,
    files: Vec<String>,
}

/// The tree's items for these paths: folders first, then files, each by name without regard to case. A folder's
/// id is its path, a file's too; `expanded` are the folders shown open.
fn tree_items(files: &[String], expanded: &HashSet<SharedString>) -> Vec<TreeItem> {
    let mut root = Node::default();
    for f in files {
        let mut node = &mut root;
        let mut parts = f.split('/').peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_some() {
                node = node.dirs.entry(part.to_owned()).or_default();
            } else {
                node.files.push(part.to_owned());
            }
        }
    }
    fn items(node: &Node, prefix: &str, expanded: &HashSet<SharedString>) -> Vec<TreeItem> {
        let mut dirs: Vec<(&String, &Node)> = node.dirs.iter().collect();
        dirs.sort_by_key(|(name, _)| name.to_lowercase());
        let mut out: Vec<TreeItem> = dirs
            .into_iter()
            .map(|(name, child)| {
                let id = format!("{prefix}{name}");
                let inside = items(child, &format!("{id}/"), expanded);
                let open = expanded.contains(id.as_str());
                TreeItem::new(id, name.clone()).children(inside).expanded(open)
            })
            .collect();
        let mut files: Vec<&String> = node.files.iter().collect();
        files.sort_by_key(|n| n.to_lowercase());
        out.extend(files.into_iter().map(|name| TreeItem::new(format!("{prefix}{name}"), name.clone())));
        out
    }
    items(&root, "", expanded)
}

/// The letter and color a change is marked with.
pub(crate) fn mark(status: &str) -> (&'static str, Hsla) {
    match status {
        "added" => ("A", style::green()),
        "untracked" => ("U", style::green()),
        "deleted" => ("D", style::red()),
        "renamed" => ("R", muted()),
        _ => ("M", style::tint(style::blue(), 0.3)),
    }
}

/// A one-line statement in the middle of the panel.
fn note(text: impl Into<SharedString>) -> AnyElement {
    div().flex_1().flex().items_center().justify_center().px_4().text_size(px(13.)).text_color(muted()).text_center().child(text.into()).into_any_element()
}

/// `+N −M` as the tool rows write it.
fn counts(add: i64, del: i64) -> Div {
    h_flex()
        .flex_none()
        .gap(px(6.))
        .font_family(lila_term::theme::FONT)
        .text_size(px(11.))
        .child(div().text_color(style::tint(style::green(), 0.3)).child(format!("+{add}")))
        .child(div().text_color(style::tint(style::red(), 0.3)).child(format!("−{del}")))
}

/// The lines of a diff, hashed: a viewer is made again when what it shows changed.
fn fingerprint(d: &Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for l in d["lines"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        l.hash(&mut h);
    }
    h.finish()
}

impl LilaApp {
    // ---------------- state ----------------

    /// The thread the panel shows: the shown one, unless it was merged (its folder is gone).
    pub fn panel_thread(&self) -> Option<Value> {
        self.shown_thread().filter(|t| self.panel.open && !t["merged_ms"].is_number())
    }

    pub fn save_panel(&self) {
        crate::data::save_panel(self.paths(), self.panel.open, self.panel.view.key(), self.panel.width);
    }

    /// The header button: opens that view; pressed again, it closes the panel.
    pub fn toggle_panel(&mut self, view: View, cx: &mut Context<Self>) {
        if self.panel.open && self.panel.view == view {
            self.panel.open = false;
        } else {
            self.panel.open = true;
            self.panel.view = view;
            self.panel.followed = None;
            self.panel_refresh(cx);
        }
        self.save_panel();
        cx.notify();
    }

    /// Reads what the visible view shows, for the shown thread; what it had stays on screen until the new arrives.
    pub fn panel_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(t) = self.panel_thread() else { return };
        let id = s(&t, "id");
        self.panel.followed = Some((id.clone(), t["changes"].clone(), s(&t, "state")));
        match self.panel.view {
            View::Changes => self.fetch_changes(id, cx),
            View::Files => self.fetch_files(id, cx),
        }
    }

    /// After a snapshot: reads again when the shown thread's changes or state are not what the panel last looked at.
    pub fn panel_follow(&mut self, cx: &mut Context<Self>) {
        let Some(t) = self.panel_thread() else { return };
        let now = (s(&t, "id"), t["changes"].clone(), s(&t, "state"));
        if self.panel.followed.as_ref() != Some(&now) {
            self.panel_refresh(cx);
        }
    }

    // ---------------- reading ----------------

    pub fn fetch_changes(&mut self, thread: String, cx: &mut Context<Self>) {
        let c = self.panel.changes.entry(thread.clone()).or_default();
        if c.loading {
            c.again = true;
            return;
        }
        c.loading = true;
        let id = thread.clone();
        self.call("thread.changes", json!({"thread": thread}), cx, move |this, r, cx| {
            let c = this.panel.changes.entry(id.clone()).or_default();
            c.loading = false;
            let again = std::mem::take(&mut c.again);
            let mut reread = Vec::new();
            match r {
                Ok(v) => {
                    c.error = None;
                    // The diffs open are of files that are still changed; they are read again, the old ones stay in view meanwhile.
                    let paths: HashSet<String> = v["files"].as_array().into_iter().flatten().filter_map(|f| f["path"].as_str().map(String::from)).collect();
                    c.open.retain(|p| paths.contains(p));
                    c.diffs.retain(|p, _| paths.contains(p));
                    reread = c.open.iter().cloned().collect();
                    c.data = Some(Rc::new(v));
                }
                Err(e) => c.error = Some(format!("{e:#}")),
            }
            for p in reread {
                this.fetch_diff(id.clone(), p, cx);
            }
            if again {
                this.fetch_changes(id, cx);
            }
            cx.notify();
        });
    }

    fn fetch_diff(&mut self, thread: String, path: String, cx: &mut Context<Self>) {
        let c = self.panel.changes.entry(thread.clone()).or_default();
        c.diffs.entry(path.clone()).or_insert(Diff::Loading);
        self.call("thread.diff", json!({"thread": thread, "path": path}), cx, move |this, r, cx| {
            let c = this.panel.changes.entry(thread).or_default();
            // Closed meanwhile: nobody asks for it.
            if c.open.contains(&path) {
                c.diffs.insert(path, match r {
                    Ok(v) => Diff::Ready(v["diffs"].get(0).cloned().unwrap_or(Value::Null)),
                    Err(e) => Diff::Failed(format!("{e:#}")),
                });
            }
            cx.notify();
        });
    }

    fn toggle_diff(&mut self, thread: &str, path: &str, cx: &mut Context<Self>) {
        let c = self.panel.changes.entry(thread.to_owned()).or_default();
        if !c.open.remove(path) {
            c.open.insert(path.to_owned());
            self.fetch_diff(thread.to_owned(), path.to_owned(), cx);
        }
        cx.notify();
    }

    fn toggle_commits(&mut self, thread: &str, cx: &mut Context<Self>) {
        let c = self.panel.changes.entry(thread.to_owned()).or_default();
        c.commits_open = !c.commits_open;
        cx.notify();
    }

    pub fn fetch_files(&mut self, thread: String, cx: &mut Context<Self>) {
        if self.panel.files.get(&thread).is_some_and(|f| f.loading) {
            return;
        }
        if let Some(f) = self.panel.files.get_mut(&thread) {
            f.loading = true;
        }
        let id = thread.clone();
        self.call("thread.files", json!({"thread": thread}), cx, move |this, r, cx| {
            match r {
                Ok(v) => {
                    let files: Vec<String> = v["files"].as_array().into_iter().flatten().filter_map(|f| f.as_str().map(String::from)).collect();
                    match this.panel.files.get_mut(&id) {
                        Some(ft) => {
                            ft.loading = false;
                            ft.error = None;
                            // The same list: the tree stays as it is (its open folders, its scroll).
                            if ft.files != files {
                                let items = tree_items(&files, &ft.expanded.borrow());
                                ft.state.update(cx, |st, cx| st.set_items(items, cx));
                                ft.files = files;
                            }
                        }
                        None => {
                            let expanded: Rc<RefCell<HashSet<SharedString>>> = Default::default();
                            let items = tree_items(&files, &expanded.borrow());
                            let state = cx.new(|cx| TreeState::new(cx).items(items));
                            let seen = expanded.clone();
                            let sub = cx.subscribe(&state, move |_, _, ev: &TreeEvent, _| match ev {
                                TreeEvent::Expanded(id) => {
                                    seen.borrow_mut().insert(id.clone());
                                }
                                TreeEvent::Collapsed(id) => {
                                    seen.borrow_mut().remove(id);
                                }
                            });
                            this.panel.files.insert(id, FileTree { files, state, expanded, loading: false, error: None, preview: None, _sub: sub });
                        }
                    }
                }
                Err(e) => {
                    if let Some(ft) = this.panel.files.get_mut(&id) {
                        ft.loading = false;
                        ft.error = Some(format!("{e:#}"));
                    }
                }
            }
            cx.notify();
        });
    }

    /// Shows a file in the files view: read from lilad, colored by what it is called.
    pub fn open_preview(&mut self, thread: String, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ft) = self.panel.files.get_mut(&thread) else { return };
        ft.preview = Some(Preview { path: path.clone(), body: PreviewBody::Loading });
        cx.notify();
        self.request_result("thread.file", json!({"thread": thread, "path": path}), window, cx, move |this, r, window, cx| {
            let Some(ft) = this.panel.files.get_mut(&thread) else { return };
            // Another file was asked for, or the preview closed, meanwhile.
            if ft.preview.as_ref().is_none_or(|p| p.path != path) {
                return;
            }
            let body = match r {
                Err(e) => PreviewBody::Failed(format!("{e:#}")),
                Ok(v) if v["binary"] == true => PreviewBody::Binary,
                Ok(v) if v["too_large"] == true => PreviewBody::TooLarge,
                Ok(v) => {
                    let text = s(&v, "text");
                    let language = language_of(&path);
                    PreviewBody::Text(cx.new(|cx| {
                        let mut st = EditorState::new(window, cx).language(language);
                        st.set_value(text, window, cx);
                        st
                    }))
                }
            };
            if let Some(p) = ft.preview.as_mut() {
                p.body = body;
            }
            cx.notify();
        });
    }

    /// Puts `@path ` into the thread's box, where the cursor is, and goes there.
    fn at_file(&mut self, thread: &str, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        // The box is under the conversation: not there while the thread shows as its terminal.
        if self.tui.contains(thread) {
            self.set_view(thread, false, cx);
        }
        let text = crate::composer::at_text(path);
        let input = self.composer(thread, window, cx).input.clone();
        input.update(cx, |st, cx| {
            let before = st.value()[..st.cursor().min(st.value().len())].chars().next_back();
            let lead = if before.is_some_and(|c| !c.is_whitespace()) { " " } else { "" };
            st.replace(format!("{lead}{text}"), window, cx);
        });
        window.focus(&input.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    /// The commit a merged thread went in as, read once from lilad's log of the thread.
    pub fn fetch_merged_sha(&mut self, thread: String, cx: &mut Context<Self>) {
        if self.panel.merged_sha.contains_key(&thread) {
            return;
        }
        self.panel.merged_sha.insert(thread.clone(), None);
        self.call("thread.log", json!({"thread": thread}), cx, move |this, r, cx| {
            // `2 commit(s) into main as f898670`
            let sha = r.ok().and_then(|v| v.as_array().and_then(|a| a.iter().find(|e| e["kind"] == "merged").and_then(|e| e["detail"].as_str().and_then(|d| d.rsplit_once(" as ").map(|(_, sha)| sha.trim().to_owned())))));
            this.panel.merged_sha.insert(thread, sha);
            cx.notify();
        });
    }

    // ---------------- doing ----------------

    /// Asks the agent to commit what changed.
    fn commit(&mut self, thread: String, window: &mut Window, cx: &mut Context<Self>) {
        let c = self.panel.changes.entry(thread.clone()).or_default();
        c.committing = true;
        c.failed = None;
        cx.notify();
        self.request_result("thread.commit", json!({"thread": thread}), window, cx, move |this, r, _, cx| {
            let c = this.panel.changes.entry(thread.clone()).or_default();
            c.committing = false;
            if let Err(e) = r {
                c.failed = Some(format!("{e:#}"));
            }
            this.refresh_now();
            this.fetch_changes(thread, cx);
            cx.notify();
        });
    }

    /// Merges a worktree thread back (after the person confirmed): it ends archived, and the window leaves it as it does an archived one.
    fn merge(&mut self, thread: String, window: &mut Window, cx: &mut Context<Self>) {
        self.panel.merge_confirm = None;
        let c = self.panel.changes.entry(thread.clone()).or_default();
        c.merging = true;
        c.failed = None;
        cx.notify();
        self.request_result("thread.merge", json!({"thread": thread}), window, cx, move |this, r, window, cx| {
            let c = this.panel.changes.entry(thread.clone()).or_default();
            c.merging = false;
            match r {
                Ok(v) => {
                    this.panel.merged_sha.insert(thread.clone(), v["commit"].as_str().map(String::from));
                    // Leaves the thread, as for an archived one: its workspace's new session.
                    if this.thread.as_deref() == Some(thread.as_str())
                        && let Some(project) = this.shown_thread().and_then(|t| t["project"].as_str().map(String::from))
                    {
                        this.show_new(project, window, cx);
                    }
                }
                Err(e) => {
                    c.failed = Some(format!("{e:#}"));
                    this.fetch_changes(thread, cx);
                }
            }
            this.refresh_now();
            cx.notify();
        });
    }

    // ---------------- the header's buttons ----------------

    /// The files and changes buttons of the header: pressed ones are filled. The changes button carries `+add −del` while there are changes.
    pub fn panel_buttons(&mut self, t: &Value, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if t["merged_ms"].is_number() {
            return Vec::new();
        }
        let quiet = style::quiet(cx);
        let on = |view: View| self.panel.open && self.panel.view == view;
        let tool = |id: &'static str, icon: &'static str, view: View| {
            let on = on(view);
            Button::new(id)
                .custom(quiet)
                .xsmall()
                .icon(style::icon(icon).text_color(if on { style::fg() } else { muted() }))
                .when(on, |b| b.bg(Hsla::from(rgba(0xffffff14))))
                .tooltip(match view {
                    View::Files => "文件 (⇧⌘E)",
                    View::Changes => "改动 (⇧⌘G)",
                })
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_panel(view, cx)))
        };
        let c = &t["changes"];
        let (add, del) = (c["add"].as_i64().unwrap_or(0), c["del"].as_i64().unwrap_or(0));
        let changed = c["files"].as_i64().unwrap_or(0) > 0;
        vec![tool("panel-files", "lila/folder-tree.svg", View::Files).into_any_element(), tool("panel-changes", "lila/git-compare.svg", View::Changes).when(changed, |b| b.child(counts(add, del))).into_any_element()]
    }

    // ---------------- the card ----------------

    /// The shown thread's card and, when the panel is open, the panel's card beside it, the gutter between them
    /// the resize handle. In a narrow window the panel gives way first (down to `MIN_WIDTH`), so the thread keeps `MIN_CENTER`.
    pub fn work(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let center = self.center(window, cx).into_any_element();
        let Some(t) = self.panel_thread() else { return center };
        let card = self.panel_card(&t, window, cx);
        let side = if self.sidebar_hidden { 0. } else { self.sidebar_size.read(cx).sizes().first().map(|w| f32::from(*w)).unwrap_or(260.) + style::GAP };
        let room = f32::from(window.viewport_size().width) - 2. * style::GAP - side - style::GAP - MIN_CENTER;
        let most = room.clamp(MIN_WIDTH, MAX_WIDTH);
        gpui::base::h_resizable("work")
            .with_state(&self.panel.size)
            .with_handle_appearance(crate::app::gutter())
            .child(gpui::component::resizable_panel().size_range(px(HARD_CENTER)..Pixels::MAX).child(div().size_full().pr(px(3.)).child(center)))
            .child(gpui::component::resizable_panel().size(px(self.panel.width.min(most))).size_range(px(MIN_WIDTH)..px(most)).fixed().child(div().size_full().pl(px(4.)).child(card)))
            .into_any_element()
    }

    /// The panel's card for the thread `t`.
    pub fn panel_card(&mut self, t: &Value, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let body = match self.panel.view {
            View::Changes => self.changes_view(t, window, cx),
            View::Files => self.files_view(t, cx),
        };
        card().size_full().flex().flex_col().child(body).into_any_element()
    }

    /// A first row like the shown thread's: as tall, a Divider under it, and the window moves by it.
    fn panel_bar(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        drag_bar(h_flex().id("panel-bar"), cx).h(px(BAR)).flex_none().px_3().gap_2().whitespace_nowrap().border_b_1().border_color(style::divider())
    }

    fn changes_view(&mut self, t: &Value, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let id = s(t, "id");
        let empty = Changes::default();
        let c = self.panel.changes.get(&id).unwrap_or(&empty);
        let data = c.data.clone();
        let (open, diffs) = (c.open.clone(), c.diffs.iter().map(|(p, d)| (p.clone(), match d {
            Diff::Loading => Diff::Loading,
            Diff::Ready(v) => Diff::Ready(v.clone()),
            Diff::Failed(e) => Diff::Failed(e.clone()),
        })).collect::<HashMap<_, _>>());
        let (commits_open, committing, merging_here, failed) = (c.commits_open, c.committing, c.merging, c.failed.clone());
        let error = c.error.clone();
        let bar = self.panel_bar(cx);
        let Some(d) = data else {
            let body = match error {
                Some(e) => note(e),
                None => div().flex_1().flex().items_center().justify_center().child(Spinner::new().color(muted())).into_any_element(),
            };
            return v_flex().size_full().child(bar).child(body).into_any_element();
        };
        if d["repo"] != true {
            return v_flex().size_full().child(bar).child(note("不是 git 仓库")).into_any_element();
        }
        let worktree = d["worktree"] == true;
        let base = d["base_branch"].as_str().map(String::from);
        let files: Vec<Value> = d["files"].as_array().cloned().unwrap_or_default();
        let commits: Vec<Value> = d["commits"].as_array().cloned().unwrap_or_default();
        let ahead = d["ahead"].as_i64().unwrap_or(commits.len() as i64);
        let state = s(t, "state");
        let busy_remote = t["merging"] == true;

        // First row: where the work is, and for a worktree thread where it goes.
        let branch = d["branch"].as_str().map(String::from).unwrap_or_else(|| "HEAD".to_owned());
        let bar = bar
            .child(style::icon("lila/git-branch.svg").size(px(14.)).text_color(muted()))
            .child(div().min_w_0().truncate().text_size(px(13.)).font_semibold().child(branch))
            .when_some(base.clone().filter(|_| worktree), |d, base| d.child(Icon::new(IconName::ArrowRight).size(px(12.)).text_color(muted2())).child(div().flex_none().max_w(relative(0.4)).truncate().text_size(px(13.)).font_semibold().child(base)))
            .child(div().flex_1())
            .when(ahead > 0, |d| {
                let thread = id.clone();
                d.child(
                    h_flex()
                        .id("panel-commits")
                        .flex_none()
                        .h(px(24.))
                        .px_1()
                        .gap_1()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|d| d.bg(style::hover()))
                        .text_size(px(12.))
                        .text_color(muted())
                        .child(format!("{ahead} 个提交"))
                        .child(Icon::new(if commits_open || files.is_empty() { IconName::ChevronUp } else { IconName::ChevronDown }).size(px(12.)).text_color(muted2()))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_commits(&thread, cx))),
                )
            });

        // The commits (a worktree thread): shown when asked for, and when there is nothing else to show.
        let commit_list = (ahead > 0 && (commits_open || files.is_empty()) && !commits.is_empty()).then(|| {
            v_flex().flex_none().px_2().py_1().border_b_1().border_color(style::divider()).children(commits.iter().enumerate().map(|(n, k)| {
                h_flex()
                    .id(SharedString::from(format!("commit-{n}")))
                    .h(px(24.))
                    .px_2()
                    .gap_2()
                    .child(div().flex_none().font_family(lila_term::theme::FONT).text_size(px(11.)).text_color(muted2()).child(s(k, "sha")))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(muted()).child(s(k, "subject")))
            }))
        });

        let mut rows: Vec<AnyElement> = Vec::new();
        for f in &files {
            rows.push(self.change_row(&id, f, &open, &diffs, window, cx));
        }
        let list = if rows.is_empty() && commits.is_empty() {
            note("没有改动")
        } else {
            v_flex().id("changes-scroll").flex_1().min_h_0().overflow_y_scrollbar().children(commit_list).child(v_flex().px_2().py_1().children(rows)).into_any_element()
        };

        // The bottom: the buttons, and why one did not go.
        let can_commit = !files.is_empty() && state == "idle" && !committing && !merging_here && !busy_remote;
        let can_merge = worktree && files.is_empty() && ahead > 0 && matches!(state.as_str(), "idle" | "ended") && !committing && !merging_here && !busy_remote;
        let merging = merging_here || busy_remote;
        let commit_id = id.clone();
        let merge_id = id.clone();
        let commit_button = Button::new("changes-commit")
            .small()
            .small_button()
            .when(worktree, |b| b.outline())
            .when(!worktree, |b| b.primary())
            .label("提交")
            .when(committing, |b| b.child(Spinner::new().xsmall().color(muted())))
            .disabled(!can_commit)
            .on_click(cx.listener(move |this, _, window, cx| this.commit(commit_id.clone(), window, cx)));
        let merge_button = base.filter(|_| worktree).map(|base| {
            Button::new("changes-merge")
                .small()
                .small_button()
                .primary()
                .label(format!("合并到 {base}"))
                .when(merging, |b| b.child(Spinner::new().xsmall().color(muted())))
                .disabled(!can_merge)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.panel.merge_confirm = Some(merge_id.clone());
                    cx.notify();
                }))
        });
        let bottom = v_flex()
            .flex_none()
            .gap_2()
            .px_3()
            .py_3()
            .border_t_1()
            .border_color(style::divider())
            .children(failed.map(|e| div().id("changes-failed").max_h(px(96.)).overflow_y_scroll().text_size(px(12.)).text_color(style::tint(style::red(), 0.2)).child(e)))
            .child(h_flex().gap_2().justify_end().child(commit_button).children(merge_button));
        v_flex().size_full().child(bar).child(list).child(bottom).into_any_element()
    }

    /// One changed file: its mark, name and counts, and below it, when opened, its diff.
    fn change_row(&mut self, thread: &str, f: &Value, open: &HashSet<String>, diffs: &HashMap<String, Diff>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let path = s(f, "path");
        let (letter, color) = mark(&s(f, "status"));
        let (add, del) = (f["add"].as_i64().unwrap_or(0), f["del"].as_i64().unwrap_or(0));
        let binary = f["binary"] == true;
        let can_open = !binary && add + del > 0;
        let is_open = can_open && open.contains(&path);
        let (dir, name) = split(&path);
        // A rename reads old → new; the folder follows in grey when it did not change.
        let (name, dir) = match f["old_path"].as_str() {
            Some(old) if split(old).0 == dir => (format!("{} → {name}", split(old).1), dir.to_owned()),
            Some(old) => (format!("{old} → {path}"), String::new()),
            None => (name.to_owned(), dir.to_owned()),
        };
        let (thread_id, toggle) = (thread.to_owned(), path.clone());
        let head = h_flex()
            .id(SharedString::from(format!("change-{path}")))
            .h(px(28.))
            .px_2()
            .gap_2()
            .rounded(px(6.))
            .when(can_open, |d| d.cursor_pointer().hover(|d| d.bg(style::hover())))
            .when(is_open, |d| d.bg(style::fill()))
            .child(div().w(px(12.)).flex_none().font_family(lila_term::theme::FONT).text_size(px(11.)).font_semibold().text_color(color).child(letter))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(6.))
                    .overflow_hidden()
                    .child(div().flex_none().max_w(relative(0.8)).truncate().text_size(px(13.)).child(name))
                    .when(!dir.is_empty(), |d| d.child(div().min_w_0().truncate().text_size(px(12.)).text_color(muted2()).child(dir))),
            )
            .when(binary, |d| d.child(div().flex_none().text_size(px(11.)).text_color(muted2()).child("二进制")))
            .when(!binary && add + del > 0, |d| d.child(counts(add, del)))
            .when(can_open, |d| d.child(Icon::new(if is_open { IconName::ChevronUp } else { IconName::ChevronDown }).size(px(12.)).text_color(muted2())))
            .when(can_open, |d| d.on_click(cx.listener(move |this, _, _, cx| this.toggle_diff(&thread_id, &toggle, cx))));
        let body = is_open.then(|| match diffs.get(&path) {
            Some(Diff::Ready(d)) if !d.is_null() => {
                let key = format!("panel-{thread}-{path}-{:x}", fingerprint(d));
                v_flex()
                    .gap_1()
                    .child(crate::conversation::diff_view(&key, std::slice::from_ref(d), window, cx))
                    .when(d["truncated"] == true, |v| v.child(div().text_size(px(12.)).text_color(muted2()).child("太长，只显示了前面一部分")))
                    .into_any_element()
            }
            Some(Diff::Failed(e)) => div().text_size(px(12.)).text_color(style::tint(style::red(), 0.2)).child(e.clone()).into_any_element(),
            Some(Diff::Ready(_)) => div().into_any_element(),
            _ => div().h(px(24.)).flex().items_center().child(Spinner::new().xsmall().color(muted())).into_any_element(),
        });
        v_flex().w_full().child(head).children(body.map(|b| div().px_2().pt_1().pb_2().child(b))).into_any_element()
    }

    fn files_view(&mut self, t: &Value, cx: &mut Context<Self>) -> AnyElement {
        let quiet = style::quiet(cx);
        let id = s(t, "id");
        let bar = self.panel_bar(cx);
        let Some(ft) = self.panel.files.get(&id) else {
            return v_flex().size_full().child(bar).child(div().flex_1().flex().items_center().justify_center().child(Spinner::new().color(muted()))).into_any_element();
        };
        if let Some(p) = &ft.preview {
            let (thread, path) = (id.clone(), p.path.clone());
            let (dir, name) = split(&path);
            let (dir, name) = (dir.to_owned(), name.to_owned());
            let (back_id, at_id, at_path) = (id.clone(), id.clone(), path.clone());
            let bar = bar
                .pl_2()
                .child(Button::new("preview-back").custom(quiet).xsmall().icon(Icon::new(IconName::ArrowLeft).text_color(muted())).tooltip("文件").on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(ft) = this.panel.files.get_mut(&back_id) {
                        ft.preview = None;
                    }
                    cx.notify();
                })))
                .child(
                    h_flex()
                        .id("preview-path")
                        .flex_1()
                        .min_w_0()
                        .gap(px(6.))
                        .overflow_hidden()
                        .tooltip(move |window, cx| gpui::component::tooltip::Tooltip::new(path.clone()).build(window, cx))
                        .child(div().flex_none().max_w(relative(0.8)).truncate().text_size(px(13.)).font_semibold().child(name))
                        .when(!dir.is_empty(), |d| d.child(div().min_w_0().truncate().text_size(px(12.)).text_color(muted2()).child(dir))),
                )
                .child(Button::new("preview-at").custom(quiet).xsmall().icon(style::icon("lila/at-sign.svg").text_color(muted())).tooltip("在输入框中引用").on_click(cx.listener(move |this, _, window, cx| this.at_file(&at_id, &at_path, window, cx))));
            let _ = thread;
            let body = match &p.body {
                PreviewBody::Loading => div().flex_1().flex().items_center().justify_center().child(Spinner::new().color(muted())).into_any_element(),
                PreviewBody::Binary => note("二进制文件"),
                PreviewBody::TooLarge => note("文件太大，不预览"),
                PreviewBody::Failed(e) => note(e.clone()),
                PreviewBody::Text(state) => div()
                    .flex_1()
                    .min_h_0()
                    .child(Editor::new(state).readonly(true).appearance(false).bordered(false).h(relative(1.)).font_family(lila_term::theme::FONT).text_size(px(12.)))
                    .into_any_element(),
            };
            return v_flex().size_full().child(bar).child(body).into_any_element();
        }
        let label = self.project(&s(t, "project")).map(|p| p.label()).unwrap_or_default();
        let bar = bar.child(Icon::new(IconName::FolderOpen).size(px(14.)).text_color(muted())).child(div().min_w_0().truncate().text_size(px(13.)).font_semibold().child(label));
        let body = match (&ft.error, ft.files.is_empty(), ft.loading) {
            (Some(e), true, _) => note(e.clone()),
            (None, true, true) => div().flex_1().flex().items_center().justify_center().child(Spinner::new().color(muted())).into_any_element(),
            (None, true, false) => note("没有文件"),
            _ => {
                let weak = cx.entity().downgrade();
                let thread = id.clone();
                div()
                    .flex_1()
                    .min_h_0()
                    .px_1()
                    .py_1()
                    .child(tree(&ft.state, move |_, entry, _selected, _, _| {
                        let item = entry.item();
                        let folder = entry.is_folder();
                        let path = item.id.to_string();
                        let (weak, thread) = (weak.clone(), thread.clone());
                        let icon = if folder {
                            Icon::new(if entry.is_expanded() { IconName::FolderOpen } else { IconName::Folder })
                        } else {
                            Icon::new(IconName::FileText)
                        };
                        let chevron: AnyElement = if folder {
                            Icon::new(if entry.is_expanded() { IconName::ChevronDown } else { IconName::ChevronRight }).size(px(12.)).text_color(muted2()).into_any_element()
                        } else {
                            div().size(px(12.)).into_any_element()
                        };
                        ListItem::new(SharedString::from(format!("file-{path}")))
                            .h(px(26.))
                            .py_0()
                            .pl(px(6. + entry.depth() as f32 * 14.))
                            .pr_2()
                            .rounded(px(6.))
                            .text_size(px(13.))
                            .on_click(move |_, window, cx| {
                                if !folder {
                                    let _ = weak.update(cx, |this, cx| this.open_preview(thread.clone(), path.clone(), window, cx));
                                }
                            })
                            .child(h_flex().gap(px(6.)).items_center().child(chevron).child(icon.size(px(14.)).text_color(muted())).child(div().min_w_0().truncate().child(item.label.clone())))
                    }))
                    .into_any_element()
            }
        };
        v_flex().size_full().child(bar).child(body).into_any_element()
    }

    /// The question before a merge: what it does to the session and the worktree.
    pub fn merge_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let quiet = style::quiet(cx);
        let id = self.panel.merge_confirm.clone()?;
        let t = self.snap.threads.iter().find(|t| t["id"] == id.as_str())?;
        let base = s(t, "base_branch");
        let n = self.panel.changes.get(&id).and_then(|c| c.data.as_ref()).and_then(|d| d["ahead"].as_i64());
        let text = format!("{}压成一个提交并入 {base}。会话归档，worktree 和分支删除。", n.map(|n| format!("lila/{id} 的 {n} 个提交")).unwrap_or_else(|| format!("lila/{id} 的提交")));
        let go = id.clone();
        Some(crate::dialog::small(
            "merge-dialog",
            format!("合并到 {base}？"),
            Some(text),
            None,
            vec![
                Button::new("merge-no").custom(quiet).label("取消").on_click(cx.listener(|this, _, _, cx| {
                    this.panel.merge_confirm = None;
                    cx.notify();
                }))
                .into_any_element(),
                Button::new("merge-yes").primary().label("合并").on_click(cx.listener(move |this, _, window, cx| this.merge(go.clone(), window, cx))).into_any_element(),
            ],
            Box::new(|this, _| this.panel.merge_confirm = None),
            cx,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{View, language_of, split, tree_items};
    use gpui::SharedString;
    use gpui::component::tree::TreeItem;
    use std::collections::HashSet;

    fn ids(items: &[TreeItem]) -> Vec<(String, bool)> {
        items.iter().map(|i| (i.id.to_string(), i.is_folder())).collect()
    }

    #[test]
    fn the_tree_is_folders_first_then_files_each_by_name() {
        let files: Vec<String> = ["README.md", "src/main.rs", "src/ui/b.rs", "Cargo.toml", "docs/a.md", "src/Lib.rs"].iter().map(|s| s.to_string()).collect();
        let items = tree_items(&files, &HashSet::new());
        assert_eq!(ids(&items), [("docs".into(), true), ("src".into(), true), ("Cargo.toml".into(), false), ("README.md".into(), false)]);
        let src = &items[1];
        assert_eq!(ids(&src.children), [("src/ui".into(), true), ("src/Lib.rs".into(), false), ("src/main.rs".into(), false)]);
        assert_eq!(ids(&src.children[0].children), [("src/ui/b.rs".into(), false)]);
        // Closed at first; a folder that was open stays open.
        assert!(items.iter().all(|i| !i.is_expanded()));
        let open: HashSet<SharedString> = ["src".into()].into_iter().collect();
        assert!(tree_items(&files, &open)[1].is_expanded());
    }

    #[test]
    fn a_file_is_colored_by_its_extension_or_its_name() {
        assert_eq!(language_of("src/main.RS"), "rs");
        assert_eq!(language_of("Makefile"), "Makefile");
        assert_eq!(language_of("a/b/Dockerfile"), "Dockerfile");
        assert_eq!(language_of(".gitignore"), ".gitignore");
        assert_eq!(language_of("Cargo.lock"), "toml");
        assert_eq!(language_of("x.tar.gz"), "gz");
    }

    #[test]
    fn a_view_is_kept_by_its_key() {
        for v in [View::Files, View::Changes] {
            assert_eq!(View::of(v.key()), Some(v));
        }
        assert_eq!(View::of("git"), None);
    }

    #[test]
    fn a_path_is_a_folder_and_a_name() {
        assert_eq!(split("a/b/c.rs"), ("a/b", "c.rs"));
        assert_eq!(split("c.rs"), ("", "c.rs"));
    }
}
