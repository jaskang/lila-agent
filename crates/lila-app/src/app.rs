//! The main window: workspaces (projects) and their threads on the left; the shown thread's
//! conversation in the middle. The window talks to lilad only
//! through its socket; nothing here decides anything.

use crate::data::{self, Project, Snapshot};
use gpui::component::{resizable_panel, v_flex};
use gpui::prelude::*;
use gpui::*;
use lila::paths::Paths;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::Duration;

/// What the person chose for a new session, over the project's defaults. A choice is kept
/// until the session is started.
#[derive(Clone, Default, Debug)]
pub struct Draft {
    /// A model other than the project's (empty: omp's own default).
    pub model: Option<String>,
    pub permission: Option<String>,
    /// The session gets a worktree of its own (only offered where the folder is a repository).
    pub worktree: bool,
}

impl Draft {
    /// What a session started now would run: `(model, permission)`; the model is the project's, empty for omp's own default.
    pub fn effective(&self, p: &Project) -> (String, String) {
        (self.model.clone().unwrap_or_else(|| p.model.clone()), self.permission.clone().unwrap_or_else(|| p.permission.clone()))
    }
}

/// What a notification says when a thread went from `was` to `state`, or nothing: it began waiting for
/// the person (a card), or finished a turn (`recently`: it said so a moment ago, so a state that flickers is not repeated).
fn what_to_say(was: Option<&str>, state: &str, recently: bool) -> Option<&'static str> {
    match (was, state) {
        (w, "waiting") if w != Some("waiting") => Some("要你允许"),
        (Some("running"), "idle") if !recently => Some("这一轮做完了"),
        _ => None,
    }
}

/// Where the restart of a lilad that speaks another protocol stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Restart {
    #[default]
    Idle,
    /// Running in the background.
    Running,
    /// Done, and lilad still differs: the `lila` on disk is the old one, so another restart changes nothing.
    StillOld,
}

/// The rename dialog.
pub struct Rename {
    pub thread: String,
    pub input: Entity<gpui::component::input::InputState>,
}

pub struct LilaApp {
    paths: Paths,
    /// The window's own focus. Keys only reach `shortcut` along the focused
    /// element's path, so with nothing focused the window holds focus itself,
    /// or ⌘, and the other shortcuts would go nowhere.
    focus: FocusHandle,
    pub snap: Snapshot,
    /// The restart of a lilad of another protocol (the sidebar's notice).
    pub restart: Restart,
    /// The thread shown, or none.
    pub thread: Option<String>,
    /// While no thread is shown: the project whose new-session box is (`composer` key `new:<project>`).
    pub new_in: Option<String>,
    /// What the person changed for the next new session of a project (the rest is the project's defaults).
    pub drafts: HashMap<String, Draft>,
    /// The project shown last time, for the first look.
    last_project: Option<String>,
    /// The thread shown last time was restored (after the first snapshot).
    restored: bool,
    /// The project settings (⇧⌘,), while open.
    pub settings: Option<crate::settings::Form>,
    /// The thread being renamed, while its dialog is open.
    pub renaming: Option<Rename>,
    /// The models omp offers, `(id, name)`, asked once when a choice of them is first shown (`ensure_models`).
    pub models: Vec<(String, String)>,
    pub models_asked: bool,
    /// The boxes the question cards take an answer in, by card (`ui::question_card`).
    pub question_inputs: std::collections::HashMap<String, Entity<gpui::component::input::InputState>>,
    /// Projects folded in the sidebar, and those showing every thread instead of the latest few.
    pub folded: std::collections::HashSet<String>,
    pub unfolded: std::collections::HashSet<String>,
    /// Workspaces whose archived threads the sidebar lists too (their right-click menu).
    pub archived_shown: std::collections::HashSet<String>,
    /// The sidebar is folded away (kept across launches); its width is kept in `sidebar_size`.
    pub sidebar_hidden: bool,
    pub sidebar_size: Entity<gpui::base::ResizableState>,
    /// The window is the one in front (notifications are for when it is not).
    active: bool,
    /// Each thread's state at the last look, to tell what changed; none before the first look.
    seen: Option<HashMap<String, (String, bool)>>,
    /// The thread just made by a first message, until a list that has it arrives: a list fetched before it
    /// was made must not read as "the shown thread is gone".
    pub just_made: Option<String>,
    /// When each thread last notified that a turn was done, so a flickering state doesn't repeat it.
    notified: HashMap<String, i64>,
    /// The right panel: files and changes of the shown thread.
    pub panel: crate::panel::Panel,
    /// Going back to a message: the question being asked, and what is under way.
    pub restore: crate::restore::Restore,
    /// The global settings (about), while open.
    pub prefs: bool,
    /// Where the open dialog was dragged to, from the window's center.
    pub dialog_offset: Point<Pixels>,
    /// While dragging a dialog: where the press was and the offset then.
    pub dialog_drag: Option<(Point<Pixels>, Point<Pixels>)>,
    /// The last action's error, shown until the next one.
    pub notice: Option<String>,
    /// A press on a top bar that becomes a window drag once the mouse moves.
    pub drag_armed: bool,
    /// The project "remove from the list" asks about.
    pub removing: Option<Project>,
    /// The shown thread's conversation, as far as it was read.
    pub conv: Option<crate::conversation::Conv>,
    /// Messages just sent to a thread that its record does not show yet.
    pub pending: Vec<crate::conversation::Pending>,
    pub next_pending: u64,
    /// Tool calls opened to show their output.
    pub conv_open: std::collections::HashSet<String>,
    /// The conversation's virtual list, and what it was last told it holds (`conversation::sync_conv`).
    pub conv_list: Entity<gpui::component::message_scroller::MessageScrollerState>,
    pub conv_rows: Vec<crate::conversation::Row>,
    pub conv_key: Option<(String, Option<String>)>,
    /// The box under each thread's conversation, by thread id: its draft, files and menu.
    pub composers: HashMap<String, crate::composer::Composer>,
    /// What the `@` and `/` menus list for a thread, as lilad last said.
    pub menu_files: HashMap<String, Vec<String>>,
    pub menu_commands: HashMap<String, crate::composer::Commands>,
    /// Refreshes now, without waiting for lilad to say something changed.
    kick: async_channel::Sender<()>,
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
                let Ok((paths, want)) = this.update(cx, |this, _| (this.paths.clone(), this.conv_want())) else { return };
                let snap = cx.background_executor().spawn(async move { data::fetch(&paths, want.as_ref()) }).await;
                if std::env::var_os("LILA_APP_DEBUG").is_some() {
                    eprintln!("[lila-app] {} refreshed: {} projects, {} threads", data::clock_ms(), snap.projects.len(), snap.threads.len());
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
        // lilad is the only writer: the window starts it when it is not running.
        let start = paths.clone();
        // `LILA_APP_FAKE` (screenshots) stands in for lilad: none is started.
        let fake = cfg!(feature = "shots") && std::env::var_os("LILA_APP_FAKE").is_some();
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move { if fake || lila::client::is_running(&start) { Ok(()) } else { lila::client::start_daemon(&start) } })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Err(e) = r {
                    this.notice = Some(format!("启动不了 lilad：{e:#}"));
                }
                this.refresh_now();
                cx.notify();
            });
        })
        .detach();
        let sidebar_hidden = data::load_sidebar_hidden(&paths);
        let panel = crate::panel::Panel::new(&paths, cx);
        let mut app = LilaApp {
            paths,
            focus: cx.focus_handle(),
            snap: Snapshot::default(),
            restart: Restart::Idle,
            thread: None,
            new_in: None,
            drafts: HashMap::new(),
            last_project: None,
            restored: false,
            settings: None,
            renaming: None,
            models: Vec::new(),
            models_asked: false,
            question_inputs: std::collections::HashMap::new(),
            folded: Default::default(),
            unfolded: Default::default(),
            archived_shown: Default::default(),
            sidebar_hidden,
            sidebar_size: cx.new(|_| Default::default()),
            active: true,
            seen: None,
            just_made: None,
            notified: HashMap::new(),
            panel,
            restore: Default::default(),
            prefs: false,
            dialog_offset: Point::default(),
            dialog_drag: None,
            notice: None,
            drag_armed: false,
            removing: None,
            conv: None,
            pending: Vec::new(),
            next_pending: 0,
            conv_open: Default::default(),
            conv_list: Self::conv_list_state(cx),
            conv_rows: Vec::new(),
            conv_key: None,
            composers: HashMap::new(),
            menu_files: HashMap::new(),
            menu_commands: HashMap::new(),
            kick,
            _poll: poll,
        };
        cx.observe_window_activation(window, |this, window, cx| {
            // `--features shots`: `LILA_APP_INACTIVE=1` acts as a window that is never in front.
            this.active = window.is_window_active() && !(cfg!(feature = "shots") && std::env::var_os("LILA_APP_INACTIVE").is_some());
            cx.notify();
        })
        .detach();
        if cfg!(feature = "shots") && std::env::var_os("LILA_APP_INACTIVE").is_some() {
            app.active = false;
        }
        // Pictures pasted into boxes long ago go.
        let paths = app.paths.clone();
        cx.background_executor().spawn(async move { crate::composer::prune(&paths) }).detach();
        app
    }

    /// Restarts lilad (its sessions keep running) off the UI thread, then looks again.
    pub fn restart_lilad(&mut self, cx: &mut Context<Self>) {
        if self.restart == Restart::Running {
            return;
        }
        self.restart = Restart::Running;
        self.notice = None;
        let paths = self.paths.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { data::restart_daemon(&paths) }).await;
            let _ = this.update(cx, |this, cx| {
                this.restart = match r {
                    Ok(true) => Restart::StillOld,
                    Ok(false) => Restart::Idle,
                    Err(e) => {
                        this.notice = Some(format!("重启不了 lilad：{e:#}"));
                        Restart::Idle
                    }
                };
                this.refresh_now();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn apply(&mut self, snap: Snapshot, cx: &mut Context<Self>) {
        let mut snap = snap;
        // A lilad that matches again ends "the `lila` on disk is old".
        if self.restart == Restart::StillOld && !snap.protocol_differs() {
            self.restart = Restart::Idle;
        }
        // Lilad is not there (starting, or restarting): keep what is shown.
        if !snap.daemon {
            self.snap.daemon = false;
            cx.notify();
            return;
        }
        // First look: the thread (or project) shown last time.
        if !self.restored && snap.error.is_none() {
            self.restored = true;
            let (thread, project) = data::load_last(&self.paths);
            self.last_project = project;
            if let Some(id) = thread.filter(|id| snap.threads.iter().any(|t| t["id"] == id.as_str())) {
                self.thread = Some(id);
                self.refresh_now();
            }
        }
        if self.just_made.as_ref().is_some_and(|id| snap.threads.iter().any(|t| t["id"] == id.as_str())) {
            self.just_made = None;
        }
        // The shown thread is gone (its project was removed): none.
        if let Some(id) = self.thread.clone().filter(|id| self.just_made.as_ref() != Some(id))
            && snap.error.is_none()
            && !snap.threads.iter().any(|t| t["id"] == id.as_str())
            && !snap.threads.is_empty()
        {
            self.thread = None;
            self.conv = None;
        }
        // The shown thread was archived (and its workspace does not list archived ones): the workspace's new session instead.
        if let Some(t) = self.thread.as_ref().and_then(|id| snap.threads.iter().find(|t| t["id"] == id.as_str()))
            && t["archived"] == true
            && !t["project"].as_str().is_some_and(|p| self.archived_shown.contains(p))
        {
            self.new_in = t["project"].as_str().map(String::from);
            self.thread = None;
            self.conv = None;
        }
        if let Some(c) = snap.conversation.take() {
            self.take_conv(c);
        }
        if snap.error.is_none() {
            self.notify_changes(&snap);
            // Without a thread, the window shows a new session's box: there is always something to type into.
            if self.thread.is_none() && !self.new_in.as_ref().is_some_and(|p| snap.projects.iter().any(|x| x.id == *p)) {
                self.new_in = self.last_project.clone().filter(|p| snap.projects.iter().any(|x| x.id == *p)).or_else(|| snap.projects.first().map(|p| p.id.clone()));
            }
            // A project settings dialog for a project that is gone closes.
            if self.settings.as_ref().is_some_and(|f| !snap.projects.iter().any(|p| p.id == f.project.id)) {
                self.settings = None;
            }
        }
        self.snap = snap;
        self.panel_follow(cx);
        self.merged_note(cx);
        cx.notify();
    }

    /// The shown thread was merged: reads which commit it went in as.
    fn merged_note(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.shown_thread().filter(|t| t["merged_ms"].is_number()).map(|t| crate::ui::s(&t, "id")) {
            self.fetch_merged_sha(id, cx);
        }
    }

    /// A system notification for a thread that began waiting for the person, or finished its turn,
    /// while the window is not in front. The first look only takes note.
    fn notify_changes(&mut self, snap: &Snapshot) {
        let now: HashMap<String, (String, bool)> = snap.threads.iter().filter(|t| t["archived"] != true).map(|t| (crate::ui::s(t, "id"), (crate::ui::s(t, "state"), t["waiting"].is_string()))).collect();
        let at = data::now_ms();
        if let Some(before) = &self.seen
            && !self.active
        {
            for t in snap.threads.iter().filter(|t| t["archived"] != true) {
                let id = crate::ui::s(t, "id");
                let Some((state, _)) = now.get(&id) else { continue };
                let recently = self.notified.get(&id).is_some_and(|n| at - n <= 10_000);
                if let Some(body) = what_to_say(before.get(&id).map(|(s, _)| s.as_str()), state, recently) {
                    self.notified.insert(id, at);
                    let project = snap.projects.iter().find(|p| t["project"] == p.id.as_str()).map(|p| p.label()).unwrap_or_default();
                    data::notify(&format!("{project} · {}", crate::ui::s(t, "title")), body);
                }
            }
        }
        self.seen = Some(now);
    }

    /// Shows a thread in the middle card. Its conversation is read right away:
    /// left to the next poll, the card stays empty for up to five seconds.
    pub fn show_thread(&mut self, id: String, cx: &mut Context<Self>) {
        if self.thread.as_deref() != Some(id.as_str()) {
            let project = self.snap.threads.iter().find(|t| t["id"] == id.as_str()).and_then(|t| t["project"].as_str().map(String::from));
            self.last_project = project.or(self.last_project.take());
            data::save_last(&self.paths, Some(&id), self.last_project.as_deref());
            self.thread = Some(id);
            self.new_in = None;
            self.refresh_now();
            self.panel_refresh(cx);
            self.merged_note(cx);
        }
        cx.notify();
    }

    /// Shows the box for a new session in `project`, with the cursor in it. The session is made when the first message is sent.
    pub fn show_new(&mut self, project: String, window: &mut Window, cx: &mut Context<Self>) {
        self.thread = None;
        self.conv = None;
        self.last_project = Some(project.clone());
        data::save_last(&self.paths, None, Some(&project));
        let key = crate::composer::new_key(&project);
        let input = self.composer(&key, window, cx).input.clone();
        self.new_in = Some(project);
        window.focus(&input.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    /// Folds the sidebar away, or brings it back; the choice is kept.
    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_hidden = !self.sidebar_hidden;
        data::save_sidebar_hidden(&self.paths, self.sidebar_hidden);
        cx.notify();
    }

    /// The project a new session would go to without being asked: the one shown, or last shown.
    pub fn current_project(&self) -> Option<String> {
        let shown = self.shown_thread().and_then(|t| t["project"].as_str().map(String::from));
        shown.or_else(|| self.new_in.clone()).or_else(|| self.last_project.clone()).filter(|p| self.snap.projects.iter().any(|x| x.id == *p)).or_else(|| self.snap.projects.first().map(|p| p.id.clone()))
    }

    pub fn project(&self, id: &str) -> Option<Project> {
        self.snap.projects.iter().find(|p| p.id == id).cloned()
    }

    /// Asks lilad for the models omp offers, once (it asks an omp of its own, which takes a moment), for the menus that choose among them.
    pub fn ensure_models(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.models_asked, true) {
            return;
        }
        let paths = self.paths.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { data::act(&paths, "models.list", json!({}), false) }).await;
            let _ = this.update(cx, |this, cx| {
                // Without them the menus show the plain words.
                if let Ok(v) = r {
                    this.models = v["models"].as_array().into_iter().flatten().map(|m| (crate::ui::s(m, "id"), crate::ui::s(m, "name"))).collect();
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Fetches everything again right away.
    pub fn refresh_now(&self) {
        let _ = self.kick.try_send(());
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// A project's threads, latest first; archived ones only when asked for.
    pub fn threads_of(&self, project: &str) -> Vec<Value> {
        self.snap.threads.iter().filter(|t| t["project"] == project && t["archived"] != true).cloned().collect()
    }

    pub fn archived_of(&self, project: &str) -> Vec<Value> {
        self.snap.threads.iter().filter(|t| t["project"] == project && t["archived"] == true).cloned().collect()
    }

    /// The thread shown, as lilad last described it.
    pub fn shown_thread(&self) -> Option<Value> {
        self.thread.as_ref().and_then(|id| self.snap.threads.iter().find(|t| t["id"] == id.as_str()).cloned())
    }

    /// Runs one request off the UI thread, then refreshes.
    pub fn act(&mut self, method: &'static str, params: Value, cx: &mut Context<Self>) {
        let paths = self.paths.clone();
        self.notice = None;
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { data::act(&paths, method, params, false) }).await;
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

    /// Adds a folder as a project (starting lilad if needed), and opens a new session in it.
    pub fn add_project(&mut self, dir: std::path::PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let paths = self.paths.clone();
        self.notice = None;
        cx.spawn_in(window, async move |this, cx| {
            let cwd = dir.to_string_lossy().to_string();
            let r = cx.background_executor().spawn(async move { data::act(&paths, "project.add", json!({"cwd": cwd}), true) }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                match r {
                    Ok(p) => {
                        if let Some(id) = p["id"].as_str() {
                            // The list the box reads its defaults from is fetched again first.
                            this.snap.projects.retain(|x| x.id != id);
                            this.snap.projects.push(Project::of(&p));
                            this.snap.projects.sort_by(|a, b| a.id.cmp(&b.id));
                            this.show_new(id.to_owned(), window, cx);
                        }
                    }
                    Err(e) => this.notice = Some(format!("{e:#}")),
                }
                this.refresh_now();
                cx.notify();
            });
        })
        .detach();
    }

    /// Removes `p` from Lila (after the person confirmed); its folder stays.
    pub fn remove_project(&mut self, p: Project, cx: &mut Context<Self>) {
        self.removing = None;
        self.act("project.remove", json!({"project": p.id}), cx);
    }

    /// Like `act`, then `done` with the result.
    pub fn request(&mut self, method: &'static str, params: Value, cx: &mut Context<Self>, done: impl FnOnce(&mut Self, Value, &mut Context<Self>) + 'static) {
        let paths = self.paths.clone();
        self.notice = None;
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { data::act(&paths, method, params, false) }).await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(v) => done(this, v, cx),
                    Err(e) => this.notice = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Like `request`, but `done` gets the error too (and a window to change the boxes with): for a caller
    /// that keeps what it was doing until it knows the request went.
    pub fn request_result(&mut self, method: &'static str, params: Value, window: &mut Window, cx: &mut Context<Self>, done: impl FnOnce(&mut Self, anyhow::Result<Value>, &mut Window, &mut Context<Self>) + 'static) {
        let paths = self.paths.clone();
        cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_executor().spawn(async move { data::act(&paths, method, params, false) }).await;
            let _ = this.update_in(cx, |this, window, cx| done(this, r, window, cx));
        })
        .detach();
    }

    /// A request off the UI thread, then `done` with what came: for what the panel reads, which does not touch the
    /// notice or refresh anything.
    pub fn call(&mut self, method: &'static str, params: Value, cx: &mut Context<Self>, done: impl FnOnce(&mut Self, anyhow::Result<Value>, &mut Context<Self>) + 'static) {
        let paths = self.paths.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { data::act(&paths, method, params, false) }).await;
            let _ = this.update(cx, |this, cx| done(this, r, cx));
        })
        .detach();
    }
}

impl Render for LilaApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dialog = self.prefs_dialog(cx).or_else(|| self.settings_dialog(cx)).or_else(|| self.rename_dialog(cx)).or_else(|| self.remove_dialog(cx)).or_else(|| self.merge_dialog(cx)).or_else(|| self.restore_dialog(cx));
        let pad = px(crate::style::GAP);
        if window.focused(cx).is_none() {
            let focus = self.focus.clone();
            window.defer(cx, move |window, cx| focus.focus(window, cx));
        }
        v_flex()
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .p(pad)
            .bg(crate::style::glass())
            .text_color(crate::style::fg())
            .text_size(px(13.))
            .on_key_down(cx.listener(Self::shortcut))
            .child(div().flex_1().min_h_0().child(if self.sidebar_hidden {
                // Folded away: the center card takes the window, and its first row leaves room for the traffic lights.
                v_flex().size_full().child(div().flex_1().min_h_0().child(self.work(window, cx))).into_any_element()
            } else {
                // The gutter between the sidebar and the thread is the resize handle.
                gpui::base::h_resizable("main")
                    .with_state(&self.sidebar_size)
                    .with_handle_appearance(gutter())
                    .child(resizable_panel().size(px(260.)).size_range(px(200.)..px(420.)).fixed().child(div().size_full().pr(px(3.)).child(self.sidebar(cx)).into_any_element()))
                    .child(resizable_panel().child(v_flex().size_full().pl(px(4.)).child(div().flex_1().min_h_0().child(self.work(window, cx))).into_any_element()))
                    .into_any_element()
            }))
            .children(dialog)
    }
}

/// A gutter between cards that resizes them: nothing at rest, a thin blue line
/// while the pointer is on it or dragging.
pub fn gutter() -> gpui::base::ResizeHandleRenderer {
    std::rc::Rc::new(move |handle, _, _| {
        let mut on = !matches!(handle.state(), gpui::base::ResizeHandleState::Idle);
        // Checks without a mouse: `LILA_APP_GUTTERS=1` shows every line.
        if cfg!(feature = "shots") && std::env::var_os("LILA_APP_GUTTERS").is_some() {
            on = true;
        }
        let line = div().absolute().top_0().bottom_0().left(px(-0.5)).w(px(2.)).rounded(px(1.)).bg(crate::style::blue());
        Some(div().flex_none().relative().w(px(1.)).h_full().when(on, |d| d.child(line)).into_any_element())
    })
}

impl LilaApp {
    /// A dialog is open over the window.
    fn modal(&self) -> bool {
        self.prefs || self.settings.is_some() || self.renaming.is_some() || self.removing.is_some() || self.panel.merge_confirm.is_some() || self.restore.ask.is_some()
    }

    /// Closes the topmost dialog; false when none is open.
    fn close_dialog(&mut self) -> bool {
        if self.restore.ask.take().is_some() || self.panel.merge_confirm.take().is_some() || self.removing.take().is_some() || self.renaming.take().is_some() || self.settings.take().is_some() || std::mem::take(&mut self.prefs) {
            return true;
        }
        false
    }

    /// Window-wide keys.
    fn shortcut(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        if k.key == "escape" && self.close_dialog() {
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if !k.modifiers.platform || k.modifiers.control || k.modifiers.alt || self.modal() {
            return;
        }
        match (k.key.as_str(), k.modifiers.shift) {
            // A new session, in the project at hand.
            ("n", false) => {
                if let Some(p) = self.current_project() {
                    self.show_new(p, window, cx);
                }
            }
            ("b", false) => self.toggle_sidebar(cx),
            // The right panel: the files (as VS Code's explorer), the changes.
            ("e", true) | ("g", true) => {
                // Only beside a thread that has a folder to show.
                if self.shown_thread().is_none_or(|t| t["merged_ms"].is_number()) {
                    return;
                }
                self.toggle_panel(if k.key == "e" { crate::panel::View::Files } else { crate::panel::View::Changes }, cx);
            }
            (",", false) => self.open_prefs(cx),
            (",", true) | ("<", _) => {
                if let Some(p) = self.current_project() {
                    self.open_settings(p, window, cx);
                }
            }
            // Stops the turn under way, as ⌘. does everywhere on the Mac.
            (".", false) => {
                if let Some(t) = self.shown_thread().filter(|t| t["state"] == "running") {
                    self.act("thread.interrupt", json!({"thread": crate::ui::s(&t, "id")}), cx);
                }
            }
            (d, false) if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() && d != "0" => {
                let n = (d.as_bytes()[0] - b'1') as usize;
                if let Some(p) = self.snap.projects.get(n).map(|p| p.id.clone()) {
                    match self.threads_of(&p).iter().find(|t| t["archived"] != true).and_then(|t| t["id"].as_str().map(String::from)) {
                        Some(t) => self.show_thread(t, cx),
                        None => self.show_new(p, window, cx),
                    }
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }
}

#[cfg(feature = "shots")]
impl LilaApp {
    /// `--features shots`: a new session's box in a project (`LILA_APP_NEW=shop,permission=…`).
    pub fn dev_new(&mut self, spec: &str, window: &mut Window, cx: &mut Context<Self>) {
        let mut parts = spec.split(',');
        let project = parts.next().unwrap_or_default().to_owned();
        let mut d = Draft::default();
        for kv in parts {
            match kv.split_once('=') {
                Some(("permission", v)) => d.permission = Some(v.to_owned()),
                _ => {}
            }
        }
        self.drafts.insert(project.clone(), d);
        self.show_new(project, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::{Draft, what_to_say};
    use crate::data::Project;

    fn project(model: &str) -> Project {
        Project { id: "shop".into(), cwd: "/x".into(), label: "shop".into(), model: model.into(), permission: "full".into(), ..Default::default() }
    }

    #[test]
    fn a_new_session_starts_on_the_projects_defaults_until_told_otherwise() {
        let p = project("deepseek/deepseek-v4");
        assert_eq!(Draft::default().effective(&p), ("deepseek/deepseek-v4".into(), "full".into()));
        let d = Draft { permission: Some("confirm".into()), ..Default::default() };
        assert_eq!(d.effective(&p), ("deepseek/deepseek-v4".into(), "confirm".into()));
        assert_eq!(Draft::default().effective(&project("")).0, "", "omp's own default");
    }

    #[test]
    fn it_notifies_when_a_thread_starts_waiting_or_finishes_a_turn() {
        assert_eq!(what_to_say(Some("running"), "waiting", false), Some("要你允许"));
        assert_eq!(what_to_say(None, "waiting", false), Some("要你允许"), "a thread that began while the window was away");
        assert_eq!(what_to_say(Some("waiting"), "waiting", false), None, "still the same wait");
        assert_eq!(what_to_say(Some("running"), "idle", false), Some("这一轮做完了"));
        assert_eq!(what_to_say(Some("running"), "idle", true), None, "not twice in a flicker");
        assert_eq!(what_to_say(Some("idle"), "running", false), None);
        assert_eq!(what_to_say(Some("running"), "ended", false), None);
    }
}
