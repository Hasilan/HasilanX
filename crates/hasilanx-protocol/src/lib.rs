pub mod config;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMode {
    #[default]
    Local,
    Worktree,
}

impl WorkspaceMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Local => "Local",
            Self::Worktree => "Worktree",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    #[default]
    Ask,
    AcceptEdits,
    Auto,
    Plan,
}

impl ExecutionMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Ask => "Ask",
            Self::AcceptEdits => "Accept Edits",
            Self::Auto => "Auto",
            Self::Plan => "Plan",
        }
    }

    pub fn all() -> &'static [Self] {
        &[Self::Ask, Self::AcceptEdits, Self::Auto, Self::Plan]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    #[default]
    Idle,
    Running,
    WaitingApproval,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnCompletionStatus {
    Running,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Accept,
    AcceptForSession,
    Decline,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterKind {
    Acp,
    Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
}

impl FileChangeKind {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Untracked => "?",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFile {
    pub path: String,
    pub kind: FileChangeKind,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GitDiffSnapshot {
    pub is_git_repo: bool,
    pub branch: Option<String>,
    pub dirty: bool,
    pub files: Vec<ChangedFile>,
    pub unified_diff: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub path: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedPrompt {
    pub id: String,
    pub thread_id: String,
    pub prompt: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TimelineItemKind {
    UserMessage {
        text: String,
        #[serde(default)]
        attachments: Vec<Attachment>,
    },
    AssistantMessage {
        text: String,
    },
    Reasoning {
        text: String,
    },
    ToolCall {
        call_id: String,
        name: String,
        status: ToolCallStatus,
        input: Option<String>,
        output: Option<String>,
    },
    Plan {
        text: String,
    },
    SystemNotice {
        level: NoticeLevel,
        message: String,
    },
}

impl TimelineItemKind {
    pub fn append_delta(&mut self, delta: &str) {
        match self {
            Self::AssistantMessage { text }
            | Self::Reasoning { text }
            | Self::Plan { text }
            | Self::UserMessage { text, .. } => text.push_str(delta),
            Self::ToolCall { output, .. } => match output {
                Some(existing) => existing.push_str(delta),
                None => *output = Some(delta.to_string()),
            },
            Self::SystemNotice { message, .. } => message.push_str(delta),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRecord {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadRecord {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub workspace_mode: WorkspaceMode,
    pub worktree_path: Option<String>,
    pub branch_name: Option<String>,
    pub active_agent_id: String,
    pub active_model: Option<String>,
    #[serde(default)]
    pub active_thinking: Option<String>,
    pub execution_mode: ExecutionMode,
    pub status: ThreadStatus,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnRecord {
    pub id: String,
    pub thread_id: String,
    pub turn_index: i64,
    pub user_prompt: String,
    pub agent_id: String,
    pub status: TurnCompletionStatus,
    pub checkpoint_before_ref: Option<String>,
    pub checkpoint_after_ref: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineItem {
    pub id: String,
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub kind: TimelineItemKind,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalOption {
    pub id: String,
    pub label: String,
    pub decision: ApprovalDecision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingApproval {
    pub id: String,
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub tool_name: String,
    pub description: String,
    pub args_json: Option<String>,
    pub options: Vec<ApprovalOption>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDescriptor {
    pub id: String,
    pub name: String,
    pub kind: AdapterKind,
    pub command: String,
    pub args: Vec<String>,
    pub available: bool,
    pub supports_steering: bool,
    pub default_model: Option<String>,
    #[serde(default)]
    pub models: Vec<ModelInfo>,
    #[serde(default)]
    pub thinking_options: Vec<ThinkingOption>,
    #[serde(default)]
    pub default_thinking: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingOption {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub thinking_options: Vec<ThinkingOption>,
    #[serde(default)]
    pub default_thinking: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DomainEvent {
    ProjectCreated {
        project: ProjectRecord,
    },
    ProjectDeleted {
        project_id: String,
    },
    ThreadCreated {
        thread: ThreadRecord,
    },
    ThreadDeleted {
        thread_id: String,
    },
    ThreadUpdated {
        thread_id: String,
        title: Option<String>,
        active_agent_id: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_double_option"
        )]
        active_model: Option<Option<String>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_double_option"
        )]
        active_thinking: Option<Option<String>>,
        execution_mode: Option<ExecutionMode>,
        status: Option<ThreadStatus>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_double_option"
        )]
        last_error: Option<Option<String>>,
    },
    TurnStarted {
        thread_id: String,
        turn_id: String,
        turn_index: i64,
        agent_id: String,
        user_prompt: String,
        checkpoint_before_ref: Option<String>,
    },
    TurnCompleted {
        thread_id: String,
        turn_id: String,
        status: TurnCompletionStatus,
        error: Option<String>,
        checkpoint_after_ref: Option<String>,
    },
    TurnReverted {
        thread_id: String,
        turn_id: String,
        checkpoint_ref: String,
    },
    TimelineItemUpserted {
        item: TimelineItem,
    },
    TimelineItemAppended {
        thread_id: String,
        item_id: String,
        delta: String,
    },
    ApprovalRequested {
        approval: PendingApproval,
    },
    ApprovalResolved {
        approval_id: String,
        thread_id: String,
        decision: ApprovalDecision,
    },
    PromptQueued {
        item: QueuedPrompt,
    },
    PromptDequeued {
        thread_id: String,
        queue_id: String,
    },
    DiffUpdated {
        thread_id: String,
        diff: GitDiffSnapshot,
    },
}

impl DomainEvent {
    pub fn event_kind(&self) -> &'static str {
        match self {
            Self::ProjectCreated { .. } => "project_created",
            Self::ProjectDeleted { .. } => "project_deleted",
            Self::ThreadCreated { .. } => "thread_created",
            Self::ThreadDeleted { .. } => "thread_deleted",
            Self::ThreadUpdated { .. } => "thread_updated",
            Self::TurnStarted { .. } => "turn_started",
            Self::TurnCompleted { .. } => "turn_completed",
            Self::TurnReverted { .. } => "turn_reverted",
            Self::TimelineItemUpserted { .. } => "timeline_item_upserted",
            Self::TimelineItemAppended { .. } => "timeline_item_appended",
            Self::ApprovalRequested { .. } => "approval_requested",
            Self::ApprovalResolved { .. } => "approval_resolved",
            Self::PromptQueued { .. } => "prompt_queued",
            Self::PromptDequeued { .. } => "prompt_dequeued",
            Self::DiffUpdated { .. } => "diff_updated",
        }
    }

    pub fn thread_id(&self) -> Option<&str> {
        match self {
            Self::ThreadCreated { thread } => Some(&thread.id),
            Self::ThreadDeleted { thread_id }
            | Self::ThreadUpdated { thread_id, .. }
            | Self::TurnStarted { thread_id, .. }
            | Self::TurnCompleted { thread_id, .. }
            | Self::TurnReverted { thread_id, .. }
            | Self::TimelineItemAppended { thread_id, .. }
            | Self::ApprovalResolved { thread_id, .. }
            | Self::PromptDequeued { thread_id, .. }
            | Self::DiffUpdated { thread_id, .. } => Some(thread_id),
            Self::TimelineItemUpserted { item } => Some(&item.thread_id),
            Self::ApprovalRequested { approval } => Some(&approval.thread_id),
            Self::PromptQueued { item } => Some(&item.thread_id),
            Self::ProjectCreated { .. } | Self::ProjectDeleted { .. } => None,
        }
    }

    pub fn project_id(&self) -> Option<&str> {
        match self {
            Self::ProjectCreated { project } => Some(&project.id),
            Self::ProjectDeleted { project_id } => Some(project_id),
            Self::ThreadCreated { thread } => Some(&thread.project_id),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub seq: i64,
    pub event_id: String,
    pub created_at: i64,
    pub event: DomainEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WorkflowDefaults {
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub thinking: Option<String>,
    #[serde(default, alias = "mode")]
    pub execution_mode: Option<ExecutionMode>,
    #[serde(default, alias = "workspace")]
    pub workspace_mode: Option<WorkspaceMode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WorkflowRoles {
    #[serde(default)]
    pub implementer: Option<String>,
    #[serde(default)]
    pub reviewer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WorktreeHooksConfig {
    #[serde(default)]
    pub setup: Vec<String>,
    #[serde(default)]
    pub copy_files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckCommandSpec {
    pub name: String,
    #[serde(alias = "cmd")]
    pub command: String,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

fn default_only_when_dirty() -> bool {
    true
}

fn default_check_timeout_secs() -> u64 {
    120
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecksConfig {
    #[serde(default)]
    pub run_after_turn: bool,
    #[serde(default = "default_only_when_dirty")]
    pub only_when_dirty: bool,
    #[serde(default = "default_check_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub commands: Vec<CheckCommandSpec>,
}

impl Default for ChecksConfig {
    fn default() -> Self {
        Self {
            run_after_turn: false,
            only_when_dirty: default_only_when_dirty(),
            timeout_secs: default_check_timeout_secs(),
            commands: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomSlashCommand {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub prompt: String,
}

/// Repository-level workflow configuration loaded from `<project_root>/.hasilanx.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProjectWorkflowConfig {
    #[serde(default)]
    pub defaults: WorkflowDefaults,
    #[serde(default)]
    pub roles: WorkflowRoles,
    #[serde(default)]
    pub worktree: WorktreeHooksConfig,
    #[serde(default)]
    pub checks: ChecksConfig,
    #[serde(default)]
    pub commands: Vec<CustomSlashCommand>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckRunStatus {
    Running,
    Passed,
    Failed,
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResultRecord {
    pub name: String,
    pub command: String,
    pub status: CheckRunStatus,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub output: String,
    pub ran_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    pub latest_seq: i64,
    pub daemon_version: String,
    pub hostname: String,
    pub agents: Vec<AgentDescriptor>,
    pub projects: Vec<ProjectRecord>,
    pub threads: Vec<ThreadRecord>,
    #[serde(default)]
    pub turns: Vec<TurnRecord>,
    pub timeline_items: Vec<TimelineItem>,
    pub pending_approvals: Vec<PendingApproval>,
    #[serde(default)]
    pub prompt_queue: Vec<QueuedPrompt>,
    #[serde(default)]
    pub project_configs: HashMap<String, ProjectWorkflowConfig>,
    #[serde(default)]
    pub thread_checks: HashMap<String, Vec<CheckResultRecord>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SnapshotResponse {
    Full(WorkspaceSnapshot),
    Delta {
        latest_seq: i64,
        events: Vec<EventEnvelope>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub hostname: String,
    pub agents: Vec<AgentDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum RpcCommand {
    CreateProject {
        name: Option<String>,
        root_path: String,
    },
    DeleteProject {
        project_id: String,
    },
    CreateThread {
        project_id: String,
        title: Option<String>,
        agent_id: String,
        model: Option<String>,
        #[serde(default)]
        thinking: Option<String>,
        execution_mode: ExecutionMode,
        workspace_mode: WorkspaceMode,
    },
    DeleteThread {
        thread_id: String,
    },
    UpdateThreadSettings {
        thread_id: String,
        title: Option<String>,
        agent_id: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_double_option"
        )]
        model: Option<Option<String>>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_double_option"
        )]
        thinking: Option<Option<String>>,
        execution_mode: Option<ExecutionMode>,
    },
    SendPrompt {
        thread_id: String,
        prompt: String,
        #[serde(default)]
        attachments: Vec<Attachment>,
    },
    SteerTurn {
        thread_id: String,
        prompt: String,
    },
    QueuePrompt {
        thread_id: String,
        prompt: String,
        #[serde(default)]
        attachments: Vec<Attachment>,
    },
    RemoveQueuedPrompt {
        thread_id: String,
        queue_id: String,
    },
    ApprovePlan {
        thread_id: String,
        execution_mode: ExecutionMode,
    },
    CancelTurn {
        thread_id: String,
    },
    ResolveApproval {
        approval_id: String,
        decision: ApprovalDecision,
        option_id: Option<String>,
    },
    GetThreadDiff {
        thread_id: String,
    },
    RevertToTurn {
        thread_id: String,
        turn_id: String,
        use_before: bool,
    },
    GitCommit {
        thread_id: String,
        message: String,
    },
    GitPush {
        thread_id: String,
    },
    GitCreatePr {
        thread_id: String,
        title: String,
        body: String,
    },
    TerminalOpen {
        thread_id: String,
        cols: u16,
        rows: u16,
    },
    TerminalWrite {
        thread_id: String,
        data_b64: String,
    },
    TerminalResize {
        thread_id: String,
        cols: u16,
        rows: u16,
    },
    TerminalClose {
        thread_id: String,
    },
    ListModels {
        agent_id: String,
    },
    RefreshModels,
    RunProjectChecks {
        thread_id: String,
        #[serde(default)]
        check_name: Option<String>,
    },
    InitProjectWorkflow {
        project_id: String,
    },
    ReloadProjectWorkflow {
        project_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Command {
        request_id: String,
        command: RpcCommand,
    },
    Ping,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    CommandResult {
        request_id: String,
        ok: bool,
        data: Option<serde_json::Value>,
        error: Option<String>,
    },
    Event(EventEnvelope),
    DiffUpdated {
        thread_id: String,
        diff: GitDiffSnapshot,
    },
    AgentsUpdated {
        agents: Vec<AgentDescriptor>,
    },
    TerminalOutput {
        thread_id: String,
        data_b64: String,
    },
    TerminalExited {
        thread_id: String,
        code: Option<i32>,
    },
    ChecksUpdated {
        thread_id: String,
        checks: Vec<CheckResultRecord>,
    },
    ProjectConfigUpdated {
        project_id: String,
        config: Option<ProjectWorkflowConfig>,
    },
    Pong,
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Preserves the distinction between an omitted JSON field (`None`) and an explicit
/// `null` (`Some(None)`) when deserializing tri-state update fields.
fn deserialize_double_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_thread_settings_double_option_roundtrip() {
        for expected in [None, Some(None), Some(Some("gpt-5.4".to_string()))] {
            let cmd = RpcCommand::UpdateThreadSettings {
                thread_id: "t1".into(),
                title: None,
                agent_id: None,
                model: expected.clone(),
                thinking: expected.clone(),
                execution_mode: None,
            };
            let raw = serde_json::to_string(&cmd).unwrap();
            let decoded: RpcCommand = serde_json::from_str(&raw).unwrap();
            match decoded {
                RpcCommand::UpdateThreadSettings {
                    model, thinking, ..
                } => {
                    assert_eq!(model, expected);
                    assert_eq!(thinking, expected);
                }
                _ => panic!("unexpected variant"),
            }
        }
    }
}
