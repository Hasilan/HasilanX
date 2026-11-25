use crate::workflow;
use anyhow::{Context, Result, anyhow};
use base64::Engine as _;
use hasilanx_adapters::{AdapterRegistry, AgentEvent, AgentSession, ApprovalAnswer, StartSessionParams};
use hasilanx_protocol::{
    AgentDescriptor, Attachment, CheckResultRecord, CheckRunStatus, ChecksConfig, DomainEvent,
    EventEnvelope, ExecutionMode, NoticeLevel, PendingApproval, ProjectRecord,
    ProjectWorkflowConfig, QueuedPrompt, RpcCommand, SnapshotResponse, ThreadRecord, ThreadStatus,
    TimelineItem, TimelineItemKind, ToolCallStatus, TurnCompletionStatus, WorkspaceMode,
    WorkspaceSnapshot, new_id, now_ms,
};
use hasilanx_pty::{PtyEvent, PtyManager};
use hasilanx_store::EventStore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast, oneshot};
use tracing::{error, info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VendorSessionState {
    agent_id: String,
    session_id: String,
}

#[derive(Debug, Clone)]
struct RunningTurnMeta {
    turn_id: String,
    turn_index: i64,
    cwd: PathBuf,
}

struct ActiveRuntime {
    agent_id: String,
    session: Arc<dyn AgentSession>,
    current_turn: Arc<Mutex<Option<RunningTurnMeta>>>,
}

pub struct Orchestrator {
    store: EventStore,
    adapters: Arc<AdapterRegistry>,
    pty: Arc<PtyManager>,
    event_tx: broadcast::Sender<EventEnvelope>,
    diff_tx: broadcast::Sender<(String, hasilanx_protocol::GitDiffSnapshot)>,
    agents_tx: broadcast::Sender<Vec<AgentDescriptor>>,
    checks_tx: broadcast::Sender<(String, Vec<CheckResultRecord>)>,
    project_cfg_tx: broadcast::Sender<(String, Option<ProjectWorkflowConfig>)>,
    thread_checks: std::sync::Mutex<HashMap<String, Vec<CheckResultRecord>>>,
    runtimes: Mutex<HashMap<String, ActiveRuntime>>,
    approvals: Arc<Mutex<HashMap<String, (String, oneshot::Sender<ApprovalAnswer>)>>>,
    hostname: String,
}

impl Orchestrator {
    pub fn new(store: EventStore, adapters: AdapterRegistry) -> Arc<Self> {
        let (event_tx, _) = broadcast::channel(2048);
        let (diff_tx, _) = broadcast::channel(256);
        let (agents_tx, _) = broadcast::channel(64);
        let (checks_tx, _) = broadcast::channel(256);
        let (project_cfg_tx, _) = broadcast::channel(128);
        let hostname = std::env::var("HOSTNAME")
            .ok()
            .or_else(|| {
                std::fs::read_to_string("/etc/hostname")
                    .ok()
                    .map(|s| s.trim().to_string())
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "localhost".to_string());

        Arc::new(Self {
            store,
            adapters: Arc::new(adapters),
            pty: Arc::new(PtyManager::new()),
            event_tx,
            diff_tx,
            agents_tx,
            checks_tx,
            project_cfg_tx,
            thread_checks: std::sync::Mutex::new(HashMap::new()),
            runtimes: Mutex::new(HashMap::new()),
            approvals: Arc::new(Mutex::new(HashMap::new())),
            hostname,
        })
    }

    pub fn hostname(&self) -> &str {
        &self.hostname
    }

    pub fn list_agents(&self) -> Vec<AgentDescriptor> {
        self.adapters.list_agents()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.event_tx.subscribe()
    }

    pub fn subscribe_diffs(&self) -> broadcast::Receiver<(String, hasilanx_protocol::GitDiffSnapshot)> {
        self.diff_tx.subscribe()
    }

    pub fn subscribe_agents(&self) -> broadcast::Receiver<Vec<AgentDescriptor>> {
        self.agents_tx.subscribe()
    }

    pub fn subscribe_checks(&self) -> broadcast::Receiver<(String, Vec<CheckResultRecord>)> {
        self.checks_tx.subscribe()
    }

    pub fn subscribe_project_configs(
        &self,
    ) -> broadcast::Receiver<(String, Option<ProjectWorkflowConfig>)> {
        self.project_cfg_tx.subscribe()
    }

    /// Launches parallel non-blocking model discovery against every available agent's
    /// live `app-server` / `acp` protocol, broadcasting updates as each agent responds.
    pub fn spawn_model_discovery(self: &Arc<Self>) {
        for agent in self.adapters.list_agents() {
            if !agent.available {
                continue;
            }
            let orch = Arc::clone(self);
            let agent_id = agent.id;
            tokio::spawn(async move {
                match orch.adapters.refresh_agent_models(&agent_id).await {
                    Ok(updated) => {
                        info!(
                            agent = %updated.id,
                            models = updated.models.len(),
                            default_model = ?updated.default_model,
                            "loaded live models from agent"
                        );
                        let _ = orch.agents_tx.send(orch.adapters.list_agents());
                    }
                    Err(err) => {
                        warn!(agent = %agent_id, error = %err, "live model discovery failed");
                    }
                }
            });
        }
    }

    pub fn subscribe_pty(&self) -> broadcast::Receiver<PtyEvent> {
        self.pty.subscribe()
    }

    pub fn snapshot(&self, since_seq: Option<i64>) -> Result<SnapshotResponse> {
        if let Some(seq) = since_seq {
            if seq > 0 {
                let (latest_seq, events) = self.store.events_since(seq)?;
                return Ok(SnapshotResponse::Delta { latest_seq, events });
            }
        }
        let mut snap: WorkspaceSnapshot = self.store.load_snapshot(
            env!("CARGO_PKG_VERSION").to_string(),
            self.hostname.clone(),
            self.adapters.list_agents(),
        )?;
        for proj in &snap.projects {
            match workflow::load_project_workflow(Path::new(&proj.root_path)) {
                Ok(Some(cfg)) => {
                    snap.project_configs.insert(proj.id.clone(), cfg);
                }
                Ok(None) => {}
                Err(err) => {
                    warn!(project = %proj.name, error = %err, "failed to load .hasilanx.toml");
                }
            }
        }
        if let Ok(guard) = self.thread_checks.lock() {
            snap.thread_checks = guard.clone();
        }
        Ok(SnapshotResponse::Full(snap))
    }

    fn reload_project_config(&self, project_id: &str, root: &Path) -> Result<Option<ProjectWorkflowConfig>> {
        let cfg = workflow::load_project_workflow(root)?;
        let _ = self
            .project_cfg_tx
            .send((project_id.to_string(), cfg.clone()));
        Ok(cfg)
    }

    fn spawn_project_checks(
        self: &Arc<Self>,
        thread_id: String,
        cwd: PathBuf,
        checks_cfg: ChecksConfig,
        filter_name: Option<String>,
    ) {
        let specs: Vec<_> = checks_cfg
            .commands
            .into_iter()
            .filter(|c| {
                filter_name
                    .as_deref()
                    .map(|f| c.name.eq_ignore_ascii_case(f))
                    .unwrap_or(true)
            })
            .collect();
        if specs.is_empty() {
            return;
        }
        let timeout_secs = checks_cfg.timeout_secs;
        let now = now_ms();

        // Preserve untouched checks if running a single check by name
        let mut current: Vec<CheckResultRecord> = if filter_name.is_some() {
            self.thread_checks
                .lock()
                .ok()
                .and_then(|g| g.get(&thread_id).cloned())
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        for spec in &specs {
            let running = CheckResultRecord {
                name: spec.name.clone(),
                command: spec.command.clone(),
                status: CheckRunStatus::Running,
                exit_code: None,
                duration_ms: 0,
                output: String::new(),
                ran_at: now,
            };
            if let Some(slot) = current.iter_mut().find(|r| r.name == spec.name) {
                *slot = running;
            } else {
                current.push(running);
            }
        }

        if let Ok(mut g) = self.thread_checks.lock() {
            g.insert(thread_id.clone(), current.clone());
        }
        let _ = self.checks_tx.send((thread_id.clone(), current));

        let orch = Arc::clone(self);
        tokio::spawn(async move {
            for spec in specs {
                let res = workflow::run_check_command(&cwd, &spec, timeout_secs).await;
                let updated = {
                    let mut g = orch.thread_checks.lock().unwrap();
                    let list = g.entry(thread_id.clone()).or_default();
                    if let Some(slot) = list.iter_mut().find(|r| r.name == res.name) {
                        *slot = res;
                    } else {
                        list.push(res);
                    }
                    list.clone()
                };
                let _ = orch.checks_tx.send((thread_id.clone(), updated));
            }
        });
    }

    fn emit(&self, event: DomainEvent) -> Result<EventEnvelope> {
        let env = self.store.append_event(event)?;
        let _ = self.event_tx.send(env.clone());
        Ok(env)
    }

    fn resolve_thread_cwd(&self, thread: &ThreadRecord) -> Result<PathBuf> {
        if let Some(ref wt) = thread.worktree_path {
            return Ok(PathBuf::from(wt));
        }
        let project = self
            .store
            .get_project(&thread.project_id)?
            .ok_or_else(|| anyhow!("project not found: {}", thread.project_id))?;
        Ok(PathBuf::from(project.root_path))
    }

    async fn publish_thread_diff(&self, thread_id: &str, cwd: PathBuf) -> Result<hasilanx_protocol::GitDiffSnapshot> {
        let diff = tokio::task::spawn_blocking(move || hasilanx_git::diff_snapshot(&cwd)).await??;
        let _ = self.diff_tx.send((thread_id.to_string(), diff.clone()));
        Ok(diff)
    }

    pub async fn handle_command(self: &Arc<Self>, cmd: RpcCommand) -> Result<Value> {
        match cmd {
            RpcCommand::CreateProject { name, root_path } => {
                let expanded = expand_tilde(&root_path);
                let canonical = std::fs::canonicalize(&expanded)
                    .unwrap_or_else(|_| PathBuf::from(&expanded));
                if !canonical.exists() || !canonical.is_dir() {
                    return Err(anyhow!(
                        "directory does not exist: {}",
                        canonical.display()
                    ));
                }
                let proj_name = name
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| {
                        canonical
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| canonical.to_string_lossy().to_string())
                    });
                let project = ProjectRecord {
                    id: new_id(),
                    name: proj_name,
                    root_path: canonical.to_string_lossy().to_string(),
                    created_at: now_ms(),
                };
                self.emit(DomainEvent::ProjectCreated {
                    project: project.clone(),
                })?;
                if let Err(err) = self.reload_project_config(&project.id, &canonical) {
                    warn!(project = %project.name, error = %err, "failed to load .hasilanx.toml on project create");
                }
                Ok(serde_json::to_value(project)?)
            }

            RpcCommand::DeleteProject { project_id } => {
                self.emit(DomainEvent::ProjectDeleted { project_id })?;
                Ok(json!({ "deleted": true }))
            }

            RpcCommand::CreateThread {
                project_id,
                title,
                agent_id,
                model,
                thinking,
                execution_mode,
                workspace_mode,
            } => {
                let proj = self
                    .store
                    .get_project(&project_id)?
                    .ok_or_else(|| anyhow!("project not found: {project_id}"))?;
                if self.adapters.get(&agent_id).is_none() {
                    return Err(anyhow!("unknown agent: {agent_id}"));
                }

                let workflow_cfg = self
                    .reload_project_config(&project_id, Path::new(&proj.root_path))
                    .ok()
                    .flatten();

                let thread_id = new_id();
                let mut copied_worktree_files: Vec<String> = Vec::new();
                let (worktree_path, branch_name) = if workspace_mode == WorkspaceMode::Worktree {
                    let root = PathBuf::from(&proj.root_path);
                    let tid_clone = thread_id.clone();
                    let copy_list = workflow_cfg
                        .as_ref()
                        .map(|c| c.worktree.copy_files.clone())
                        .unwrap_or_default();
                    let (wt, copied) = tokio::task::spawn_blocking(move || {
                        let wt = hasilanx_git::create_worktree(&root, &tid_clone)?;
                        let copied = workflow::copy_worktree_files(&root, &wt.path, &copy_list);
                        Ok::<_, anyhow::Error>((wt, copied))
                    })
                    .await??;
                    copied_worktree_files = copied;
                    (
                        Some(wt.path.to_string_lossy().to_string()),
                        Some(wt.branch),
                    )
                } else {
                    let root = PathBuf::from(&proj.root_path);
                    let branch = tokio::task::spawn_blocking(move || {
                        hasilanx_git::current_branch(&root)
                    })
                    .await
                    .ok()
                    .flatten();
                    (None, branch)
                };

                let now = now_ms();
                let thread = ThreadRecord {
                    id: thread_id.clone(),
                    project_id,
                    title: title
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "New Thread".to_string()),
                    workspace_mode,
                    worktree_path: worktree_path.clone(),
                    branch_name,
                    active_agent_id: agent_id,
                    active_model: model.filter(|s| !s.trim().is_empty()),
                    active_thinking: thinking.filter(|s| !s.trim().is_empty()),
                    execution_mode,
                    status: ThreadStatus::Idle,
                    last_error: None,
                    created_at: now,
                    updated_at: now,
                };
                self.emit(DomainEvent::ThreadCreated {
                    thread: thread.clone(),
                })?;

                // Run .hasilanx.toml [worktree] setup hooks asynchronously in the new worktree
                if let (Some(wt_dir), Some(cfg)) = (worktree_path, workflow_cfg) {
                    let setup_cmds = cfg.worktree.setup;
                    if !copied_worktree_files.is_empty() || !setup_cmds.is_empty() {
                        let orch_wt = Arc::clone(self);
                        let tid_wt = thread_id;
                        tokio::spawn(async move {
                            let mut lines = Vec::new();
                            let mut any_failed = false;
                            if !copied_worktree_files.is_empty() {
                                lines.push(format!(
                                    "Copied from repo root: {}",
                                    copied_worktree_files.join(", ")
                                ));
                            }
                            if !setup_cmds.is_empty() {
                                let res = workflow::run_worktree_setup_commands(
                                    Path::new(&wt_dir),
                                    &setup_cmds,
                                )
                                .await;
                                for (cmd, ok, out) in res {
                                    if ok {
                                        lines.push(format!("`{cmd}` succeeded."));
                                    } else {
                                        any_failed = true;
                                        let snippet: String = out.chars().take(400).collect();
                                        lines.push(format!("`{cmd}` failed:\n```\n{snippet}\n```"));
                                    }
                                }
                            }
                            if !lines.is_empty() {
                                let ts = now_ms();
                                let _ = orch_wt.emit(DomainEvent::TimelineItemUpserted {
                                    item: TimelineItem {
                                        id: new_id(),
                                        thread_id: tid_wt,
                                        turn_id: None,
                                        kind: TimelineItemKind::SystemNotice {
                                            level: if any_failed {
                                                NoticeLevel::Warning
                                            } else {
                                                NoticeLevel::Info
                                            },
                                            message: format!(
                                                "**Worktree setup (`.hasilanx.toml`)**\n{}",
                                                lines.join("\n")
                                            ),
                                        },
                                        created_at: ts,
                                        updated_at: ts,
                                    },
                                });
                            }
                        });
                    }
                }
                Ok(serde_json::to_value(thread)?)
            }

            RpcCommand::DeleteThread { thread_id } => {
                let existing = self.store.get_thread(&thread_id)?;
                if let Some(rt) = self.runtimes.lock().await.remove(&thread_id) {
                    let _ = rt.session.cancel().await;
                }
                self.pty.close(&thread_id);
                if let Ok(mut g) = self.thread_checks.lock() {
                    g.remove(&thread_id);
                }

                if let Some(thread) = existing {
                    if thread.workspace_mode == WorkspaceMode::Worktree {
                        if let (Some(wt_path), Some(proj)) = (
                            thread.worktree_path.clone(),
                            self.store.get_project(&thread.project_id)?,
                        ) {
                            let root = PathBuf::from(proj.root_path);
                            let wt = PathBuf::from(wt_path);
                            let branch = thread.branch_name.clone();
                            let _ = tokio::task::spawn_blocking(move || {
                                hasilanx_git::remove_worktree(&root, &wt, branch.as_deref())
                            })
                            .await;
                        }
                    }
                }

                self.emit(DomainEvent::ThreadDeleted { thread_id })?;
                Ok(json!({ "deleted": true }))
            }

            RpcCommand::UpdateThreadSettings {
                thread_id,
                title,
                agent_id,
                model,
                thinking,
                execution_mode,
            } => {
                let existing = self
                    .store
                    .get_thread(&thread_id)?
                    .ok_or_else(|| anyhow!("thread not found: {thread_id}"))?;
                if let Some(ref next_agent) = agent_id {
                    if self.adapters.get(next_agent).is_none() {
                        return Err(anyhow!("unknown agent: {next_agent}"));
                    }
                    if next_agent != &existing.active_agent_id {
                        self.runtimes.lock().await.remove(&thread_id);
                    }
                }
                self.emit(DomainEvent::ThreadUpdated {
                    thread_id,
                    title,
                    active_agent_id: agent_id,
                    active_model: model,
                    active_thinking: thinking,
                    execution_mode,
                    status: None,
                    last_error: None,
                })?;
                Ok(json!({ "updated": true }))
            }

            RpcCommand::SendPrompt {
                thread_id,
                prompt,
                attachments,
            } => {
                let prompt = prompt.trim().to_string();
                if prompt.is_empty() {
                    return Err(anyhow!("prompt cannot be empty"));
                }
                let thread = self
                    .store
                    .get_thread(&thread_id)?
                    .ok_or_else(|| anyhow!("thread not found: {thread_id}"))?;

                // If thread is already busy, queue the follow-up prompt cleanly
                if matches!(
                    thread.status,
                    ThreadStatus::Running | ThreadStatus::WaitingApproval
                ) {
                    let item = QueuedPrompt {
                        id: new_id(),
                        thread_id,
                        prompt,
                        attachments,
                        created_at: now_ms(),
                    };
                    self.emit(DomainEvent::PromptQueued { item: item.clone() })?;
                    return Ok(json!({ "queued": true, "queue_id": item.id }));
                }

                let turn_id = self.dispatch_turn(&thread, prompt, attachments).await?;
                Ok(json!({ "turn_id": turn_id }))
            }

            RpcCommand::QueuePrompt {
                thread_id,
                prompt,
                attachments,
            } => {
                let prompt = prompt.trim().to_string();
                if prompt.is_empty() {
                    return Err(anyhow!("prompt cannot be empty"));
                }
                let thread = self
                    .store
                    .get_thread(&thread_id)?
                    .ok_or_else(|| anyhow!("thread not found: {thread_id}"))?;
                if matches!(
                    thread.status,
                    ThreadStatus::Running | ThreadStatus::WaitingApproval
                ) {
                    let item = QueuedPrompt {
                        id: new_id(),
                        thread_id,
                        prompt,
                        attachments,
                        created_at: now_ms(),
                    };
                    self.emit(DomainEvent::PromptQueued { item: item.clone() })?;
                    Ok(serde_json::to_value(item)?)
                } else {
                    let turn_id = self.dispatch_turn(&thread, prompt, attachments).await?;
                    Ok(json!({ "turn_id": turn_id }))
                }
            }

            RpcCommand::RemoveQueuedPrompt {
                thread_id,
                queue_id,
            } => {
                self.emit(DomainEvent::PromptDequeued {
                    thread_id,
                    queue_id,
                })?;
                Ok(json!({ "removed": true }))
            }

            RpcCommand::SteerTurn { thread_id, prompt } => {
                let prompt = prompt.trim().to_string();
                if prompt.is_empty() {
                    return Err(anyhow!("steering prompt cannot be empty"));
                }
                let thread = self
                    .store
                    .get_thread(&thread_id)?
                    .ok_or_else(|| anyhow!("thread not found: {thread_id}"))?;

                let session_opt = self
                    .runtimes
                    .lock()
                    .await
                    .get(&thread_id)
                    .map(|rt| Arc::clone(&rt.session));

                if let Some(sess) = session_opt {
                    if sess.steer(&prompt).await.unwrap_or(false) {
                        let now = now_ms();
                        self.emit(DomainEvent::TimelineItemUpserted {
                            item: TimelineItem {
                                id: new_id(),
                                thread_id: thread_id.clone(),
                                turn_id: None,
                                kind: TimelineItemKind::UserMessage {
                                    text: format!("[Steer] {prompt}"),
                                    attachments: vec![],
                                },
                                created_at: now,
                                updated_at: now,
                            },
                        })?;
                        return Ok(json!({ "steered": "native" }));
                    }

                    // Fallback for ACP agents without native steering: cancel + enqueue immediate follow-up
                    let item = QueuedPrompt {
                        id: new_id(),
                        thread_id: thread_id.clone(),
                        prompt: format!("[Steering correction] {prompt}"),
                        attachments: vec![],
                        created_at: now_ms(),
                    };
                    self.emit(DomainEvent::PromptQueued { item })?;
                    let _ = sess.cancel().await;
                    return Ok(json!({ "steered": "interrupt_and_continue" }));
                }

                // Not currently running: just send normally
                let turn_id = self.dispatch_turn(&thread, prompt, vec![]).await?;
                Ok(json!({ "turn_id": turn_id }))
            }

            RpcCommand::ApprovePlan {
                thread_id,
                execution_mode,
            } => {
                let next_mode = if execution_mode == ExecutionMode::Plan {
                    ExecutionMode::AcceptEdits
                } else {
                    execution_mode
                };
                self.emit(DomainEvent::ThreadUpdated {
                    thread_id: thread_id.clone(),
                    title: None,
                    active_agent_id: None,
                    active_model: None,
                    active_thinking: None,
                    execution_mode: Some(next_mode),
                    status: None,
                    last_error: None,
                })?;
                let updated = self
                    .store
                    .get_thread(&thread_id)?
                    .ok_or_else(|| anyhow!("thread not found: {thread_id}"))?;
                let turn_id = self
                    .dispatch_turn(
                        &updated,
                        "The plan is approved. Proceed to implement it now.".to_string(),
                        vec![],
                    )
                    .await?;
                Ok(json!({ "turn_id": turn_id }))
            }

            RpcCommand::CancelTurn { thread_id } => {
                let session = self
                    .runtimes
                    .lock()
                    .await
                    .get(&thread_id)
                    .map(|rt| Arc::clone(&rt.session));
                if let Some(sess) = session {
                    sess.cancel().await?;
                }
                Ok(json!({ "cancelled": true }))
            }

            RpcCommand::ResolveApproval {
                approval_id,
                decision,
                option_id,
            } => {
                let entry = self.approvals.lock().await.remove(&approval_id);
                let (thread_id, responder) = entry
                    .ok_or_else(|| anyhow!("pending approval not found: {approval_id}"))?;
                let _ = responder.send(ApprovalAnswer {
                    decision,
                    option_id,
                });
                self.emit(DomainEvent::ApprovalResolved {
                    approval_id,
                    thread_id,
                    decision,
                })?;
                Ok(json!({ "resolved": true }))
            }

            RpcCommand::GetThreadDiff { thread_id } => {
                let thread = self
                    .store
                    .get_thread(&thread_id)?
                    .ok_or_else(|| anyhow!("thread not found: {thread_id}"))?;
                let cwd = self.resolve_thread_cwd(&thread)?;
                let diff = self.publish_thread_diff(&thread_id, cwd).await?;
                Ok(serde_json::to_value(diff)?)
            }

            RpcCommand::RevertToTurn {
                thread_id,
                turn_id,
                use_before,
            } => {
                let thread = self
                    .store
                    .get_thread(&thread_id)?
                    .ok_or_else(|| anyhow!("thread not found: {thread_id}"))?;
                if matches!(
                    thread.status,
                    ThreadStatus::Running | ThreadStatus::WaitingApproval
                ) {
                    return Err(anyhow!("cannot revert while a turn is running"));
                }
                let turn = self
                    .store
                    .get_turn(&turn_id)?
                    .ok_or_else(|| anyhow!("turn not found: {turn_id}"))?;
                let checkpoint_ref = if use_before {
                    turn.checkpoint_before_ref
                } else {
                    turn.checkpoint_after_ref.or(turn.checkpoint_before_ref)
                }
                .ok_or_else(|| anyhow!("turn has no saved git checkpoint"))?;

                let cwd = self.resolve_thread_cwd(&thread)?;
                let cwd_restore = cwd.clone();
                let ref_clone = checkpoint_ref.clone();
                tokio::task::spawn_blocking(move || {
                    hasilanx_git::restore_checkpoint(&cwd_restore, &ref_clone)
                })
                .await??;

                self.emit(DomainEvent::TurnReverted {
                    thread_id: thread_id.clone(),
                    turn_id: turn_id.clone(),
                    checkpoint_ref: checkpoint_ref.clone(),
                })?;

                let now = now_ms();
                self.emit(DomainEvent::TimelineItemUpserted {
                    item: TimelineItem {
                        id: new_id(),
                        thread_id: thread_id.clone(),
                        turn_id: Some(turn_id),
                        kind: TimelineItemKind::SystemNotice {
                            level: NoticeLevel::Info,
                            message: format!(
                                "Reverted workspace files to checkpoint `{checkpoint_ref}` (turn #{}).",
                                turn.turn_index + 1
                            ),
                        },
                        created_at: now,
                        updated_at: now,
                    },
                })?;

                let diff = self.publish_thread_diff(&thread_id, cwd).await?;
                Ok(serde_json::to_value(diff)?)
            }

            RpcCommand::GitCommit { thread_id, message } => {
                let thread = self
                    .store
                    .get_thread(&thread_id)?
                    .ok_or_else(|| anyhow!("thread not found: {thread_id}"))?;
                let cwd = self.resolve_thread_cwd(&thread)?;
                let cwd_c = cwd.clone();
                let out = tokio::task::spawn_blocking(move || {
                    hasilanx_git::commit_all(&cwd_c, &message)
                })
                .await??;
                let now = now_ms();
                self.emit(DomainEvent::TimelineItemUpserted {
                    item: TimelineItem {
                        id: new_id(),
                        thread_id: thread_id.clone(),
                        turn_id: None,
                        kind: TimelineItemKind::SystemNotice {
                            level: NoticeLevel::Info,
                            message: format!("Git commit succeeded:\n{}", out.trim()),
                        },
                        created_at: now,
                        updated_at: now,
                    },
                })?;
                let _ = self.publish_thread_diff(&thread_id, cwd).await;
                Ok(json!({ "output": out }))
            }

            RpcCommand::GitPush { thread_id } => {
                let thread = self
                    .store
                    .get_thread(&thread_id)?
                    .ok_or_else(|| anyhow!("thread not found: {thread_id}"))?;
                let cwd = self.resolve_thread_cwd(&thread)?;
                let out = tokio::task::spawn_blocking(move || hasilanx_git::push_current(&cwd))
                    .await??;
                let now = now_ms();
                self.emit(DomainEvent::TimelineItemUpserted {
                    item: TimelineItem {
                        id: new_id(),
                        thread_id,
                        turn_id: None,
                        kind: TimelineItemKind::SystemNotice {
                            level: NoticeLevel::Info,
                            message: format!("Git push succeeded. {}", out.trim()),
                        },
                        created_at: now,
                        updated_at: now,
                    },
                })?;
                Ok(json!({ "output": out }))
            }

            RpcCommand::GitCreatePr {
                thread_id,
                title,
                body,
            } => {
                let thread = self
                    .store
                    .get_thread(&thread_id)?
                    .ok_or_else(|| anyhow!("thread not found: {thread_id}"))?;
                let cwd = self.resolve_thread_cwd(&thread)?;
                let url = tokio::task::spawn_blocking(move || {
                    hasilanx_git::create_pr(&cwd, &title, &body)
                })
                .await??;
                let now = now_ms();
                self.emit(DomainEvent::TimelineItemUpserted {
                    item: TimelineItem {
                        id: new_id(),
                        thread_id,
                        turn_id: None,
                        kind: TimelineItemKind::SystemNotice {
                            level: NoticeLevel::Info,
                            message: format!("Pull Request created: {url}"),
                        },
                        created_at: now,
                        updated_at: now,
                    },
                })?;
                Ok(json!({ "url": url }))
            }

            RpcCommand::TerminalOpen {
                thread_id,
                cols,
                rows,
            } => {
                let thread = self
                    .store
                    .get_thread(&thread_id)?
                    .ok_or_else(|| anyhow!("thread not found: {thread_id}"))?;
                let cwd = self.resolve_thread_cwd(&thread)?;
                self.pty.open(&thread_id, &cwd, cols, rows)?;
                Ok(json!({ "opened": true }))
            }

            RpcCommand::TerminalWrite {
                thread_id,
                data_b64,
            } => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(&data_b64)
                    .context("invalid base64 terminal input")?;
                self.pty.write(&thread_id, &bytes)?;
                Ok(json!({ "written": bytes.len() }))
            }

            RpcCommand::TerminalResize {
                thread_id,
                cols,
                rows,
            } => {
                self.pty.resize(&thread_id, cols, rows)?;
                Ok(json!({ "resized": true }))
            }

            RpcCommand::TerminalClose { thread_id } => {
                self.pty.close(&thread_id);
                Ok(json!({ "closed": true }))
            }

            RpcCommand::ListModels { agent_id } => {
                let models = self.adapters.list_models(&agent_id).await?;
                let _ = self.agents_tx.send(self.adapters.list_agents());
                Ok(serde_json::to_value(models)?)
            }

            RpcCommand::RefreshModels => {
                self.spawn_model_discovery();
                Ok(json!({ "refreshing": true }))
            }

            RpcCommand::RunProjectChecks {
                thread_id,
                check_name,
            } => {
                let thread = self
                    .store
                    .get_thread(&thread_id)?
                    .ok_or_else(|| anyhow!("thread not found: {thread_id}"))?;
                let proj = self
                    .store
                    .get_project(&thread.project_id)?
                    .ok_or_else(|| anyhow!("project not found: {}", thread.project_id))?;
                let cfg = self
                    .reload_project_config(&proj.id, Path::new(&proj.root_path))?
                    .ok_or_else(|| {
                        anyhow!("No .hasilanx.toml found in project `{}`", proj.name)
                    })?;
                if cfg.checks.commands.is_empty() {
                    return Err(anyhow!(
                        "No [[checks.commands]] configured in .hasilanx.toml"
                    ));
                }
                let cwd = self.resolve_thread_cwd(&thread)?;
                self.spawn_project_checks(thread_id, cwd, cfg.checks, check_name);
                Ok(json!({ "started": true }))
            }

            RpcCommand::InitProjectWorkflow { project_id } => {
                let proj = self
                    .store
                    .get_project(&project_id)?
                    .ok_or_else(|| anyhow!("project not found: {project_id}"))?;
                let (path, cfg) = workflow::init_project_workflow(Path::new(&proj.root_path))?;
                let _ = self
                    .project_cfg_tx
                    .send((project_id, Some(cfg.clone())));
                Ok(json!({
                    "path": path.to_string_lossy(),
                    "config": cfg,
                }))
            }

            RpcCommand::ReloadProjectWorkflow { project_id } => {
                let proj = self
                    .store
                    .get_project(&project_id)?
                    .ok_or_else(|| anyhow!("project not found: {project_id}"))?;
                let cfg = self.reload_project_config(&project_id, Path::new(&proj.root_path))?;
                Ok(serde_json::to_value(cfg)?)
            }
        }
    }

    fn dispatch_turn<'a>(
        self: &'a Arc<Self>,
        thread: &'a ThreadRecord,
        prompt: String,
        attachments: Vec<Attachment>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
        let cwd = self.resolve_thread_cwd(thread)?;
        let turn_index = self.store.next_turn_index(&thread.id)?;
        let short_tid: String = thread.id.chars().filter(|c| c.is_ascii_alphanumeric()).take(10).collect();

        // Capture hidden git checkpoint before turn mutates files
        let before_ref_name = format!("refs/hasilanx/checkpoints/{short_tid}/{turn_index}_before");
        let cwd_cp = cwd.clone();
        let ref_cp = before_ref_name.clone();
        let checkpoint_before_ref = tokio::task::spawn_blocking(move || {
            hasilanx_git::save_checkpoint(&cwd_cp, &ref_cp).ok().flatten()
        })
        .await
        .ok()
        .flatten();

        // Check if we are switching from another agent mid-thread (Cross-Agent Handoff!)
        let handoff_prefix = self.maybe_build_handoff_context(thread, &cwd).await?;

        // Auto-title untitled threads from first prompt
        if thread.title == "New Thread" {
            let auto_title: String = prompt.chars().take(48).collect();
            let _ = self.emit(DomainEvent::ThreadUpdated {
                thread_id: thread.id.clone(),
                title: Some(auto_title),
                active_agent_id: None,
                active_model: None,
                active_thinking: None,
                execution_mode: None,
                status: None,
                last_error: None,
            });
        }

        let turn_id = new_id();
        self.emit(DomainEvent::TurnStarted {
            thread_id: thread.id.clone(),
            turn_id: turn_id.clone(),
            turn_index,
            agent_id: thread.active_agent_id.clone(),
            user_prompt: prompt.clone(),
            checkpoint_before_ref,
        })?;

        let now = now_ms();
        self.emit(DomainEvent::TimelineItemUpserted {
            item: TimelineItem {
                id: new_id(),
                thread_id: thread.id.clone(),
                turn_id: Some(turn_id.clone()),
                kind: TimelineItemKind::UserMessage {
                    text: prompt.clone(),
                    attachments: attachments.clone(),
                },
                created_at: now,
                updated_at: now,
            },
        })?;

        let effective_prompt = build_effective_prompt(handoff_prefix.as_deref(), &prompt, &attachments);

        // Spawn session startup & prompt submission asynchronously so the UI transitions to Running in 0ms
        let orch_bg = Arc::clone(self);
        let thread_bg = thread.clone();
        let turn_id_bg = turn_id.clone();
        tokio::spawn(async move {
            let (session, turn_slot) = match orch_bg
                .ensure_runtime(
                    &thread_bg.id,
                    &thread_bg.active_agent_id,
                    cwd.clone(),
                    thread_bg.execution_mode,
                    thread_bg.active_model.clone(),
                    thread_bg.active_thinking.clone(),
                )
                .await
            {
                Ok(pair) => pair,
                Err(err) => {
                    let msg = err.to_string();
                    let _ = orch_bg.emit(DomainEvent::TurnCompleted {
                        thread_id: thread_bg.id.clone(),
                        turn_id: turn_id_bg,
                        status: TurnCompletionStatus::Failed,
                        error: Some(msg),
                        checkpoint_after_ref: None,
                    });
                    return;
                }
            };

            *turn_slot.lock().await = Some(RunningTurnMeta {
                turn_id: turn_id_bg.clone(),
                turn_index,
                cwd: cwd.clone(),
            });

            if let Err(err) = session
                .send_prompt(
                    &effective_prompt,
                    thread_bg.execution_mode,
                    thread_bg.active_model.as_deref(),
                    thread_bg.active_thinking.as_deref(),
                )
                .await
            {
                let msg = err.to_string();
                let _ = orch_bg.emit(DomainEvent::TurnCompleted {
                    thread_id: thread_bg.id.clone(),
                    turn_id: turn_id_bg,
                    status: TurnCompletionStatus::Failed,
                    error: Some(msg),
                    checkpoint_after_ref: None,
                });
            }
        });

        Ok(turn_id)
        })
    }

    async fn maybe_build_handoff_context(
        &self,
        thread: &ThreadRecord,
        cwd: &Path,
    ) -> Result<Option<String>> {
        let turns = self.store.list_thread_turns(&thread.id)?;
        let Some(last_turn) = turns.last() else {
            return Ok(None);
        };
        if last_turn.agent_id == thread.active_agent_id {
            return Ok(None);
        }

        // Only inject handoff when transitioning from a previous vendor to a different active agent
        let timeline = self.store.list_thread_timeline(&thread.id)?;
        let mut summary = String::new();
        summary.push_str(&format!(
            "[Context Handoff from `{}` to `{}`]\n\
             You are continuing an existing HasilanX coding thread. Here is the recent conversation & workspace state:\n\n",
            last_turn.agent_id, thread.active_agent_id
        ));

        for item in timeline.iter().rev().take(12).rev() {
            match &item.kind {
                TimelineItemKind::UserMessage { text, .. } => {
                    let clip: String = text.chars().take(400).collect();
                    summary.push_str(&format!("User: {clip}\n"));
                }
                TimelineItemKind::AssistantMessage { text } => {
                    let clip: String = text.chars().take(600).collect();
                    summary.push_str(&format!("Assistant: {clip}\n"));
                }
                TimelineItemKind::Plan { text } => {
                    let clip: String = text.chars().take(600).collect();
                    summary.push_str(&format!("Plan:\n{clip}\n"));
                }
                TimelineItemKind::ToolCall { name, status, .. } => {
                    summary.push_str(&format!("Tool `{name}` ({status:?})\n"));
                }
                _ => {}
            }
        }

        let cwd_owned = cwd.to_path_buf();
        if let Ok(Ok(diff)) = tokio::task::spawn_blocking(move || hasilanx_git::diff_snapshot(&cwd_owned)).await {
            if diff.is_git_repo {
                summary.push_str(&format!(
                    "\nWorkspace Git Branch: {}\n",
                    diff.branch.as_deref().unwrap_or("unknown")
                ));
                if !diff.files.is_empty() {
                    summary.push_str("Modified files:\n");
                    for f in diff.files.iter().take(25) {
                        summary.push_str(&format!(
                            "- [{}] {} (+{}/-{})\n",
                            f.kind.code(),
                            f.path,
                            f.additions,
                            f.deletions
                        ));
                    }
                }
            }
        }

        summary.push_str("\n[End Handoff — Now address the user's new request below]\n\n");

        let now = now_ms();
        let _ = self.emit(DomainEvent::TimelineItemUpserted {
            item: TimelineItem {
                id: new_id(),
                thread_id: thread.id.clone(),
                turn_id: None,
                kind: TimelineItemKind::SystemNotice {
                    level: NoticeLevel::Info,
                    message: format!(
                        "Automatically handed off conversation & workspace context from `{}` to `{}`.",
                        last_turn.agent_id, thread.active_agent_id
                    ),
                },
                created_at: now,
                updated_at: now,
            },
        });

        Ok(Some(summary))
    }

    async fn ensure_runtime(
        self: &Arc<Self>,
        thread_id: &str,
        agent_id: &str,
        cwd: PathBuf,
        execution_mode: ExecutionMode,
        model: Option<String>,
        thinking: Option<String>,
    ) -> Result<(Arc<dyn AgentSession>, Arc<Mutex<Option<RunningTurnMeta>>>)> {
        {
            let map = self.runtimes.lock().await;
            if let Some(rt) = map.get(thread_id) {
                if rt.agent_id == agent_id {
                    return Ok((Arc::clone(&rt.session), Arc::clone(&rt.current_turn)));
                }
            }
        }

        let resume_session_id = self
            .store
            .get_vendor_session(thread_id)?
            .and_then(|raw| serde_json::from_str::<VendorSessionState>(&raw).ok())
            .filter(|v| v.agent_id == agent_id)
            .map(|v| v.session_id);

        info!(
            thread_id = %thread_id,
            agent_id = %agent_id,
            cwd = %cwd.display(),
            resuming = resume_session_id.is_some(),
            "starting agent session"
        );

        let (session, mut rx) = self
            .adapters
            .start_session(
                agent_id,
                StartSessionParams {
                    cwd,
                    resume_session_id,
                    execution_mode,
                    model,
                    thinking,
                },
            )
            .await
            .with_context(|| format!("failed to start `{agent_id}` session"))?;

        if let Some(sid) = session.vendor_session_id() {
            let state = VendorSessionState {
                agent_id: agent_id.to_string(),
                session_id: sid,
            };
            if let Ok(json_str) = serde_json::to_string(&state) {
                let _ = self.store.set_vendor_session(thread_id, Some(&json_str));
            }
        }

        let turn_slot = Arc::new(Mutex::new(None::<RunningTurnMeta>));
        {
            let mut map = self.runtimes.lock().await;
            map.insert(
                thread_id.to_string(),
                ActiveRuntime {
                    agent_id: agent_id.to_string(),
                    session: Arc::clone(&session),
                    current_turn: Arc::clone(&turn_slot),
                },
            );
        }

        let orch = Arc::clone(self);
        let tid = thread_id.to_string();
        let turn_slot_loop = Arc::clone(&turn_slot);
        tokio::spawn(async move {
            let mut active_assistant_item: Option<String> = None;
            let mut active_reasoning_item: Option<String> = None;
            let mut active_plan_item: Option<String> = None;
            let mut active_tools: HashMap<String, TimelineItem> = HashMap::new();
            let mut pending_ev: Option<AgentEvent> = None;

            loop {
                let ev = match pending_ev.take() {
                    Some(e) => e,
                    None => match rx.recv().await {
                        Some(e) => e,
                        None => break,
                    },
                };
                let current_turn_id = turn_slot_loop
                    .lock()
                    .await
                    .as_ref()
                    .map(|m| m.turn_id.clone());

                match ev {
                    AgentEvent::AssistantDelta(mut delta) => {
                        while let Ok(next) = rx.try_recv() {
                            match next {
                                AgentEvent::AssistantDelta(more) => delta.push_str(&more),
                                other => {
                                    pending_ev = Some(other);
                                    break;
                                }
                            }
                        }
                        active_reasoning_item = None;
                        if let Some(ref item_id) = active_assistant_item {
                            let _ = orch.emit(DomainEvent::TimelineItemAppended {
                                thread_id: tid.clone(),
                                item_id: item_id.clone(),
                                delta,
                            });
                        } else {
                            let item_id = new_id();
                            let now = now_ms();
                            let _ = orch.emit(DomainEvent::TimelineItemUpserted {
                                item: TimelineItem {
                                    id: item_id.clone(),
                                    thread_id: tid.clone(),
                                    turn_id: current_turn_id.clone(),
                                    kind: TimelineItemKind::AssistantMessage { text: delta },
                                    created_at: now,
                                    updated_at: now,
                                },
                            });
                            active_assistant_item = Some(item_id);
                        }
                    }
                    AgentEvent::ReasoningDelta(mut delta) => {
                        while let Ok(next) = rx.try_recv() {
                            match next {
                                AgentEvent::ReasoningDelta(more) => delta.push_str(&more),
                                other => {
                                    pending_ev = Some(other);
                                    break;
                                }
                            }
                        }
                        active_assistant_item = None;
                        if let Some(ref item_id) = active_reasoning_item {
                            let _ = orch.emit(DomainEvent::TimelineItemAppended {
                                thread_id: tid.clone(),
                                item_id: item_id.clone(),
                                delta,
                            });
                        } else {
                            let item_id = new_id();
                            let now = now_ms();
                            let _ = orch.emit(DomainEvent::TimelineItemUpserted {
                                item: TimelineItem {
                                    id: item_id.clone(),
                                    thread_id: tid.clone(),
                                    turn_id: current_turn_id.clone(),
                                    kind: TimelineItemKind::Reasoning { text: delta },
                                    created_at: now,
                                    updated_at: now,
                                },
                            });
                            active_reasoning_item = Some(item_id);
                        }
                    }
                    AgentEvent::PlanDelta(mut delta) => {
                        while let Ok(next) = rx.try_recv() {
                            match next {
                                AgentEvent::PlanDelta(more) => delta.push_str(&more),
                                other => {
                                    pending_ev = Some(other);
                                    break;
                                }
                            }
                        }
                        if let Some(ref item_id) = active_plan_item {
                            let _ = orch.emit(DomainEvent::TimelineItemAppended {
                                thread_id: tid.clone(),
                                item_id: item_id.clone(),
                                delta,
                            });
                        } else {
                            let item_id = new_id();
                            let now = now_ms();
                            let _ = orch.emit(DomainEvent::TimelineItemUpserted {
                                item: TimelineItem {
                                    id: item_id.clone(),
                                    thread_id: tid.clone(),
                                    turn_id: current_turn_id.clone(),
                                    kind: TimelineItemKind::Plan { text: delta },
                                    created_at: now,
                                    updated_at: now,
                                },
                            });
                            active_plan_item = Some(item_id);
                        }
                    }
                    AgentEvent::PlanReplaced(text) => {
                        let item_id = active_plan_item.clone().unwrap_or_else(new_id);
                        let now = now_ms();
                        let _ = orch.emit(DomainEvent::TimelineItemUpserted {
                            item: TimelineItem {
                                id: item_id.clone(),
                                thread_id: tid.clone(),
                                turn_id: current_turn_id.clone(),
                                kind: TimelineItemKind::Plan { text },
                                created_at: now,
                                updated_at: now,
                            },
                        });
                        active_plan_item = Some(item_id);
                    }
                    AgentEvent::ToolCallUpsert {
                        call_id,
                        name,
                        status,
                        input,
                        output,
                    } => {
                        active_assistant_item = None;
                        active_reasoning_item = None;
                        let now = now_ms();
                        let input = input.filter(|s| !s.trim().is_empty());
                        let output = output.filter(|s| !s.trim().is_empty());
                        let item = if let Some(mut existing) = active_tools.remove(&call_id) {
                            if let TimelineItemKind::ToolCall {
                                name: ref mut ename,
                                status: ref mut estatus,
                                input: ref mut einput,
                                output: ref mut eoutput,
                                ..
                            } = existing.kind
                            {
                                let incoming_is_generic =
                                    name.is_empty() || name == "Tool Call" || name == "Tool Output";
                                if !incoming_is_generic {
                                    *ename = name;
                                }
                                *estatus = status;
                                if let Some(inp) = input {
                                    *einput = Some(inp);
                                }
                                if let Some(out) = output {
                                    *eoutput = Some(out);
                                }
                            }
                            existing.updated_at = now;
                            existing
                        } else {
                            let resolved_name = if name.is_empty() {
                                "Tool Call".to_string()
                            } else {
                                name
                            };
                            TimelineItem {
                                id: new_id(),
                                thread_id: tid.clone(),
                                turn_id: current_turn_id.clone(),
                                kind: TimelineItemKind::ToolCall {
                                    call_id: call_id.clone(),
                                    name: resolved_name,
                                    status,
                                    input,
                                    output,
                                },
                                created_at: now,
                                updated_at: now,
                            }
                        };
                        active_tools.insert(call_id, item.clone());
                        let _ = orch.emit(DomainEvent::TimelineItemUpserted { item });
                    }
                    AgentEvent::ToolOutputDelta {
                        call_id,
                        mut delta,
                    } => {
                        while let Ok(next) = rx.try_recv() {
                            match next {
                                AgentEvent::ToolOutputDelta {
                                    call_id: ref next_cid,
                                    delta: ref more,
                                } if next_cid == &call_id => {
                                    delta.push_str(more);
                                }
                                other => {
                                    pending_ev = Some(other);
                                    break;
                                }
                            }
                        }
                        let now = now_ms();
                        if let Some(existing) = active_tools.get_mut(&call_id) {
                            existing.kind.append_delta(&delta);
                            existing.updated_at = now;
                            let _ = orch.emit(DomainEvent::TimelineItemAppended {
                                thread_id: tid.clone(),
                                item_id: existing.id.clone(),
                                delta,
                            });
                        } else {
                            let item = TimelineItem {
                                id: new_id(),
                                thread_id: tid.clone(),
                                turn_id: current_turn_id.clone(),
                                kind: TimelineItemKind::ToolCall {
                                    call_id: call_id.clone(),
                                    name: "Tool Output".to_string(),
                                    status: ToolCallStatus::Running,
                                    input: None,
                                    output: Some(delta),
                                },
                                created_at: now,
                                updated_at: now,
                            };
                            active_tools.insert(call_id, item.clone());
                            let _ = orch.emit(DomainEvent::TimelineItemUpserted { item });
                        }
                    }
                    AgentEvent::ApprovalRequested {
                        tool_name,
                        description,
                        args_json,
                        options,
                        responder,
                    } => {
                        let approval_id = new_id();
                        let approval = PendingApproval {
                            id: approval_id.clone(),
                            thread_id: tid.clone(),
                            turn_id: current_turn_id.clone(),
                            tool_name,
                            description,
                            args_json,
                            options,
                            created_at: now_ms(),
                        };
                        orch.approvals
                            .lock()
                            .await
                            .insert(approval_id, (tid.clone(), responder));
                        if let Err(err) = orch.emit(DomainEvent::ApprovalRequested { approval }) {
                            error!(error = %err, "failed to emit approval requested event");
                        }
                    }
                    AgentEvent::TurnCompleted { status, error } => {
                        let meta = turn_slot_loop.lock().await.take();
                        active_assistant_item = None;
                        active_reasoning_item = None;
                        active_plan_item = None;
                        active_tools.clear();

                        let (turn_id, checkpoint_after_ref, maybe_cwd) = match meta {
                            Some(m) => {
                                let short_tid: String = tid
                                    .chars()
                                    .filter(|c| c.is_ascii_alphanumeric())
                                    .take(10)
                                    .collect();
                                let after_ref = format!(
                                    "refs/hasilanx/checkpoints/{short_tid}/{}_after",
                                    m.turn_index
                                );
                                let cwd_cp = m.cwd.clone();
                                let saved = tokio::task::spawn_blocking(move || {
                                    hasilanx_git::save_checkpoint(&cwd_cp, &after_ref)
                                        .ok()
                                        .flatten()
                                })
                                .await
                                .ok()
                                .flatten();
                                (m.turn_id, saved, Some(m.cwd))
                            }
                            None => (new_id(), None, None),
                        };

                        let _ = orch.emit(DomainEvent::TurnCompleted {
                            thread_id: tid.clone(),
                            turn_id,
                            status,
                            error,
                            checkpoint_after_ref,
                        });

                        // Refresh live diff after turn settles & auto-run .hasilanx.toml Quality Gates
                        if let Some(cwd_diff) = maybe_cwd {
                            let diff_dirty = orch
                                .publish_thread_diff(&tid, cwd_diff.clone())
                                .await
                                .map(|d| d.dirty || !d.files.is_empty())
                                .unwrap_or(false);

                            if status == TurnCompletionStatus::Completed {
                                if let Ok(Some(th)) = orch.store.get_thread(&tid) {
                                    if let Ok(Some(proj)) = orch.store.get_project(&th.project_id) {
                                        if let Ok(Some(cfg)) = orch.reload_project_config(
                                            &proj.id,
                                            Path::new(&proj.root_path),
                                        ) {
                                            let should_run = cfg.checks.run_after_turn
                                                && !cfg.checks.commands.is_empty()
                                                && (!cfg.checks.only_when_dirty || diff_dirty);
                                            if should_run {
                                                orch.spawn_project_checks(
                                                    tid.clone(),
                                                    cwd_diff,
                                                    cfg.checks,
                                                    None,
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Auto-dispatch next queued prompt (on Completed or Cancelled from steering)
                        if matches!(
                            status,
                            TurnCompletionStatus::Completed | TurnCompletionStatus::Cancelled
                        ) {
                            if let Ok(Some(next_q)) = orch.store.peek_next_queued_prompt(&tid) {
                                let _ = orch.emit(DomainEvent::PromptDequeued {
                                    thread_id: tid.clone(),
                                    queue_id: next_q.id.clone(),
                                });
                                let orch_next = Arc::clone(&orch);
                                let tid_next = tid.clone();
                                tokio::spawn(async move {
                                    if let Ok(Some(th)) = orch_next.store.get_thread(&tid_next) {
                                        if let Err(err) = orch_next
                                            .dispatch_turn(&th, next_q.prompt, next_q.attachments)
                                            .await
                                        {
                                            warn!(thread_id = %tid_next, error = %err, "queued prompt dispatch failed");
                                        }
                                    }
                                });
                            }
                        }
                    }
                    AgentEvent::Notice { level, message } => {
                        let now = now_ms();
                        let _ = orch.emit(DomainEvent::TimelineItemUpserted {
                            item: TimelineItem {
                                id: new_id(),
                                thread_id: tid.clone(),
                                turn_id: current_turn_id.clone(),
                                kind: TimelineItemKind::SystemNotice { level, message },
                                created_at: now,
                                updated_at: now,
                            },
                        });
                        if level == NoticeLevel::Error {
                            warn!(thread_id = %tid, "agent emitted error notice");
                        }
                    }
                }
            }

            orch.runtimes.lock().await.remove(&tid);
            if let Some(stale) = turn_slot_loop.lock().await.take() {
                let _ = orch.emit(DomainEvent::TurnCompleted {
                    thread_id: tid.clone(),
                    turn_id: stale.turn_id,
                    status: TurnCompletionStatus::Failed,
                    error: Some("Agent process terminated unexpectedly".to_string()),
                    checkpoint_after_ref: None,
                });
            }
        });

        Ok((session, turn_slot))
    }
}

fn build_effective_prompt(
    handoff_prefix: Option<&str>,
    prompt: &str,
    attachments: &[Attachment],
) -> String {
    let mut out = String::new();
    if let Some(prefix) = handoff_prefix {
        out.push_str(prefix);
    }
    if !attachments.is_empty() {
        out.push_str("[Attached files]\n");
        for att in attachments {
            out.push_str(&format!("- {} ({})\n", att.label, att.path));
        }
        out.push('\n');
    }
    out.push_str(prompt);
    out
}

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().to_string();
        }
    } else if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().to_string();
        }
    }
    path.to_string()
}
