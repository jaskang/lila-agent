//! The main window: team on the left, a member's terminal in the middle,
//! what needs you, the task board and recent activity on the right.

use crate::data::{self, Project, Snapshot};
use crate::ui::s;
use gpui::component::input::InputState;
use gpui::component::{ActiveTheme, h_resizable, resizable_panel, v_flex};
use gpui::prelude::*;
use gpui::*;
use lila::paths::Paths;
use lila_term::{Core, TerminalView};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

pub struct LilaApp {
    paths: Paths,
    pub snap: Snapshot,
    /// The project shown: one of `tabs`, or none.
    pub selected: Option<String>,
    /// The projects open as tabs, in order.
    pub tabs: Vec<String>,
    /// The saved tabs were restored (after the first snapshot).
    restored: bool,
    /// The project settings form, while open.
    pub settings: Option<crate::settings::Form>,
    /// The global settings (role library), while open.
    pub prefs: Option<crate::prefs::Prefs>,
    /// Where the open dialog was dragged to, from the window's center.
    pub dialog_offset: Point<Pixels>,
    /// While dragging a dialog: where the press was and the offset then.
    pub dialog_drag: Option<(Point<Pixels>, Point<Pixels>)>,
    /// The member whose terminal fills the middle; `None` is the owner.
    pub member: Option<String>,
    core: Option<Arc<Core>>,
    pub core_error: Option<String>,
    terminals: HashMap<String, Entity<TerminalView>>,
    pub answers: HashMap<String, Entity<InputState>>,
    /// The last action's error, shown until the next one.
    pub notice: Option<String>,
    /// The side panel (inbox, tasks, activity) is shown.
    pub right_open: bool,
    /// A press on a top bar that becomes a window drag once the mouse moves.
    pub drag_armed: bool,
    /// The task open in the detail panel.
    pub task: Option<String>,
    append: Option<Entity<InputState>>,
    /// The task whose "cancel" was pressed once (press again to confirm).
    pub confirm_cancel: Option<String>,
    /// The new-task form, while open.
    pub draft: Option<crate::compose::Draft>,
    /// A queued task being rewritten in the detail panel.
    pub rewrite: Option<crate::compose::Rewrite>,
    /// Refreshes now, without waiting for lilad to say something changed.
    kick: async_channel::Sender<()>,
    /// The last snapshot of each project, shown at once when switching back to it.
    cache: HashMap<String, Snapshot>,
    _poll: Task<()>,
}

impl LilaApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let paths = Paths::from_env();
        // lilad says when anything changed; refresh then. A slow poll covers a
        // daemon that is down or restarting.
        let (wake_tx, wake) = async_channel::unbounded::<()>();
        let (kick, kicked) = async_channel::unbounded::<()>();
        let watch_paths = paths.clone();
        std::thread::spawn(move || {
            loop {
                if let Ok(events) = lila::client::subscribe(&watch_paths) {
                    let _ = wake_tx.send_blocking(());
                    for _ in events {
                        if wake_tx.send_blocking(()).is_err() {
                            return;
                        }
                    }
                }
                if wake_tx.is_closed() {
                    return;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        });
        let poll = cx.spawn_in(window, async move |this, cx| {
            loop {
                let Ok((paths, selected, task)) = this.update(cx, |this, _| (this.paths.clone(), this.selected.clone(), this.task.clone())) else { return };
                let snap = cx.background_executor().spawn(async move { data::fetch(&paths, selected.as_deref(), task.as_deref()) }).await;
                if std::env::var_os("LILA_APP_DEBUG").is_some() {
                    eprintln!("[lila-app] {} refreshed {}: {} tasks", data::clock_ms(), snap.project.as_deref().unwrap_or("-"), snap.tasks.len());
                }
                if this.update(cx, |this, cx| this.apply(snap, cx)).is_err() {
                    return;
                }
                if kicked.try_recv().is_ok() {
                    while kicked.try_recv().is_ok() {}
                    continue;
                }
                let woken = std::pin::pin!(wake.recv());
                let asked = std::pin::pin!(kicked.recv());
                let timeout = std::pin::pin!(cx.background_executor().timer(Duration::from_secs(5)));
                match futures::future::select(futures::future::select(woken, asked), timeout).await {
                    futures::future::Either::Left((futures::future::Either::Right(_), _)) => {}
                    _ => {
                        // Changes come in bursts (a delivery, then a status): take them together.
                        cx.background_executor().timer(Duration::from_millis(80)).await;
                    }
                }
                while wake.try_recv().is_ok() {}
                while kicked.try_recv().is_ok() {}
            }
        });
        cx.spawn(async move |this, cx| {
            let core = cx.background_executor().spawn(async { Core::connect() }).await;
            let _ = this.update(cx, |this, cx| {
                match core {
                    Ok(c) => this.core = Some(c),
                    Err(e) => this.core_error = Some(format!("cannot connect to herdr-core: {e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        LilaApp { paths, snap: Snapshot::default(), selected: None, tabs: Vec::new(), restored: false, settings: None, prefs: None, dialog_offset: Point::default(), dialog_drag: None, member: None, core: None, core_error: None, terminals: HashMap::new(), answers: HashMap::new(), notice: None, right_open: true, drag_armed: false, task: None, append: None, confirm_cancel: None, draft: None, rewrite: None, kick, cache: HashMap::new(), _poll: poll }
    }

    fn apply(&mut self, snap: Snapshot, cx: &mut Context<Self>) {
        // First look: the tabs open last time (else the first project).
        if !self.restored {
            self.restored = true;
            let saved = data::load_tabs(&self.paths);
            let known = |id: &String| snap.projects.iter().any(|p| &p.id == id);
            self.tabs = match &saved {
                Some(t) => t.open.iter().filter(|id| known(id)).cloned().collect(),
                None => snap.projects.first().map(|p| vec![p.id.clone()]).unwrap_or_default(),
            };
            let pick = saved.and_then(|t| t.selected).filter(|id| self.tabs.contains(id)).or_else(|| self.tabs.first().cloned());
            self.snap.projects = snap.projects.clone();
            if let Some(id) = pick {
                self.select_project(id, cx);
                return;
            }
        }
        // A project that went away closes its tab.
        let before = self.tabs.len();
        self.tabs.retain(|id| snap.projects.iter().any(|p| &p.id == id));
        if self.tabs.len() != before {
            self.save_tabs();
        }
        if let Some(id) = &snap.project {
            self.cache.insert(id.clone(), Snapshot { detail: None, ..snap.clone() });
            // Fetched for the project shown before a switch: keep it for later, fetch the shown one.
            if snap.project != self.selected {
                self.refresh_now();
                return;
            }
        }
        let open: Vec<String> = snap.inbox.iter().map(|i| s(i, "id")).collect();
        self.answers.retain(|id, _| open.contains(id));
        // Something new needs the person: bring the inbox forward.
        if snap.project == self.snap.project && snap.inbox.iter().any(|i| !self.snap.inbox.iter().any(|o| o["id"] == i["id"])) && snap.status.is_some() {
            self.right_open = true;
        }
        // A detail fetched for another task (the panel moved on meanwhile) is dropped.
        let mut snap = snap;
        if snap.detail.as_ref().and_then(|d| d["id"].as_str()) != self.task.as_deref() {
            snap.detail = self.snap.detail.take().filter(|d| d["id"].as_str() == self.task.as_deref());
        }
        self.snap = snap;
        cx.notify();
    }

    /// Fetches everything again right away.
    pub fn refresh_now(&self) {
        let _ = self.kick.try_send(());
    }

    fn save_tabs(&self) {
        data::save_tabs(&self.paths, &data::Tabs { open: self.tabs.clone(), selected: self.selected.clone() });
    }

    /// Opens a project from the sidebar: its tab (added after the others) comes forward.
    pub fn open_project(&mut self, id: String, cx: &mut Context<Self>) {
        if !self.tabs.contains(&id) {
            self.tabs.push(id.clone());
        }
        self.select_project(id, cx);
    }

    /// Closes a project's tab (its team keeps running). The tab beside it comes forward.
    pub fn close_tab(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(ix) = self.tabs.iter().position(|t| t == id) else { return };
        self.tabs.remove(ix);
        if self.selected.as_deref() == Some(id) {
            match self.tabs.get(ix).or(self.tabs.last()).cloned() {
                Some(next) => self.select_project(next, cx),
                None => {
                    self.member = None;
                    self.close_task(cx);
                    self.draft = None;
                    self.settings = None;
                    self.selected = None;
                    self.snap = Snapshot { daemon: self.snap.daemon, projects: std::mem::take(&mut self.snap.projects), ..Default::default() };
                }
            }
        }
        self.save_tabs();
        cx.notify();
    }

    /// Shows another project: its last snapshot at once, then fresh data.
    pub fn select_project(&mut self, id: String, cx: &mut Context<Self>) {
        if !self.tabs.contains(&id) {
            self.tabs.push(id.clone());
        }
        if self.selected.as_deref() == Some(&id) {
            self.refresh_now();
            return;
        }
        self.settings = None;
        self.member = None;
        self.close_task(cx);
        self.draft = None;
        let cached = self.cache.get(&id).cloned().unwrap_or_else(|| Snapshot { project: Some(id.clone()), ..Default::default() });
        self.snap = Snapshot { daemon: self.snap.daemon, projects: std::mem::take(&mut self.snap.projects), ..cached };
        self.selected = Some(id);
        self.save_tabs();
        self.refresh_now();
        cx.notify();
    }

    /// A project's state for its tab, from the last time it was fetched: the
    /// most pressing member status (none while all are idle) and open inbox items.
    pub fn project_summary(&self, id: &str) -> (Option<&'static str>, usize) {
        let snap = if self.snap.project.as_deref() == Some(id) { Some(&self.snap) } else { self.cache.get(id) };
        let Some(snap) = snap else { return (None, 0) };
        let members = snap.status.as_ref().and_then(|s| s["members"].as_array());
        let has = |st: &str| members.is_some_and(|m| m.iter().any(|m| m["status"] == st));
        let status = if has("blocked") || has("error") {
            Some("blocked")
        } else if has("working") {
            Some("working")
        } else {
            None
        };
        (status, snap.inbox.len())
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    pub fn project(&self) -> Option<Project> {
        let id = self.selected.as_deref()?;
        self.snap.projects.iter().find(|p| p.id == id).cloned()
    }

    pub fn members(&self) -> Vec<Value> {
        self.snap.status.as_ref().and_then(|s| s["members"].as_array().cloned()).unwrap_or_default()
    }

    /// The member shown in the middle: the chosen one, else the owner.
    pub fn shown_member(&self) -> Option<Value> {
        let members = self.members();
        let chosen = self.member.as_ref().and_then(|id| members.iter().find(|m| m["id"] == id.as_str()).cloned());
        chosen.or_else(|| members.iter().find(|m| m["owner"] == true).cloned())
    }

    /// Runs one request off the UI thread, then refreshes.
    pub fn act(&mut self, method: &'static str, params: Value, autostart: bool, cx: &mut Context<Self>) {
        let Some(project) = self.project() else { return };
        let paths = self.paths.clone();
        self.notice = None;
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { data::act(&paths, &project, method, params, autostart) }).await;
            let _ = this.update(cx, |this, cx| {
                if let Err(e) = r {
                    this.notice = Some(format!("{e:#}"));
                }
                this.refresh_now();
                cx.notify();
            });
        })
        .detach();
    }

    /// Like `act`, then `done` with the result. Errors go to the open form, else the notice.
    pub fn request(&mut self, method: &'static str, params: Value, cx: &mut Context<Self>, done: impl FnOnce(&mut Self, Value, &mut Context<Self>) + 'static) {
        let Some(project) = self.project() else { return };
        let paths = self.paths.clone();
        self.notice = None;
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { data::act(&paths, &project, method, params, false) }).await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(v) => done(this, v, cx),
                    Err(e) => match &mut this.draft {
                        Some(d) => d.error = Some(format!("{e:#}")),
                        None => this.notice = Some(format!("{e:#}")),
                    },
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn open_task(&mut self, id: String, _window: &mut Window, cx: &mut Context<Self>) {
        self.show_task(id, cx);
    }

    /// Shows a task in the detail panel; its details load with the next poll (and now).
    pub fn show_task(&mut self, id: String, cx: &mut Context<Self>) {
        self.draft = None;
        if self.task.as_deref() != Some(&id) {
            self.snap.detail = None;
            self.confirm_cancel = None;
            self.rewrite = None;
        }
        self.task = Some(id.clone());
        let (paths, project) = (self.paths.clone(), self.project());
        cx.spawn(async move |this, cx| {
            let Some(p) = project else { return };
            let v = cx.background_executor().spawn(async move { data::act(&paths, &p, "task.show", json!({"id": id}), false) }).await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(v) = v
                    && this.task.as_deref() == v["id"].as_str()
                {
                    this.snap.detail = Some(v);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn close_task(&mut self, cx: &mut Context<Self>) {
        self.task = None;
        self.snap.detail = None;
        self.confirm_cancel = None;
        self.rewrite = None;
        cx.notify();
    }

    pub fn append_input(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        self.append.get_or_insert_with(|| cx.new(|cx| InputState::new(window, cx).placeholder("补充要求或修正…"))).clone()
    }

    /// Pick a folder, make it a project if needed, start its team.
    pub fn new_project(&mut self, cx: &mut Context<Self>) {
        let paths = self.paths.clone();
        let picked = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("用这个文件夹".into()) });
        self.notice = None;
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(dirs))) = picked.await else { return };
            let Some(dir) = dirs.into_iter().next() else { return };
            let r = cx.background_executor().spawn(async move { data::create_and_start(&paths, &dir) }).await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(id) if !id.is_empty() => this.select_project(id, cx),
                    Ok(_) => {}
                    Err(e) => this.notice = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn answer(&mut self, id: String, text: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(input) = self.answers.get(&id) {
            input.update(cx, |s, cx| s.set_value("", window, cx));
        }
        // Gone at once, so a quick second click has nothing to press.
        self.snap.inbox.retain(|i| i["id"] != id.as_str());
        cx.notify();
        self.act("answer", json!({"id": id, "text": text}), false, cx);
    }

    pub fn terminal(&mut self, address: &str, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<TerminalView>> {
        let core = self.core.clone()?;
        // A view whose stream ended while its member still runs (a lost
        // connection) is replaced on the next look.
        let alive = self.members().iter().any(|m| m["terminal"] == address && m["status"] != "gone");
        let stale = self.terminals.get(address).is_some_and(|v| v.read(cx).ended().is_some() && alive);
        if stale || !self.terminals.contains_key(address) {
            let cwd = self.project().map(|p| std::path::PathBuf::from(p.cwd));
            let view = cx.new(|cx| {
                let mut view = TerminalView::new(core, address.to_owned(), false, window, cx);
                if let Some(cwd) = cwd {
                    view.set_working_directory(cwd);
                }
                view
            });
            self.terminals.insert(address.to_owned(), view);
        }
        self.terminals.get(address).cloned()
    }
}

impl Render for LilaApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let compose = self.compose_panel(cx);
        let dialog = self.settings_dialog(cx).or_else(|| self.prefs_dialog(cx));
        let detail = if compose.is_some() { None } else { self.task.as_ref().and(self.snap.detail.clone()) };
        let loading = compose.is_none() && self.task.is_some() && detail.is_none();
        let overlay = |d: Div| d.absolute().top_0().right_0().bottom(px(26.)).w(px(600.)).bg(theme.popover).border_l_1().border_color(theme.border).shadow_lg().occlude();
        v_flex()
            .size_full()
            .relative()
            .bg(theme.sidebar)
            .text_color(theme.foreground)
            .on_key_down(cx.listener(Self::shortcut))
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("main")
                        .child(resizable_panel().size(px(250.)).size_range(px(200.)..px(420.)).fixed().child(self.sidebar(cx).into_any_element()))
                        .child(resizable_panel().child(self.center(window, cx).into_any_element()))
                        .child(resizable_panel().visible(self.right_open).size(px(360.)).size_range(px(300.)..px(640.)).fixed().child(self.right(window, cx).into_any_element())),
                ),
            )
            .child(self.status_bar(cx))
            .children(compose.map(|c| overlay(div()).child(c)))
            .when_some(detail, |d, t| d.child(self.detail_panel(t, window, cx)))
            .when(loading, |d| d.child(overlay(div()).flex().items_center().justify_center().text_color(theme.muted_foreground).child("正在读取任务…")))
            .children(dialog)
    }
}

impl LilaApp {
    /// Window-wide keys. The terminal handles copy, paste and find; other ⌘ chords bubble here.
    fn shortcut(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        if k.key == "escape" && (self.settings.is_some() || self.prefs.is_some()) {
            self.settings = None;
            self.prefs = None;
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if k.key == "escape" && (self.task.is_some() || self.draft.is_some()) {
            self.draft = None;
            self.close_task(cx);
            cx.stop_propagation();
            return;
        }
        if !k.modifiers.platform || k.modifiers.control || k.modifiers.alt {
            return;
        }
        // ⇧⌘, the project's settings; ⌘, Lila's.
        if k.modifiers.shift {
            if matches!(k.key.as_str(), "," | "<") && self.project().is_some() {
                self.open_settings(window, cx);
                cx.stop_propagation();
            }
            return;
        }
        match k.key.as_str() {
            "n" if self.project().is_some() => self.compose(None, None, window, cx),
            "w" => match self.selected.clone() {
                Some(id) => self.close_tab(&id, cx),
                None => return,
            },
            "," => self.open_prefs(crate::prefs::PrefPage::Roles, window, cx),
            "i" => {
                self.right_open = !self.right_open;
                cx.notify();
            }
            d if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() && d != "0" => {
                let n = (d.as_bytes()[0] - b'1') as usize;
                if let Some(id) = self.tabs.get(n).cloned() {
                    self.select_project(id, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }
}
