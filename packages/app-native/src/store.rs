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
    ApprovalMode, CommandsListResult, ConnectionState, ContextUsageInfo, FilesSearchResult,
    ImageInput, ModelInfo, ModelsListResult, PendingAnswer, PendingRequest, Project,
    ProjectAddResult, ProjectsListResult, SessionConfigureResult, SessionEarlierResult,
    SessionOpenResult, SessionResult, SessionRewindResult, SessionState, SessionStatus,
    SessionSummary, SessionsListResult, SlashCommand, SubagentStatus, SubagentToolInfo,
    ThinkingLevel, ToolStatus, TranscriptItem, WatchSlot,
};
use crate::shell_bridge;
use crate::transcript_layout::{TranscriptBlock, layout_transcript};
use crate::ui::theme_setup::{
    SIDEBAR_DEFAULT_W, SIDEBAR_MAX_W, SIDEBAR_MIN_W, ThemePreference,
};
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
    sidebar_width: Option<f32>,
    theme_preference: Option<ThemePreference>,
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
    /// Fired when the active transcript list should reset to a fresh snapshot
    /// (session switch, initial open, or clear).
    TranscriptReset {
        block_count: usize,
    },
    /// Fired on incremental live updates within the active session so the
    /// scroller preserves the user's scroll position when reading history.
    TranscriptSync {
        prev_count: usize,
        next_count: usize,
    },
    /// Fired when earlier history items are prepended so scroll anchor stays put.
    TranscriptPrepend {
        added_count: usize,
    },
    /// Fired when an existing row changes height (e.g. expand/collapse toggle).
    TranscriptRemeasure,
    FocusComposer,
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
    pub demo_mode: bool,

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
    pub sidebar_width: f32,
    pub theme_preference: ThemePreference,

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
        Self::init_with_demo(false, cx)
    }

    pub fn init_with_demo(demo: bool, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| {
            if demo {
                Self::new_demo()
            } else {
                Self::new(cx)
            }
        });
        cx.set_global(GlobalStore(entity.clone()));
        entity
    }

    pub fn persisted_theme_preference() -> ThemePreference {
        load_prefs().theme_preference.unwrap_or_default()
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
                let keep = this.update(cx, |store, cx| match evt {
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
            demo_mode: false,
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
            sidebar_width: prefs
                .sidebar_width
                .unwrap_or(SIDEBAR_DEFAULT_W)
                .clamp(SIDEBAR_MIN_W, SIDEBAR_MAX_W),
            theme_preference: prefs.theme_preference.unwrap_or_default(),
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

    /// Rich offline fixture store (`--demo`) for instant visual QA of all UI states.
    fn new_demo() -> Self {
        let prefs = load_prefs();
        let now = "2026-11-24T08:40:00Z".to_owned();
        let earlier = "2026-11-24T07:15:00Z".to_owned();
        let yesterday = "2026-11-23T14:20:00Z".to_owned();

        let projects = vec![
            Project {
                id: "proj-lila".to_owned(),
                name: "lila-agent".to_owned(),
                path: "/Users/jarax/codes/lila-agent".to_owned(),
            },
            Project {
                id: "proj-studio".to_owned(),
                name: "creator-studio".to_owned(),
                path: "/Users/jarax/codes/creator-studio".to_owned(),
            },
        ];

        let mut sessions = HashMap::new();
        sessions.insert(
            "demo-main".to_owned(),
            SessionSummary {
                id: "demo-main".to_owned(),
                project_id: "proj-lila".to_owned(),
                title: "重构原生桌面端设计系统".to_owned(),
                updated_at: now.clone(),
                status: SessionStatus::Waiting,
                pending_count: 1,
                parent_id: None,
                agent_id: None,
                read_only: false,
            },
        );
        sessions.insert(
            "demo-sub".to_owned(),
            SessionSummary {
                id: "demo-sub".to_owned(),
                project_id: "proj-lila".to_owned(),
                title: "对比度与无障碍检查".to_owned(),
                updated_at: now.clone(),
                status: SessionStatus::Running,
                pending_count: 0,
                parent_id: Some("demo-main".to_owned()),
                agent_id: Some("reviewer".to_owned()),
                read_only: true,
            },
        );
        sessions.insert(
            "demo-ws".to_owned(),
            SessionSummary {
                id: "demo-ws".to_owned(),
                project_id: "proj-lila".to_owned(),
                title: "排查 WebSocket 断连重连退避".to_owned(),
                updated_at: earlier,
                status: SessionStatus::Idle,
                pending_count: 0,
                parent_id: None,
                agent_id: None,
                read_only: false,
            },
        );
        sessions.insert(
            "demo-studio".to_owned(),
            SessionSummary {
                id: "demo-studio".to_owned(),
                project_id: "proj-studio".to_owned(),
                title: "修复登录重定向与角色拉取".to_owned(),
                updated_at: yesterday,
                status: SessionStatus::Idle,
                pending_count: 0,
                parent_id: None,
                agent_id: None,
                read_only: false,
            },
        );

        let mut args_edit = HashMap::new();
        args_edit.insert(
            "path".to_owned(),
            serde_json::Value::String("packages/app-native/themes/lila.json".to_owned()),
        );
        let mut args_bash = HashMap::new();
        args_bash.insert(
            "command".to_owned(),
            serde_json::Value::String(
                "cargo test --manifest-path packages/app-native/Cargo.toml".to_owned(),
            ),
        );

        let demo_items = vec![
            TranscriptItem::User {
                id: "u1".to_owned(),
                text: "帮我把主工作区改为单纸画布 + 暖纸墨色板，并补齐输入区键盘焦点和图片粘贴。".to_owned(),
                images: Vec::new(),
                can_rewind: true,
            },
            TranscriptItem::Assistant {
                id: "a1-think".to_owned(),
                text: String::new(),
                thinking: "先核对现有 lila.json 与 theme_setup.rs 的所有硬编码色，收敛成 100% 主题驱动的语义 Token，再将侧栏直接落在原生 Blurred 玻璃上。".to_owned(),
                model: Some("claude-sonnet-4-5".to_owned()),
                streaming: false,
                error: None,
            },
            TranscriptItem::Tool {
                id: "t1".to_owned(),
                name: "edit".to_owned(),
                args: args_edit,
                status: ToolStatus::Done,
                output: "Updated packages/app-native/themes/lila.json".to_owned(),
                diff: Some(
                    "@@ -10,6 +10,6 @@\n-      \"font.size\": 16,\n-      \"radius.lg\": 16,\n+      \"font.size\": 14.5,\n+      \"radius.lg\": 12,\n       \"colors\": {\n-        \"foreground\": \"#20252c\",\n+        \"foreground\": \"#1a1918\",".to_owned(),
                ),
                duration_ms: Some(420.0),
                subagent: None,
            },
            TranscriptItem::Tool {
                id: "t2".to_owned(),
                name: "bash".to_owned(),
                args: args_bash,
                status: ToolStatus::Done,
                output: "running 5 tests\ntest result: ok. 5 passed; 0 failed".to_owned(),
                diff: None,
                duration_ms: Some(1_840.0),
                subagent: None,
            },
            TranscriptItem::Tool {
                id: "t3".to_owned(),
                name: "Agent".to_owned(),
                args: HashMap::new(),
                status: ToolStatus::Running,
                output: String::new(),
                diff: None,
                duration_ms: None,
                subagent: Some(SubagentToolInfo {
                    id: Some("demo-sub".to_owned()),
                    kind: Some("reviewer".to_owned()),
                    description: "核查浅/深色 WCAG AA 对比度".to_owned(),
                    prompt: None,
                    status: SubagentStatus::Running,
                    background: true,
                    activity: Some("rg rgb\\(|rgba\\( packages/app-native/src/ui".to_owned()),
                    model: Some("claude-sonnet-4-5".to_owned()),
                    turns: Some(3),
                    max_turns: Some(8),
                    tool_uses: Some(5),
                    tokens: Some(12_400),
                    cost: Some(0.032),
                    duration_ms: None,
                    output: None,
                    error: None,
                }),
            },
            TranscriptItem::Assistant {
                id: "a2".to_owned(),
                text: "已完成核心改造：\n\n1. **单纸画布骨架**：左侧栏直接承接 macOS `Blurred` 材质并支持拖拽调宽；右侧收敛为单一 `12px` 圆角画布，顶栏融入内部。\n2. **100% 主题驱动 Token**：浅色采用暖纸墨（`#f4f3ef` / `#ffffff` / `#1a1918`），深色采用温润石墨（`#1c1c1a` / `#232321` / `#ecebe6`），语义色全部满足 WCAG AA。\n\n```rust\n// 所有语义色均由 themes/lila.json 实时热重载\npub fn accent_ember(cx: &App) -> Hsla {\n    cx.theme().sidebar_primary\n}\n```".to_owned(),
                thinking: String::new(),
                model: Some("claude-sonnet-4-5".to_owned()),
                streaming: false,
                error: None,
            },
        ];

        let demo_state = SessionState {
            status: SessionStatus::Waiting,
            access: "rw".to_owned(),
            startup_error: None,
            run_active: false,
            model_id: Some("claude-sonnet-4-5".to_owned()),
            thinking_level: ThinkingLevel::Medium,
            thinking_levels: vec![
                ThinkingLevel::Off,
                ThinkingLevel::Low,
                ThinkingLevel::Medium,
                ThinkingLevel::High,
            ],
            approval_mode: ApprovalMode::AskExec,
            queue: Default::default(),
            context: Some(ContextUsageInfo {
                tokens: Some(42_800),
                window: 200_000,
                percent: Some(21.4),
            }),
            cost: 0.148,
            compacting: false,
            retry: None,
            pending: vec![PendingRequest {
                id: "pend-demo-1".to_owned(),
                kind: "approval".to_owned(),
                title: "允许执行终端命令？".to_owned(),
                message: Some(
                    "cargo clippy --all-targets --manifest-path packages/app-native/Cargo.toml"
                        .to_owned(),
                ),
                options: Vec::new(),
                placeholder: None,
                prefill: None,
                tool_call_id: Some("tc-demo-1".to_owned()),
                tool_name: Some("bash".to_owned()),
                questions: None,
            }],
            status_lines: HashMap::new(),
            subagents: None,
        };

        let mut main_slot = SlotState::default();
        main_slot.reset(
            Some("demo-main".to_owned()),
            demo_items,
            false,
            Some(demo_state),
            Some("/Users/jarax/codes/lila-agent"),
        );

        let mut expanded_parents = HashSet::new();
        expanded_parents.insert("demo-main".to_owned());

        Self {
            rpc: CoreRpc::noop(),
            demo_mode: true,
            connection: ConnectionState::Connected,
            connection_detail: None,
            core_version: "demo".to_owned(),
            memory_mb: shell_bridge::memory_mb(),
            projects,
            models: vec![
                ModelInfo {
                    id: "claude-sonnet-4-5".to_owned(),
                    label: "Claude Sonnet 4.5".to_owned(),
                    provider: "anthropic".to_owned(),
                    reasoning: true,
                    thinking_levels: vec![
                        ThinkingLevel::Off,
                        ThinkingLevel::Low,
                        ThinkingLevel::Medium,
                        ThinkingLevel::High,
                    ],
                    default_thinking_level: ThinkingLevel::Medium,
                    images: true,
                    context_window: Some(200_000),
                },
                ModelInfo {
                    id: "claude-opus-4-5".to_owned(),
                    label: "Claude Opus 4.5".to_owned(),
                    provider: "anthropic".to_owned(),
                    reasoning: true,
                    thinking_levels: vec![
                        ThinkingLevel::Off,
                        ThinkingLevel::Low,
                        ThinkingLevel::Medium,
                        ThinkingLevel::High,
                    ],
                    default_thinking_level: ThinkingLevel::High,
                    images: true,
                    context_window: Some(200_000),
                },
                ModelInfo {
                    id: "gpt-5.3-codex".to_owned(),
                    label: "GPT-5.3 Codex".to_owned(),
                    provider: "openai".to_owned(),
                    reasoning: true,
                    thinking_levels: vec![
                        ThinkingLevel::Off,
                        ThinkingLevel::Low,
                        ThinkingLevel::Medium,
                        ThinkingLevel::High,
                    ],
                    default_thinking_level: ThinkingLevel::Medium,
                    images: true,
                    context_window: Some(256_000),
                },
            ],
            default_model_id: Some("claude-sonnet-4-5".to_owned()),
            models_loading: false,
            models_error: None,
            sessions,
            selected_id: Some("demo-main".to_owned()),
            new_session: NewSessionOptions {
                project_id: Some("proj-lila".to_owned()),
                model_id: Some("claude-sonnet-4-5".to_owned()),
                thinking_level: Some(ThinkingLevel::Medium),
                approval_mode: ApprovalMode::AskExec,
            },
            sidebar_open: true,
            sidebar_width: prefs
                .sidebar_width
                .unwrap_or(SIDEBAR_DEFAULT_W)
                .clamp(SIDEBAR_MIN_W, SIDEBAR_MAX_W),
            theme_preference: prefs.theme_preference.unwrap_or_default(),
            drafts: HashMap::new(),
            draft_images: HashMap::new(),
            sending_drafts: HashSet::new(),
            expanded_keys: HashSet::new(),
            collapsed_projects: HashSet::new(),
            expanded_parents,
            main_slot,
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
        if self.demo_mode {
            return;
        }
        save_prefs(&NativePrefs {
            selected_session: self.selected_id.clone(),
            sidebar_open: Some(self.sidebar_open),
            sidebar_width: Some(self.sidebar_width),
            theme_preference: Some(self.theme_preference),
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

    pub fn set_sidebar_width(&mut self, width: f32, persist: bool, cx: &mut Context<Self>) {
        let clamped = width.clamp(SIDEBAR_MIN_W, SIDEBAR_MAX_W);
        if (self.sidebar_width - clamped).abs() > 0.25 {
            self.sidebar_width = clamped;
            cx.notify();
        }
        if persist {
            self.persist_prefs();
        }
    }

    pub fn set_theme_preference(&mut self, pref: ThemePreference, cx: &mut Context<Self>) {
        self.theme_preference = pref;
        if !self.demo_mode {
            self.persist_prefs();
        }
        cx.notify();
    }

    pub fn cycle_theme_preference(&mut self, cx: &mut Context<Self>) -> ThemePreference {
        let next = self.theme_preference.next();
        self.set_theme_preference(next, cx);
        next
    }

    pub fn reconnect_now(&self) {
        self.rpc.reconnect_now();
    }

    pub fn open_core_log(&self, cx: &mut Context<Self>) {
        let path = data_dir().join("core.log");
        if let Err(err) = shell_bridge::open_whitelisted_path(&path.to_string_lossy()) {
            self.emit_toast(ToastTone::Warning, format!("无法打开日志：{err}"), cx);
        }
    }

    pub fn select_adjacent_session(&mut self, delta: isize, cx: &mut Context<Self>) {
        let mut ordered: Vec<&SessionSummary> = self
            .sessions
            .values()
            .filter(|s| s.parent_id.is_none())
            .collect();
        ordered.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        if ordered.is_empty() {
            return;
        }
        let current_idx = self
            .selected_id
            .as_deref()
            .and_then(|sid| ordered.iter().position(|s| s.id == sid));
        let next_idx = match current_idx {
            Some(idx) => (idx as isize + delta).rem_euclid(ordered.len() as isize) as usize,
            None => 0,
        };
        let next_id = ordered[next_idx].id.clone();
        self.select_session(Some(next_id), cx);
    }
    pub fn is_expanded(&self, key: &str) -> bool {
        self.expanded_keys.contains(key)
    }

    pub fn toggle_expanded(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.expanded_keys.remove(&key) {
            self.expanded_keys.insert(key);
        }
        cx.emit(AppStoreEvent::TranscriptRemeasure);
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
        let prev_block_count = self.main_slot.blocks.len();
        let mut transcript_touched = false;
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
                        self.new_session.project_id = self.projects.first().map(|p| p.id.clone());
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
                        self.main_slot.upsert_item(item, main_pp.as_deref());
                        transcript_touched = true;
                    }
                }
                PushFrame::Delta {
                    session_id,
                    item_id,
                    channel,
                    delta,
                } => {
                    if self.main_slot.watching.as_deref() == Some(&session_id) {
                        self.main_slot
                            .apply_delta(&item_id, &channel, &delta, main_pp.as_deref());
                        transcript_touched = true;
                    }
                }
                PushFrame::State { session_id, state } => {
                    if self.main_slot.watching.as_deref() == Some(&session_id) {
                        let subagents_changed = self.main_slot.state.as_ref().map(|s| &s.subagents)
                            != Some(&state.subagents);
                        self.main_slot.state = Some(state);
                        if subagents_changed {
                            transcript_touched = true;
                        }
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

        if transcript_touched {
            let next_block_count = self.main_slot.blocks.len();
            cx.emit(AppStoreEvent::TranscriptSync {
                prev_count: prev_block_count,
                next_count: next_block_count,
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
            cx.emit(AppStoreEvent::FocusComposer);
            cx.notify();
            return;
        };

        let is_readonly = self
            .sessions
            .get(&sid)
            .is_some_and(|s| s.parent_id.is_some());

        if self.demo_mode {
            self.main_slot.session_id = Some(sid.clone());
            cx.emit(AppStoreEvent::TranscriptReset {
                block_count: self.main_slot.blocks.len(),
            });
            if !is_readonly {
                cx.emit(AppStoreEvent::FocusComposer);
            }
            cx.notify();
            return;
        }

        if self.connection == ConnectionState::Connected {
            self.open_main_slot(sid, cx);
        } else {
            self.main_slot.watching = None;
            if self.main_slot.session_id.as_deref() != Some(sid.as_str()) {
                self.main_slot.reset(Some(sid), Vec::new(), false, None, None);
                cx.emit(AppStoreEvent::TranscriptReset { block_count: 0 });
            }
            cx.notify();
        }
        if !is_readonly {
            cx.emit(AppStoreEvent::FocusComposer);
        }
    }

    pub fn start_new(&mut self, project_id: Option<String>, cx: &mut Context<Self>) {
        if let Some(pid) = project_id.or_else(|| {
            self.selected_id
                .as_ref()
                .and_then(|id| self.sessions.get(id))
                .map(|session| session.project_id.clone())
        }) {
            self.new_session.project_id = Some(pid);
        }
        self.select_session(None, cx);
        cx.emit(AppStoreEvent::FocusComposer);
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
                    store.emit_toast(
                        ToastTone::Success,
                        format!("已添加项目：{}", project.name),
                        cx,
                    );
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
                        store.new_session.project_id = store.projects.first().map(|p| p.id.clone());
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
                        store
                            .drafts
                            .insert(created_id.clone(), carried_text.clone());
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
        if self.demo_mode {
            if let Some(state) = self.main_slot.state.as_mut() {
                state.pending.retain(|p| p.id != pending_id);
                if state.pending.is_empty() {
                    state.status = SessionStatus::Idle;
                    if let Some(summary) = self.sessions.get_mut(&session_id) {
                        summary.status = SessionStatus::Idle;
                    }
                }
            }
            self.emit_toast(ToastTone::Success, "演示模式：已响应待处理请求", cx);
            cx.notify();
            return;
        }
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
                        let prev_len = store.main_slot.blocks.len();
                        let pp = store.project_path_for_session(&sid);
                        store.main_slot.rebuild_blocks(pp.as_deref());
                        let next_len = store.main_slot.blocks.len();
                        if next_len > prev_len {
                            cx.emit(AppStoreEvent::TranscriptPrepend {
                                added_count: next_len - prev_len,
                            });
                        } else {
                            cx.emit(AppStoreEvent::TranscriptRemeasure);
                        }
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
        self.main_slot.state.as_ref().is_some_and(|s| s.run_active)
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
