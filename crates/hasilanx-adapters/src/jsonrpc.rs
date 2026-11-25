use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc, oneshot};
use tracing::{debug, warn};

#[derive(Debug)]
pub enum IncomingRpc {
    Notification {
        method: String,
        params: Value,
    },
    ServerRequest {
        id: Value,
        method: String,
        params: Value,
    },
    Closed,
}

pub struct JsonRpcProcess {
    include_jsonrpc_field: bool,
    next_id: AtomicI64,
    writer: Arc<Mutex<tokio::process::ChildStdin>>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value>>>>>,
    _child: Arc<Mutex<Child>>,
}

impl JsonRpcProcess {
    pub async fn spawn(
        program: &str,
        args: &[String],
        cwd: Option<&Path>,
        include_jsonrpc_field: bool,
    ) -> Result<(Arc<Self>, mpsc::UnboundedReceiver<IncomingRpc>)> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn `{program}` with args {args:?}"))?;

        let stdin = child.stdin.take().context("child stdin unavailable")?;
        let stdout = child.stdout.take().context("child stdout unavailable")?;
        let stderr = child.stderr.take().context("child stderr unavailable")?;

        let writer = Arc::new(Mutex::new(stdin));
        let pending: Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = mpsc::unbounded_channel();

        // Drain stderr to tracing
        let prog_name = program.to_string();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.trim().is_empty() {
                    debug!(target: "adapter_stderr", program = %prog_name, "{line}");
                }
            }
        });

        // Read stdout newline-delimited JSON-RPC frames
        let pending_reader = Arc::clone(&pending);
        let tx_reader = tx.clone();
        let prog_name2 = program.to_string();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let val: Value = match serde_json::from_str(trimmed) {
                    Ok(v) => v,
                    Err(err) => {
                        debug!(program = %prog_name2, error = %err, line = %trimmed, "ignoring non-json stdout line");
                        continue;
                    }
                };

                let has_method = val.get("method").and_then(Value::as_str).is_some();
                let id_opt = val.get("id").cloned().filter(|v| !v.is_null());

                match (has_method, id_opt) {
                    // Server-initiated request (has both method and id)
                    (true, Some(id)) => {
                        let method = val["method"].as_str().unwrap_or_default().to_string();
                        let params = val.get("params").cloned().unwrap_or(Value::Null);
                        let _ = tx_reader.send(IncomingRpc::ServerRequest { id, method, params });
                    }
                    // Notification (method without id)
                    (true, None) => {
                        let method = val["method"].as_str().unwrap_or_default().to_string();
                        let params = val.get("params").cloned().unwrap_or(Value::Null);
                        let _ = tx_reader.send(IncomingRpc::Notification { method, params });
                    }
                    // Response to our request (id without method)
                    (false, Some(id)) => {
                        let key = id_key(&id);
                        let sender = pending_reader.lock().await.remove(&key);
                        if let Some(ch) = sender {
                            if let Some(err) = val.get("error") {
                                let msg = err
                                    .get("message")
                                    .and_then(Value::as_str)
                                    .unwrap_or("rpc error");
                                let _ = ch.send(Err(anyhow!("RPC error: {msg} ({err})")));
                            } else {
                                let result = val.get("result").cloned().unwrap_or(Value::Null);
                                let _ = ch.send(Ok(result));
                            }
                        } else {
                            warn!(program = %prog_name2, id = %key, "received response for unknown request id");
                        }
                    }
                    (false, None) => {
                        debug!(program = %prog_name2, frame = %trimmed, "unhandled json frame");
                    }
                }
            }

            // Fail any in-flight requests when stdout closes
            let mut map = pending_reader.lock().await;
            for (_, ch) in map.drain() {
                let _ = ch.send(Err(anyhow!("agent process exited unexpectedly")));
            }
            let _ = tx_reader.send(IncomingRpc::Closed);
        });

        let proc = Arc::new(Self {
            include_jsonrpc_field,
            next_id: AtomicI64::new(1),
            writer,
            pending,
            _child: Arc::new(Mutex::new(child)),
        });
        Ok((proc, rx))
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id_num = self.next_id.fetch_add(1, Ordering::Relaxed);
        let id_val = Value::from(id_num);
        let key = id_key(&id_val);

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(key.clone(), tx);

        let frame = if self.include_jsonrpc_field {
            json!({
                "jsonrpc": "2.0",
                "id": id_num,
                "method": method,
                "params": params,
            })
        } else {
            json!({
                "id": id_num,
                "method": method,
                "params": params,
            })
        };

        if let Err(err) = self.write_json(&frame).await {
            self.pending.lock().await.remove(&key);
            return Err(err);
        }

        rx.await.context("rpc response channel dropped")?
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let frame = if self.include_jsonrpc_field {
            json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params,
            })
        } else {
            json!({
                "method": method,
                "params": params,
            })
        };
        self.write_json(&frame).await
    }

    pub async fn respond_ok(&self, id: Value, result: Value) -> Result<()> {
        let frame = if self.include_jsonrpc_field {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result,
            })
        } else {
            json!({
                "id": id,
                "result": result,
            })
        };
        self.write_json(&frame).await
    }

    pub async fn respond_error(&self, id: Value, code: i64, message: &str) -> Result<()> {
        let frame = if self.include_jsonrpc_field {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": code, "message": message },
            })
        } else {
            json!({
                "id": id,
                "error": { "code": code, "message": message },
            })
        };
        self.write_json(&frame).await
    }

    async fn write_json(&self, value: &Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(value)?;
        bytes.push(b'\n');
        let mut w = self.writer.lock().await;
        w.write_all(&bytes).await?;
        w.flush().await?;
        Ok(())
    }
}

fn id_key(id: &Value) -> String {
    match id {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
