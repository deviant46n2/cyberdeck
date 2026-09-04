//! Tauri command bridge for agent sessions.
//!
//! Every coding task launched through Cyberdeck is tracked as a session —
//! persisted in SQLite, streamable via events, and resumable across restarts.
//! This module adapts the deck-core store operations to Tauri command shapes.

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;
use tauri::Emitter;

use deck_core::store::{self, Session, SessionEvent, SessionStatus};

fn conn() -> Result<Connection> {
    let db = store::default_db_path();
    store::open(&db).map_err(anyhow::Error::from)
}

/// Serializable session view for the frontend.
#[derive(Serialize)]
pub struct SessionView {
    pub id: String,
    pub project_dir: String,
    pub agent: String,
    pub model: String,
    pub task: String,
    pub status: String,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub auto_mode: bool,
    pub ctx_size: u32,
    pub exit_code: Option<i32>,
    pub error_message: Option<String>,
    pub has_handoff: bool,
}

impl From<&Session> for SessionView {
    fn from(s: &Session) -> Self {
        Self {
            id: s.id.clone(),
            project_dir: s.project_dir.clone(),
            agent: s.agent.clone(),
            model: s.model.clone(),
            task: s.task.clone(),
            status: s.status.to_string(),
            created_at: s.created_at,
            started_at: s.started_at,
            completed_at: s.completed_at,
            auto_mode: s.auto_mode,
            ctx_size: s.ctx_size,
            exit_code: s.exit_code,
            error_message: s.error_message.clone(),
            has_handoff: s.handoff_json.is_some() || s.status == SessionStatus::Complete || s.status == SessionStatus::Stopped,
        }
    }
}

/// List sessions, most recent first.
pub fn list_sessions(status: Option<&str>, limit: usize) -> Result<Vec<SessionView>> {
    let c = conn()?;
    let sessions = store::list_sessions(&c, status, limit)?;
    Ok(sessions.iter().map(SessionView::from).collect())
}

/// Get a single session.
pub fn get_session(id: &str) -> Result<Option<SessionView>> {
    let c = conn()?;
    let s = store::get_session(&c, id)?;
    Ok(s.as_ref().map(SessionView::from))
}

/// Create a new session record. Returns the session id.
pub fn create_session(
    project_dir: &str,
    agent: &str,
    model: &str,
    task: &str,
    auto_mode: bool,
    ctx_size: u32,
) -> Result<String> {
    let id = format!("sess-{}", now());
    let session = Session {
        id: id.clone(),
        project_dir: project_dir.to_string(),
        agent: agent.to_string(),
        model: model.to_string(),
        task: task.to_string(),
        status: SessionStatus::Pending,
        created_at: now(),
        started_at: None,
        completed_at: None,
        auto_mode,
        ctx_size,
        exit_code: None,
        handoff_json: None,
        error_message: None,
    };
    let c = conn()?;
    store::insert_session(&c, &session)?;
    Ok(id)
}

/// Transition a session to running status.
pub fn mark_session_running(id: &str) -> Result<()> {
    let c = conn()?;
    store::update_session(&c, id, Some(SessionStatus::Running), Some(Some(now())), None, None, None, None)
}

/// Transition a session to complete status.
pub fn mark_session_complete(id: &str, exit_code: i32) -> Result<()> {
    let c = conn()?;
    store::update_session(
        &c,
        id,
        Some(SessionStatus::Complete),
        None,
        Some(Some(now())),
        Some(Some(exit_code)),
        None,
        None,
    )
}

/// Transition a session to stopped status.
pub fn mark_session_stopped(id: &str) -> Result<()> {
    let c = conn()?;
    store::update_session(&c, id, Some(SessionStatus::Stopped), None, Some(Some(now())), None, None, None)
}

/// Transition a session to error status.
pub fn mark_session_error(id: &str, error: &str) -> Result<()> {
    let c = conn()?;
    store::update_session(
        &c,
        id,
        Some(SessionStatus::Error),
        None,
        Some(Some(now())),
        None,
        None,
        Some(Some(error.to_string())),
    )
}

/// Append an output event to a session.
pub fn add_session_event(
    session_id: &str,
    kind: &str,
    stream: &str,
    text: &str,
) -> Result<()> {
    let c = conn()?;
    let event = SessionEvent {
        id: 0,
        session_id: session_id.to_string(),
        timestamp: now(),
        kind: kind.to_string(),
        stream: stream.to_string(),
        text: text.to_string(),
    };
    store::insert_session_event(&c, &event)
}

/// Get all events for a session (for log replay).
pub fn get_session_events(session_id: &str) -> Result<Vec<SessionEvent>> {
    let c = conn()?;
    store::get_session_events(&c, session_id)
}

/// Generate a structured handoff document for a session.
pub fn generate_handoff(session_id: &str) -> Result<String> {
    let c = conn()?;
    store::generate_handoff(&c, session_id)
}

/// Delete a session and its events.
pub fn delete_session(id: &str) -> Result<()> {
    let c = conn()?;
    store::delete_session(&c, id)
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Emit a session event to the frontend (combines DB write + Tauri event).
pub fn emit_session_event(
    app: &tauri::AppHandle,
    session_id: &str,
    kind: &str,
    stream: &str,
    text: &str,
) {
    // Persist to DB (best-effort — emit even if DB write fails).
    let _ = add_session_event(session_id, kind, stream, text);

    // Emit to frontend.
    let _ = app.emit(
        "session-event",
        SessionEventPayload {
            session_id: session_id.to_string(),
            kind: kind.to_string(),
            stream: stream.to_string(),
            text: text.to_string(),
        },
    );
}

#[derive(Clone, serde::Serialize)]
pub struct SessionEventPayload {
    pub session_id: String,
    pub kind: String,
    pub stream: String,
    pub text: String,
}

#[derive(Clone, serde::Serialize)]
pub struct SessionStatusPayload {
    pub session_id: String,
    pub status: String,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
}

/// Emit a session status change to the frontend (combines DB write + Tauri event).
pub fn emit_session_status(
    app: &tauri::AppHandle,
    session_id: &str,
    status: SessionStatus,
    exit_code: Option<i32>,
    error: Option<&str>,
) {
    // Update DB.
    match status {
        SessionStatus::Running => {
            let _ = mark_session_running(session_id);
        }
        SessionStatus::Complete => {
            let _ = mark_session_complete(session_id, exit_code.unwrap_or(-1));
        }
        SessionStatus::Stopped => {
            let _ = mark_session_stopped(session_id);
        }
        SessionStatus::Error => {
            let _ = mark_session_error(session_id, error.unwrap_or("unknown error"));
        }
        _ => {}
    }

    // Emit to frontend.
    let _ = app.emit(
        "session-status",
        SessionStatusPayload {
            session_id: session_id.to_string(),
            status: status.to_string(),
            exit_code,
            error: error.map(String::from),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_list_sessions() {
        let id = create_session("/tmp", "opencode", "qwen3.8-27b", "test task", true, 32768).unwrap();
        let sessions = list_sessions(None, 10).unwrap();
        assert!(!sessions.is_empty());
        let s = sessions.iter().find(|s| s.id == id).unwrap();
        assert_eq!(s.status, "pending");
        assert_eq!(s.model, "qwen3.8-27b");

        mark_session_running(&id).unwrap();
        let s = get_session(&id).unwrap().unwrap();
        assert_eq!(s.status, "running");
        assert!(s.started_at.is_some());

        mark_session_complete(&id, 0).unwrap();
        let s = get_session(&id).unwrap().unwrap();
        assert_eq!(s.status, "complete");
        assert!(s.completed_at.is_some());
    }

    #[test]
    fn handoff_roundtrip() {
        let id = create_session("/tmp", "opencode", "test-model", "do something", false, 16384).unwrap();
        mark_session_running(&id).unwrap();
        add_session_event(&id, "line", "stdout", "started working").unwrap();
        add_session_event(&id, "line", "stdout", "finished working").unwrap();
        mark_session_complete(&id, 0).unwrap();

        let h = generate_handoff(&id).unwrap();
        assert!(h.contains("do something"));
        assert!(h.contains("finished working"));
    }
}
