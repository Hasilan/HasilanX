pub mod acp;
pub mod codex;
pub mod jsonrpc;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use hasilanx_protocol::config::CustomAgentConfig;
use hasilanx_protocol::{
    AdapterKind, AgentDescriptor, ApprovalDecision, ApprovalOption, ExecutionMode, ModelInfo,
    NoticeLevel, ThinkingOption, ToolCallStatus, TurnCompletionStatus,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone, Default)]
pub struct DiscoveredModels {
    pub models: Vec<ModelInfo>,
    pub default_model: Option<String>,
    pub thinking_options: Vec<ThinkingOption>,
    pub default_thinking: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ApprovalAnswer {
    pub decision: ApprovalDecision,
    pub option_id: Option<String>,
}

#[derive(Debug)]
pub enum AgentEvent {
    AssistantDelta(String),
    ReasoningDelta(String),
    PlanDelta(String),
    PlanReplaced(String),
    ToolCallUpsert {
        call_id: String,
        name: String,
        status: ToolCallStatus,
        input: Option<String>,
        output: Option<String>,
    },
    ToolOutputDelta {
        call_id: String,
        delta: String,
    },
    ApprovalRequested {
        tool_name: String,
        description: String,
        args_json: Option<String>,
        options: Vec<ApprovalOption>,
        responder: oneshot::Sender<ApprovalAnswer>,
    },
    TurnCompleted {
        status: TurnCompletionStatus,
        error: Option<String>,
    },
    Notice {
        level: NoticeLevel,
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct StartSessionParams {
    pub cwd: PathBuf,
    pub resume_session_id: Option<String>,
    pub execution_mode: ExecutionMode,
    pub model: Option<String>,
    pub thinking: Option<String>,
}

#[async_trait]
pub trait AgentSession: Send + Sync {
    fn vendor_session_id(&self) -> Option<String>;
    async fn send_prompt(
        &self,
        prompt: &str,
        execution_mode: ExecutionMode,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<()>;
    async fn steer(&self, _prompt: &str) -> Result<bool> {
        Ok(false)
    }
    async fn cancel(&self) -> Result<()>;
}

pub struct AdapterRegistry {
    descriptors: RwLock<HashMap<String, AgentDescriptor>>,
    order: Vec<String>,
}

impl AdapterRegistry {
    pub fn discover(custom_agents: &[CustomAgentConfig]) -> Self {
        let mut descriptors = HashMap::new();
        let mut order = Vec::new();

        let builtins = vec![
            (
                "codex",
                "Codex CLI",
                AdapterKind::Codex,
                "codex",
                vec!["app-server".to_string()],
                true,
            ),
            (
                "agy",
                "Antigravity (ACP)",
                AdapterKind::Acp,
                "agy",
                vec!["acp".to_string()],
                false,
            ),
            (
                "opencode",
                "OpenCode (ACP)",
                AdapterKind::Acp,
                "opencode",
                vec!["acp".to_string()],
                true,
            ),
        ];

        for (id, name, kind, cmd, args, supports_steering) in builtins {
            let resolved = resolve_executable(cmd);
            let available = resolved.is_some();
            let command = resolved
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| cmd.to_string());
            descriptors.insert(
                id.to_string(),
                AgentDescriptor {
                    id: id.to_string(),
                    name: name.to_string(),
                    kind,
                    command,
                    args,
                    available,
                    supports_steering,
                    default_model: None,
                    models: Vec::new(),
                    thinking_options: Vec::new(),
                    default_thinking: None,
                },
            );
            order.push(id.to_string());
        }

        for custom in custom_agents {
            let resolved = resolve_executable(&custom.command);
            let available = resolved.is_some();
            let command = resolved
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| custom.command.clone());
            if !descriptors.contains_key(&custom.id) {
                order.push(custom.id.clone());
            }
            descriptors.insert(
                custom.id.clone(),
                AgentDescriptor {
                    id: custom.id.clone(),
                    name: custom.name.clone(),
                    kind: custom.kind,
                    command,
                    args: custom.args.clone(),
                    available,
                    supports_steering: custom.kind == AdapterKind::Codex,
                    default_model: custom.default_model.clone(),
                    models: Vec::new(),
                    thinking_options: Vec::new(),
                    default_thinking: None,
                },
            );
        }

        Self {
            descriptors: RwLock::new(descriptors),
            order,
        }
    }

    pub fn list_agents(&self) -> Vec<AgentDescriptor> {
        let guard = self.descriptors.read().expect("descriptors lock poisoned");
        self.order
            .iter()
            .filter_map(|id| guard.get(id).cloned())
            .collect()
    }

    pub fn get(&self, agent_id: &str) -> Option<AgentDescriptor> {
        self.descriptors
            .read()
            .expect("descriptors lock poisoned")
            .get(agent_id)
            .cloned()
    }

    pub async fn start_session(
        &self,
        agent_id: &str,
        params: StartSessionParams,
    ) -> Result<(Arc<dyn AgentSession>, mpsc::UnboundedReceiver<AgentEvent>)> {
        let desc = self
            .get(agent_id)
            .ok_or_else(|| anyhow!("unknown agent `{agent_id}`"))?;
        if !desc.available {
            return Err(anyhow!(
                "agent `{}` command `{}` was not found in PATH",
                desc.name,
                desc.command
            ));
        }

        match desc.kind {
            AdapterKind::Acp => {
                let (session, rx) =
                    acp::AcpSession::start(&desc.command, &desc.args, params).await?;
                Ok((Arc::new(session), rx))
            }
            AdapterKind::Codex => {
                let (session, rx) =
                    codex::CodexSession::start(&desc.command, &desc.args, params).await?;
                Ok((Arc::new(session), rx))
            }
        }
    }

    /// Queries the live agent process (`app-server` or `acp`) for its current model catalog
    /// and caches the result in this registry.
    pub async fn refresh_agent_models(&self, agent_id: &str) -> Result<AgentDescriptor> {
        let desc = self
            .get(agent_id)
            .ok_or_else(|| anyhow!("unknown agent `{agent_id}`"))?;
        if !desc.available {
            return Ok(desc);
        }

        let probe_future = async {
            match desc.kind {
                AdapterKind::Codex => codex::list_models(&desc.command, &desc.args).await,
                AdapterKind::Acp => acp::list_models(&desc.command, &desc.args).await,
            }
        };

        let discovered = tokio::time::timeout(Duration::from_secs(15), probe_future)
            .await
            .map_err(|_| anyhow!("timed out querying live models from `{agent_id}`"))??;

        let mut guard = self.descriptors.write().expect("descriptors lock poisoned");
        let entry = guard
            .get_mut(agent_id)
            .ok_or_else(|| anyhow!("unknown agent `{agent_id}`"))?;
        if !discovered.models.is_empty() {
            entry.models = discovered.models;
        }
        if discovered.default_model.is_some() {
            entry.default_model = discovered.default_model;
        }
        if !discovered.thinking_options.is_empty() {
            entry.thinking_options = discovered.thinking_options;
        }
        if discovered.default_thinking.is_some() {
            entry.default_thinking = discovered.default_thinking;
        }
        Ok(entry.clone())
    }

    pub async fn list_models(&self, agent_id: &str) -> Result<Vec<ModelInfo>> {
        let desc = self.refresh_agent_models(agent_id).await?;
        Ok(desc.models)
    }
}

pub fn resolve_executable(cmd: &str) -> Option<PathBuf> {
    let p = Path::new(cmd);
    if p.components().count() > 1 {
        return p.exists().then(|| p.to_path_buf());
    }

    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(cmd);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    if let Some(home) = dirs::home_dir() {
        let local_bin = home.join(".local/bin").join(cmd);
        if local_bin.is_file() {
            return Some(local_bin);
        }
    }
    None
}
