//! The main window: projects and the team on the left; the requests in the
//! middle (or a member's terminal, to look at the work itself); the selected
//! request on the right, with what it needs from the person.

use crate::data::{self, Project, Snapshot};
use crate::ui::s;
use gpui::component::input::InputState;
use gpui::component::{resizable_panel, v_flex};
use gpui::prelude::*;
use gpui::*;
use lila::paths::Paths;
use lila_term::{Core, TerminalView};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Away this long, the window says what happened meanwhile.
const AWAY_MS: i64 = 10 * 60_000;

pub struct LilaApp {
    paths: Paths,
    /// The window's own focus. Keys only reach `shortcut` along the focused
    /// element's path, so with no terminal focused (no team running) the window
    /// holds focus itself, or ⌘, and the other shortcuts would go nowhere.
    focus: FocusHandle,
    pub snap: Snapshot,
    /// The project shown, or none.
    pub selected: Option<String>,
    /// The project shown last time was restored (after the first snapshot).
    restored: bool,
    /// Projects whose missing config the window has written again (once each).
    restoring: std::collections::HashSet<String>,
    /// The project settings form, while open.
    pub settings: Option<crate::settings::Form>,
    /// The global settings (role library), while open.
    pub prefs: Option<crate::prefs::Prefs>,
    /// The 用量 section: tokens per member, read in the background.
    pub usage: crate::usage::UsageState,
    /// The new-project wizard, while open.
    pub wizard: Option<crate::wizard::Wizard>,
    /// Where the open dialog was dragged to, from the window's center.
    pub dialog_offset: Point<Pixels>,
    /// While dragging a dialog: where the press was and the offset then.
    pub dialog_drag: Option<(Point<Pixels>, Point<Pixels>)>,
    /// The member whose terminal fills the middle; `None`: the requests do.
    pub member: Option<String>,
    /// The request on the right (a root task's id), or an inbox item about no request.
    pub request: Option<String>,
    /// The window is in front: notifications go out only while it is not.
    active: bool,
    /// When the window went to the back.
    away_since: Option<i64>,
    /// Back after a while: the list leads with what happened since then.
    pub recap: Option<i64>,
    /// The list shows only what changed while the person was away.
    pub recap_only: bool,
    /// Per project, the inbox items that needed the person at the last look: only new ones notify.
    needs_seen: HashMap<String, std::collections::HashSet<String>>,
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
    /// The task open in the side panel, pushed over its request.
    pub task: Option<String>,
    /// The tasks opened before it, the latest last: back goes to them, then to the request.
    pub task_back: Vec<String>,
    append: Option<Entity<InputState>>,
    /// The task whose "cancel" was pressed once (press again to confirm).
    pub confirm_cancel: Option<String>,
    /// "Tidy the PM's context" was pressed once (press again to confirm).
    pub confirm_refresh: bool,
    /// The project "remove from the list" asks about.
    pub removing: Option<data::Project>,
    /// The new-task form, while open.
    pub draft: Option<crate::compose::Draft>,
    /// Members shown as their terminal instead of their conversation ("project/member").
    pub tui: std::collections::HashSet<String>,
    /// The shown member's conversation, as far as it was read.
    pub conv: Option<crate::conversation::Conv>,
    /// Tool calls opened to show their output.
    pub conv_open: std::collections::HashSet<String>,
    /// The conversation's virtual list, and what it was last told it holds (`conversation::sync_conv`).
    pub conv_list: Entity<gpui::component::message_scroller::MessageScrollerState>,
    pub conv_rows: Vec<crate::conversation::Row>,
    pub conv_key: Option<(String, String, Option<String>)>,
    /// The request list's scroll: ↑ and ↓ bring the selected row into view.
    pub req_scroll: ScrollHandle,
    /// The box under each member's conversation, by "project/member": its draft, files and menu.
    pub composers: HashMap<String, crate::composer::Composer>,
    /// What the `@` and `/` menus list for a member, as lilad last said.
    pub menu_files: HashMap<String, Vec<String>>,
    pub menu_commands: HashMap<String, crate::composer::Commands>,
    /// The box a project without requests opens with: a first request for the PM.
    pub ask: Option<Entity<gpui::component::input::TextareaState>>,
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
                let Ok((paths, selected, task, request, conv)) = this.update(cx, |this, _| (this.paths.clone(), this.selected.clone(), this.task.clone(), this.request.clone(), this.conv_want())) else { return };
                let snap = cx.background_executor().spawn(async move { data::fetch(&paths, selected.as_deref(), task.as_deref(), request.as_deref(), conv.as_ref()) }).await;
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
                    Err(e) => this.core_error = Some(format!("连不上 herdr-core（hc）：{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        let tui = data::load_views(&paths);
        let mut app = LilaApp { paths, focus: cx.focus_handle(), usage: Default::default(), snap: Snapshot::default(), selected: None, restored: false, restoring: Default::default(), settings: None, prefs: None, wizard: None, dialog_offset: Point::default(), dialog_drag: None, member: None, request: None, active: true, away_since: None, recap: None, recap_only: false, needs_seen: HashMap::new(), core: None, core_error: None, terminals: HashMap::new(), answers: HashMap::new(), notice: None, right_open: true, drag_armed: false, task: None, task_back: Vec::new(), append: None, confirm_cancel: None, confirm_refresh: false, removing: None, draft: None, tui, conv: None, conv_open: Default::default(), conv_list: Self::conv_list_state(cx), conv_rows: Vec::new(), conv_key: None, req_scroll: ScrollHandle::new(), composers: HashMap::new(), menu_files: HashMap::new(), menu_commands: HashMap::new(), ask: None, rewrite: None, kick, cache: HashMap::new(), _poll: poll };
        app.start_usage(window, cx);
        // Pictures pasted into boxes long ago go.
        let paths = app.paths.clone();
        cx.background_executor().spawn(async move { crate::composer::prune(&paths) }).detach();
        cx.observe_window_activation(window, |this, window, cx| this.activated(window.is_window_active(), cx)).detach();
        app
    }

    fn apply(&mut self, snap: Snapshot, cx: &mut Context<Self>) {
        // Checks without running agents: `LILA_APP_FAKE_STATUS=designer:working,engineer:blocked`.
        #[cfg(feature = "shots")]
        let snap = {
            let mut snap = snap;
            if let (Ok(spec), Some(members)) = (std::env::var("LILA_APP_FAKE_STATUS"), snap.status.as_mut().and_then(|s| s["members"].as_array_mut())) {
                for pair in spec.split(',') {
                    if let Some((id, st)) = pair.split_once(':')
                        && let Some(m) = members.iter_mut().find(|m| m["id"] == id)
                    {
                        m["status"] = json!(st);
                        m["msg"] = json!(match st { "working" => "画结算页的三种状态", "blocked" => "终端在等确认：运行 pnpm test", _ => "" });
                    }
                }
            }
            // `LILA_APP_FAKE_CONTEXT=pm:104000,engineer:47000`: context sizes as if read from sessions.
            if let (Ok(spec), Some(members)) = (std::env::var("LILA_APP_FAKE_CONTEXT"), snap.status.as_mut().and_then(|s| s["members"].as_array_mut())) {
                for pair in spec.split(',') {
                    if let Some((id, n)) = pair.split_once(':')
                        && let Some(m) = members.iter_mut().find(|m| m["id"] == id)
                    {
                        m["context"] = json!(n.parse::<u64>().unwrap_or(0));
                    }
                }
            }
            snap
        };
        if std::env::var_os("LILA_APP_DEBUG").is_some() {
            eprintln!("[lila-app] apply: {} projects (unknown {}), for {:?}, selected {:?}, err {:?}", snap.projects.len(), snap.projects_unknown, snap.project, self.selected, snap.error);
        }
        // A database that couldn't be read says nothing new: keep what is shown.
        if snap.projects_unknown {
            self.snap.error = snap.error;
            cx.notify();
            return;
        }
        let mut snap = snap;
        // First look: the project shown last time (else the first one).
        if !self.restored {
            self.restored = true;
            let known = |id: &String| snap.projects.iter().any(|p| &p.id == id);
            let pick = data::load_last(&self.paths).filter(known).or_else(|| snap.projects.first().map(|p| p.id.clone()));
            self.snap.projects = snap.projects.clone();
            if let Some(id) = pick {
                self.select_project(id, cx);
                return;
            }
        }
        // The shown project went away: show the first one left, or none.
        if let Some(id) = self.selected.clone()
            && !snap.projects.iter().any(|p| p.id == id)
        {
            self.forget_project(&id, cx);
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
        if snap.detail.as_ref().and_then(|d| d["id"].as_str()) != self.task.as_deref() {
            snap.detail = self.snap.detail.take().filter(|d| d["id"].as_str() == self.task.as_deref());
        }
        let shows = |v: &Option<Value>, id: Option<&str>| v.as_ref().and_then(|r| r["request"]["id"].as_str()) == id;
        if !shows(&snap.request, self.request.as_deref()) {
            snap.request = self.snap.request.take().filter(|r| shows(&Some(r.clone()), self.request.as_deref()));
        }
        if let Some(c) = snap.conversation.take() {
            self.take_conv(c);
        }
        self.snap = snap;
        self.keep_a_request();
        self.notify_needs();
        if let Some(id) = self.selected.clone() {
            self.restore_config(&id, cx);
        }
        cx.notify();
    }

    /// The window came to the front or went to the back. Back after `AWAY_MS`:
    /// the list leads with what happened meanwhile.
    fn activated(&mut self, active: bool, cx: &mut Context<Self>) {
        let now = data::now_ms();
        if !active {
            self.away_since.get_or_insert(now);
        } else if let Some(since) = self.away_since.take()
            && now - since >= AWAY_MS
        {
            self.recap = Some(since);
            self.recap_only = false;
        }
        self.active = active;
        cx.notify();
    }

    /// A system notification for each new thing that needs the person, while
    /// the window is not in front. The first look at a project only takes note.
    fn notify_needs(&mut self) {
        let Some(pid) = self.snap.project.clone().filter(|_| self.snap.status.is_some()) else { return };
        let now: std::collections::HashSet<String> = self.snap.inbox.iter().filter(|i| i["kind"] != "report").map(|i| crate::ui::s(i, "id")).collect();
        let fresh: Vec<String> = match self.needs_seen.get(&pid) {
            Some(seen) if !self.active => now.iter().filter(|id| !seen.contains(*id)).cloned().collect(),
            _ => Vec::new(),
        };
        self.needs_seen.insert(pid.clone(), now);
        let project = self.project().map(|p| p.label()).unwrap_or(pid);
        for id in fresh {
            let Some(item) = self.snap.inbox.iter().find(|i| i["id"] == id.as_str()) else { continue };
            let words = crate::requests::inbox_words(item["kind"].as_str().unwrap_or(""));
            let request = self.snap.requests.iter().find(|r| r["inbox"].as_array().is_some_and(|a| a.iter().any(|x| x == id.as_str())));
            let body = match request {
                Some(r) => format!("{} {}：{words}", crate::ui::s(r, "id"), crate::ui::s(r, "title")),
                None => crate::ui::s(item, "text").lines().next().unwrap_or(words).to_owned(),
            };
            data::notify(&project, &body);
        }
    }

    /// Inbox items about no request (Codex's quota, a member that left): the
    /// list shows them with the requests that need the person.
    pub fn loose_inbox(&self) -> Vec<Value> {
        let claimed: Vec<&str> = self.snap.requests.iter().flat_map(|r| r["inbox"].as_array().into_iter().flatten().filter_map(|i| i.as_str())).collect();
        self.snap.inbox.iter().filter(|i| !claimed.contains(&i["id"].as_str().unwrap_or(""))).cloned().collect()
    }

    /// Something is always on the right while there is anything: the selected
    /// request while it exists, else what needs the person first, else the first request.
    fn keep_a_request(&mut self) {
        if self.snap.status.is_none() {
            return;
        }
        let loose = self.loose_inbox();
        let exists = |id: &str| self.snap.requests.iter().any(|r| r["id"] == id) || loose.iter().any(|i| i["id"] == id);
        if self.request.as_deref().is_some_and(exists) {
            return;
        }
        let needs = self.snap.requests.iter().find(|r| r["group"] == "needs_you").map(|r| crate::ui::s(r, "id"));
        let pick = needs.or_else(|| loose.first().map(|i| crate::ui::s(i, "id"))).or_else(|| self.snap.requests.first().map(|r| crate::ui::s(r, "id")));
        if pick != self.request {
            self.request = pick;
            self.snap.request = None;
            self.refresh_now();
        }
    }

    /// Shows a request (or a loose inbox item) on the right.
    pub fn select_request(&mut self, id: String, cx: &mut Context<Self>) {
        if self.request.as_deref() != Some(&id) {
            self.request = Some(id);
            self.snap.request = None;
            self.refresh_now();
        }
        // The panel shows the request itself, not a task pushed over the last one.
        if self.task.is_some() {
            self.close_task(cx);
        }
        self.right_open = true;
        cx.notify();
    }

    /// Shows a member in the middle card. Its conversation is read right away:
    /// left to the next poll, the card stays empty for up to five seconds.
    pub fn show_member(&mut self, member: String, cx: &mut Context<Self>) {
        if self.member.as_deref() != Some(member.as_str()) {
            self.member = Some(member);
            self.refresh_now();
        }
        cx.notify();
    }

    /// Fetches everything again right away.
    pub fn refresh_now(&self) {
        let _ = self.kick.try_send(());
    }

    /// The project `id` is gone from the list: shows the first one left, or none.
    pub fn forget_project(&mut self, id: &str, cx: &mut Context<Self>) {
        self.cache.remove(id);
        if self.selected.as_deref() != Some(id) {
            return;
        }
        match self.snap.projects.iter().find(|p| p.id != id).map(|p| p.id.clone()) {
            Some(next) => self.select_project(next, cx),
            None => {
                self.member = None;
                self.request = None;
                self.close_task(cx);
                self.draft = None;
                self.settings = None;
                self.selected = None;
                self.snap = Snapshot { daemon: self.snap.daemon, projects: std::mem::take(&mut self.snap.projects), ..Default::default() };
                data::save_last(&self.paths, None);
            }
        }
        cx.notify();
    }

    /// A project opened without its `.lila/project.toml` gets the default one
    /// back (see `data::restore_config`), then everything is fetched again.
    fn restore_config(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(p) = self.snap.projects.iter().find(|p| p.id == id && p.missing).cloned() else { return };
        if !self.restoring.insert(id.to_owned()) {
            return;
        }
        let (paths, daemon, id) = (self.paths.clone(), self.snap.daemon, id.to_owned());
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { data::restore_config(&paths, &p, daemon) }).await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(()) => {
                        for p in this.snap.projects.iter_mut().filter(|p| p.id == id) {
                            p.missing = false;
                        }
                    }
                    Err(e) => this.notice = Some(format!("没能给 {id} 创建配置：{e:#}")),
                }
                this.refresh_now();
                cx.notify();
            });
        })
        .detach();
    }

    /// Shows another project: its last snapshot at once, then fresh data.
    pub fn select_project(&mut self, id: String, cx: &mut Context<Self>) {
        if self.selected.as_deref() == Some(&id) {
            self.refresh_now();
            return;
        }
        self.settings = None;
        self.member = None;
        self.request = None;
        self.close_task(cx);
        self.draft = None;
        let cached = self.cache.get(&id).cloned().unwrap_or_else(|| Snapshot { project: Some(id.clone()), ..Default::default() });
        self.snap = Snapshot { daemon: self.snap.daemon, projects: std::mem::take(&mut self.snap.projects), ..cached };
        data::save_last(&self.paths, Some(&id));
        self.selected = Some(id);
        self.refresh_now();
        self.usage.refresh();
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

    /// Whether dispatch is paused in project `id`, as last fetched.
    pub fn paused_of(&self, id: &str) -> bool {
        let snap = if self.snap.project.as_deref() == Some(id) { Some(&self.snap) } else { self.cache.get(id) };
        snap.and_then(|s| s.status.as_ref()).is_some_and(|s| s["paused"] == true)
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

    /// The member whose terminal is in the middle, if one was chosen.
    pub fn shown_member(&self) -> Option<Value> {
        let members = self.members();
        self.member.as_ref().and_then(|id| members.iter().find(|m| m["id"] == id.as_str()).cloned())
    }

    /// The team has a running member.
    pub fn team_running(&self) -> bool {
        self.members().iter().any(|m| m["terminal"].is_string())
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

    /// Removes `p` from Lila (after the person confirmed); its folder stays.
    pub fn remove_project(&mut self, p: data::Project, cx: &mut Context<Self>) {
        let paths = self.paths.clone();
        self.removing = None;
        self.notice = None;
        let id = p.id.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { data::remove_project(&paths, &p) }).await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(()) => this.forget_project(&id, cx),
                    Err(e) => this.notice = Some(format!("移除失败：{e:#}")),
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

    /// Like `request`, but `done` gets the error too (and a window to change the boxes with): for a caller
    /// that keeps what it was doing until it knows the request went.
    pub fn request_result(&mut self, method: &'static str, params: Value, window: &mut Window, cx: &mut Context<Self>, done: impl FnOnce(&mut Self, anyhow::Result<Value>, &mut Window, &mut Context<Self>) + 'static) {
        let Some(project) = self.project() else { return };
        let paths = self.paths.clone();
        cx.spawn_in(window, async move |this, cx| {
            let r = cx.background_executor().spawn(async move { data::act(&paths, &project, method, params, false) }).await;
            let _ = this.update_in(cx, |this, window, cx| done(this, r, window, cx));
        })
        .detach();
    }

    /// Pushes a task over the side panel; back returns to what was there.
    pub fn show_task(&mut self, id: String, cx: &mut Context<Self>) {
        if let Some(prev) = self.task.clone().filter(|p| *p != id) {
            self.task_back.push(prev);
        }
        self.open_task(id, cx);
    }

    /// One step back: the task opened before this one, else the request.
    pub fn back_task(&mut self, cx: &mut Context<Self>) {
        match self.task_back.pop() {
            Some(prev) => self.open_task(prev, cx),
            None => self.close_task(cx),
        }
    }

    /// Shows a task in the side panel; its details load with the next poll (and now).
    fn open_task(&mut self, id: String, cx: &mut Context<Self>) {
        self.draft = None;
        self.right_open = true;
        if self.task.as_deref() != Some(&id) {
            self.snap.detail = None;
            self.confirm_cancel = None;
            self.rewrite = None;
        }
        self.task = Some(id.clone());
        let (paths, project) = (self.paths.clone(), self.project());
        cx.spawn(async move |this, cx| {
            let Some(p) = project else { return };
            let asked = id.clone();
            let v = cx.background_executor().spawn(async move { data::act(&paths, &p, "task.show", json!({"id": asked}), false) }).await;
            let _ = this.update(cx, |this, cx| {
                match v {
                    Ok(v) if this.task.as_deref() == v["id"].as_str() => this.snap.detail = Some(v),
                    Ok(_) => {}
                    // Nothing to show: no spinner left turning.
                    Err(e) if this.task.as_deref() == Some(&id) => {
                        this.close_task(cx);
                        this.notice = Some(format!("{e:#}"));
                    }
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn close_task(&mut self, cx: &mut Context<Self>) {
        self.task = None;
        self.task_back.clear();
        self.snap.detail = None;
        self.confirm_cancel = None;
        self.rewrite = None;
        cx.notify();
    }

    pub fn append_input(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        self.append.get_or_insert_with(|| cx.new(|cx| InputState::new(window, cx).placeholder("追加…"))).clone()
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
        let compose = self.compose_panel(cx);
        let dialog = self.settings_dialog(cx).or_else(|| self.prefs_dialog(cx)).or_else(|| self.wizard_dialog(cx)).or_else(|| self.remove_dialog(cx));
        let overlay = |d: Div| crate::ui::drawer_frame(d, 600.);
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
            .child(
                div().flex_1().min_h_0().child(
                    // The gutters between the sidebar, the terminal and the overview are the resize handles.
                    gpui::base::h_resizable("main")
                        .with_handle_appearance(gutter(0.))
                        .child(resizable_panel().size(px(250.)).size_range(px(200.)..px(420.)).fixed().child(div().size_full().pr(px(3.)).child(self.sidebar(cx)).into_any_element()))
                        .child(
                            resizable_panel().child(
                                v_flex()
                                    .size_full()
                                    .pl(px(4.))
                                    .child(
                                        div().flex_1().min_h_0().child(
                                            gpui::base::h_resizable("work")
                                                .with_handle_appearance(gutter(0.))
                                                .child(resizable_panel().child(self.center(window, cx).into_any_element()))
                                                .child(resizable_panel().visible(self.right_open && self.selected.is_some() && (self.request.is_some() || self.task.is_some())).size(px(360.)).size_range(px(300.)..px(640.)).fixed().child(self.right(window, cx).into_any_element())),
                                        ),
                                    )
                                    .into_any_element(),
                            ),
                        ),
                ),
            )
            .children(compose.map(|c| overlay(div()).child(c)))
            .children(dialog)
    }
}

/// A gutter between cards that resizes them: nothing at rest, a thin blue line
/// while the pointer is on it or dragging. The line runs along the cards beside
/// it: `top` leaves out what sits above them (the tab row, beside the sidebar).
fn gutter(top: f32) -> gpui::base::ResizeHandleRenderer {
    std::rc::Rc::new(move |handle, _, _| {
        let mut on = !matches!(handle.state(), gpui::base::ResizeHandleState::Idle);
        // Checks without a mouse: `LILA_APP_GUTTERS=1` shows every line.
        if cfg!(feature = "shots") && std::env::var_os("LILA_APP_GUTTERS").is_some() {
            on = true;
        }
        let line = div().absolute().top(px(top)).bottom_0().left(px(-0.5)).w(px(2.)).rounded(px(1.)).bg(crate::style::blue());
        Some(div().flex_none().relative().w(px(1.)).h_full().when(on, |d| d.child(line)).into_any_element())
    })
}

impl LilaApp {
    /// Window-wide keys. The terminal handles copy, paste and find; other ⌘ chords bubble here.
    fn shortcut(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        if k.key == "escape" && self.removing.is_some() {
            self.removing = None;
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if k.key == "escape" && (self.settings.is_some() || self.prefs.is_some() || self.wizard.is_some()) {
            self.settings = None;
            self.prefs = None;
            if !self.wizard.as_ref().is_some_and(|w| w.busy) {
                self.wizard = None;
            }
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if k.key == "escape" && self.draft.is_some() {
            self.draft = None;
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if k.key == "escape" && self.task.is_some() {
            self.back_task(cx);
            cx.stop_propagation();
            return;
        }
        // ↑ ↓ (and k j) walk the requests while nothing else has the keys: no dialog,
        // no terminal or input in focus.
        if !k.modifiers.modified() && self.member.is_none() && self.draft.is_none() && self.focus.is_focused(window) {
            let down = match k.key.as_str() {
                "down" | "j" => Some(true),
                "up" | "k" => Some(false),
                _ => None,
            };
            if let Some(down) = down {
                self.step_request(down, cx);
                cx.stop_propagation();
                return;
            }
            if k.key == "enter" && self.look_at_request(cx) {
                cx.stop_propagation();
                return;
            }
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
            "," => self.open_prefs(crate::prefs::PrefPage::Roles, window, cx),
            "i" => {
                self.right_open = !self.right_open;
                cx.notify();
            }
            // Back from a terminal to the requests.
            "0" | "[" if self.member.is_some() => {
                self.member = None;
                cx.notify();
            }
            d if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() && d != "0" => {
                let n = (d.as_bytes()[0] - b'1') as usize;
                if let Some(id) = self.snap.projects.get(n).map(|p| p.id.clone()) {
                    self.select_project(id, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }
}
