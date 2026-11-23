//! GPUI Entity-based reactive store driven by `ws_client::CoreRpc` (16ms coalesced frames).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::core_host::data_dir;
use crate::protocol::{
    ApprovalMode, CommandsListResult, ConnectionState, FilesSearchResult, ImageInput, ModelInfo,
    ModelsListResult, PendingAnswer, Project, ProjectAddResult, ProjectsListResult,
    SessionConfigureResult, SessionEarlierResult, SessionOpenResult, SessionResult,
    SessionRewindResult, SessionState, SessionStatus, SessionSummary, SessionsListResult,
    SlashCommand, ThinkingLevel, TranscriptItem, WatchSlot,
};
use crate::shell_bridge;
use crate::transcript_layout::{TranscriptBlock, layout_transcript};
use crate::ws_client::{CoreRpc, PushFrame, TransportEvent};

pub const NEW_DRAFT: &str = "new";
const OPEN_LIMIT: usize = 300;
const EARLIER_LIMIT: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionOptions {
    pub project_id: Option<String>,
    pub model_id: Option<String>,
    pub thinking_level: Option<ThinkingLevel>,
    #[serde(default)]
    pub approval_mode: ApprovalMode,
}

impl Default for NewSessionOptions {
    fn default() -> Self {
        Self {
            project_id: None,
            model_id: None,
            thinking_level: None,
            approval_mode: ApprovalMode::Auto,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativePrefs {
    selected_session: Option<String>,
    sidebar_open: Option<bool>,
    new_session: Option<NewSessionOptions>,
}

fn prefs_path() -> PathBuf {
    data_dir().join("native-prefs.json")
}

fn load_prefs() -> NativePrefs {
    fs::read_to_string(prefs_path())
        .ok()
        .and_then(|raw| serde_json::from_str::<NativePrefs>(&raw).ok())
        .unwrap_or_default()
}

fn save_prefs(prefs: &NativePrefs) {
    let dir = data_dir();
    if !dir.is_dir() {
        let _ = fs::create_dir_all(&dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(prefs) {
        let _ = fs::write(prefs_path(), json);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftImage {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub data: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastTone {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub enum AppStoreEvent {
    Toast {
        tone: ToastTone,
        text: String,
    },
    /// Fired when the active transcript list should reset or resize its virtual list.
    TranscriptReset {
        block_count: usize,
    },
    /// Fired when the active draft text was programmatically replaced (send / rewind / switch).
    DraftSync {
        key: String,
        text: String,
    },
}

#[derive(Default)]
pub struct SlotState {
    pub session_id: Option<String>,
    pub items: Vec<TranscriptItem>,
    pub blocks: Vec<TranscriptBlock>,
    pub has_earlier: bool,
    pub loading: bool,
    pub loading_earlier: bool,
    pub error: Option<String>,
    pub state: Option<SessionState>,
    pub watching: Option<String>,
    pub open_seq: u64,
}

impl SlotState {
    fn rebuild_blocks(&mut self, project_path: Option<&str>) {
        self.blocks = layout_transcript(&self.items, project_path);
    }

    fn reset(
        &mut self,
        session_id: Option<String>,
        items: Vec<TranscriptItem>,
        has_earlier: bool,
        state: Option<SessionState>,
        project_path: Option<&str>,
    ) {
        self.session_id = session_id;
        self.items = items;
        self.has_earlier = has_earlier;
        self.loading = false;
        self.loading_earlier = false;
        self.error = None;
        self.state = state;
        self.rebuild_blocks(project_path);
    }

    /// Returns `true` if block count/structure changed.
    fn upsert_item(&mut self, item: TranscriptItem, project_path: Option<&str>) -> bool {
        let id = item.id().to_owned();
        if let Some(existing) = self.items.iter_mut().find(|x| x.id() == id) {
            *existing = item;
        } else {
            self.items.push(item);
        }
        let prev_len = self.blocks.len();
        self.rebuild_blocks(project_path);
        self.blocks.len() != prev_len
    }

    /// Returns `true` if block count/structure changed.
    fn apply_delta(
        &mut self,
        item_id: &str,
        channel: &str,
        delta: &str,
        project_path: Option<&str>,
    ) -> bool {
        if let Some(existing) = self.items.iter_mut().find(|x| x.id() == item_id) {
            let mut became_text = false;
            if let TranscriptItem::Assistant { text, thinking, .. } = existing {
                if channel == "thinking" {
                    thinking.push_str(delta);
                } else {
                    if text.is_empty() && !delta.is_empty() {
                        became_text = true;
                    }
                    text.push_str(delta);
                }
            }
            if became_text {
                let prev = self.blocks.len();
                self.rebuild_blocks(project_path);
                return self.blocks.len() != prev;
            }
            return false;
        }
        let fresh = TranscriptItem::Assistant {
            id: item_id.to_owned(),
            text: if channel == "thinking" {
                String::new()
            } else {
                delta.to_owned()
            },
            thinking: if channel == "thinking" {
                delta.to_owned()
            } else {
                String::new()
            },
            model: None,
            streaming: true,
            error: None,
        };
        self.items.push(fresh);
        let prev = self.blocks.len();
        self.rebuild_blocks(project_path);
        self.blocks.len() != prev
    }

    pub fn item_by_id(&self, id: &str) -> Option<&TranscriptItem> {
        self.items.iter().find(|x| x.id() == id)
    }
}

pub struct GlobalStore(pub Entity<AppStore>);
impl Global for GlobalStore {}

pub struct AppStore {
    pub rpc: CoreRpc,

    pub connection: ConnectionState,
    pub connection_detail: Option<String>,
    pub core_version: String,
    pub memory_mb: f64,

    pub projects: Vec<Project>,
    pub models: Vec<ModelInfo>,
    pub default_model_id: Option<String>,
    pub models_loading: bool,
    pub models_error: Option<String>,

    pub sessions: HashMap<String, SessionSummary>,
    pub selected_id: Option<String>,

    pub new_session: NewSessionOptions,
    pub sidebar_open: bool,

    pub drafts: HashMap<String, String>,
    pub draft_images: HashMap<String, Vec<DraftImage>>,
    pub sending_drafts: HashSet<String>,

    pub expanded_keys: HashSet<String>,
    pub collapsed_projects: HashSet<String>,
    pub expanded_parents: HashSet<String>,

    pub main_slot: SlotState,

    selection_version: u64,
}

impl EventEmitter<AppStoreEvent> for AppStore {}

impl AppStore {
    pub fn init(cx: &mut App) -> Entity<Self> {
        let entity = cx.new(Self::new);
        cx.set_global(GlobalStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalStore>().0.clone()
    }

    fn new(cx: &mut Context<Self>) -> Self {
        let prefs = load_prefs();
        let (rpc, mut evt_rx) = CoreRpc::start();

        // Pump Rust WS events from tokio::mpsc into the GPUI main thread.
        cx.spawn(async move |this, cx| {
            while let Some(evt) = evt_rx.recv().await {
                let keep = this.update(cx, |store, cx| {
                    match evt {
                        TransportEvent::Connection { state, detail } => {
                            store.connection = state;
                            store.connection_detail = detail;
                            cx.notify();
                        }
                        TransportEvent::Ready { core_version } => {
                            store.core_version = core_version;
                            store.on_ready(cx);
                        }
                        TransportEvent::Batch(frames) => {
                            store.handle_batch(frames, cx);
                        }
                    }
                });
                if keep.is_err() {
                    break;
                }
            }
        })
        .detach();

        // Periodically sample process RSS memory (every 3s).
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let mem = shell_bridge::memory_mb();
                let keep = this.update(cx, |store, cx| {
                    store.memory_mb = mem;
                    cx.notify();
                });
                if keep.is_err() {
                    break;
                }
            }
        })
        .detach();

        Self {
            rpc,
            connection: ConnectionState::Connecting,
            connection_detail: None,
            core_version: String::new(),
            memory_mb: shell_bridge::memory_mb(),
            projects: Vec::new(),
            models: Vec::new(),
            default_model_id: None,
            models_loading: false,
            models_error: None,
            sessions: HashMap::new(),
            selected_id: prefs.selected_session,
            new_session: prefs.new_session.unwrap_or_default(),
            sidebar_open: prefs.sidebar_open.unwrap_or(true),
            drafts: HashMap::new(),
            draft_images: HashMap::new(),
            sending_drafts: HashSet::new(),
            expanded_keys: HashSet::new(),
            collapsed_projects: HashSet::new(),
            expanded_parents: HashSet::new(),
            main_slot: SlotState::default(),
            selection_version: 0,
        }
    }

    pub fn active_draft_key(&self) -> String {
        self.selected_id
            .clone()
            .unwrap_or_else(|| NEW_DRAFT.to_owned())
    }

    pub fn active_project(&self) -> Option<&Project> {
        let wanted = self.new_session.project_id.as_deref();
        wanted
            .and_then(|id| self.projects.iter().find(|p| p.id == id))
            .or_else(|| self.projects.first())
    }

    pub fn project_path_for_session(&self, session_id: &str) -> Option<String> {
        let sess = self.sessions.get(session_id)?;
        self.projects
            .iter()
            .find(|p| p.id == sess.project_id)
            .map(|p| p.path.clone())
    }

    pub fn effective_new_model_id(&self) -> Option<String> {
        self.new_session
            .model_id
            .clone()
            .or_else(|| self.default_model_id.clone())
            .or_else(|| self.models.first().map(|m| m.id.clone()))
    }

    pub fn persist_prefs(&self) {
        save_prefs(&NativePrefs {
            selected_session: self.selected_id.clone(),
            sidebar_open: Some(self.sidebar_open),
            new_session: Some(self.new_session.clone()),
        });
    }

    pub fn emit_toast(&self, tone: ToastTone, text: impl Into<String>, cx: &mut Context<Self>) {
        cx.emit(AppStoreEvent::Toast {
            tone,
            text: text.into(),
        });
    }

    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        self.persist_prefs();
        cx.notify();
    }

    pub fn is_expanded(&self, key: &str) -> bool {
        self.expanded_keys.contains(key)
    }

    pub fn toggle_expanded(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.expanded_keys.remove(&key) {
            self.expanded_keys.insert(key);
        }
        cx.notify();
    }

    pub fn toggle_project_collapsed(&mut self, project_id: String, cx: &mut Context<Self>) {
        if !self.collapsed_projects.remove(&project_id) {
            self.collapsed_projects.insert(project_id);
        }
        cx.notify();
    }

    pub fn toggle_parent_expanded(&mut self, session_id: String, cx: &mut Context<Self>) {
        if !self.expanded_parents.remove(&session_id) {
            self.expanded_parents.insert(session_id);
        }
        cx.notify();
    }

    pub fn draft_text(&self, key: &str) -> String {
        self.drafts.get(key).cloned().unwrap_or_default()
    }

    pub fn set_draft_text(&mut self, key: &str, text: String, cx: &mut Context<Self>) {
        if text.is_empty() {
            self.drafts.remove(key);
        } else {
            self.drafts.insert(key.to_owned(), text);
        }
        cx.notify();
    }

    pub fn draft_images_for(&self, key: &str) -> Vec<DraftImage> {
        self.draft_images.get(key).cloned().unwrap_or_default()
    }

    pub fn add_draft_images(&mut self, key: &str, imgs: Vec<DraftImage>, cx: &mut Context<Self>) {
        if imgs.is_empty() {
            return;
        }
        self.draft_images
            .entry(key.to_owned())
            .or_default()
            .extend(imgs);
        cx.notify();
    }

    pub fn remove_draft_image(&mut self, key: &str, image_id: &str, cx: &mut Context<Self>) {
        if let Some(list) = self.draft_images.get_mut(key) {
            list.retain(|x| x.id != image_id);
            if list.is_empty() {
                self.draft_images.remove(key);
            }
        }
        cx.notify();
    }

    // -----------------------------------------------------------------------
    // Push batch handling

    fn handle_batch(&mut self, frames: Vec<PushFrame>, cx: &mut Context<Self>) {
        let mut reset_session: Option<String> = None;
        let mut list_count_changed = false;
        let main_pp = self
            .main_slot
            .watching
            .as_deref()
            .and_then(|sid| self.project_path_for_session(sid));

        for frame in frames {
            match frame {
                PushFrame::Projects(list) => {
                    self.projects = list;
                    let valid = self
                        .new_session
                        .project_id
                        .as_deref()
                        .is_some_and(|pid| self.projects.iter().any(|p| p.id == pid));
                    if !valid {
                        self.new_session.project_id =
                            self.projects.first().map(|p| p.id.clone());
                    }
                }
                PushFrame::Session(summary) => {
                    self.sessions.insert(summary.id.clone(), summary);
                }
                PushFrame::SessionRemoved(sid) => {
                    self.forget_session(&sid, cx);
                }
                PushFrame::Item { session_id, item } => {
                    if self.main_slot.watching.as_deref() == Some(&session_id) {
                        if self.main_slot.upsert_item(item, main_pp.as_deref()) {
                            list_count_changed = true;
                        }
                    }
                }
                PushFrame::Delta {
                    session_id,
                    item_id,
                    channel,
                    delta,
                } => {
                    if self.main_slot.watching.as_deref() == Some(&session_id) {
                        if self
                            .main_slot
                            .apply_delta(&item_id, &channel, &delta, main_pp.as_deref())
                        {
                            list_count_changed = true;
                        }
                    }
                }
                PushFrame::State { session_id, state } => {
                    if self.main_slot.watching.as_deref() == Some(&session_id) {
                        self.main_slot.state = Some(state);
                    }
                }
                PushFrame::Reset { session_id } => {
                    if self.main_slot.watching.as_deref() == Some(&session_id) {
                        reset_session = Some(session_id);
                    }
                }
                PushFrame::Notify { level, message, .. } => {
                    let tone = match level.as_str() {
                        "error" => ToastTone::Error,
                        "warning" => ToastTone::Warning,
                        _ => ToastTone::Info,
                    };
                    self.emit_toast(tone, message, cx);
                }
                PushFrame::Error { message, .. } => {
                    self.emit_toast(ToastTone::Error, message, cx);
                }
            }
        }

        if list_count_changed {
            cx.emit(AppStoreEvent::TranscriptReset {
                block_count: self.main_slot.blocks.len(),
            });
        }
        cx.notify();

        if let Some(sid) = reset_session {
            self.open_main_slot(sid, cx);
        }
    }

    fn forget_session(&mut self, session_id: &str, cx: &mut Context<Self>) {
        let was_selected = self.selected_id.as_deref() == Some(session_id);
        self.sessions.remove(session_id);
        self.drafts.remove(session_id);
        self.draft_images.remove(session_id);
        self.sending_drafts.remove(session_id);
        if was_selected {
            self.selected_id = None;
            self.unwatch_main(cx);
            self.main_slot.reset(None, Vec::new(), false, None, None);
            self.persist_prefs();
            cx.emit(AppStoreEvent::TranscriptReset { block_count: 0 });
            let draft = self.draft_text(NEW_DRAFT);
            cx.emit(AppStoreEvent::DraftSync {
                key: NEW_DRAFT.to_owned(),
                text: draft,
            });
        }
    }

    // -----------------------------------------------------------------------
    // Core RPC actions

    fn on_ready(&mut self, cx: &mut Context<Self>) {
        self.load_models(cx);
        let rpc = self.rpc.clone();
        cx.spawn(async move |this, cx| {
            let proj_res = rpc
                .call::<ProjectsListResult>(json!({ "type": "projects.list" }))
                .await;
            let sess_res = rpc
                .call::<SessionsListResult>(json!({ "type": "sessions.list" }))
                .await;
            let _ = this.update(cx, |store, cx| {
                let (proj_list, sess_list) = match (proj_res, sess_res) {
                    (Ok(p), Ok(s)) => (p.projects, s.sessions),
                    (Err(e), _) | (_, Err(e)) => {
                        store.emit_toast(ToastTone::Error, e, cx);
                        return;
                    }
                };
                let mut by_id = HashMap::with_capacity(sess_list.len());
                for s in sess_list {
                    by_id.insert(s.id.clone(), s);
                }
                let missing = store
                    .selected_id
                    .as_deref()
                    .is_some_and(|id| !by_id.contains_key(id));
                store.projects = proj_list;
                store.sessions = by_id;
                if missing {
                    store.selected_id = None;
                    store.unwatch_main(cx);
                    store.main_slot.reset(None, Vec::new(), false, None, None);
                    store.persist_prefs();
                }
                let valid_proj = store
                    .new_session
                    .project_id
                    .as_deref()
                    .is_some_and(|pid| store.projects.iter().any(|p| p.id == pid));
                if !valid_proj {
                    store.new_session.project_id = store.projects.first().map(|p| p.id.clone());
                }
                cx.notify();
                if let Some(sid) = store.selected_id.clone() {
                    store.open_main_slot(sid, cx);
                }
            });
        })
        .detach();
    }

    pub fn load_models(&mut self, cx: &mut Context<Self>) {
        if self.models_loading {
            return;
        }
        self.models_loading = true;
        cx.notify();
        let rpc = self.rpc.clone();
        cx.spawn(async move |this, cx| {
            let res = rpc
                .call::<ModelsListResult>(json!({ "type": "models.list" }))
                .await;
            let _ = this.update(cx, |store, cx| {
                store.models_loading = false;
                match res {
                    Ok(data) => {
                        store.models = data.models;
                        store.default_model_id = data.default_model_id;
                        store.models_error = None;
                    }
                    Err(err) => {
                        store.models_error = Some(err);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn unwatch_main(&mut self, cx: &mut Context<Self>) {
        let prev = self.main_slot.watching.take();
        if prev.is_some() && self.connection == ConnectionState::Connected {
            let rpc = self.rpc.clone();
            cx.spawn(async move |_, _| {
                let _ = rpc
                    .call_unit(json!({
                        "type": "session.unwatch",
                        "slot": WatchSlot::Main.as_str(),
                    }))
                    .await;
            })
            .detach();
        }
    }

    pub fn open_main_slot(&mut self, session_id: String, cx: &mut Context<Self>) {
        self.main_slot.open_seq += 1;
        let seq = self.main_slot.open_seq;
        let switched = self.main_slot.session_id.as_deref() != Some(session_id.as_str());
        self.main_slot.watching = Some(session_id.clone());
        self.main_slot.session_id = Some(session_id.clone());
        if switched {
            self.main_slot.items.clear();
            self.main_slot.blocks.clear();
            self.main_slot.has_earlier = false;
            self.main_slot.state = None;
            cx.emit(AppStoreEvent::TranscriptReset { block_count: 0 });
        }
        self.main_slot.loading = true;
        self.main_slot.error = None;
        cx.notify();

        let rpc = self.rpc.clone();
        cx.spawn(async move |this, cx| {
            let res = rpc
                .call::<SessionOpenResult>(json!({
                    "type": "session.open",
                    "sessionId": session_id,
                    "limit": OPEN_LIMIT,
                    "slot": WatchSlot::Main.as_str(),
                }))
                .await;
            let _ = this.update(cx, |store, cx| {
                if store.main_slot.open_seq != seq
                    || store.main_slot.watching.as_deref() != Some(session_id.as_str())
                {
                    return;
                }
                match res {
                    Ok(data) => {
                        let pp = store.project_path_for_session(&session_id);
                        store.main_slot.reset(
                            Some(session_id),
                            data.items,
                            data.has_earlier,
                            Some(data.state),
                            pp.as_deref(),
                        );
                        cx.emit(AppStoreEvent::TranscriptReset {
                            block_count: store.main_slot.blocks.len(),
                        });
                    }
                    Err(err) => {
                        store.main_slot.loading = false;
                        store.main_slot.error = Some(err);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn select_session(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.selection_version += 1;
        self.selected_id = id.clone();
        self.persist_prefs();

        let draft_key = self.active_draft_key();
        let draft_text = self.draft_text(&draft_key);
        cx.emit(AppStoreEvent::DraftSync {
            key: draft_key,
            text: draft_text,
        });

        let Some(sid) = id else {
            self.unwatch_main(cx);
            self.main_slot.reset(None, Vec::new(), false, None, None);
            cx.emit(AppStoreEvent::TranscriptReset { block_count: 0 });
            cx.notify();
            return;
        };

        if self.connection == ConnectionState::Connected {
            self.open_main_slot(sid, cx);
        } else {
            self.main_slot.watching = None;
            cx.notify();
        }
    }

    pub fn start_new(&mut self, project_id: Option<String>, cx: &mut Context<Self>) {
        if let Some(pid) = project_id {
            self.new_session.project_id = Some(pid);
        }
        self.select_session(None, cx);
    }

    pub fn update_new_session(
        &mut self,
        f: impl FnOnce(&mut NewSessionOptions),
        cx: &mut Context<Self>,
    ) {
        f(&mut self.new_session);
        self.persist_prefs();
        cx.notify();
    }

    pub fn add_project(&mut self, cx: &mut Context<Self>) {
        let rpc = self.rpc.clone();
        cx.spawn(async move |this, cx| {
            let Some(path) = shell_bridge::pick_directory().await else {
                return;
            };
            let res = rpc
                .call::<ProjectAddResult>(json!({ "type": "projects.add", "path": path }))
                .await;
            let _ = this.update(cx, |store, cx| match res {
                Ok(data) => {
                    let project = data.project;
                    if !store.projects.iter().any(|p| p.id == project.id) {
                        store.projects.push(project.clone());
                    }
                    if store.new_session.project_id.is_none() {
                        store.new_session.project_id = Some(project.id.clone());
                        store.persist_prefs();
                    }
                    store.emit_toast(ToastTone::Success, format!("已添加项目：{}", project.name), cx);
                    cx.notify();
                }
                Err(err) => store.emit_toast(ToastTone::Error, err, cx),
            });
        })
        .detach();
    }

    pub fn remove_project(&mut self, project_id: String, cx: &mut Context<Self>) {
        let rpc = self.rpc.clone();
        cx.spawn(async move |this, cx| {
            let res = rpc
                .call_unit(json!({ "type": "projects.remove", "projectId": project_id }))
                .await;
            let _ = this.update(cx, |store, cx| match res {
                Ok(()) => {
                    let removed: Vec<String> = store
                        .sessions
                        .values()
                        .filter(|s| s.project_id == project_id)
                        .map(|s| s.id.clone())
                        .collect();
                    for id in removed {
                        store.forget_session(&id, cx);
                    }
                    store.projects.retain(|p| p.id != project_id);
                    if store.new_session.project_id.as_deref() == Some(project_id.as_str()) {
                        store.new_session.project_id =
                            store.projects.first().map(|p| p.id.clone());
                        store.persist_prefs();
                    }
                    store.emit_toast(ToastTone::Info, "已移除项目", cx);
                    cx.notify();
                }
                Err(err) => store.emit_toast(ToastTone::Error, err, cx),
            });
        })
        .detach();
    }

    pub fn create_and_prompt(&mut self, cx: &mut Context<Self>) {
        if self.sending_drafts.contains(NEW_DRAFT) {
            return;
        }
        let Some(project_id) = self.active_project().map(|p| p.id.clone()) else {
            return;
        };
        let text = self.draft_text(NEW_DRAFT);
        let images = self.draft_images_for(NEW_DRAFT);
        if text.trim().is_empty() && images.is_empty() {
            return;
        }
        let model_id = self.effective_new_model_id();
        let thinking = self.new_session.thinking_level;
        let approval = self.new_session.approval_mode;
        let version = self.selection_version;

        self.drafts.remove(NEW_DRAFT);
        self.draft_images.remove(NEW_DRAFT);
        self.sending_drafts.insert(NEW_DRAFT.to_owned());
        cx.emit(AppStoreEvent::DraftSync {
            key: NEW_DRAFT.to_owned(),
            text: String::new(),
        });
        cx.notify();

        let rpc = self.rpc.clone();
        cx.spawn(async move |this, cx| {
            let mut cmd = json!({
                "type": "session.create",
                "projectId": project_id,
                "approvalMode": approval,
            });
            if let Some(mid) = model_id {
                cmd["modelId"] = json!(mid);
            }
            if let Some(tl) = thinking {
                cmd["thinkingLevel"] = json!(tl);
            }
            let created = match rpc.call::<SessionResult>(cmd).await {
                Ok(res) => res.session,
                Err(err) => {
                    let _ = this.update(cx, |store, cx| {
                        store.restore_failed_draft(NEW_DRAFT, text, images, cx);
                        store.emit_toast(ToastTone::Error, err, cx);
                        store.load_models(cx);
                    });
                    return;
                }
            };
            let created_id = created.id.clone();
            let _ = this.update(cx, |store, cx| {
                let follow = store.selection_version == version && store.selected_id.is_none();
                store.sessions.insert(created_id.clone(), created);
                store.sending_drafts.remove(NEW_DRAFT);
                store.sending_drafts.insert(created_id.clone());
                if follow {
                    let carried_text = store.draft_text(NEW_DRAFT);
                    let carried_imgs = store.draft_images_for(NEW_DRAFT);
                    store.drafts.remove(NEW_DRAFT);
                    store.draft_images.remove(NEW_DRAFT);
                    if !carried_text.is_empty() {
                        store.drafts.insert(created_id.clone(), carried_text.clone());
                    }
                    if !carried_imgs.is_empty() {
                        store.draft_images.insert(created_id.clone(), carried_imgs);
                    }
                    store.selected_id = Some(created_id.clone());
                    store.persist_prefs();
                    cx.emit(AppStoreEvent::DraftSync {
                        key: created_id.clone(),
                        text: carried_text,
                    });
                    if store.connection == ConnectionState::Connected {
                        store.open_main_slot(created_id.clone(), cx);
                    }
                }
                cx.notify();
            });

            let send_res = send_prompt_rpc(&rpc, &created_id, text.trim(), &images, None).await;
            let _ = this.update(cx, |store, cx| match send_res {
                Ok(toast) => {
                    store.sending_drafts.remove(&created_id);
                    if let Some((tone, msg)) = toast {
                        store.emit_toast(tone, msg, cx);
                    }
                    cx.notify();
                }
                Err(err) => {
                    store.restore_failed_draft(&created_id, text, images, cx);
                    store.emit_toast(ToastTone::Error, err, cx);
                }
            });
        })
        .detach();
    }

    pub fn send_draft(
        &mut self,
        session_id: String,
        mode: Option<&'static str>,
        cx: &mut Context<Self>,
    ) {
        if self.sending_drafts.contains(&session_id) {
            return;
        }
        let text = self.draft_text(&session_id);
        let images = self.draft_images_for(&session_id);
        if text.trim().is_empty() && images.is_empty() {
            return;
        }
        self.drafts.remove(&session_id);
        self.draft_images.remove(&session_id);
        self.sending_drafts.insert(session_id.clone());
        cx.emit(AppStoreEvent::DraftSync {
            key: session_id.clone(),
            text: String::new(),
        });
        cx.notify();

        let rpc = self.rpc.clone();
        cx.spawn(async move |this, cx| {
            let res = send_prompt_rpc(&rpc, &session_id, text.trim(), &images, mode).await;
            let _ = this.update(cx, |store, cx| match res {
                Ok(toast) => {
                    store.sending_drafts.remove(&session_id);
                    if let Some((tone, msg)) = toast {
                        store.emit_toast(tone, msg, cx);
                    }
                    cx.notify();
                }
                Err(err) => {
                    store.restore_failed_draft(&session_id, text, images, cx);
                    store.emit_toast(ToastTone::Error, err, cx);
                }
            });
        })
        .detach();
    }

    fn restore_failed_draft(
        &mut self,
        key: &str,
        unsent_text: String,
        unsent_images: Vec<DraftImage>,
        cx: &mut Context<Self>,
    ) {
        self.sending_drafts.remove(key);
        let typed = self.draft_text(key);
        let merged = match (unsent_text.is_empty(), typed.is_empty()) {
            (true, _) => typed,
            (_, true) => unsent_text,
            (false, false) => format!("{unsent_text}\n\n{typed}"),
        };
        if !merged.is_empty() {
            self.drafts.insert(key.to_owned(), merged.clone());
        }
        if !unsent_images.is_empty() {
            let entry = self.draft_images.entry(key.to_owned()).or_default();
            let mut combined = unsent_images;
            combined.append(entry);
            *entry = combined;
        }
        if self.active_draft_key() == key {
            cx.emit(AppStoreEvent::DraftSync {
                key: key.to_owned(),
                text: merged,
            });
        }
        cx.notify();
    }

    pub fn abort(&mut self, session_id: String, cx: &mut Context<Self>) {
        let rpc = self.rpc.clone();
        cx.spawn(async move |this, cx| {
            if let Err(err) = rpc
                .call_unit(json!({ "type": "session.abort", "sessionId": session_id }))
                .await
            {
                let _ = this.update(cx, |store, cx| store.emit_toast(ToastTone::Error, err, cx));
            }
        })
        .detach();
    }

    pub fn configure(
        &mut self,
        session_id: String,
        model_id: Option<String>,
        thinking_level: Option<ThinkingLevel>,
        approval_mode: Option<ApprovalMode>,
        cx: &mut Context<Self>,
    ) {
        let rpc = self.rpc.clone();
        cx.spawn(async move |this, cx| {
            let mut cmd = json!({ "type": "session.configure", "sessionId": session_id });
            if let Some(m) = model_id {
                cmd["modelId"] = json!(m);
            }
            if let Some(t) = thinking_level {
                cmd["thinkingLevel"] = json!(t);
            }
            if let Some(a) = approval_mode {
                cmd["approvalMode"] = json!(a);
            }
            let res = rpc.call::<SessionConfigureResult>(cmd).await;
            let _ = this.update(cx, |store, cx| match res {
                Ok(data) => {
                    if store.main_slot.watching.as_deref() == Some(session_id.as_str()) {
                        store.main_slot.state = Some(data.state);
                        cx.notify();
                    }
                }
                Err(err) => store.emit_toast(ToastTone::Error, err, cx),
            });
        })
        .detach();
    }

    pub fn rename_session(&mut self, session_id: String, title: String, cx: &mut Context<Self>) {
        let trimmed = title.trim().to_owned();
        if trimmed.is_empty() {
            return;
        }
        let rpc = self.rpc.clone();
        cx.spawn(async move |this, cx| {
            let res = rpc
                .call::<SessionResult>(json!({
                    "type": "session.rename",
                    "sessionId": session_id,
                    "title": trimmed,
                }))
                .await;
            let _ = this.update(cx, |store, cx| match res {
                Ok(data) => {
                    store.sessions.insert(data.session.id.clone(), data.session);
                    cx.notify();
                }
                Err(err) => store.emit_toast(ToastTone::Error, err, cx),
            });
        })
        .detach();
    }

    pub fn delete_session(&mut self, session_id: String, cx: &mut Context<Self>) {
        let rpc = self.rpc.clone();
        cx.spawn(async move |this, cx| {
            let res = rpc
                .call_unit(json!({ "type": "session.delete", "sessionId": session_id }))
                .await;
            let _ = this.update(cx, |store, cx| match res {
                Ok(()) => {
                    store.forget_session(&session_id, cx);
                    store.emit_toast(ToastTone::Info, "会话已移入回收目录", cx);
                    cx.notify();
                }
                Err(err) => store.emit_toast(ToastTone::Error, err, cx),
            });
        })
        .detach();
    }

    pub fn continue_subsession(&mut self, session_id: String, cx: &mut Context<Self>) {
        let version = self.selection_version;
        let rpc = self.rpc.clone();
        cx.spawn(async move |this, cx| {
            let res = rpc
                .call::<SessionResult>(json!({ "type": "session.fork", "sessionId": session_id }))
                .await;
            let _ = this.update(cx, |store, cx| match res {
                Ok(data) => {
                    let new_id = data.session.id.clone();
                    store.sessions.insert(new_id.clone(), data.session);
                    if store.selection_version == version {
                        store.select_session(Some(new_id), cx);
                    }
                    cx.notify();
                }
                Err(err) => store.emit_toast(ToastTone::Error, err, cx),
            });
        })
        .detach();
    }

    pub fn compact(&mut self, session_id: String, cx: &mut Context<Self>) {
        let rpc = self.rpc.clone();
        cx.spawn(async move |this, cx| {
            let res = rpc
                .call_unit(json!({ "type": "session.compact", "sessionId": session_id }))
                .await;
            let _ = this.update(cx, |store, cx| match res {
                Ok(()) => store.emit_toast(ToastTone::Info, "已开始压缩上下文", cx),
                Err(err) => store.emit_toast(ToastTone::Error, err, cx),
            });
        })
        .detach();
    }

    pub fn reload(&mut self, session_id: String, cx: &mut Context<Self>) {
        let rpc = self.rpc.clone();
        cx.spawn(async move |this, cx| {
            let res = rpc
                .call_unit(json!({ "type": "session.reload", "sessionId": session_id }))
                .await;
            let _ = this.update(cx, |store, cx| match res {
                Ok(()) => store.emit_toast(ToastTone::Success, "已重新加载技能、扩展和 MCP", cx),
                Err(err) => store.emit_toast(ToastTone::Error, err, cx),
            });
        })
        .detach();
    }

    pub fn rewind(
        &mut self,
        session_id: String,
        item_id: String,
        images: Vec<ImageInput>,
        cx: &mut Context<Self>,
    ) {
        let rpc = self.rpc.clone();
        cx.spawn(async move |this, cx| {
            let res = rpc
                .call::<SessionRewindResult>(json!({
                    "type": "session.rewind",
                    "sessionId": session_id,
                    "itemId": item_id,
                }))
                .await;
            let _ = this.update(cx, |store, cx| match res {
                Ok(data) => {
                    store.drafts.insert(session_id.clone(), data.text.clone());
                    if !images.is_empty() {
                        let mapped: Vec<DraftImage> = images
                            .into_iter()
                            .enumerate()
                            .map(|(i, img)| DraftImage {
                                id: format!("rewind-{i}"),
                                name: format!("图片 {}", i + 1),
                                mime_type: img.mime_type,
                                data: img.data,
                            })
                            .collect();
                        store.draft_images.insert(session_id.clone(), mapped);
                    }
                    cx.emit(AppStoreEvent::DraftSync {
                        key: session_id.clone(),
                        text: data.text,
                    });
                    store.emit_toast(ToastTone::Info, "已回退到该条消息，可编辑后重发", cx);
                    cx.notify();
                }
                Err(err) => store.emit_toast(ToastTone::Error, err, cx),
            });
        })
        .detach();
    }

    pub fn respond_pending(
        &mut self,
        session_id: String,
        pending_id: String,
        answer: PendingAnswer,
        cx: &mut Context<Self>,
    ) {
        let rpc = self.rpc.clone();
        cx.spawn(async move |this, cx| {
            if let Err(err) = rpc
                .call_unit(json!({
                    "type": "session.respond",
                    "sessionId": session_id,
                    "pendingId": pending_id,
                    "answer": answer,
                }))
                .await
            {
                let _ = this.update(cx, |store, cx| store.emit_toast(ToastTone::Error, err, cx));
            }
        })
        .detach();
    }

    pub fn load_earlier(&mut self, cx: &mut Context<Self>) {
        let Some(sid) = self.main_slot.session_id.clone() else {
            return;
        };
        let Some(before_id) = self.main_slot.items.first().map(|x| x.id().to_owned()) else {
            return;
        };
        if !self.main_slot.has_earlier || self.main_slot.loading_earlier {
            return;
        }
        self.main_slot.loading_earlier = true;
        cx.notify();

        let rpc = self.rpc.clone();
        cx.spawn(async move |this, cx| {
            let res = rpc
                .call::<SessionEarlierResult>(json!({
                    "type": "session.earlier",
                    "sessionId": sid,
                    "beforeId": before_id,
                    "limit": EARLIER_LIMIT,
                }))
                .await;
            let _ = this.update(cx, |store, cx| {
                store.main_slot.loading_earlier = false;
                if store.main_slot.session_id.as_deref() != Some(sid.as_str()) {
                    return;
                }
                match res {
                    Ok(data) => {
                        let existing_ids: HashSet<String> = store
                            .main_slot
                            .items
                            .iter()
                            .map(|x| x.id().to_owned())
                            .collect();
                        let mut prepended: Vec<TranscriptItem> = data
                            .items
                            .into_iter()
                            .filter(|x| !existing_ids.contains(x.id()))
                            .collect();
                        prepended.append(&mut store.main_slot.items);
                        store.main_slot.items = prepended;
                        store.main_slot.has_earlier = data.has_earlier;
                        let pp = store.project_path_for_session(&sid);
                        store.main_slot.rebuild_blocks(pp.as_deref());
                        cx.emit(AppStoreEvent::TranscriptReset {
                            block_count: store.main_slot.blocks.len(),
                        });
                    }
                    Err(err) => store.emit_toast(ToastTone::Error, err, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn list_commands_rpc(
        &self,
        project_id: String,
        session_id: Option<String>,
    ) -> impl std::future::Future<Output = Vec<SlashCommand>> + use<> {
        let rpc = self.rpc.clone();
        async move {
            let mut cmd = json!({ "type": "commands.list", "projectId": project_id });
            if let Some(sid) = session_id {
                cmd["sessionId"] = json!(sid);
            }
            rpc.call::<CommandsListResult>(cmd)
                .await
                .map(|r| r.commands)
                .unwrap_or_default()
        }
    }

    pub fn search_files_rpc(
        &self,
        project_id: String,
        query: String,
    ) -> impl std::future::Future<Output = Vec<String>> + use<> {
        let rpc = self.rpc.clone();
        async move {
            rpc.call::<FilesSearchResult>(json!({
                "type": "files.search",
                "projectId": project_id,
                "query": query,
                "limit": 30,
            }))
            .await
            .map(|r| r.files)
            .unwrap_or_default()
        }
    }

    pub fn is_busy(&self) -> bool {
        self.main_slot
            .state
            .as_ref()
            .is_some_and(|s| s.run_active)
    }

    pub fn session_status_label(status: SessionStatus) -> &'static str {
        match status {
            SessionStatus::Running => "运行中",
            SessionStatus::Waiting => "等待处理",
            SessionStatus::Error => "出错",
            SessionStatus::Idle => "空闲",
        }
    }
}

async fn send_prompt_rpc(
    rpc: &CoreRpc,
    session_id: &str,
    trimmed: &str,
    images: &[DraftImage],
    mode: Option<&str>,
) -> Result<Option<(ToastTone, String)>, String> {
    if trimmed == "/compact" {
        rpc.call_unit(json!({ "type": "session.compact", "sessionId": session_id }))
            .await?;
        return Ok(Some((ToastTone::Info, "已开始压缩上下文".to_owned())));
    }
    if trimmed == "/reload" {
        rpc.call_unit(json!({ "type": "session.reload", "sessionId": session_id }))
            .await?;
        return Ok(Some((
            ToastTone::Success,
            "已重新加载技能、扩展和 MCP".to_owned(),
        )));
    }
    let mut cmd = json!({
        "type": "session.prompt",
        "sessionId": session_id,
        "text": trimmed,
    });
    if !images.is_empty() {
        let payload: Vec<ImageInput> = images
            .iter()
            .map(|i| ImageInput {
                mime_type: i.mime_type.clone(),
                data: i.data.clone(),
            })
            .collect();
        cmd["images"] = json!(payload);
    }
    if let Some(m) = mode {
        cmd["mode"] = json!(m);
    }
    rpc.call_unit(cmd).await?;
    Ok(None)
}
