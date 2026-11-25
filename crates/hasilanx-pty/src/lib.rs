use anyhow::{Context, Result, anyhow};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use tracing::debug;

#[derive(Debug, Clone)]
pub enum PtyEvent {
    Output { thread_id: String, data: Vec<u8> },
    Exited { thread_id: String, code: Option<i32> },
}

struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

pub struct PtyManager {
    sessions: Arc<Mutex<HashMap<String, PtySession>>>,
    event_tx: broadcast::Sender<PtyEvent>,
}

impl PtyManager {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(4096);
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            event_tx,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PtyEvent> {
        self.event_tx.subscribe()
    }

    pub fn open(&self, thread_id: &str, cwd: &Path, cols: u16, rows: u16) -> Result<()> {
        {
            let guard = self.sessions.lock().map_err(|_| anyhow!("pty mutex poisoned"))?;
            if guard.contains_key(thread_id) {
                return Ok(());
            }
        }

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: rows.max(4),
                cols: cols.max(20),
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to open PTY")?;

        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/bash".to_string());

        let mut cmd = CommandBuilder::new(&shell);
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "HasilanX");

        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("failed to spawn shell `{shell}`"))?;

        let mut reader = pair
            .master
            .try_clone_reader()
            .context("failed to clone PTY reader")?;
        let writer = pair
            .master
            .take_writer()
            .context("failed to take PTY writer")?;

        let session = PtySession {
            master: pair.master,
            writer,
            child,
        };

        {
            let mut guard = self.sessions.lock().map_err(|_| anyhow!("pty mutex poisoned"))?;
            guard.insert(thread_id.to_string(), session);
        }

        let tid = thread_id.to_string();
        let tx = self.event_tx.clone();
        let sessions_ref = Arc::clone(&self.sessions);

        std::thread::Builder::new()
            .name(format!("hasilanx-pty-{}", &tid[..tid.len().min(8)]))
            .spawn(move || {
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let _ = tx.send(PtyEvent::Output {
                                thread_id: tid.clone(),
                                data: buf[..n].to_vec(),
                            });
                        }
                        Err(err) => {
                            debug!(thread_id = %tid, error = %err, "PTY reader ended");
                            break;
                        }
                    }
                }
                let exit_code = sessions_ref
                    .lock()
                    .ok()
                    .and_then(|mut map| map.remove(&tid))
                    .and_then(|mut s| s.child.try_wait().ok().flatten())
                    .map(|st| st.exit_code() as i32);
                let _ = tx.send(PtyEvent::Exited {
                    thread_id: tid,
                    code: exit_code,
                });
            })?;

        Ok(())
    }

    pub fn write(&self, thread_id: &str, data: &[u8]) -> Result<()> {
        let mut guard = self.sessions.lock().map_err(|_| anyhow!("pty mutex poisoned"))?;
        let sess = guard
            .get_mut(thread_id)
            .ok_or_else(|| anyhow!("no active terminal for thread `{thread_id}`"))?;
        sess.writer.write_all(data)?;
        sess.writer.flush()?;
        Ok(())
    }

    pub fn resize(&self, thread_id: &str, cols: u16, rows: u16) -> Result<()> {
        let guard = self.sessions.lock().map_err(|_| anyhow!("pty mutex poisoned"))?;
        if let Some(sess) = guard.get(thread_id) {
            sess.master.resize(PtySize {
                rows: rows.max(4),
                cols: cols.max(20),
                pixel_width: 0,
                pixel_height: 0,
            })?;
        }
        Ok(())
    }

    pub fn close(&self, thread_id: &str) {
        if let Ok(mut guard) = self.sessions.lock() {
            if let Some(mut sess) = guard.remove(thread_id) {
                let _ = sess.child.kill();
            }
        }
    }
}

impl Default for PtyManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn pty_spawn_write_read_resize_and_close() {
        let mgr = PtyManager::new();
        let mut rx = mgr.subscribe();
        let cwd = std::env::temp_dir();

        mgr.open("t-pty-1", &cwd, 80, 24).unwrap();
        mgr.resize("t-pty-1", 100, 30).unwrap();
        mgr.write("t-pty-1", b"echo HASILANX_PTY_OK\n").unwrap();

        let mut collected = String::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
                Ok(Ok(PtyEvent::Output { thread_id, data })) if thread_id == "t-pty-1" => {
                    collected.push_str(&String::from_utf8_lossy(&data));
                    if collected.contains("HASILANX_PTY_OK") {
                        break;
                    }
                }
                _ => {}
            }
        }

        mgr.close("t-pty-1");
        assert!(
            collected.contains("HASILANX_PTY_OK"),
            "expected echo output, got: {collected:?}"
        );
    }
}
