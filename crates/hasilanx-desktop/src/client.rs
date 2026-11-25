use anyhow::{Context, Result, anyhow};
use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use hasilanx_protocol::config::{DaemonConfig, HostsConfig, RemoteHostConfig};
use hasilanx_protocol::{
    AgentDescriptor, CheckResultRecord, ClientMessage, EventEnvelope, GitDiffSnapshot,
    HealthResponse, ProjectWorkflowConfig, RpcCommand, ServerMessage, SnapshotResponse,
    WorkspaceSnapshot, new_id,
};
use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

pub const LOCAL_HOST_ID: &str = "local";

#[derive(Debug)]
pub struct HostUpdate {
    pub host_id: String,
    pub kind: ClientUpdateKind,
}

#[derive(Debug)]
pub enum ClientUpdateKind {
    Connected {
        hostname: String,
        daemon_version: String,
    },
    Disconnected(String),
    FullSnapshot(WorkspaceSnapshot),
    Event(EventEnvelope),
    DiffUpdated {
        thread_id: String,
        diff: GitDiffSnapshot,
    },
    AgentsUpdated(Vec<AgentDescriptor>),
    TerminalOutput {
        thread_id: String,
        data: Vec<u8>,
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
    CommandError(String),
}

#[derive(Clone)]
struct HostConnectionSpec {
    host_id: String,
    http_url: String,
    ws_url: String,
    token: String,
    allow_autospawn: bool,
}

type SharedUpdateQueue = Arc<(Mutex<Vec<HostUpdate>>, AtomicBool)>;

#[derive(Clone)]
pub struct MultiHostManager {
    senders: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<RpcCommand>>>>,
    updates: SharedUpdateQueue,
    hosts_cfg: Arc<Mutex<HostsConfig>>,
}

impl MultiHostManager {
    pub fn start() -> Result<(Self, Vec<(String, String, bool)>)> {
        let custom_cfg_path = std::env::var("HASILANX_DAEMON_CONFIG")
            .ok()
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from);
        let local_cfg = DaemonConfig::load_or_create(custom_cfg_path.as_deref())?;
        let hosts_cfg = if custom_cfg_path.is_some() {
            HostsConfig::default()
        } else {
            HostsConfig::load_or_default().unwrap_or_default()
        };

        let mgr = Self {
            senders: Arc::new(Mutex::new(HashMap::new())),
            updates: Arc::new((Mutex::new(Vec::new()), AtomicBool::new(false))),
            hosts_cfg: Arc::new(Mutex::new(hosts_cfg.clone())),
        };

        let mut initial_hosts = Vec::new();
        mgr.spawn_connection(HostConnectionSpec {
            host_id: LOCAL_HOST_ID.to_string(),
            http_url: local_cfg.base_http_url(),
            ws_url: local_cfg.base_ws_url(),
            token: local_cfg.token,
            allow_autospawn: true,
        })?;
        initial_hosts.push((LOCAL_HOST_ID.to_string(), "Local".to_string(), true));

        for remote in hosts_cfg.remotes {
            let ws_url = remote.ws_url();
            mgr.spawn_connection(HostConnectionSpec {
                host_id: remote.id.clone(),
                http_url: remote.http_url.trim_end_matches('/').to_string(),
                ws_url,
                token: remote.token,
                allow_autospawn: false,
            })?;
            initial_hosts.push((remote.id, remote.name, false));
        }

        Ok((mgr, initial_hosts))
    }

    pub fn add_remote(&self, name: String, http_url: String, token: String) -> Result<RemoteHostConfig> {
        let clean_url = http_url.trim().trim_end_matches('/').to_string();
        if clean_url.is_empty() || token.trim().is_empty() {
            return Err(anyhow!("URL and Bearer token are required"));
        }
        let remote = RemoteHostConfig {
            id: new_id(),
            name: if name.trim().is_empty() {
                clean_url.clone()
            } else {
                name.trim().to_string()
            },
            http_url: clean_url.clone(),
            token: token.trim().to_string(),
        };

        let ws_url = remote.ws_url();
        self.spawn_connection(HostConnectionSpec {
            host_id: remote.id.clone(),
            http_url: clean_url,
            ws_url,
            token: remote.token.clone(),
            allow_autospawn: false,
        })?;

        let mut cfg = self.hosts_cfg.lock().unwrap();
        cfg.remotes.push(remote.clone());
        let _ = cfg.save();
        Ok(remote)
    }

    pub fn remove_remote(&self, host_id: &str) {
        if host_id == LOCAL_HOST_ID {
            return;
        }
        self.senders.lock().unwrap().remove(host_id);
        let mut cfg = self.hosts_cfg.lock().unwrap();
        cfg.remotes.retain(|r| r.id != host_id);
        let _ = cfg.save();
    }

    pub fn send(&self, host_id: &str, command: RpcCommand) {
        if let Ok(guard) = self.senders.lock() {
            if let Some(tx) = guard.get(host_id) {
                let _ = tx.send(command);
            }
        }
    }

    pub fn has_pending_updates(&self) -> bool {
        self.updates.1.load(Ordering::Acquire)
    }

    pub fn drain_updates(&self) -> Vec<HostUpdate> {
        let mut guard = self.updates.0.lock().unwrap();
        self.updates.1.store(false, Ordering::Release);
        std::mem::take(&mut *guard)
    }

    fn spawn_connection(&self, spec: HostConnectionSpec) -> Result<()> {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        self.senders
            .lock()
            .unwrap()
            .insert(spec.host_id.clone(), cmd_tx);

        let updates = Arc::clone(&self.updates);
        let thread_name = format!(
            "hasilanx-host-{}",
            &spec.host_id[..spec.host_id.len().min(8)]
        );
        std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build host tokio runtime");
                rt.block_on(run_client_loop(spec, cmd_rx, updates));
            })?;
        Ok(())
    }
}

fn push_update(queue: &SharedUpdateQueue, host_id: &str, kind: ClientUpdateKind) {
    if let Ok(mut guard) = queue.0.lock() {
        guard.push(HostUpdate {
            host_id: host_id.to_string(),
            kind,
        });
        queue.1.store(true, Ordering::Release);
    }
}

async fn run_client_loop(
    spec: HostConnectionSpec,
    mut cmd_rx: mpsc::UnboundedReceiver<RpcCommand>,
    updates: SharedUpdateQueue,
) {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(6))
        .build()
        .expect("reqwest client");

    let mut last_seq: i64 = 0;
    let mut attempted_autospawn = false;

    loop {
        if cmd_rx.is_closed() {
            return;
        }

        let health_url = format!("{}/api/v1/health", spec.http_url);
        let health = match http.get(&health_url).send().await {
            Ok(resp) if resp.status().is_success() => resp.json::<HealthResponse>().await.ok(),
            _ => None,
        };

        let health = match health {
            Some(h) => h,
            None => {
                if spec.allow_autospawn && !attempted_autospawn {
                    attempted_autospawn = true;
                    push_update(
                        &updates,
                        &spec.host_id,
                        ClientUpdateKind::Disconnected("Starting local hasilanx-daemon...".into()),
                    );
                    if let Err(err) = spawn_local_daemon() {
                        warn!(error = %err, "failed to auto-spawn hasilanx-daemon");
                        push_update(
                            &updates,
                            &spec.host_id,
                            ClientUpdateKind::Disconnected(format!(
                                "Daemon unreachable (auto-start failed: {err})"
                            )),
                        );
                    } else {
                        tokio::time::sleep(Duration::from_millis(700)).await;
                        continue;
                    }
                } else {
                    push_update(
                        &updates,
                        &spec.host_id,
                        ClientUpdateKind::Disconnected(format!(
                            "Waiting for {}...",
                            spec.http_url
                        )),
                    );
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };

        let snap_url = if last_seq > 0 {
            format!("{}/api/v1/snapshot?since_seq={}", spec.http_url, last_seq)
        } else {
            format!("{}/api/v1/snapshot", spec.http_url)
        };

        let snap_res = http
            .get(&snap_url)
            .bearer_auth(&spec.token)
            .send()
            .await
            .and_then(|r| r.error_for_status());

        match snap_res {
            Ok(resp) => match resp.json::<SnapshotResponse>().await {
                Ok(SnapshotResponse::Full(snap)) => {
                    last_seq = snap.latest_seq;
                    push_update(&updates, &spec.host_id, ClientUpdateKind::FullSnapshot(snap));
                }
                Ok(SnapshotResponse::Delta { latest_seq, events }) => {
                    last_seq = latest_seq;
                    for ev in events {
                        push_update(&updates, &spec.host_id, ClientUpdateKind::Event(ev));
                    }
                }
                Err(err) => {
                    push_update(
                        &updates,
                        &spec.host_id,
                        ClientUpdateKind::Disconnected(format!("Snapshot decode error: {err}")),
                    );
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            },
            Err(err) => {
                push_update(
                    &updates,
                    &spec.host_id,
                    ClientUpdateKind::Disconnected(format!("Snapshot failed: {err}")),
                );
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        }

        let ws_url = format!("{}?token={}", spec.ws_url, spec.token);
        let ws_conn = tokio_tungstenite::connect_async(&ws_url).await;
        let (ws_stream, _) = match ws_conn {
            Ok(pair) => pair,
            Err(err) => {
                push_update(
                    &updates,
                    &spec.host_id,
                    ClientUpdateKind::Disconnected(format!("WebSocket failed: {err}")),
                );
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };

        info!(host = %spec.host_id, url = %spec.http_url, "connected to hasilanx-daemon");
        push_update(
            &updates,
            &spec.host_id,
            ClientUpdateKind::Connected {
                hostname: health.hostname,
                daemon_version: health.version,
            },
        );

        let (mut ws_sink, mut ws_read) = ws_stream.split();
        let mut ping_interval = tokio::time::interval(Duration::from_secs(20));

        loop {
            tokio::select! {
                _ = ping_interval.tick() => {
                    if let Ok(text) = serde_json::to_string(&ClientMessage::Ping) {
                        if ws_sink.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                }
                maybe_cmd = cmd_rx.recv() => {
                    match maybe_cmd {
                        Some(command) => {
                            let msg = ClientMessage::Command {
                                request_id: new_id(),
                                command,
                            };
                            if let Ok(text) = serde_json::to_string(&msg) {
                                if ws_sink.send(Message::Text(text.into())).await.is_err() {
                                    break;
                                }
                            }
                        }
                        None => return,
                    }
                }
                maybe_frame = ws_read.next() => {
                    match maybe_frame {
                        Some(Ok(Message::Text(text))) => {
                            match serde_json::from_str::<ServerMessage>(&text) {
                                Ok(ServerMessage::Event(env)) => {
                                    if env.seq > last_seq {
                                        last_seq = env.seq;
                                    }
                                    push_update(&updates, &spec.host_id, ClientUpdateKind::Event(env));
                                }
                                Ok(ServerMessage::DiffUpdated { thread_id, diff }) => {
                                    push_update(
                                        &updates,
                                        &spec.host_id,
                                        ClientUpdateKind::DiffUpdated { thread_id, diff },
                                    );
                                }
                                Ok(ServerMessage::AgentsUpdated { agents }) => {
                                    push_update(
                                        &updates,
                                        &spec.host_id,
                                        ClientUpdateKind::AgentsUpdated(agents),
                                    );
                                }
                                Ok(ServerMessage::TerminalOutput { thread_id, data_b64 }) => {
                                    if let Ok(data) = base64::engine::general_purpose::STANDARD.decode(&data_b64) {
                                        push_update(
                                            &updates,
                                            &spec.host_id,
                                            ClientUpdateKind::TerminalOutput { thread_id, data },
                                        );
                                    }
                                }
                                Ok(ServerMessage::TerminalExited { thread_id, code }) => {
                                    push_update(
                                        &updates,
                                        &spec.host_id,
                                        ClientUpdateKind::TerminalExited { thread_id, code },
                                    );
                                }
                                Ok(ServerMessage::ChecksUpdated { thread_id, checks }) => {
                                    push_update(
                                        &updates,
                                        &spec.host_id,
                                        ClientUpdateKind::ChecksUpdated { thread_id, checks },
                                    );
                                }
                                Ok(ServerMessage::ProjectConfigUpdated { project_id, config }) => {
                                    push_update(
                                        &updates,
                                        &spec.host_id,
                                        ClientUpdateKind::ProjectConfigUpdated {
                                            project_id,
                                            config,
                                        },
                                    );
                                }
                                Ok(ServerMessage::CommandResult { ok: false, error, .. }) => {
                                    if let Some(msg) = error {
                                        push_update(&updates, &spec.host_id, ClientUpdateKind::CommandError(msg));
                                    }
                                }
                                Ok(_) => {}
                                Err(err) => warn!(error = %err, "failed to parse server ws frame"),
                            }
                        }
                        Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                        _ => {}
                    }
                }
            }
        }

        push_update(
            &updates,
            &spec.host_id,
            ClientUpdateKind::Disconnected("Disconnected; reconnecting...".into()),
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn spawn_local_daemon() -> Result<()> {
    let exe = std::env::current_exe().context("cannot determine current executable")?;
    let sibling = exe
        .parent()
        .map(|p| p.join("hasilanx-daemon"))
        .filter(|p| p.is_file());

    let bin = sibling
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "hasilanx-daemon".to_string());

    Command::new(&bin)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("failed to launch `{bin}`: {e}"))?;

    Ok(())
}
