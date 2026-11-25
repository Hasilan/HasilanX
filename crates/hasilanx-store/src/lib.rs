use hasilanx_protocol::{
    AgentDescriptor, ApprovalOption, Attachment, DomainEvent, EventEnvelope, ExecutionMode,
    PendingApproval, ProjectRecord, QueuedPrompt, ThreadRecord, ThreadStatus, TimelineItem,
    TimelineItemKind, TurnCompletionStatus, TurnRecord, WorkspaceMode, WorkspaceSnapshot, new_id,
    now_ms,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::path::Path;
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("store mutex poisoned")]
    Poisoned,
    #[error("not found: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Clone)]
pub struct EventStore {
    conn: Arc<Mutex<Connection>>,
}

impl EventStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;
            PRAGMA synchronous = NORMAL;

            CREATE TABLE IF NOT EXISTS events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                event_id TEXT NOT NULL UNIQUE,
                thread_id TEXT,
                project_id TEXT,
                kind TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_events_thread_seq ON events(thread_id, seq);

            CREATE TABLE IF NOT EXISTS projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                root_path TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS threads (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                title TEXT NOT NULL,
                workspace_mode TEXT NOT NULL,
                worktree_path TEXT,
                branch_name TEXT,
                active_agent_id TEXT NOT NULL,
                active_model TEXT,
                active_thinking TEXT,
                execution_mode TEXT NOT NULL,
                status TEXT NOT NULL,
                last_error TEXT,
                vendor_session_json TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_threads_project ON threads(project_id);

            CREATE TABLE IF NOT EXISTS turns (
                id TEXT PRIMARY KEY,
                thread_id TEXT NOT NULL,
                turn_index INTEGER NOT NULL,
                user_prompt TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                status TEXT NOT NULL,
                checkpoint_before_ref TEXT,
                checkpoint_after_ref TEXT,
                started_at INTEGER NOT NULL,
                finished_at INTEGER,
                FOREIGN KEY (thread_id) REFERENCES threads(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_turns_thread ON turns(thread_id, turn_index);

            CREATE TABLE IF NOT EXISTS timeline_items (
                id TEXT PRIMARY KEY,
                thread_id TEXT NOT NULL,
                turn_id TEXT,
                kind_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                FOREIGN KEY (thread_id) REFERENCES threads(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_timeline_thread_created ON timeline_items(thread_id, created_at);

            CREATE TABLE IF NOT EXISTS pending_approvals (
                id TEXT PRIMARY KEY,
                thread_id TEXT NOT NULL,
                turn_id TEXT,
                tool_name TEXT NOT NULL,
                description TEXT NOT NULL,
                args_json TEXT,
                options_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                FOREIGN KEY (thread_id) REFERENCES threads(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS prompt_queue (
                id TEXT PRIMARY KEY,
                thread_id TEXT NOT NULL,
                prompt TEXT NOT NULL,
                attachments_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                FOREIGN KEY (thread_id) REFERENCES threads(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_queue_thread ON prompt_queue(thread_id, created_at);
            "#,
        )?;
        // Safe additive migration for existing SQLite databases
        let _ = conn.execute("ALTER TABLE threads ADD COLUMN active_thinking TEXT", []);
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Clean up any stale in-flight state left over from a previous daemon crash.
    pub fn recover_stale_runs(&self) -> Result<()> {
        let mut conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE threads SET status = 'idle' WHERE status IN ('running', 'waiting_approval')",
            [],
        )?;
        tx.execute(
            "UPDATE turns SET status = 'cancelled' WHERE status = 'running'",
            [],
        )?;
        tx.execute("DELETE FROM pending_approvals", [])?;
        tx.commit()?;
        Ok(())
    }

    pub fn append_event(&self, event: DomainEvent) -> Result<EventEnvelope> {
        let mut conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let tx = conn.transaction()?;

        let event_id = new_id();
        let created_at = now_ms();
        let payload_json = serde_json::to_string(&event)?;
        let kind = event.event_kind();
        let thread_id = event.thread_id().map(ToOwned::to_owned);
        let project_id = event.project_id().map(ToOwned::to_owned);

        tx.execute(
            "INSERT INTO events (event_id, thread_id, project_id, kind, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![event_id, thread_id, project_id, kind, payload_json, created_at],
        )?;
        let seq = tx.last_insert_rowid();

        apply_projection(&tx, &event, created_at)?;
        tx.commit()?;

        Ok(EventEnvelope {
            seq,
            event_id,
            created_at,
            event,
        })
    }

    pub fn latest_seq(&self) -> Result<i64> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let seq: i64 = conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| r.get(0))?;
        Ok(seq)
    }

    pub fn events_since(&self, since_seq: i64) -> Result<(i64, Vec<EventEnvelope>)> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let latest_seq: i64 =
            conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| r.get(0))?;
        let mut stmt = conn.prepare(
            "SELECT seq, event_id, payload_json, created_at FROM events WHERE seq > ?1 ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map(params![since_seq], |row| {
            let seq: i64 = row.get(0)?;
            let event_id: String = row.get(1)?;
            let payload_json: String = row.get(2)?;
            let created_at: i64 = row.get(3)?;
            Ok((seq, event_id, payload_json, created_at))
        })?;

        let mut events = Vec::new();
        for row in rows {
            let (seq, event_id, payload_json, created_at) = row?;
            let event: DomainEvent = serde_json::from_str(&payload_json)?;
            events.push(EventEnvelope {
                seq,
                event_id,
                created_at,
                event,
            });
        }
        Ok((latest_seq, events))
    }

    pub fn load_snapshot(
        &self,
        daemon_version: String,
        hostname: String,
        agents: Vec<AgentDescriptor>,
    ) -> Result<WorkspaceSnapshot> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let latest_seq: i64 =
            conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| r.get(0))?;

        let projects = {
            let mut stmt = conn.prepare(
                "SELECT id, name, root_path, created_at FROM projects ORDER BY created_at ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(ProjectRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    root_path: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };

        let threads = {
            let mut stmt = conn.prepare(
                "SELECT id, project_id, title, workspace_mode, worktree_path, branch_name,
                        active_agent_id, active_model, active_thinking, execution_mode, status,
                        last_error, created_at, updated_at
                 FROM threads ORDER BY updated_at DESC",
            )?;
            let rows = stmt.query_map([], map_thread_row)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r??);
            }
            out
        };

        let turns = {
            let mut stmt = conn.prepare(
                "SELECT id, thread_id, turn_index, user_prompt, agent_id, status,
                        checkpoint_before_ref, checkpoint_after_ref, started_at, finished_at
                 FROM turns ORDER BY started_at ASC",
            )?;
            let rows = stmt.query_map([], map_turn_row)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r??);
            }
            out
        };

        let timeline_items = {
            let mut stmt = conn.prepare(
                "SELECT id, thread_id, turn_id, kind_json, created_at, updated_at
                 FROM timeline_items ORDER BY created_at ASC, rowid ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, thread_id, turn_id, kind_json, created_at, updated_at) = row?;
                let kind: TimelineItemKind = serde_json::from_str(&kind_json)?;
                out.push(TimelineItem {
                    id,
                    thread_id,
                    turn_id,
                    kind,
                    created_at,
                    updated_at,
                });
            }
            out
        };

        let pending_approvals = {
            let mut stmt = conn.prepare(
                "SELECT id, thread_id, turn_id, tool_name, description, args_json, options_json, created_at
                 FROM pending_approvals ORDER BY created_at ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, thread_id, turn_id, tool_name, description, args_json, options_json, created_at) = row?;
                let options: Vec<ApprovalOption> = serde_json::from_str(&options_json)?;
                out.push(PendingApproval {
                    id,
                    thread_id,
                    turn_id,
                    tool_name,
                    description,
                    args_json,
                    options,
                    created_at,
                });
            }
            out
        };

        let prompt_queue = {
            let mut stmt = conn.prepare(
                "SELECT id, thread_id, prompt, attachments_json, created_at
                 FROM prompt_queue ORDER BY created_at ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, thread_id, prompt, attachments_json, created_at) = row?;
                let attachments: Vec<Attachment> = serde_json::from_str(&attachments_json)?;
                out.push(QueuedPrompt {
                    id,
                    thread_id,
                    prompt,
                    attachments,
                    created_at,
                });
            }
            out
        };

        Ok(WorkspaceSnapshot {
            latest_seq,
            daemon_version,
            hostname,
            agents,
            projects,
            threads,
            turns,
            timeline_items,
            pending_approvals,
            prompt_queue,
            project_configs: std::collections::HashMap::new(),
            thread_checks: std::collections::HashMap::new(),
        })
    }

    pub fn get_project(&self, project_id: &str) -> Result<Option<ProjectRecord>> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let res = conn
            .query_row(
                "SELECT id, name, root_path, created_at FROM projects WHERE id = ?1",
                params![project_id],
                |row| {
                    Ok(ProjectRecord {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        root_path: row.get(2)?,
                        created_at: row.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(res)
    }

    pub fn get_thread(&self, thread_id: &str) -> Result<Option<ThreadRecord>> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let res = conn
            .query_row(
                "SELECT id, project_id, title, workspace_mode, worktree_path, branch_name,
                        active_agent_id, active_model, active_thinking, execution_mode, status,
                        last_error, created_at, updated_at
                 FROM threads WHERE id = ?1",
                params![thread_id],
                map_thread_row,
            )
            .optional()?;
        match res {
            Some(inner) => Ok(Some(inner?)),
            None => Ok(None),
        }
    }

    pub fn next_turn_index(&self, thread_id: &str) -> Result<i64> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let max_idx: i64 = conn.query_row(
            "SELECT COALESCE(MAX(turn_index), -1) FROM turns WHERE thread_id = ?1",
            params![thread_id],
            |r| r.get(0),
        )?;
        Ok(max_idx + 1)
    }

    pub fn get_turn(&self, turn_id: &str) -> Result<Option<TurnRecord>> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let res = conn
            .query_row(
                "SELECT id, thread_id, turn_index, user_prompt, agent_id, status,
                        checkpoint_before_ref, checkpoint_after_ref, started_at, finished_at
                 FROM turns WHERE id = ?1",
                params![turn_id],
                map_turn_row,
            )
            .optional()?;
        match res {
            Some(inner) => Ok(Some(inner?)),
            None => Ok(None),
        }
    }

    pub fn list_thread_turns(&self, thread_id: &str) -> Result<Vec<TurnRecord>> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let mut stmt = conn.prepare(
            "SELECT id, thread_id, turn_index, user_prompt, agent_id, status,
                    checkpoint_before_ref, checkpoint_after_ref, started_at, finished_at
             FROM turns WHERE thread_id = ?1 ORDER BY turn_index ASC",
        )?;
        let rows = stmt.query_map(params![thread_id], map_turn_row)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r??);
        }
        Ok(out)
    }

    pub fn list_thread_timeline(&self, thread_id: &str) -> Result<Vec<TimelineItem>> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let mut stmt = conn.prepare(
            "SELECT id, thread_id, turn_id, kind_json, created_at, updated_at
             FROM timeline_items WHERE thread_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(params![thread_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, thread_id, turn_id, kind_json, created_at, updated_at) = row?;
            let kind: TimelineItemKind = serde_json::from_str(&kind_json)?;
            out.push(TimelineItem {
                id,
                thread_id,
                turn_id,
                kind,
                created_at,
                updated_at,
            });
        }
        Ok(out)
    }

    pub fn peek_next_queued_prompt(&self, thread_id: &str) -> Result<Option<QueuedPrompt>> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let row = conn
            .query_row(
                "SELECT id, thread_id, prompt, attachments_json, created_at
                 FROM prompt_queue WHERE thread_id = ?1 ORDER BY created_at ASC LIMIT 1",
                params![thread_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()?;
        match row {
            Some((id, thread_id, prompt, attachments_json, created_at)) => {
                let attachments: Vec<Attachment> = serde_json::from_str(&attachments_json)?;
                Ok(Some(QueuedPrompt {
                    id,
                    thread_id,
                    prompt,
                    attachments,
                    created_at,
                }))
            }
            None => Ok(None),
        }
    }

    pub fn get_vendor_session(&self, thread_id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let res: Option<Option<String>> = conn
            .query_row(
                "SELECT vendor_session_json FROM threads WHERE id = ?1",
                params![thread_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(res.flatten())
    }

    pub fn set_vendor_session(&self, thread_id: &str, vendor_session_json: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        conn.execute(
            "UPDATE threads SET vendor_session_json = ?1 WHERE id = ?2",
            params![vendor_session_json, thread_id],
        )?;
        Ok(())
    }
}

fn map_thread_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<ThreadRecord>> {
    let id: String = row.get(0)?;
    let project_id: String = row.get(1)?;
    let title: String = row.get(2)?;
    let workspace_mode_s: String = row.get(3)?;
    let worktree_path: Option<String> = row.get(4)?;
    let branch_name: Option<String> = row.get(5)?;
    let active_agent_id: String = row.get(6)?;
    let active_model: Option<String> = row.get(7)?;
    let active_thinking: Option<String> = row.get(8)?;
    let execution_mode_s: String = row.get(9)?;
    let status_s: String = row.get(10)?;
    let last_error: Option<String> = row.get(11)?;
    let created_at: i64 = row.get(12)?;
    let updated_at: i64 = row.get(13)?;

    let parsed = (|| -> Result<ThreadRecord> {
        let workspace_mode: WorkspaceMode =
            serde_json::from_str(&format!("\"{workspace_mode_s}\""))?;
        let execution_mode: ExecutionMode =
            serde_json::from_str(&format!("\"{execution_mode_s}\""))?;
        let status: ThreadStatus = serde_json::from_str(&format!("\"{status_s}\""))?;
        Ok(ThreadRecord {
            id,
            project_id,
            title,
            workspace_mode,
            worktree_path,
            branch_name,
            active_agent_id,
            active_model,
            active_thinking,
            execution_mode,
            status,
            last_error,
            created_at,
            updated_at,
        })
    })();
    Ok(parsed)
}

fn map_turn_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<TurnRecord>> {
    let id: String = row.get(0)?;
    let thread_id: String = row.get(1)?;
    let turn_index: i64 = row.get(2)?;
    let user_prompt: String = row.get(3)?;
    let agent_id: String = row.get(4)?;
    let status_s: String = row.get(5)?;
    let checkpoint_before_ref: Option<String> = row.get(6)?;
    let checkpoint_after_ref: Option<String> = row.get(7)?;
    let started_at: i64 = row.get(8)?;
    let finished_at: Option<i64> = row.get(9)?;

    let parsed = (|| -> Result<TurnRecord> {
        let status: TurnCompletionStatus = serde_json::from_str(&format!("\"{status_s}\""))?;
        Ok(TurnRecord {
            id,
            thread_id,
            turn_index,
            user_prompt,
            agent_id,
            status,
            checkpoint_before_ref,
            checkpoint_after_ref,
            started_at,
            finished_at,
        })
    })();
    Ok(parsed)
}

fn enum_to_db<T: serde::Serialize>(v: &T) -> Result<String> {
    let s = serde_json::to_string(v)?;
    Ok(s.trim_matches('"').to_string())
}

fn apply_projection(tx: &Transaction<'_>, event: &DomainEvent, now: i64) -> Result<()> {
    match event {
        DomainEvent::ProjectCreated { project } => {
            tx.execute(
                "INSERT OR REPLACE INTO projects (id, name, root_path, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![project.id, project.name, project.root_path, project.created_at],
            )?;
        }
        DomainEvent::ProjectDeleted { project_id } => {
            tx.execute("DELETE FROM projects WHERE id = ?1", params![project_id])?;
        }
        DomainEvent::ThreadCreated { thread } => {
            tx.execute(
                "INSERT OR REPLACE INTO threads (
                    id, project_id, title, workspace_mode, worktree_path, branch_name,
                    active_agent_id, active_model, active_thinking, execution_mode, status,
                    last_error, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    thread.id,
                    thread.project_id,
                    thread.title,
                    enum_to_db(&thread.workspace_mode)?,
                    thread.worktree_path,
                    thread.branch_name,
                    thread.active_agent_id,
                    thread.active_model,
                    thread.active_thinking,
                    enum_to_db(&thread.execution_mode)?,
                    enum_to_db(&thread.status)?,
                    thread.last_error,
                    thread.created_at,
                    thread.updated_at,
                ],
            )?;
        }
        DomainEvent::ThreadDeleted { thread_id } => {
            tx.execute("DELETE FROM threads WHERE id = ?1", params![thread_id])?;
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
            if let Some(t) = title {
                tx.execute(
                    "UPDATE threads SET title = ?1, updated_at = ?2 WHERE id = ?3",
                    params![t, now, thread_id],
                )?;
            }
            if let Some(agent_id) = active_agent_id {
                tx.execute(
                    "UPDATE threads SET active_agent_id = ?1, updated_at = ?2 WHERE id = ?3",
                    params![agent_id, now, thread_id],
                )?;
            }
            if let Some(model_opt) = active_model {
                tx.execute(
                    "UPDATE threads SET active_model = ?1, updated_at = ?2 WHERE id = ?3",
                    params![model_opt, now, thread_id],
                )?;
            }
            if let Some(thinking_opt) = active_thinking {
                tx.execute(
                    "UPDATE threads SET active_thinking = ?1, updated_at = ?2 WHERE id = ?3",
                    params![thinking_opt, now, thread_id],
                )?;
            }
            if let Some(mode) = execution_mode {
                tx.execute(
                    "UPDATE threads SET execution_mode = ?1, updated_at = ?2 WHERE id = ?3",
                    params![enum_to_db(mode)?, now, thread_id],
                )?;
            }
            if let Some(st) = status {
                tx.execute(
                    "UPDATE threads SET status = ?1, updated_at = ?2 WHERE id = ?3",
                    params![enum_to_db(st)?, now, thread_id],
                )?;
            }
            if let Some(err_opt) = last_error {
                tx.execute(
                    "UPDATE threads SET last_error = ?1, updated_at = ?2 WHERE id = ?3",
                    params![err_opt, now, thread_id],
                )?;
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
            tx.execute(
                "INSERT OR REPLACE INTO turns (
                    id, thread_id, turn_index, user_prompt, agent_id, status,
                    checkpoint_before_ref, checkpoint_after_ref, started_at, finished_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'running', ?6, NULL, ?7, NULL)",
                params![
                    turn_id,
                    thread_id,
                    turn_index,
                    user_prompt,
                    agent_id,
                    checkpoint_before_ref,
                    now,
                ],
            )?;
            tx.execute(
                "UPDATE threads SET status = 'running', last_error = NULL, updated_at = ?1 WHERE id = ?2",
                params![now, thread_id],
            )?;
        }
        DomainEvent::TurnCompleted {
            thread_id,
            turn_id,
            status,
            error,
            checkpoint_after_ref,
        } => {
            let next_status = match status {
                TurnCompletionStatus::Completed
                | TurnCompletionStatus::Cancelled
                | TurnCompletionStatus::Running => "idle",
                TurnCompletionStatus::Failed => "error",
            };
            tx.execute(
                "UPDATE turns SET status = ?1, checkpoint_after_ref = COALESCE(?2, checkpoint_after_ref), finished_at = ?3
                 WHERE id = ?4",
                params![enum_to_db(status)?, checkpoint_after_ref, now, turn_id],
            )?;
            tx.execute(
                "UPDATE threads SET status = ?1, last_error = ?2, updated_at = ?3 WHERE id = ?4",
                params![next_status, error, now, thread_id],
            )?;
            tx.execute(
                "DELETE FROM pending_approvals WHERE thread_id = ?1",
                params![thread_id],
            )?;
        }
        DomainEvent::TurnReverted { thread_id, .. } => {
            tx.execute(
                "UPDATE threads SET updated_at = ?1 WHERE id = ?2",
                params![now, thread_id],
            )?;
        }
        DomainEvent::TimelineItemUpserted { item } => {
            let kind_json = serde_json::to_string(&item.kind)?;
            tx.execute(
                "INSERT OR REPLACE INTO timeline_items (id, thread_id, turn_id, kind_json, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    item.id,
                    item.thread_id,
                    item.turn_id,
                    kind_json,
                    item.created_at,
                    item.updated_at,
                ],
            )?;
            tx.execute(
                "UPDATE threads SET updated_at = ?1 WHERE id = ?2",
                params![now, item.thread_id],
            )?;
        }
        DomainEvent::TimelineItemAppended {
            thread_id,
            item_id,
            delta,
        } => {
            let existing: Option<String> = tx
                .query_row(
                    "SELECT kind_json FROM timeline_items WHERE id = ?1",
                    params![item_id],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(kind_json) = existing {
                let mut kind: TimelineItemKind = serde_json::from_str(&kind_json)?;
                kind.append_delta(delta);
                let updated_json = serde_json::to_string(&kind)?;
                tx.execute(
                    "UPDATE timeline_items SET kind_json = ?1, updated_at = ?2 WHERE id = ?3",
                    params![updated_json, now, item_id],
                )?;
                tx.execute(
                    "UPDATE threads SET updated_at = ?1 WHERE id = ?2",
                    params![now, thread_id],
                )?;
            }
        }
        DomainEvent::ApprovalRequested { approval } => {
            let options_json = serde_json::to_string(&approval.options)?;
            tx.execute(
                "INSERT OR REPLACE INTO pending_approvals (
                    id, thread_id, turn_id, tool_name, description, args_json, options_json, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    approval.id,
                    approval.thread_id,
                    approval.turn_id,
                    approval.tool_name,
                    approval.description,
                    approval.args_json,
                    options_json,
                    approval.created_at,
                ],
            )?;
            tx.execute(
                "UPDATE threads SET status = 'waiting_approval', updated_at = ?1 WHERE id = ?2",
                params![now, approval.thread_id],
            )?;
        }
        DomainEvent::ApprovalResolved {
            approval_id,
            thread_id,
            ..
        } => {
            tx.execute(
                "DELETE FROM pending_approvals WHERE id = ?1",
                params![approval_id],
            )?;
            let remaining: i64 = tx.query_row(
                "SELECT COUNT(*) FROM pending_approvals WHERE thread_id = ?1",
                params![thread_id],
                |r| r.get(0),
            )?;
            if remaining == 0 {
                tx.execute(
                    "UPDATE threads SET status = 'running', updated_at = ?1 WHERE id = ?2 AND status = 'waiting_approval'",
                    params![now, thread_id],
                )?;
            }
        }
        DomainEvent::PromptQueued { item } => {
            let attachments_json = serde_json::to_string(&item.attachments)?;
            tx.execute(
                "INSERT OR REPLACE INTO prompt_queue (id, thread_id, prompt, attachments_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    item.id,
                    item.thread_id,
                    item.prompt,
                    attachments_json,
                    item.created_at
                ],
            )?;
        }
        DomainEvent::PromptDequeued { queue_id, .. } => {
            tx.execute("DELETE FROM prompt_queue WHERE id = ?1", params![queue_id])?;
        }
        DomainEvent::DiffUpdated { .. } => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hasilanx_protocol::{ApprovalDecision, ToolCallStatus};

    #[test]
    fn event_sourcing_projection_turns_queue_and_delta_sync() {
        let store = EventStore::open_in_memory().unwrap();
        let now = now_ms();

        let p = ProjectRecord {
            id: "proj-1".into(),
            name: "HasilanX".into(),
            root_path: "/tmp/hasilanx".into(),
            created_at: now,
        };
        let e1 = store
            .append_event(DomainEvent::ProjectCreated { project: p })
            .unwrap();
        assert_eq!(e1.seq, 1);

        let t = ThreadRecord {
            id: "thread-1".into(),
            project_id: "proj-1".into(),
            title: "First thread".into(),
            workspace_mode: WorkspaceMode::Local,
            worktree_path: None,
            branch_name: None,
            active_agent_id: "agy".into(),
            active_model: None,
            active_thinking: Some("high".into()),
            execution_mode: ExecutionMode::Ask,
            status: ThreadStatus::Idle,
            last_error: None,
            created_at: now,
            updated_at: now,
        };
        let e2 = store
            .append_event(DomainEvent::ThreadCreated { thread: t })
            .unwrap();
        assert_eq!(e2.seq, 2);

        store
            .append_event(DomainEvent::TurnStarted {
                thread_id: "thread-1".into(),
                turn_id: "turn-1".into(),
                turn_index: 0,
                agent_id: "agy".into(),
                user_prompt: "hello".into(),
                checkpoint_before_ref: Some("refs/hasilanx/checkpoints/thread-1/0_before".into()),
            })
            .unwrap();

        store
            .append_event(DomainEvent::PromptQueued {
                item: QueuedPrompt {
                    id: "q-1".into(),
                    thread_id: "thread-1".into(),
                    prompt: "follow up".into(),
                    attachments: vec![],
                    created_at: now,
                },
            })
            .unwrap();
        assert_eq!(
            store.peek_next_queued_prompt("thread-1").unwrap().unwrap().id,
            "q-1"
        );

        let msg_item = TimelineItem {
            id: "item-1".into(),
            thread_id: "thread-1".into(),
            turn_id: Some("turn-1".into()),
            kind: TimelineItemKind::AssistantMessage {
                text: "Hel".into(),
            },
            created_at: now,
            updated_at: now,
        };
        store
            .append_event(DomainEvent::TimelineItemUpserted { item: msg_item })
            .unwrap();
        store
            .append_event(DomainEvent::TimelineItemAppended {
                thread_id: "thread-1".into(),
                item_id: "item-1".into(),
                delta: "lo world!".into(),
            })
            .unwrap();

        let approval = PendingApproval {
            id: "appr-1".into(),
            thread_id: "thread-1".into(),
            turn_id: Some("turn-1".into()),
            tool_name: "bash".into(),
            description: "Run ls".into(),
            args_json: Some("{\"cmd\":\"ls\"}".into()),
            options: vec![ApprovalOption {
                id: "allow".into(),
                label: "Allow".into(),
                decision: ApprovalDecision::Accept,
            }],
            created_at: now,
        };
        store
            .append_event(DomainEvent::ApprovalRequested { approval })
            .unwrap();

        let snap = store
            .load_snapshot("0.1.0".into(), "test-host".into(), vec![])
            .unwrap();
        assert_eq!(snap.projects.len(), 1);
        assert_eq!(snap.threads.len(), 1);
        assert_eq!(snap.turns.len(), 1);
        assert_eq!(snap.prompt_queue.len(), 1);
        assert_eq!(snap.threads[0].status, ThreadStatus::WaitingApproval);
        assert_eq!(snap.pending_approvals.len(), 1);
        assert_eq!(
            snap.timeline_items[0].kind,
            TimelineItemKind::AssistantMessage {
                text: "Hello world!".into()
            }
        );

        store
            .append_event(DomainEvent::ApprovalResolved {
                approval_id: "appr-1".into(),
                thread_id: "thread-1".into(),
                decision: ApprovalDecision::Accept,
            })
            .unwrap();

        let (latest, delta) = store.events_since(e2.seq).unwrap();
        assert_eq!(latest, 8);
        assert_eq!(delta.len(), 6);

        let tool = TimelineItem {
            id: "tool-1".into(),
            thread_id: "thread-1".into(),
            turn_id: Some("turn-1".into()),
            kind: TimelineItemKind::ToolCall {
                call_id: "c1".into(),
                name: "read_file".into(),
                status: ToolCallStatus::Running,
                input: Some("README.md".into()),
                output: None,
            },
            created_at: now,
            updated_at: now,
        };
        store
            .append_event(DomainEvent::TimelineItemUpserted { item: tool })
            .unwrap();

        store
            .append_event(DomainEvent::TurnCompleted {
                thread_id: "thread-1".into(),
                turn_id: "turn-1".into(),
                status: TurnCompletionStatus::Completed,
                error: None,
                checkpoint_after_ref: Some("refs/hasilanx/checkpoints/thread-1/0_after".into()),
            })
            .unwrap();

        store
            .append_event(DomainEvent::PromptDequeued {
                thread_id: "thread-1".into(),
                queue_id: "q-1".into(),
            })
            .unwrap();

        assert!(store.peek_next_queued_prompt("thread-1").unwrap().is_none());
        let turn = store.get_turn("turn-1").unwrap().unwrap();
        assert_eq!(turn.status, TurnCompletionStatus::Completed);
        assert!(turn.checkpoint_after_ref.is_some());
        assert_eq!(store.next_turn_index("thread-1").unwrap(), 1);
    }
}
