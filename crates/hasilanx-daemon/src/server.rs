use crate::orchestrator::Orchestrator;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use hasilanx_protocol::{ClientMessage, HealthResponse, ServerMessage};
use hasilanx_pty::PtyEvent;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, warn};

#[derive(Clone)]
pub struct AppState {
    pub orchestrator: Arc<Orchestrator>,
    pub token: String,
}

#[derive(Deserialize)]
struct SnapshotQuery {
    since_seq: Option<i64>,
}

#[derive(Deserialize)]
struct WsQuery {
    token: Option<String>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/snapshot", get(snapshot))
        .route("/api/v1/ws", get(ws_handler))
        .with_state(state)
}

fn check_auth(headers: &HeaderMap, query_token: Option<&str>, expected: &str) -> bool {
    if let Some(val) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if let Some(tok) = val.strip_prefix("Bearer ") {
            if tok == expected {
                return true;
            }
        }
    }
    if let Some(tok) = query_token {
        if tok == expected {
            return true;
        }
    }
    false
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        hostname: state.orchestrator.hostname().to_string(),
        agents: state.orchestrator.list_agents(),
    })
}

async fn snapshot(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SnapshotQuery>,
) -> impl IntoResponse {
    if !check_auth(&headers, None, &state.token) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    match state.orchestrator.snapshot(query.since_seq) {
        Ok(resp) => Json(resp).into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<WsQuery>,
) -> impl IntoResponse {
    if !check_auth(&headers, query.token.as_deref(), &state.token) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    ws.on_upgrade(move |socket| handle_socket(socket, state.orchestrator))
        .into_response()
}

async fn handle_socket(socket: WebSocket, orchestrator: Arc<Orchestrator>) {
    let (mut ws_sink, mut ws_stream) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<ServerMessage>();
    let mut events = orchestrator.subscribe();
    let mut diffs = orchestrator.subscribe_diffs();
    let mut agents = orchestrator.subscribe_agents();
    let mut checks = orchestrator.subscribe_checks();
    let mut project_cfgs = orchestrator.subscribe_project_configs();
    let mut pty_events = orchestrator.subscribe_pty();

    // Push current discovered agents once on WS connect so any probe completing
    // between GET /snapshot and WS upgrade is never missed.
    let _ = out_tx.send(ServerMessage::AgentsUpdated {
        agents: orchestrator.list_agents(),
    });

    let out_tx_events = out_tx.clone();
    let event_task = tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(env) => {
                    if out_tx_events.send(ServerMessage::Event(env)).is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(skipped, "websocket client lagged behind event broadcast");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let out_tx_diffs = out_tx.clone();
    let diff_task = tokio::spawn(async move {
        loop {
            match diffs.recv().await {
                Ok((thread_id, diff)) => {
                    if out_tx_diffs
                        .send(ServerMessage::DiffUpdated { thread_id, diff })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let out_tx_agents = out_tx.clone();
    let agents_task = tokio::spawn(async move {
        loop {
            match agents.recv().await {
                Ok(agents_list) => {
                    if out_tx_agents
                        .send(ServerMessage::AgentsUpdated {
                            agents: agents_list,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let out_tx_checks = out_tx.clone();
    let checks_task = tokio::spawn(async move {
        loop {
            match checks.recv().await {
                Ok((thread_id, checks_list)) => {
                    if out_tx_checks
                        .send(ServerMessage::ChecksUpdated {
                            thread_id,
                            checks: checks_list,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let out_tx_pcfg = out_tx.clone();
    let pcfg_task = tokio::spawn(async move {
        loop {
            match project_cfgs.recv().await {
                Ok((project_id, config)) => {
                    if out_tx_pcfg
                        .send(ServerMessage::ProjectConfigUpdated { project_id, config })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let out_tx_pty = out_tx.clone();
    let pty_task = tokio::spawn(async move {
        loop {
            match pty_events.recv().await {
                Ok(PtyEvent::Output { thread_id, data }) => {
                    let data_b64 = base64::engine::general_purpose::STANDARD.encode(&data);
                    if out_tx_pty
                        .send(ServerMessage::TerminalOutput {
                            thread_id,
                            data_b64,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(PtyEvent::Exited { thread_id, code }) => {
                    if out_tx_pty
                        .send(ServerMessage::TerminalExited { thread_id, code })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let writer_task = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            match serde_json::to_string(&msg) {
                Ok(text) => {
                    if ws_sink.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                Err(err) => warn!(error = %err, "failed to serialize server ws message"),
            }
        }
    });

    while let Some(frame) = ws_stream.next().await {
        match frame {
            Ok(Message::Text(text)) => {
                let parsed: ClientMessage = match serde_json::from_str(&text) {
                    Ok(m) => m,
                    Err(err) => {
                        debug!(error = %err, "invalid client ws message");
                        continue;
                    }
                };
                match parsed {
                    ClientMessage::Ping => {
                        let _ = out_tx.send(ServerMessage::Pong);
                    }
                    ClientMessage::Command {
                        request_id,
                        command,
                    } => {
                        let orch = Arc::clone(&orchestrator);
                        let tx = out_tx.clone();
                        tokio::spawn(async move {
                            let res = orch.handle_command(command).await;
                            let reply = match res {
                                Ok(data) => ServerMessage::CommandResult {
                                    request_id,
                                    ok: true,
                                    data: Some(data),
                                    error: None,
                                },
                                Err(err) => ServerMessage::CommandResult {
                                    request_id,
                                    ok: false,
                                    data: None,
                                    error: Some(err.to_string()),
                                },
                            };
                            let _ = tx.send(reply);
                        });
                    }
                }
            }
            Ok(Message::Close(_)) | Err(_) => break,
            _ => {}
        }
    }

    event_task.abort();
    diff_task.abort();
    agents_task.abort();
    checks_task.abort();
    pcfg_task.abort();
    pty_task.abort();
    writer_task.abort();
}
