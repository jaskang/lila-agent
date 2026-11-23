//! Strongly-typed Rust mirror of `@lila/protocol` (PROTOCOL_VERSION = 7).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ApprovalMode {
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "ask-exec")]
    AskExec,
    #[serde(rename = "ask-all")]
    AskAll,
}

impl ApprovalMode {
    pub const ALL: [Self; 3] = [Self::Auto, Self::AskExec, Self::AskAll];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "自动执行",
            Self::AskExec => "执行命令前询问",
            Self::AskAll => "修改和执行都询问",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Auto => "所有工具直接执行，适合你信任的项目。",
            Self::AskExec => "改文件不问，运行命令、MCP 和子 Agent 前先问你。",
            Self::AskAll => "改文件和运行命令前都先问你。",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ThinkingLevel {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "关闭思考",
            Self::Minimal => "极简",
            Self::Low => "低",
            Self::Medium => "中",
            Self::High => "高",
            Self::Xhigh => "极高",
            Self::Max => "最大",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageInput {
    pub mime_type: String,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub path: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub provider: String,
    pub label: String,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub thinking_levels: Vec<ThinkingLevel>,
    #[serde(default = "default_thinking_off")]
    pub default_thinking_level: ThinkingLevel,
    #[serde(default)]
    pub images: bool,
    pub context_window: Option<u64>,
}

fn default_thinking_off() -> ThinkingLevel {
    ThinkingLevel::Off
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    #[default]
    Idle,
    Running,
    Waiting,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub updated_at: String,
    pub status: SessionStatus,
    #[serde(default)]
    pub pending_count: u32,
    pub parent_id: Option<String>,
    pub agent_id: Option<String>,
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolStatus {
    Running,
    Done,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Completed,
    Aborted,
    Error,
    Incomplete,
}

impl RunStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Completed => "已完成",
            Self::Aborted => "已中止",
            Self::Error => "出错",
            Self::Incomplete => "未完成",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubagentStatus {
    Queued,
    Running,
    Completed,
    Steered,
    Stopped,
    Aborted,
    Error,
}

impl SubagentStatus {
    pub fn is_error(self) -> bool {
        matches!(self, Self::Error | Self::Aborted | Self::Stopped)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "排队中",
            Self::Running => "运行中",
            Self::Completed => "完成",
            Self::Steered => "已插话",
            Self::Stopped => "已停止",
            Self::Aborted => "已中止",
            Self::Error => "出错",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentToolInfo {
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub description: String,
    pub prompt: Option<String>,
    #[serde(default)]
    pub background: bool,
    pub status: SubagentStatus,
    pub activity: Option<String>,
    pub model: Option<String>,
    pub turns: Option<u32>,
    pub max_turns: Option<u32>,
    pub tool_uses: Option<u32>,
    pub tokens: Option<u64>,
    pub cost: Option<f64>,
    pub duration_ms: Option<f64>,
    pub output: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentNotification {
    pub id: Option<String>,
    pub description: String,
    pub status: SubagentStatus,
    pub tool_uses: Option<u32>,
    pub turn_count: Option<u32>,
    pub total_tokens: Option<u64>,
    pub total_cost: Option<f64>,
    pub duration_ms: Option<f64>,
    pub error: Option<String>,
    pub preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TranscriptItem {
    #[serde(rename_all = "camelCase")]
    User {
        id: String,
        #[serde(default)]
        text: String,
        #[serde(default)]
        images: Vec<ImageInput>,
        #[serde(default)]
        can_rewind: bool,
    },
    #[serde(rename_all = "camelCase")]
    Assistant {
        id: String,
        #[serde(default)]
        text: String,
        #[serde(default)]
        thinking: String,
        model: Option<String>,
        #[serde(default)]
        streaming: bool,
        error: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Tool {
        id: String,
        name: String,
        #[serde(default)]
        args: HashMap<String, Value>,
        status: ToolStatus,
        #[serde(default)]
        output: String,
        diff: Option<String>,
        subagent: Option<SubagentToolInfo>,
        duration_ms: Option<f64>,
    },
    #[serde(rename_all = "camelCase")]
    Custom {
        id: String,
        custom_type: String,
        #[serde(default)]
        text: String,
        notifications: Option<Vec<SubagentNotification>>,
    },
    #[serde(rename_all = "camelCase")]
    Notice {
        id: String,
        tone: String,
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    Run {
        id: String,
        status: RunStatus,
        duration_ms: Option<f64>,
    },
}

impl TranscriptItem {
    pub fn id(&self) -> &str {
        match self {
            Self::User { id, .. }
            | Self::Assistant { id, .. }
            | Self::Tool { id, .. }
            | Self::Custom { id, .. }
            | Self::Notice { id, .. }
            | Self::Run { id, .. } => id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingQuestion {
    pub id: String,
    pub question: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub multiple: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRequest {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub message: Option<String>,
    #[serde(default)]
    pub options: Vec<String>,
    pub placeholder: Option<String>,
    pub prefill: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub questions: Option<Vec<PendingQuestion>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskQuestionAnswer {
    pub id: String,
    pub selected: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PendingAnswer {
    Cancel,
    Option { value: String },
    Confirm { confirmed: bool },
    Text { value: String },
    Ask { answers: Vec<AskQuestionAnswer> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentProgress {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub description: String,
    pub status: SubagentStatus,
    #[serde(default)]
    pub background: bool,
    pub started_at: u64,
    pub activity: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub tool_uses: u32,
    #[serde(default)]
    pub turns: u32,
    #[serde(default)]
    pub tokens: u64,
    #[serde(default)]
    pub cost: f64,
    pub duration_ms: Option<f64>,
    pub output: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionQueue {
    #[serde(default)]
    pub steering: Vec<String>,
    #[serde(default)]
    pub follow_up: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextUsageInfo {
    pub tokens: Option<u64>,
    pub window: u64,
    pub percent: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryInfo {
    pub attempt: u32,
    pub max: u32,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionState {
    pub status: SessionStatus,
    pub access: String,
    pub startup_error: Option<String>,
    #[serde(default)]
    pub run_active: bool,
    pub model_id: Option<String>,
    #[serde(default = "default_thinking_off")]
    pub thinking_level: ThinkingLevel,
    #[serde(default)]
    pub thinking_levels: Vec<ThinkingLevel>,
    pub approval_mode: ApprovalMode,
    #[serde(default)]
    pub queue: SessionQueue,
    pub context: Option<ContextUsageInfo>,
    #[serde(default)]
    pub cost: f64,
    #[serde(default)]
    pub compacting: bool,
    pub retry: Option<RetryInfo>,
    #[serde(default)]
    pub pending: Vec<PendingRequest>,
    #[serde(default)]
    pub status_lines: HashMap<String, String>,
    pub subagents: Option<Vec<SubagentProgress>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlashCommand {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub source: String,
}

impl SlashCommand {
    pub fn source_label(&self) -> &'static str {
        match self.source.as_str() {
            "builtin" => "内置",
            "extension" => "扩展",
            "prompt" => "模板",
            "skill" => "技能",
            _ => "命令",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Connected,
    Disconnected,
    Incompatible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchSlot {
    Main,
    Preview,
}

impl WatchSlot {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Preview => "preview",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionOpenResult {
    pub items: Vec<TranscriptItem>,
    pub has_earlier: bool,
    pub state: SessionState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEarlierResult {
    pub items: Vec<TranscriptItem>,
    pub has_earlier: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsListResult {
    pub models: Vec<ModelInfo>,
    pub default_model_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectsListResult {
    pub projects: Vec<Project>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectAddResult {
    pub project: Project,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionsListResult {
    pub sessions: Vec<SessionSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionResult {
    pub session: SessionSummary,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionConfigureResult {
    pub state: SessionState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRewindResult {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandsListResult {
    pub commands: Vec<SlashCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesSearchResult {
    pub files: Vec<String>,
}
