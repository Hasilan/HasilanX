use crate::terminal::TerminalBuffer;
use hasilanx_protocol::{
    AgentDescriptor, CheckResultRecord, DomainEvent, EventEnvelope, GitDiffSnapshot,
    PendingApproval, ProjectRecord, ProjectWorkflowConfig, QueuedPrompt, ThreadRecord,
    ThreadStatus, TimelineItem, TurnCompletionStatus, TurnRecord, WorkspaceSnapshot,
};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ThreadKey {
    pub host_id: String,
    pub thread_id: String,
}

impl ThreadKey {
    pub fn new(host_id: impl Into<String>, thread_id: impl Into<String>) -> Self {
        Self {
            host_id: host_id.into(),
            thread_id: thread_id.into(),
        }
    }
}

pub struct HostWorkspaceState {
    pub host_id: String,
    pub label: String,
    pub is_local: bool,
    pub connected: bool,
    pub status_message: String,
    pub hostname: String,
    pub daemon_version: String,
    pub latest_seq: i64,
    pub agents: Vec<AgentDescriptor>,
    pub projects: Vec<ProjectRecord>,
    pub threads: Vec<ThreadRecord>,
    pub turns: Vec<TurnRecord>,
    pub timeline_items: Vec<TimelineItem>,
    pub pending_approvals: Vec<PendingApproval>,
    pub prompt_queue: Vec<QueuedPrompt>,
    pub diffs: HashMap<String, GitDiffSnapshot>,
    pub terminals: HashMap<String, TerminalBuffer>,
    pub project_configs: HashMap<String, ProjectWorkflowConfig>,
    pub thread_checks: HashMap<String, Vec<CheckResultRecord>>,
}

impl HostWorkspaceState {
    pub fn new(host_id: String, label: String, is_local: bool) -> Self {
        Self {
            host_id,
            label,
            is_local,
            connected: false,
            status_message: "Connecting...".into(),
            hostname: String::new(),
            daemon_version: String::new(),
            latest_seq: 0,
            agents: Vec::new(),
            projects: Vec::new(),
            threads: Vec::new(),
            turns: Vec::new(),
            timeline_items: Vec::new(),
            pending_approvals: Vec::new(),
            prompt_queue: Vec::new(),
            diffs: HashMap::new(),
            terminals: HashMap::new(),
            project_configs: HashMap::new(),
            thread_checks: HashMap::new(),
        }
    }

    pub fn apply_snapshot(&mut self, snap: WorkspaceSnapshot) {
        self.latest_seq = snap.latest_seq;
        self.daemon_version = snap.daemon_version;
        self.hostname = snap.hostname;
        self.agents = snap.agents;
        self.projects = snap.projects;
        self.threads = snap.threads;
        self.turns = snap.turns;
        self.timeline_items = snap.timeline_items;
        self.pending_approvals = snap.pending_approvals;
        self.prompt_queue = snap.prompt_queue;
        self.project_configs = snap.project_configs;
        self.thread_checks = snap.thread_checks;
    }

    pub fn apply_event(&mut self, env: EventEnvelope) {
        if env.seq <= self.latest_seq {
            return;
        }
        self.latest_seq = env.seq;
        let now = env.created_at;

        match env.event {
            DomainEvent::ProjectCreated { project } => {
                if let Some(existing) = self.projects.iter_mut().find(|p| p.id == project.id) {
                    *existing = project;
                } else {
                    self.projects.push(project);
                }
            }
            DomainEvent::ProjectDeleted { project_id } => {
                self.projects.retain(|p| p.id != project_id);
                let removed_threads: Vec<String> = self
                    .threads
                    .iter()
                    .filter(|t| t.project_id == project_id)
                    .map(|t| t.id.clone())
                    .collect();
                self.threads.retain(|t| t.project_id != project_id);
                self.turns.retain(|tr| !removed_threads.contains(&tr.thread_id));
                self.timeline_items
                    .retain(|i| !removed_threads.contains(&i.thread_id));
                self.pending_approvals
                    .retain(|a| !removed_threads.contains(&a.thread_id));
                self.prompt_queue
                    .retain(|q| !removed_threads.contains(&q.thread_id));
                self.project_configs.remove(&project_id);
                for tid in removed_threads {
                    self.diffs.remove(&tid);
                    self.terminals.remove(&tid);
                    self.thread_checks.remove(&tid);
                }
            }
            DomainEvent::ThreadCreated { thread } => {
                if let Some(existing) = self.threads.iter_mut().find(|t| t.id == thread.id) {
                    *existing = thread;
                } else {
                    self.threads.insert(0, thread);
                }
            }
            DomainEvent::ThreadDeleted { thread_id } => {
                self.threads.retain(|t| t.id != thread_id);
                self.turns.retain(|tr| tr.thread_id != thread_id);
                self.timeline_items.retain(|i| i.thread_id != thread_id);
                self.pending_approvals.retain(|a| a.thread_id != thread_id);
                self.prompt_queue.retain(|q| q.thread_id != thread_id);
                self.diffs.remove(&thread_id);
                self.terminals.remove(&thread_id);
                self.thread_checks.remove(&thread_id);
            }
            DomainEvent::ThreadUpdated {
                thread_id,
                title,
                active_agent_id,
                active_model,
                active_thinking,
                execution_mode,
                status,
                last_error,
            } => {
                if let Some(t) = self.threads.iter_mut().find(|t| t.id == thread_id) {
                    if let Some(v) = title {
                        t.title = v;
                    }
                    if let Some(v) = active_agent_id {
                        t.active_agent_id = v;
                    }
                    if let Some(v) = active_model {
                        t.active_model = v;
                    }
                    if let Some(v) = active_thinking {
                        t.active_thinking = v;
                    }
                    if let Some(v) = execution_mode {
                        t.execution_mode = v;
                    }
                    if let Some(v) = status {
                        t.status = v;
                    }
                    if let Some(v) = last_error {
                        t.last_error = v;
                    }
                    t.updated_at = now;
                }
            }
            DomainEvent::TurnStarted {
                thread_id,
                turn_id,
                turn_index,
                agent_id,
                user_prompt,
                checkpoint_before_ref,
            } => {
                self.turns.push(TurnRecord {
                    id: turn_id,
                    thread_id: thread_id.clone(),
                    turn_index,
                    user_prompt,
                    agent_id,
                    status: TurnCompletionStatus::Running,
                    checkpoint_before_ref,
                    checkpoint_after_ref: None,
                    started_at: now,
                    finished_at: None,
                });
                if let Some(t) = self.threads.iter_mut().find(|t| t.id == thread_id) {
                    t.status = ThreadStatus::Running;
                    t.last_error = None;
                    t.updated_at = now;
                }
            }
            DomainEvent::TurnCompleted {
                thread_id,
                turn_id,
                status,
                error,
                checkpoint_after_ref,
            } => {
                if let Some(turn) = self.turns.iter_mut().find(|tr| tr.id == turn_id) {
                    turn.status = status;
                    if checkpoint_after_ref.is_some() {
                        turn.checkpoint_after_ref = checkpoint_after_ref;
                    }
                    turn.finished_at = Some(now);
                }
                if let Some(t) = self.threads.iter_mut().find(|t| t.id == thread_id) {
                    t.status = match status {
                        TurnCompletionStatus::Completed
                        | TurnCompletionStatus::Cancelled
                        | TurnCompletionStatus::Running => ThreadStatus::Idle,
                        TurnCompletionStatus::Failed => ThreadStatus::Error,
                    };
                    t.last_error = error;
                    t.updated_at = now;
                }
                self.pending_approvals.retain(|a| a.thread_id != thread_id);
            }
            DomainEvent::TurnReverted { thread_id, .. } => {
                if let Some(t) = self.threads.iter_mut().find(|t| t.id == thread_id) {
                    t.updated_at = now;
                }
            }
            DomainEvent::TimelineItemUpserted { item } => {
                if let Some(existing) = self.timeline_items.iter_mut().find(|i| i.id == item.id) {
                    *existing = item;
                } else {
                    self.timeline_items.push(item);
                }
            }
            DomainEvent::TimelineItemAppended {
                item_id, delta, ..
            } => {
                if let Some(existing) = self.timeline_items.iter_mut().find(|i| i.id == item_id) {
                    existing.kind.append_delta(&delta);
                    existing.updated_at = now;
                }
            }
            DomainEvent::ApprovalRequested { approval } => {
                let tid = approval.thread_id.clone();
                self.pending_approvals.push(approval);
                if let Some(t) = self.threads.iter_mut().find(|t| t.id == tid) {
                    t.status = ThreadStatus::WaitingApproval;
                    t.updated_at = now;
                }
            }
            DomainEvent::ApprovalResolved {
                approval_id,
                thread_id,
                ..
            } => {
                self.pending_approvals.retain(|a| a.id != approval_id);
                let has_more = self
                    .pending_approvals
                    .iter()
                    .any(|a| a.thread_id == thread_id);
                if !has_more {
                    if let Some(t) = self.threads.iter_mut().find(|t| t.id == thread_id) {
                        if t.status == ThreadStatus::WaitingApproval {
                            t.status = ThreadStatus::Running;
                            t.updated_at = now;
                        }
                    }
                }
            }
            DomainEvent::PromptQueued { item } => {
                self.prompt_queue.push(item);
            }
            DomainEvent::PromptDequeued { queue_id, .. } => {
                self.prompt_queue.retain(|q| q.id != queue_id);
            }
            DomainEvent::DiffUpdated { thread_id, diff } => {
                self.diffs.insert(thread_id, diff);
            }
        }
    }

    pub fn default_agent_id(&self) -> String {
        self.agents
            .iter()
            .find(|a| a.available)
            .or_else(|| self.agents.first())
            .map(|a| a.id.clone())
            .unwrap_or_else(|| "codex".to_string())
    }
}
