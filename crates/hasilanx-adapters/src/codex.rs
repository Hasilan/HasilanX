use crate::jsonrpc::{IncomingRpc, JsonRpcProcess};
use crate::{AgentEvent, AgentSession, ApprovalAnswer, DiscoveredModels, StartSessionParams};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use hasilanx_protocol::{
    ApprovalDecision, ApprovalOption, ExecutionMode, ModelInfo, NoticeLevel, ThinkingOption,
    ToolCallStatus, TurnCompletionStatus,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc, oneshot};
use tracing::{debug, warn};

pub struct CodexSession {
    rpc: Arc<JsonRpcProcess>,
    thread_id: String,
    cwd: String,
    active_turn_id: Arc<RwLock<Option<String>>>,
    execution_mode: Arc<RwLock<ExecutionMode>>,
}

impl CodexSession {
    pub async fn start(
        command: &str,
        args: &[String],
        params: StartSessionParams,
    ) -> Result<(Self, mpsc::UnboundedReceiver<AgentEvent>)> {
        let (rpc, mut incoming) =
            JsonRpcProcess::spawn(command, args, Some(&params.cwd), false).await?;

        let _init = rpc
            .request(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": "hasilanx",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "capabilities": {
                        "experimentalApi": true
                    }
                }),
            )
            .await
            .context("codex app-server initialize failed")?;

        let _ = rpc.notify("initialized", json!({})).await;

        let cwd_str = params.cwd.to_string_lossy().to_string();
        let (approval_policy, sandbox) = map_execution_mode(params.execution_mode);

        let thread_id = if let Some(existing_id) = params.resume_session_id {
            match rpc
                .request(
                    "thread/resume",
                    json!({
                        "threadId": existing_id,
                        "cwd": cwd_str,
                    }),
                )
                .await
            {
                Ok(resp) => extract_thread_id(&resp).unwrap_or(existing_id),
                Err(err) => {
                    debug!(error = %err, "codex thread/resume failed, starting new thread");
                    start_new_thread(&rpc, &cwd_str, params.model.as_deref(), approval_policy, sandbox)
                        .await?
                }
            }
        } else {
            start_new_thread(&rpc, &cwd_str, params.model.as_deref(), approval_policy, sandbox)
                .await?
        };

        let active_turn_id = Arc::new(RwLock::new(None));
        let execution_mode = Arc::new(RwLock::new(params.execution_mode));
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        let rpc_loop = Arc::clone(&rpc);
        let turn_loop = Arc::clone(&active_turn_id);
        let mode_loop = Arc::clone(&execution_mode);
        let tx_loop = event_tx.clone();

        tokio::spawn(async move {
            while let Some(msg) = incoming.recv().await {
                match msg {
                    IncomingRpc::Notification { method, params } => {
                        handle_codex_notification(&method, &params, &turn_loop, &tx_loop).await;
                    }
                    IncomingRpc::ServerRequest { id, method, params } => {
                        handle_codex_server_request(
                            Arc::clone(&rpc_loop),
                            id,
                            &method,
                            params,
                            tx_loop.clone(),
                            Arc::clone(&mode_loop),
                        )
                        .await;
                    }
                    IncomingRpc::Closed => {
                        let _ = tx_loop.send(AgentEvent::Notice {
                            level: NoticeLevel::Warning,
                            message: "Codex app-server exited.".to_string(),
                        });
                        break;
                    }
                }
            }
        });

        Ok((
            Self {
                rpc,
                thread_id,
                cwd: cwd_str,
                active_turn_id,
                execution_mode,
            },
            event_rx,
        ))
    }
}

async fn start_new_thread(
    rpc: &JsonRpcProcess,
    cwd: &str,
    model: Option<&str>,
    approval_policy: &str,
    sandbox: &str,
) -> Result<String> {
    let mut body = json!({
        "cwd": cwd,
        "approvalPolicy": approval_policy,
        "sandbox": sandbox,
    });
    if let Some(m) = model {
        body["model"] = json!(m);
    }
    let resp = rpc
        .request("thread/start", body)
        .await
        .context("codex thread/start failed")?;
    extract_thread_id(&resp)
        .ok_or_else(|| anyhow!("codex thread/start missing thread.id: {resp}"))
}

fn extract_thread_id(resp: &Value) -> Option<String> {
    resp.get("thread")
        .and_then(|t| t.get("id"))
        .and_then(Value::as_str)
        .or_else(|| resp.get("threadId").and_then(Value::as_str))
        .map(ToOwned::to_owned)
}

fn map_execution_mode(mode: ExecutionMode) -> (&'static str, &'static str) {
    match mode {
        ExecutionMode::Ask => ("on-request", "workspace-write"),
        ExecutionMode::AcceptEdits => ("on-request", "workspace-write"),
        ExecutionMode::Auto => ("never", "workspace-write"),
        ExecutionMode::Plan => ("on-request", "read-only"),
    }
}

#[async_trait]
impl AgentSession for CodexSession {
    fn vendor_session_id(&self) -> Option<String> {
        Some(self.thread_id.clone())
    }

    async fn send_prompt(
        &self,
        prompt: &str,
        execution_mode: ExecutionMode,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<()> {
        *self.execution_mode.write().await = execution_mode;
        let (approval_policy, _) = map_execution_mode(execution_mode);

        let mut body = json!({
            "threadId": self.thread_id,
            "cwd": self.cwd,
            "approvalPolicy": approval_policy,
            "input": [
                {
                    "type": "text",
                    "text": prompt,
                }
            ]
        });
        if let Some(m) = model {
            body["model"] = json!(m);
        }
        if let Some(effort) = thinking {
            body["effort"] = json!(effort);
        }

        let resp = self
            .rpc
            .request("turn/start", body)
            .await
            .context("codex turn/start failed")?;
        if let Some(tid) = resp
            .get("turn")
            .and_then(|t| t.get("id"))
            .and_then(Value::as_str)
            .or_else(|| resp.get("turnId").and_then(Value::as_str))
        {
            *self.active_turn_id.write().await = Some(tid.to_string());
        }
        Ok(())
    }

    async fn steer(&self, prompt: &str) -> Result<bool> {
        let turn_id = self.active_turn_id.read().await.clone();
        let Some(expected_turn_id) = turn_id else {
            return Ok(false);
        };
        self.rpc
            .request(
                "turn/steer",
                json!({
                    "threadId": self.thread_id,
                    "expectedTurnId": expected_turn_id,
                    "input": [
                        { "type": "text", "text": prompt }
                    ]
                }),
            )
            .await
            .context("codex turn/steer failed")?;
        Ok(true)
    }

    async fn cancel(&self) -> Result<()> {
        let turn_id = self.active_turn_id.read().await.clone();
        if let Some(tid) = turn_id {
            let _ = self
                .rpc
                .request(
                    "turn/interrupt",
                    json!({ "threadId": self.thread_id, "turnId": tid }),
                )
                .await?;
        }
        Ok(())
    }
}

pub async fn list_models(command: &str, args: &[String]) -> Result<DiscoveredModels> {
    let (rpc, _rx) = JsonRpcProcess::spawn(command, args, None, false).await?;
    let _ = rpc
        .request(
            "initialize",
            json!({
                "clientInfo": { "name": "hasilanx", "version": env!("CARGO_PKG_VERSION") },
                "capabilities": { "experimentalApi": true }
            }),
        )
        .await?;
    let _ = rpc.notify("initialized", json!({})).await;

    let mut out = Vec::new();
    let mut default_model = None;
    let mut agent_thinking_options = Vec::new();
    let mut agent_default_thinking = None;
    let mut cursor: Option<String> = None;

    for _page in 0..8 {
        let mut params = json!({ "limit": 100 });
        if let Some(ref cur) = cursor {
            params["cursor"] = json!(cur);
        }
        let resp = rpc.request("model/list", params).await?;
        let arr = resp
            .get("data")
            .or_else(|| resp.get("models"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        for m in arr {
            if m.get("hidden").and_then(Value::as_bool).unwrap_or(false) {
                continue;
            }
            let id = m
                .get("id")
                .or_else(|| m.get("model"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if id.is_empty() {
                continue;
            }
            let is_default = m.get("isDefault").and_then(Value::as_bool).unwrap_or(false);
            if is_default && default_model.is_none() {
                default_model = Some(id.clone());
            }
            let name = m
                .get("displayName")
                .or_else(|| m.get("name"))
                .and_then(Value::as_str)
                .unwrap_or(&id)
                .to_string();
            let description = m
                .get("description")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);

            let mut thinking_options = Vec::new();
            if let Some(efforts) = m.get("supportedReasoningEfforts").and_then(Value::as_array) {
                for eff in efforts {
                    let eff_id = eff
                        .get("reasoningEffort")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    if eff_id.is_empty() {
                        continue;
                    }
                    let eff_desc = eff
                        .get("description")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    thinking_options.push(ThinkingOption {
                        name: format_effort_label(&eff_id),
                        id: eff_id,
                        description: eff_desc,
                    });
                }
            }
            let default_thinking = m
                .get("defaultReasoningEffort")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);

            if (is_default || agent_thinking_options.is_empty()) && !thinking_options.is_empty() {
                agent_thinking_options = thinking_options.clone();
                agent_default_thinking = default_thinking.clone();
            }

            out.push(ModelInfo {
                id,
                name,
                description,
                thinking_options,
                default_thinking,
            });
        }

        let next_cursor = resp
            .get("nextCursor")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        if next_cursor.is_none() {
            break;
        }
        cursor = next_cursor;
    }

    Ok(DiscoveredModels {
        models: out,
        default_model,
        thinking_options: agent_thinking_options,
        default_thinking: agent_default_thinking,
    })
}

fn format_effort_label(id: &str) -> String {
    match id {
        "none" => "None".into(),
        "minimal" => "Minimal".into(),
        "low" => "Low".into(),
        "medium" => "Medium".into(),
        "high" => "High".into(),
        "xhigh" => "Extra High".into(),
        "max" => "Max".into(),
        "ultra" => "Ultra".into(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        }
    }
}

async fn handle_codex_notification(
    method: &str,
    params: &Value,
    active_turn: &Arc<RwLock<Option<String>>>,
    tx: &mpsc::UnboundedSender<AgentEvent>,
) {
    match method {
        "turn/started" => {
            if let Some(tid) = params
                .get("turn")
                .and_then(|t| t.get("id"))
                .and_then(Value::as_str)
                .or_else(|| params.get("turnId").and_then(Value::as_str))
            {
                *active_turn.write().await = Some(tid.to_string());
            }
        }
        "item/agentMessage/delta" => {
            if let Some(delta) = params.get("delta").and_then(Value::as_str) {
                let _ = tx.send(AgentEvent::AssistantDelta(delta.to_string()));
            }
        }
        "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta" => {
            if let Some(delta) = params.get("delta").and_then(Value::as_str) {
                let _ = tx.send(AgentEvent::ReasoningDelta(delta.to_string()));
            }
        }
        "item/plan/delta" => {
            if let Some(delta) = params.get("delta").and_then(Value::as_str) {
                let _ = tx.send(AgentEvent::PlanDelta(delta.to_string()));
            }
        }
        "item/started" | "item/completed" => {
            if let Some(item) = params.get("item") {
                if let Some(ev) = parse_codex_tool_item(item, method == "item/completed") {
                    let _ = tx.send(ev);
                }
            }
        }
        "item/fileChange/patchUpdated" => {
            if let Some(call_id) = params.get("itemId").and_then(Value::as_str) {
                let (input, output) = format_codex_file_changes(params.get("changes"));
                let _ = tx.send(AgentEvent::ToolCallUpsert {
                    call_id: call_id.to_string(),
                    name: "File Change".to_string(),
                    status: ToolCallStatus::Running,
                    input,
                    output,
                });
            }
        }
        "item/mcpToolCall/progress" => {
            if let (Some(call_id), Some(msg)) = (
                params.get("itemId").and_then(Value::as_str),
                params.get("message").and_then(Value::as_str),
            ) {
                if !msg.is_empty() {
                    let _ = tx.send(AgentEvent::ToolOutputDelta {
                        call_id: call_id.to_string(),
                        delta: format!("{msg}\n"),
                    });
                }
            }
        }
        "item/commandExecution/outputDelta" | "item/fileChange/outputDelta" => {
            if let (Some(call_id), Some(delta)) = (
                params.get("itemId").and_then(Value::as_str),
                params.get("delta").and_then(Value::as_str),
            ) {
                let _ = tx.send(AgentEvent::ToolOutputDelta {
                    call_id: call_id.to_string(),
                    delta: delta.to_string(),
                });
            }
        }
        "turn/completed" => {
            *active_turn.write().await = None;
            let turn = params.get("turn").unwrap_or(params);
            let status_str = turn
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("completed");
            let error = turn
                .get("error")
                .and_then(|e| e.get("message").and_then(Value::as_str).or_else(|| e.as_str()))
                .map(ToOwned::to_owned);
            let status = match status_str {
                "interrupted" | "cancelled" => TurnCompletionStatus::Cancelled,
                "failed" => TurnCompletionStatus::Failed,
                _ => TurnCompletionStatus::Completed,
            };
            let _ = tx.send(AgentEvent::TurnCompleted { status, error });
        }
        "error" => {
            let msg = params
                .get("error")
                .and_then(|e| e.get("message").and_then(Value::as_str))
                .or_else(|| params.get("message").and_then(Value::as_str))
                .unwrap_or("Codex error")
                .to_string();
            let _ = tx.send(AgentEvent::Notice {
                level: NoticeLevel::Error,
                message: msg,
            });
        }
        _ => {
            debug!(method = %method, "ignoring codex notification");
        }
    }
}

async fn handle_codex_server_request(
    rpc: Arc<JsonRpcProcess>,
    id: Value,
    method: &str,
    params: Value,
    tx: mpsc::UnboundedSender<AgentEvent>,
    mode_lock: Arc<RwLock<ExecutionMode>>,
) {
    match method {
        "item/commandExecution/requestApproval"
        | "item/fileChange/requestApproval"
        | "execCommandApproval"
        | "applyPatchApproval" => {
            let is_file_change = method.contains("fileChange") || method.contains("Patch");
            let tool_name = if is_file_change {
                "File Change".to_string()
            } else {
                "Shell Command".to_string()
            };
            let description = params
                .get("reason")
                .and_then(Value::as_str)
                .or_else(|| params.get("command").and_then(Value::as_str))
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| format!("Codex requests approval for {tool_name}"));
            let args_json = serde_json::to_string_pretty(&params).ok();

            let mode = *mode_lock.read().await;
            let auto_accept = mode == ExecutionMode::Auto
                || (mode == ExecutionMode::AcceptEdits && is_file_change);
            if auto_accept {
                let _ = rpc.respond_ok(id, json!({ "decision": "accept" })).await;
                return;
            }

            let options = vec![
                ApprovalOption {
                    id: "accept".into(),
                    label: "Approve".into(),
                    decision: ApprovalDecision::Accept,
                },
                ApprovalOption {
                    id: "acceptForSession".into(),
                    label: "Approve for Session".into(),
                    decision: ApprovalDecision::AcceptForSession,
                },
                ApprovalOption {
                    id: "decline".into(),
                    label: "Decline".into(),
                    decision: ApprovalDecision::Decline,
                },
            ];

            let (resp_tx, resp_rx) = oneshot::channel::<ApprovalAnswer>();
            let _ = tx.send(AgentEvent::ApprovalRequested {
                tool_name,
                description,
                args_json,
                options,
                responder: resp_tx,
            });

            tokio::spawn(async move {
                let decision_str = match resp_rx.await {
                    Ok(ans) => match ans.decision {
                        ApprovalDecision::Accept => "accept",
                        ApprovalDecision::AcceptForSession => "acceptForSession",
                        ApprovalDecision::Decline => "decline",
                        ApprovalDecision::Cancel => "cancel",
                    },
                    Err(_) => "cancel",
                };
                if let Err(err) = rpc
                    .respond_ok(id, json!({ "decision": decision_str }))
                    .await
                {
                    warn!(error = %err, "failed to respond to codex approval request");
                }
            });
        }
        _ => {
            debug!(method = %method, "unsupported codex server request");
            let _ = rpc
                .respond_error(id, -32601, &format!("unsupported method: {method}"))
                .await;
        }
    }
}

fn non_empty_json_pretty(v: Option<&Value>) -> Option<String> {
    let val = v?;
    match val {
        Value::Null => None,
        Value::String(s) if s.trim().is_empty() => None,
        Value::String(s) => Some(s.clone()),
        Value::Array(a) if a.is_empty() => None,
        Value::Object(m) if m.is_empty() => None,
        other => serde_json::to_string_pretty(other).ok(),
    }
}

fn parse_codex_item_status(item: &Value, is_completed: bool) -> ToolCallStatus {
    match item.get("status").and_then(Value::as_str) {
        Some("failed") | Some("declined") | Some("error") | Some("cancelled") | Some("canceled") => {
            ToolCallStatus::Failed
        }
        Some("pending") => ToolCallStatus::Pending,
        Some("inProgress") | Some("in_progress") | Some("running") => ToolCallStatus::Running,
        Some("completed") => ToolCallStatus::Completed,
        _ if is_completed => ToolCallStatus::Completed,
        _ => ToolCallStatus::Running,
    }
}

fn format_codex_file_changes(changes: Option<&Value>) -> (Option<String>, Option<String>) {
    let Some(arr) = changes.and_then(Value::as_array) else {
        return (non_empty_json_pretty(changes), None);
    };
    if arr.is_empty() {
        return (None, None);
    }

    let mut summary_lines = Vec::new();
    let mut diff_blocks = Vec::new();
    for ch in arr {
        let path = ch.get("path").and_then(Value::as_str).unwrap_or("(unknown)");
        let kind_str = ch
            .get("kind")
            .and_then(|k| k.get("type").and_then(Value::as_str).or_else(|| k.as_str()))
            .unwrap_or("update");
        let move_to = ch
            .get("kind")
            .and_then(|k| k.get("move_path").and_then(Value::as_str));
        if let Some(dest) = move_to {
            summary_lines.push(format!("{kind_str} {path} -> {dest}"));
        } else {
            summary_lines.push(format!("{kind_str} {path}"));
        }
        if let Some(diff) = ch.get("diff").and_then(Value::as_str) {
            if !diff.trim().is_empty() {
                if arr.len() > 1 {
                    diff_blocks.push(format!("# {path}\n{diff}"));
                } else {
                    diff_blocks.push(diff.to_string());
                }
            }
        }
    }

    let input = if summary_lines.is_empty() {
        None
    } else {
        Some(summary_lines.join("\n"))
    };
    let output = if diff_blocks.is_empty() {
        None
    } else {
        Some(diff_blocks.join("\n\n"))
    };
    (input, output)
}

fn format_codex_content_items(items: Option<&Value>) -> Option<String> {
    let arr = items?.as_array()?;
    let mut parts = Vec::new();
    for entry in arr {
        if let Some(s) = entry.as_str() {
            if !s.trim().is_empty() {
                parts.push(s.to_string());
            }
            continue;
        }
        if let Some(text) = entry.get("text").and_then(Value::as_str) {
            if !text.trim().is_empty() {
                parts.push(text.to_string());
            }
            continue;
        }
        if let Some(url) = entry
            .get("imageUrl")
            .or_else(|| entry.get("audioUrl"))
            .and_then(Value::as_str)
        {
            parts.push(url.to_string());
            continue;
        }
        if let Ok(s) = serde_json::to_string_pretty(entry) {
            parts.push(s);
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn format_codex_web_search_results(results: Option<&Value>) -> Option<String> {
    let arr = results?.as_array()?;
    if arr.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    for entry in arr {
        let title = entry.get("title").and_then(Value::as_str);
        let url = entry.get("url").and_then(Value::as_str);
        let snippet = entry
            .get("snippet")
            .or_else(|| entry.get("summary"))
            .or_else(|| entry.get("description"))
            .and_then(Value::as_str);
        if title.is_some() || url.is_some() {
            let mut head = String::new();
            if let Some(t) = title {
                head.push_str(t);
            }
            if let Some(u) = url {
                if !head.is_empty() {
                    head.push_str(" — ");
                }
                head.push_str(u);
            }
            if let Some(sn) = snippet {
                lines.push(format!("- {head}\n  {sn}"));
            } else {
                lines.push(format!("- {head}"));
            }
        } else if let Ok(s) = serde_json::to_string_pretty(entry) {
            lines.push(s);
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

fn parse_codex_tool_item(item: &Value, is_completed: bool) -> Option<AgentEvent> {
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
    let item_id = item
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("item")
        .to_string();
    let mut status = parse_codex_item_status(item, is_completed);

    match item_type {
        "commandExecution" => {
            let cmd = item
                .get("command")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(ToOwned::to_owned);
            let name = match item
                .get("commandActions")
                .and_then(Value::as_array)
                .and_then(|arr| (arr.len() == 1).then(|| &arr[0]))
                .and_then(|act| act.get("type").and_then(Value::as_str))
            {
                Some("read") => "Read File".to_string(),
                Some("search") => "Search Files".to_string(),
                Some("listFiles") => "List Files".to_string(),
                _ => "Shell Command".to_string(),
            };
            let exit_code = item.get("exitCode").and_then(Value::as_i64);
            let mut output = item
                .get("aggregatedOutput")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned);
            if is_completed {
                if let Some(code) = exit_code {
                    if code != 0 {
                        if status == ToolCallStatus::Completed {
                            status = ToolCallStatus::Failed;
                        }
                        if output.is_none() {
                            output = Some(format!("Exit code: {code}"));
                        }
                    }
                }
            }
            Some(AgentEvent::ToolCallUpsert {
                call_id: item_id,
                name,
                status,
                input: cmd,
                output,
            })
        }
        "fileChange" => {
            let (input, output) = format_codex_file_changes(item.get("changes"));
            Some(AgentEvent::ToolCallUpsert {
                call_id: item_id,
                name: "File Change".to_string(),
                status,
                input,
                output,
            })
        }
        "mcpToolCall" => {
            let server = item
                .get("server")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            let tool = item
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or("MCP Tool");
            let name = match server {
                Some(srv) => format!("{srv} · {tool}"),
                None => tool.to_string(),
            };
            let input = non_empty_json_pretty(item.get("arguments"));
            let output = item
                .get("error")
                .and_then(|e| {
                    e.get("message")
                        .and_then(Value::as_str)
                        .map(|m| format!("Error: {m}"))
                        .or_else(|| non_empty_json_pretty(Some(e)))
                })
                .or_else(|| {
                    let res = item.get("result")?;
                    if res.is_null() {
                        return None;
                    }
                    format_codex_content_items(res.get("content"))
                        .or_else(|| non_empty_json_pretty(res.get("structuredContent")))
                        .or_else(|| non_empty_json_pretty(Some(res)))
                });
            if item.get("error").is_some_and(|e| !e.is_null()) && is_completed {
                status = ToolCallStatus::Failed;
            }
            Some(AgentEvent::ToolCallUpsert {
                call_id: item_id,
                name,
                status,
                input,
                output,
            })
        }
        "webSearch" => {
            let action = item.get("action").filter(|v| !v.is_null());
            let action_type = action
                .and_then(|a| a.get("type"))
                .and_then(Value::as_str)
                .unwrap_or("search");
            let name = match action_type {
                "openPage" => "Open Page",
                "findInPage" => "Find in Page",
                _ => "Web Search",
            }
            .to_string();

            let input = match action_type {
                "openPage" => action
                    .and_then(|a| a.get("url").and_then(Value::as_str))
                    .or_else(|| item.get("query").and_then(Value::as_str))
                    .filter(|s| !s.trim().is_empty())
                    .map(ToOwned::to_owned),
                "findInPage" => {
                    let pattern = action.and_then(|a| a.get("pattern").and_then(Value::as_str));
                    let url = action.and_then(|a| a.get("url").and_then(Value::as_str));
                    match (pattern, url) {
                        (Some(p), Some(u)) => Some(format!("{p} ({u})")),
                        (Some(p), None) => Some(p.to_string()),
                        (None, Some(u)) => Some(u.to_string()),
                        (None, None) => item
                            .get("query")
                            .and_then(Value::as_str)
                            .filter(|s| !s.trim().is_empty())
                            .map(ToOwned::to_owned),
                    }
                }
                _ => {
                    let queries = action
                        .and_then(|a| a.get("queries"))
                        .and_then(Value::as_array)
                        .map(|arr| {
                            arr.iter()
                                .filter_map(Value::as_str)
                                .filter(|s| !s.trim().is_empty())
                                .collect::<Vec<_>>()
                        })
                        .filter(|v| !v.is_empty());
                    if let Some(qs) = queries {
                        Some(qs.join("\n"))
                    } else {
                        item.get("query")
                            .and_then(Value::as_str)
                            .or_else(|| action.and_then(|a| a.get("query").and_then(Value::as_str)))
                            .filter(|s| !s.trim().is_empty())
                            .map(ToOwned::to_owned)
                    }
                }
            };
            let output = format_codex_web_search_results(item.get("results"));
            Some(AgentEvent::ToolCallUpsert {
                call_id: item_id,
                name,
                status,
                input,
                output,
            })
        }
        "dynamicToolCall" => {
            let ns = item
                .get("namespace")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            let tool = item
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or("Tool");
            let name = match ns {
                Some(n) => format!("{n}.{tool}"),
                None => tool.to_string(),
            };
            if item.get("success").and_then(Value::as_bool) == Some(false) && is_completed {
                status = ToolCallStatus::Failed;
            }
            let input = non_empty_json_pretty(item.get("arguments"));
            let output = format_codex_content_items(item.get("contentItems"));
            Some(AgentEvent::ToolCallUpsert {
                call_id: item_id,
                name,
                status,
                input,
                output,
            })
        }
        "collabAgentToolCall" => {
            let tool = item
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or("collab");
            let name = format!("Sub-agent · {tool}");
            let prompt = item
                .get("prompt")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty());
            let model = item
                .get("model")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty());
            let input = match (model, prompt) {
                (Some(m), Some(p)) => Some(format!("[{m}]\n{p}")),
                (None, Some(p)) => Some(p.to_string()),
                (Some(m), None) => Some(format!("model: {m}")),
                (None, None) => None,
            };
            let output = non_empty_json_pretty(item.get("agentsStates"));
            Some(AgentEvent::ToolCallUpsert {
                call_id: item_id,
                name,
                status,
                input,
                output,
            })
        }
        "functionCallOutput" => {
            let ns = item
                .get("namespace")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            let fname = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Function");
            let name = match ns {
                Some(n) => format!("{n}.{fname}"),
                None => fname.to_string(),
            };
            let output = item.get("output").and_then(|v| {
                v.as_str()
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned)
                    .or_else(|| format_codex_content_items(Some(v)))
                    .or_else(|| non_empty_json_pretty(Some(v)))
            });
            Some(AgentEvent::ToolCallUpsert {
                call_id: item_id,
                name,
                status,
                input: None,
                output,
            })
        }
        "imageView" => {
            let path = item
                .get("path")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(ToOwned::to_owned);
            Some(AgentEvent::ToolCallUpsert {
                call_id: item_id,
                name: "View Image".to_string(),
                status,
                input: path,
                output: None,
            })
        }
        "imageGeneration" => {
            let input = item
                .get("revisedPrompt")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(ToOwned::to_owned);
            let failure = non_empty_json_pretty(item.get("failure"));
            if failure.is_some() && is_completed {
                status = ToolCallStatus::Failed;
            }
            let output = failure.or_else(|| {
                item.get("savedPath")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .map(|p| format!("Saved: {p}"))
            });
            Some(AgentEvent::ToolCallUpsert {
                call_id: item_id,
                name: "Generate Image".to_string(),
                status,
                input,
                output,
            })
        }
        "subAgentActivity" => {
            let path = item.get("agentPath").and_then(Value::as_str).unwrap_or("");
            let kind = item
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("activity");
            let model = item.get("model").and_then(Value::as_str);
            let mut parts = Vec::new();
            if !path.is_empty() {
                parts.push(path.to_string());
            }
            if let Some(m) = model {
                parts.push(format!("model={m}"));
            }
            let input = if parts.is_empty() {
                None
            } else {
                Some(parts.join(" · "))
            };
            Some(AgentEvent::ToolCallUpsert {
                call_id: item_id,
                name: format!("Sub-agent · {kind}"),
                status,
                input,
                output: None,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codex_web_search_and_command_and_file_change() {
        let ws = json!({
            "type": "webSearch",
            "id": "ws_1",
            "query": "名古屋 今日の天気",
            "action": { "type": "search", "query": "名古屋 今日の天気" },
            "results": [{ "title": "Yahoo!天気", "url": "https://weather.yahoo.co.jp" }]
        });
        match parse_codex_tool_item(&ws, true) {
            Some(AgentEvent::ToolCallUpsert {
                call_id,
                name,
                status,
                input,
                output,
            }) => {
                assert_eq!(call_id, "ws_1");
                assert_eq!(name, "Web Search");
                assert_eq!(status, ToolCallStatus::Completed);
                assert_eq!(input.as_deref(), Some("名古屋 今日の天気"));
                assert!(output.unwrap().contains("Yahoo!天気"));
            }
            other => panic!("unexpected: {other:?}"),
        }

        let cmd = json!({
            "type": "commandExecution",
            "id": "exec_1",
            "command": "/usr/bin/bash -lc 'head -n 5 Cargo.toml'",
            "commandActions": [{ "type": "read", "command": "head -n 5 Cargo.toml" }],
            "aggregatedOutput": "[workspace]\nresolver = \"2\"\n",
            "exitCode": 0,
            "status": "completed"
        });
        match parse_codex_tool_item(&cmd, true) {
            Some(AgentEvent::ToolCallUpsert {
                name,
                status,
                input,
                output,
                ..
            }) => {
                assert_eq!(name, "Read File");
                assert_eq!(status, ToolCallStatus::Completed);
                assert!(input.unwrap().contains("Cargo.toml"));
                assert!(output.unwrap().contains("resolver"));
            }
            other => panic!("unexpected: {other:?}"),
        }

        let fc = json!({
            "type": "fileChange",
            "id": "fc_1",
            "status": "completed",
            "changes": [{
                "path": "src/main.rs",
                "kind": { "type": "update" },
                "diff": "@@ -1 +1 @@\n-old\n+new"
            }]
        });
        match parse_codex_tool_item(&fc, true) {
            Some(AgentEvent::ToolCallUpsert {
                name,
                input,
                output,
                ..
            }) => {
                assert_eq!(name, "File Change");
                assert_eq!(input.as_deref(), Some("update src/main.rs"));
                assert_eq!(output.as_deref(), Some("@@ -1 +1 @@\n-old\n+new"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
