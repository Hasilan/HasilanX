use crate::jsonrpc::{IncomingRpc, JsonRpcProcess};
use crate::{AgentEvent, AgentSession, ApprovalAnswer, DiscoveredModels, StartSessionParams};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use hasilanx_protocol::{
    ApprovalDecision, ApprovalOption, ExecutionMode, ModelInfo, NoticeLevel, ThinkingOption,
    ToolCallStatus, TurnCompletionStatus,
};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc, oneshot};
use tracing::{debug, warn};

pub struct AcpSession {
    rpc: Arc<JsonRpcProcess>,
    session_id: String,
    thought_config_id: Arc<RwLock<Option<String>>>,
    execution_mode: Arc<RwLock<ExecutionMode>>,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
}

impl AcpSession {
    pub async fn start(
        command: &str,
        args: &[String],
        params: StartSessionParams,
    ) -> Result<(Self, mpsc::UnboundedReceiver<AgentEvent>)> {
        let (rpc, mut incoming) =
            JsonRpcProcess::spawn(command, args, Some(&params.cwd), true).await?;

        let _init = rpc
            .request(
                "initialize",
                json!({
                    "protocolVersion": 1,
                    "clientInfo": {
                        "name": "hasilanx",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "clientCapabilities": {}
                }),
            )
            .await
            .context("ACP initialize failed")?;

        let cwd_str = params.cwd.to_string_lossy().to_string();
        let (session_id, initial_thought_id) = if let Some(existing_id) = params.resume_session_id {
            match rpc
                .request(
                    "session/load",
                    json!({
                        "sessionId": existing_id,
                        "cwd": cwd_str,
                        "mcpServers": [],
                    }),
                )
                .await
            {
                Ok(resp) => (existing_id, extract_thought_config_id(&resp)),
                Err(err) => {
                    debug!(error = %err, "ACP session/load failed, falling back to session/new");
                    create_new_session(&rpc, &cwd_str).await?
                }
            }
        } else {
            create_new_session(&rpc, &cwd_str).await?
        };

        let thought_config_id = Arc::new(RwLock::new(initial_thought_id));

        if let Some(model) = params.model.as_deref() {
            set_acp_model(&rpc, &session_id, model, &thought_config_id).await;
        }
        if let Some(thinking) = params.thinking.as_deref() {
            set_acp_thinking(&rpc, &session_id, &thought_config_id, thinking).await;
        }

        let execution_mode = Arc::new(RwLock::new(params.execution_mode));
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        // Spawn reader loop for ACP notifications & server requests
        let rpc_loop = Arc::clone(&rpc);
        let tx_loop = event_tx.clone();
        let mode_loop = Arc::clone(&execution_mode);
        tokio::spawn(async move {
            while let Some(msg) = incoming.recv().await {
                match msg {
                    IncomingRpc::Notification { method, params } => {
                        if method == "session/update" {
                            handle_acp_update(&params, &tx_loop);
                        } else {
                            debug!(method = %method, "unhandled ACP notification");
                        }
                    }
                    IncomingRpc::ServerRequest { id, method, params } => {
                        if method == "session/request_permission" {
                            handle_acp_permission(
                                Arc::clone(&rpc_loop),
                                id,
                                params,
                                tx_loop.clone(),
                                Arc::clone(&mode_loop),
                            )
                            .await;
                        } else {
                            debug!(method = %method, "unsupported ACP server request");
                            let _ = rpc_loop
                                .respond_error(id, -32601, &format!("unsupported method: {method}"))
                                .await;
                        }
                    }
                    IncomingRpc::Closed => {
                        let _ = tx_loop.send(AgentEvent::Notice {
                            level: NoticeLevel::Warning,
                            message: "ACP agent process exited.".to_string(),
                        });
                        break;
                    }
                }
            }
        });

        Ok((
            Self {
                rpc,
                session_id,
                thought_config_id,
                execution_mode,
                event_tx,
            },
            event_rx,
        ))
    }
}

async fn create_new_session(rpc: &JsonRpcProcess, cwd: &str) -> Result<(String, Option<String>)> {
    let resp = rpc
        .request(
            "session/new",
            json!({
                "cwd": cwd,
                "mcpServers": [],
            }),
        )
        .await
        .context("ACP session/new failed")?;

    let sid = resp
        .get("sessionId")
        .or_else(|| resp.get("session_id"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow!("ACP session/new response missing sessionId: {resp}"))?;
    let thought_id = extract_thought_config_id(&resp);
    Ok((sid, thought_id))
}

fn extract_thought_config_id(resp: &Value) -> Option<String> {
    let opts = resp
        .get("configOptions")
        .or_else(|| resp.get("config_options"))
        .and_then(Value::as_array)?;
    for opt in opts {
        let is_thought = opt.get("category").and_then(Value::as_str) == Some("thought_level")
            || matches!(
                opt.get("id").and_then(Value::as_str),
                Some("thought_level") | Some("effort")
            );
        if is_thought {
            if let Some(id) = opt.get("id").and_then(Value::as_str) {
                return Some(id.to_string());
            }
        }
    }
    None
}

async fn set_acp_model(
    rpc: &JsonRpcProcess,
    session_id: &str,
    model: &str,
    thought_config_id: &Arc<RwLock<Option<String>>>,
) {
    // Prefer ACP standard `session/set_config_option` (works on both agy & opencode)
    match rpc
        .request(
            "session/set_config_option",
            json!({
                "sessionId": session_id,
                "configId": "model",
                "value": model,
            }),
        )
        .await
    {
        Ok(resp) => {
            if let Some(tid) = extract_thought_config_id(&resp) {
                *thought_config_id.write().await = Some(tid);
            }
        }
        Err(_) => {
            let _ = rpc
                .request(
                    "session/set_model",
                    json!({ "sessionId": session_id, "modelId": model }),
                )
                .await;
        }
    }
}

async fn set_acp_thinking(
    rpc: &JsonRpcProcess,
    session_id: &str,
    thought_config_id: &Arc<RwLock<Option<String>>>,
    thinking: &str,
) {
    let discovered = thought_config_id.read().await.clone();
    if let Some(cfg_id) = discovered {
        let _ = rpc
            .request(
                "session/set_config_option",
                json!({
                    "sessionId": session_id,
                    "configId": cfg_id,
                    "value": thinking,
                }),
            )
            .await;
        return;
    }

    for candidate in ["thought_level", "effort"] {
        if rpc
            .request(
                "session/set_config_option",
                json!({
                    "sessionId": session_id,
                    "configId": candidate,
                    "value": thinking,
                }),
            )
            .await
            .is_ok()
        {
            *thought_config_id.write().await = Some(candidate.to_string());
            break;
        }
    }
}

async fn set_acp_mode(rpc: &JsonRpcProcess, session_id: &str, mode: &str) {
    let legacy = rpc
        .request(
            "session/set_mode",
            json!({ "sessionId": session_id, "modeId": mode }),
        )
        .await;
    if legacy.is_err() {
        let _ = rpc
            .request(
                "session/set_config_option",
                json!({
                    "sessionId": session_id,
                    "configId": "mode",
                    "value": mode,
                }),
            )
            .await;
    }
}

/// Queries live available models from an ACP agent via `initialize` + `session/new`,
/// parsing both `models.availableModels` and `configOptions[category="model"]`, and
/// cleaning up the temporary probe session with `session/delete` / `session/close`.
pub async fn list_models(command: &str, args: &[String]) -> Result<DiscoveredModels> {
    let tmp_dir = std::env::temp_dir();
    let (rpc, _rx) = JsonRpcProcess::spawn(command, args, Some(&tmp_dir), true).await?;

    let _init = rpc
        .request(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientInfo": {
                    "name": "hasilanx",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "clientCapabilities": {}
            }),
        )
        .await
        .context("ACP initialize failed while listing models")?;

    let cwd_str = tmp_dir.to_string_lossy().to_string();
    let resp = rpc
        .request(
            "session/new",
            json!({
                "cwd": cwd_str,
                "mcpServers": [],
            }),
        )
        .await
        .context("ACP session/new failed while listing models")?;

    let discovered = parse_acp_models(&resp);

    // Clean up temporary probe session so we never pollute the agent's session history
    if let Some(sid) = resp
        .get("sessionId")
        .or_else(|| resp.get("session_id"))
        .and_then(Value::as_str)
    {
        let _ = rpc.request("session/delete", json!({ "sessionId": sid })).await;
        let _ = rpc.request("session/close", json!({ "sessionId": sid })).await;
    }

    Ok(discovered)
}

fn parse_acp_models(resp: &Value) -> DiscoveredModels {
    // Extract thinking/effort options from ACP `configOptions` (category == "thought_level")
    let mut agent_thinking_options = Vec::new();
    let mut agent_default_thinking = None;
    let mut config_models: Vec<ModelInfo> = Vec::new();
    let mut config_default_model: Option<String> = None;

    if let Some(config_opts) = resp
        .get("configOptions")
        .or_else(|| resp.get("config_options"))
        .and_then(Value::as_array)
    {
        for opt in config_opts {
            let is_thought = opt.get("category").and_then(Value::as_str) == Some("thought_level")
                || matches!(
                    opt.get("id").and_then(Value::as_str),
                    Some("thought_level") | Some("effort")
                );
            if is_thought {
                agent_default_thinking = opt
                    .get("currentValue")
                    .or_else(|| opt.get("current_value"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                if let Some(entries) = opt.get("options").and_then(Value::as_array) {
                    for entry in entries {
                        let id = entry
                            .get("value")
                            .or_else(|| entry.get("id"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        if id.is_empty() {
                            continue;
                        }
                        let name = entry
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or(&id)
                            .to_string();
                        let description = entry
                            .get("description")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned);
                        agent_thinking_options.push(ThinkingOption {
                            id,
                            name,
                            description,
                        });
                    }
                }
            }

            let is_model = opt.get("category").and_then(Value::as_str) == Some("model")
                || opt.get("id").and_then(Value::as_str) == Some("model");
            if is_model {
                config_default_model = opt
                    .get("currentValue")
                    .or_else(|| opt.get("current_value"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                if let Some(entries) = opt.get("options").and_then(Value::as_array) {
                    for entry in entries {
                        let id = entry
                            .get("value")
                            .or_else(|| entry.get("id"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        if id.is_empty() {
                            continue;
                        }
                        let name = entry
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or(&id)
                            .to_string();
                        let description = entry
                            .get("description")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned);
                        config_models.push(ModelInfo {
                            id,
                            name,
                            description,
                            thinking_options: Vec::new(),
                            default_thinking: None,
                        });
                    }
                }
            }
        }
    }

    // Collect legacy `models.availableModels` IDs when present (helps narrow per-model thinking on agy)
    let mut available_model_ids: HashSet<String> = HashSet::new();
    let mut legacy_models: Vec<ModelInfo> = Vec::new();
    let mut legacy_default_model: Option<String> = None;

    if let Some(models_obj) = resp.get("models") {
        legacy_default_model = models_obj
            .get("currentModelId")
            .or_else(|| models_obj.get("current_model_id"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        if let Some(arr) = models_obj
            .get("availableModels")
            .or_else(|| models_obj.get("available_models"))
            .and_then(Value::as_array)
        {
            for m in arr {
                let id = m
                    .get("modelId")
                    .or_else(|| m.get("model_id"))
                    .or_else(|| m.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if id.is_empty() {
                    continue;
                }
                available_model_ids.insert(id.clone());
                let name = m
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(&id)
                    .to_string();
                let description = m
                    .get("description")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                legacy_models.push(ModelInfo {
                    id,
                    name,
                    description,
                    thinking_options: Vec::new(),
                    default_thinking: None,
                });
            }
        }
    }

    // Prefer ACP standard `configOptions[model]` (clean base models separated from `thought_level`)
    if !config_models.is_empty() {
        for m in &mut config_models {
            if !agent_thinking_options.is_empty() {
                if !available_model_ids.is_empty() {
                    // Narrow per-model thinking options using `{model_id}-{level}` from availableModels
                    let matched: Vec<ThinkingOption> = agent_thinking_options
                        .iter()
                        .filter(|t| available_model_ids.contains(&format!("{}-{}", m.id, t.id)))
                        .cloned()
                        .collect();
                    if !matched.is_empty() {
                        m.default_thinking = agent_default_thinking
                            .as_ref()
                            .filter(|d| matched.iter().any(|t| &t.id == *d))
                            .cloned()
                            .or_else(|| matched.first().map(|t| t.id.clone()));
                        m.thinking_options = matched;
                    }
                } else {
                    m.thinking_options = agent_thinking_options.clone();
                    m.default_thinking = agent_default_thinking.clone();
                }
            }
        }
        return DiscoveredModels {
            models: config_models,
            default_model: config_default_model,
            thinking_options: agent_thinking_options,
            default_thinking: agent_default_thinking,
        };
    }

    DiscoveredModels {
        models: legacy_models,
        default_model: legacy_default_model,
        thinking_options: agent_thinking_options,
        default_thinking: agent_default_thinking,
    }
}

#[async_trait]
impl AgentSession for AcpSession {
    fn vendor_session_id(&self) -> Option<String> {
        Some(self.session_id.clone())
    }

    async fn send_prompt(
        &self,
        prompt: &str,
        execution_mode: ExecutionMode,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<()> {
        *self.execution_mode.write().await = execution_mode;

        if let Some(m) = model {
            set_acp_model(&self.rpc, &self.session_id, m, &self.thought_config_id).await;
        }
        if let Some(t) = thinking {
            set_acp_thinking(&self.rpc, &self.session_id, &self.thought_config_id, t).await;
        }

        if execution_mode == ExecutionMode::Plan {
            set_acp_mode(&self.rpc, &self.session_id, "plan").await;
        }

        let rpc = Arc::clone(&self.rpc);
        let session_id = self.session_id.clone();
        let prompt_owned = prompt.to_string();
        let tx = self.event_tx.clone();

        tokio::spawn(async move {
            let res = rpc
                .request(
                    "session/prompt",
                    json!({
                        "sessionId": session_id,
                        "prompt": [
                            { "type": "text", "text": prompt_owned }
                        ]
                    }),
                )
                .await;

            match res {
                Ok(val) => {
                    let stop_reason = val
                        .get("stopReason")
                        .or_else(|| val.get("stop_reason"))
                        .and_then(Value::as_str)
                        .unwrap_or("end_turn");
                    let status = if stop_reason == "cancelled" {
                        TurnCompletionStatus::Cancelled
                    } else {
                        TurnCompletionStatus::Completed
                    };
                    let _ = tx.send(AgentEvent::TurnCompleted {
                        status,
                        error: None,
                    });
                }
                Err(err) => {
                    let _ = tx.send(AgentEvent::TurnCompleted {
                        status: TurnCompletionStatus::Failed,
                        error: Some(err.to_string()),
                    });
                }
            }
        });

        Ok(())
    }

    async fn cancel(&self) -> Result<()> {
        self.rpc
            .notify("session/cancel", json!({ "sessionId": self.session_id }))
            .await
    }
}

fn handle_acp_update(params: &Value, tx: &mpsc::UnboundedSender<AgentEvent>) {
    let update = params.get("update").unwrap_or(params);
    let kind = update
        .get("sessionUpdate")
        .or_else(|| update.get("session_update"))
        .and_then(Value::as_str)
        .unwrap_or_default();

    match kind {
        "agent_message_chunk" => {
            if let Some(text) = extract_content_text(update.get("content")) {
                if !text.is_empty() {
                    let _ = tx.send(AgentEvent::AssistantDelta(text));
                }
            }
        }
        "agent_thought_chunk" => {
            if let Some(text) = extract_content_text(update.get("content")) {
                if !text.is_empty() {
                    let _ = tx.send(AgentEvent::ReasoningDelta(text));
                }
            }
        }
        "tool_call" | "tool_call_update" => {
            if let Some(ev) = parse_acp_tool_update(update, kind == "tool_call_update") {
                let _ = tx.send(ev);
            }
        }
        "plan" => {
            if let Some(entries) = update.get("entries").and_then(Value::as_array) {
                let mut lines = Vec::new();
                for entry in entries {
                    let content = entry
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let status = entry
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("pending");
                    let box_char = match status {
                        "completed" => "[x]",
                        "in_progress" => "[~]",
                        _ => "[ ]",
                    };
                    lines.push(format!("- {box_char} {content}"));
                }
                if !lines.is_empty() {
                    let _ = tx.send(AgentEvent::PlanReplaced(lines.join("\n")));
                }
            }
        }
        _ => {
            debug!(kind = %kind, "ignoring ACP session/update variant");
        }
    }
}

async fn handle_acp_permission(
    rpc: Arc<JsonRpcProcess>,
    id: Value,
    params: Value,
    tx: mpsc::UnboundedSender<AgentEvent>,
    mode_lock: Arc<RwLock<ExecutionMode>>,
) {
    let tool_call = params.get("toolCall").or_else(|| params.get("tool_call"));
    let tool_name = tool_call
        .and_then(|t| t.get("title").or_else(|| t.get("kind")))
        .and_then(Value::as_str)
        .unwrap_or("Permission Request")
        .to_string();
    let args_json = tool_call
        .and_then(|t| t.get("rawInput").or_else(|| t.get("raw_input")))
        .filter(|v| !v.is_null())
        .map(format_json_compact);
    let description = format!("Agent requests permission for `{tool_name}`");

    let raw_options = params
        .get("options")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut options = Vec::new();
    for opt in &raw_options {
        let opt_id = opt
            .get("optionId")
            .or_else(|| opt.get("option_id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let label = opt
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&opt_id)
            .to_string();
        let decision = match opt.get("kind").and_then(Value::as_str) {
            Some("allow_always") => ApprovalDecision::AcceptForSession,
            Some("allow_once") => ApprovalDecision::Accept,
            Some("reject_always") => ApprovalDecision::Cancel,
            _ => ApprovalDecision::Decline,
        };
        if !opt_id.is_empty() {
            options.push(ApprovalOption {
                id: opt_id,
                label,
                decision,
            });
        }
    }

    if options.is_empty() {
        options.push(ApprovalOption {
            id: "allow_once".into(),
            label: "Allow".into(),
            decision: ApprovalDecision::Accept,
        });
        options.push(ApprovalOption {
            id: "reject_once".into(),
            label: "Decline".into(),
            decision: ApprovalDecision::Decline,
        });
    }

    // Auto mode: approve immediately
    let mode = *mode_lock.read().await;
    if mode == ExecutionMode::Auto {
        if let Some(opt) = options
            .iter()
            .find(|o| o.decision == ApprovalDecision::AcceptForSession)
            .or_else(|| options.iter().find(|o| o.decision == ApprovalDecision::Accept))
        {
            let _ = rpc
                .respond_ok(
                    id,
                    json!({ "outcome": { "outcome": "selected", "optionId": opt.id } }),
                )
                .await;
            return;
        }
    }

    let (resp_tx, resp_rx) = oneshot::channel::<ApprovalAnswer>();
    let options_clone = options.clone();
    let _ = tx.send(AgentEvent::ApprovalRequested {
        tool_name,
        description,
        args_json,
        options,
        responder: resp_tx,
    });

    tokio::spawn(async move {
        let reply = match resp_rx.await {
            Ok(answer) => {
                let chosen_id = answer.option_id.or_else(|| {
                    options_clone
                        .iter()
                        .find(|o| o.decision == answer.decision)
                        .map(|o| o.id.clone())
                });
                match (answer.decision, chosen_id) {
                    (ApprovalDecision::Cancel, None) => json!({ "outcome": { "outcome": "cancelled" } }),
                    (_, Some(opt_id)) => {
                        json!({ "outcome": { "outcome": "selected", "optionId": opt_id } })
                    }
                    _ => json!({ "outcome": { "outcome": "cancelled" } }),
                }
            }
            Err(_) => json!({ "outcome": { "outcome": "cancelled" } }),
        };
        if let Err(err) = rpc.respond_ok(id, reply).await {
            warn!(error = %err, "failed to send ACP permission response");
        }
    });
}

fn extract_content_text(content: Option<&Value>) -> Option<String> {
    let val = content?;
    if let Some(s) = val.as_str() {
        return Some(s.to_string());
    }
    if let Some(s) = val.get("text").and_then(Value::as_str) {
        return Some(s.to_string());
    }
    if let Some(arr) = val.as_array() {
        let mut out = String::new();
        for item in arr {
            if let Some(t) = extract_content_text(Some(item)) {
                out.push_str(&t);
            }
        }
        return Some(out);
    }
    None
}

fn extract_tool_content(content: Option<&Value>) -> Option<String> {
    let val = content?;
    if let Some(s) = val.as_str() {
        return (!s.trim().is_empty()).then(|| s.to_string());
    }
    let arr = val.as_array()?;
    let mut parts = Vec::new();
    for entry in arr {
        let entry_type = entry.get("type").and_then(Value::as_str).unwrap_or_default();
        if entry_type == "diff" {
            let path = entry.get("path").and_then(Value::as_str).unwrap_or("file");
            let old_text = entry.get("oldText").and_then(Value::as_str).unwrap_or("");
            let new_text = entry.get("newText").and_then(Value::as_str).unwrap_or("");
            let mut block = format!("--- a/{path}\n+++ b/{path}\n");
            for line in old_text.lines() {
                block.push('-');
                block.push_str(line);
                block.push('\n');
            }
            for line in new_text.lines() {
                block.push('+');
                block.push_str(line);
                block.push('\n');
            }
            parts.push(block.trim_end().to_string());
        } else if entry_type == "terminal" {
            if let Some(tid) = entry.get("terminalId").and_then(Value::as_str) {
                parts.push(format!("[terminal: {tid}]"));
            }
        } else if let Some(inner) = entry.get("content") {
            if let Some(text) = extract_content_text(Some(inner)) {
                if !text.trim().is_empty() {
                    parts.push(text);
                }
            }
        } else if let Some(text) = extract_content_text(Some(entry)) {
            if !text.trim().is_empty() {
                parts.push(text);
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn is_non_empty_json(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::String(s) => !s.trim().is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
        _ => true,
    }
}

fn extract_locations(val: Option<&Value>) -> Option<String> {
    let arr = val?.as_array()?;
    let lines: Vec<String> = arr
        .iter()
        .filter_map(|loc| {
            let path = loc.get("path").and_then(Value::as_str)?;
            if path.trim().is_empty() {
                return None;
            }
            if let Some(line) = loc.get("line").and_then(Value::as_i64) {
                Some(format!("{path}:{line}"))
            } else {
                Some(path.to_string())
            }
        })
        .collect();
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

fn format_acp_kind(kind: &str) -> String {
    match kind {
        "read" => "Read File".to_string(),
        "edit" => "Edit File".to_string(),
        "delete" => "Delete File".to_string(),
        "move" => "Move File".to_string(),
        "search" => "Search".to_string(),
        "execute" => "Shell Command".to_string(),
        "fetch" => "Web Fetch".to_string(),
        "think" => "Think".to_string(),
        other => other.to_string(),
    }
}

fn parse_acp_tool_update(update: &Value, is_update: bool) -> Option<AgentEvent> {
    let call_id = update
        .get("toolCallId")
        .or_else(|| update.get("tool_call_id"))
        .and_then(Value::as_str)
        .unwrap_or("tool")
        .to_string();

    let title = update
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let raw_name = update
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let kind_str = update
        .get("kind")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let name = if let Some(t) = title {
        t.to_string()
    } else if let Some(n) = raw_name {
        n.to_string()
    } else if is_update {
        // Keep empty so orchestrator preserves the rich title from the initial `tool_call`.
        String::new()
    } else if let Some(k) = kind_str {
        format_acp_kind(k)
    } else {
        "Tool Call".to_string()
    };

    let status = match update.get("status").and_then(Value::as_str) {
        Some("pending") => ToolCallStatus::Pending,
        Some("in_progress") | Some("running") => ToolCallStatus::Running,
        Some("completed") => ToolCallStatus::Completed,
        Some("failed") | Some("error") | Some("cancelled") | Some("canceled") => {
            ToolCallStatus::Failed
        }
        _ => ToolCallStatus::Running,
    };

    let input = update
        .get("rawInput")
        .or_else(|| update.get("raw_input"))
        .or_else(|| update.get("args"))
        .or_else(|| update.get("input"))
        .or_else(|| update.get("arguments"))
        .filter(|v| is_non_empty_json(v))
        .map(format_json_compact)
        .or_else(|| extract_locations(update.get("locations")));

    let content_output = extract_tool_content(update.get("content"));
    let raw_output = update
        .get("rawOutput")
        .or_else(|| update.get("raw_output"))
        .or_else(|| update.get("result"))
        .or_else(|| update.get("output"))
        .or_else(|| update.get("error"))
        .filter(|v| is_non_empty_json(v))
        .map(format_json_compact);
    let output = content_output.or(raw_output);

    Some(AgentEvent::ToolCallUpsert {
        call_id,
        name,
        status,
        input,
        output,
    })
}

fn format_json_compact(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_agy_and_opencode_acp_tool_updates() {
        // Exact live trace from `opencode acp`: initial pending with empty rawInput: {}
        let oc_init = json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "call_1",
            "name": "read",
            "title": "read",
            "kind": "read",
            "status": "pending",
            "locations": [],
            "rawInput": {}
        });
        match parse_acp_tool_update(&oc_init, false) {
            Some(AgentEvent::ToolCallUpsert {
                call_id,
                name,
                status,
                input,
                output,
            }) => {
                assert_eq!(call_id, "call_1");
                assert_eq!(name, "read");
                assert_eq!(status, ToolCallStatus::Pending);
                assert_eq!(input, None);
                assert_eq!(output, None);
            }
            other => panic!("unexpected: {other:?}"),
        }

        // Exact live trace from `opencode acp`: completed omits title & kind, sends content[]
        let oc_done = json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call_1",
            "status": "completed",
            "locations": [{"path": "/tmp/Cargo.toml"}],
            "content": [{"type": "content", "content": {"type": "text", "text": "resolver = \"2\""}}]
        });
        match parse_acp_tool_update(&oc_done, true) {
            Some(AgentEvent::ToolCallUpsert {
                name,
                status,
                input,
                output,
                ..
            }) => {
                assert_eq!(name, "");
                assert_eq!(status, ToolCallStatus::Completed);
                assert_eq!(input.as_deref(), Some("/tmp/Cargo.toml"));
                assert_eq!(output.as_deref(), Some("resolver = \"2\""));
            }
            other => panic!("unexpected: {other:?}"),
        }

        // ACP diff content block
        let diff_upd = json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call_2",
            "title": "Edit lib.rs",
            "status": "completed",
            "content": [{"type": "diff", "path": "src/lib.rs", "oldText": "a", "newText": "b"}]
        });
        match parse_acp_tool_update(&diff_upd, true) {
            Some(AgentEvent::ToolCallUpsert { output, .. }) => {
                let out = output.unwrap();
                assert!(out.contains("--- a/src/lib.rs"));
                assert!(out.contains("-a\n+b"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
