use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use hasilanx_protocol::{
    ClientMessage, ExecutionMode, HealthResponse, RpcCommand, ServerMessage, SnapshotResponse,
    WorkspaceMode,
};
use std::fs;
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn init_git_repo(dir: &Path) {
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.email", "test@hasilanx.local"],
        vec!["config", "user.name", "HasilanX"],
    ] {
        assert!(Command::new("git").args(&args).current_dir(dir).status().unwrap().success());
    }
    fs::write(dir.join("hello.txt"), "initial\n").unwrap();
    assert!(Command::new("git").args(["add", "-A"]).current_dir(dir).status().unwrap().success());
    assert!(
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(dir)
            .status()
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn daemon_binary_e2e_ws_worktree_diff_pty_and_delta_sync() {
    let port = free_port();
    let token = "e2e-secret-token";
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("e2e.db");
    let repo_dir = tmp.path().join("repo");
    fs::create_dir_all(&repo_dir).unwrap();
    init_git_repo(&repo_dir);

    let bin = env!("CARGO_BIN_EXE_hasilanx-daemon");

    // Verify `service status` CLI subcommand works cleanly
    let svc_out = Command::new(bin).args(["service", "status"]).output().unwrap();
    assert!(svc_out.status.success());

    let child = Command::new(bin)
        .args([
            "run",
            "--bind",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--db",
            &db_path.to_string_lossy(),
            "--token",
            token,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn hasilanx-daemon");

    struct KillOnDrop(std::process::Child);
    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _guard = KillOnDrop(child);

    let http = reqwest::Client::new();
    let base_http = format!("http://127.0.0.1:{port}");

    // Wait for daemon health
    let mut healthy = false;
    for _ in 0..30 {
        if let Ok(resp) = http.get(format!("{base_http}/api/v1/health")).send().await {
            if resp.status().is_success() {
                let h: HealthResponse = resp.json().await.unwrap();
                assert_eq!(h.status, "ok");
                healthy = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(healthy, "daemon did not become healthy in time");

    // Unauthorized rejected
    let unauth = http
        .get(format!("{base_http}/api/v1/snapshot"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth.status().as_u16(), 401);

    // Connect WebSocket
    let ws_url = format!("ws://127.0.0.1:{port}/api/v1/ws?token={token}");
    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();

    // Helper to send command and wait for its CommandResult
    async fn send_cmd(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        req_id: &str,
        command: RpcCommand,
    ) -> serde_json::Value {
        let msg = ClientMessage::Command {
            request_id: req_id.to_string(),
            command,
        };
        ws.send(Message::Text(serde_json::to_string(&msg).unwrap().into()))
            .await
            .unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            let frame = tokio::time::timeout(Duration::from_secs(2), ws.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            if let Message::Text(txt) = frame {
                let srv: ServerMessage = serde_json::from_str(&txt).unwrap();
                if let ServerMessage::CommandResult {
                    request_id,
                    ok,
                    data,
                    error,
                } = srv
                {
                    if request_id == req_id {
                        assert!(ok, "command `{req_id}` failed: {error:?}");
                        return data.unwrap_or(serde_json::Value::Null);
                    }
                }
            }
        }
        panic!("timeout waiting for command result `{req_id}`");
    }

    // 1. Create Project
    let proj = send_cmd(
        &mut ws,
        "req-1",
        RpcCommand::CreateProject {
            name: Some("e2e-repo".into()),
            root_path: repo_dir.to_string_lossy().to_string(),
        },
    )
    .await;
    let project_id = proj["id"].as_str().unwrap().to_string();

    // 2. Create Worktree Thread
    let thread = send_cmd(
        &mut ws,
        "req-2",
        RpcCommand::CreateThread {
            project_id,
            title: Some("WT Feature".into()),
            agent_id: "codex".into(),
            model: None,
            thinking: Some("high".into()),
            execution_mode: ExecutionMode::Ask,
            workspace_mode: WorkspaceMode::Worktree,
        },
    )
    .await;
    let thread_id = thread["id"].as_str().unwrap().to_string();
    let wt_path = Path::new(thread["worktree_path"].as_str().unwrap()).to_path_buf();
    assert!(wt_path.exists(), "worktree directory should be created");

    // 3. Mutate file inside worktree -> GetThreadDiff -> GitCommit
    fs::write(wt_path.join("hello.txt"), "initial\nupdated\n").unwrap();
    let diff = send_cmd(
        &mut ws,
        "req-3",
        RpcCommand::GetThreadDiff {
            thread_id: thread_id.clone(),
        },
    )
    .await;
    assert_eq!(diff["dirty"].as_bool(), Some(true));
    assert!(diff["unified_diff"].as_str().unwrap().contains("+updated"));

    let _commit = send_cmd(
        &mut ws,
        "req-4",
        RpcCommand::GitCommit {
            thread_id: thread_id.clone(),
            message: "feat: update hello".into(),
        },
    )
    .await;

    // 4. Open integrated PTY terminal -> execute echo -> verify streaming TerminalOutput
    let _open = send_cmd(
        &mut ws,
        "req-5",
        RpcCommand::TerminalOpen {
            thread_id: thread_id.clone(),
            cols: 80,
            rows: 24,
        },
    )
    .await;
    let cmd_b64 = base64::engine::general_purpose::STANDARD.encode(b"echo E2E_TERM_WORKS\n");
    let _write = send_cmd(
        &mut ws,
        "req-6",
        RpcCommand::TerminalWrite {
            thread_id: thread_id.clone(),
            data_b64: cmd_b64,
        },
    )
    .await;

    let mut term_out = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(Message::Text(txt)))) =
            tokio::time::timeout(Duration::from_millis(500), ws.next()).await
        {
            if let Ok(ServerMessage::TerminalOutput {
                thread_id: tid,
                data_b64,
            }) = serde_json::from_str(&txt)
            {
                if tid == thread_id {
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&data_b64) {
                        term_out.push_str(&String::from_utf8_lossy(&bytes));
                        if term_out.contains("E2E_TERM_WORKS") {
                            break;
                        }
                    }
                }
            }
        }
    }
    assert!(
        term_out.contains("E2E_TERM_WORKS"),
        "expected terminal output over WS, got: {term_out:?}"
    );

    // 5. Verify Delta Sync via `since_seq=1`
    let delta: SnapshotResponse = http
        .get(format!("{base_http}/api/v1/snapshot?since_seq=1"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    match delta {
        SnapshotResponse::Delta { latest_seq, events } => {
            assert!(latest_seq >= 3);
            assert!(!events.is_empty());
        }
        other => panic!("expected Delta snapshot, got {other:?}"),
    }

    // 6. Delete worktree thread -> verify worktree directory cleaned up
    let _del = send_cmd(
        &mut ws,
        "req-7",
        RpcCommand::DeleteThread {
            thread_id: thread_id.clone(),
        },
    )
    .await;
    assert!(!wt_path.exists(), "worktree directory should be removed on thread delete");
}
