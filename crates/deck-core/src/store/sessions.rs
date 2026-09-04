//! Agent session persistence: the `sessions` and `session_events` tables
//! track every coding task launched through Cyberdeck (via `opencode run`).
//!
//! Sessions survive app restarts — reopening Cyberdeck shows what happened
//! and lets the user continue recoverable sessions. Events provide the raw
//! log for live streaming and post-hoc handoff generation.

use anyhow::{Context, Result};
use rusqlite::Connection;

/// Session status values — mirrors the frontend status display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Pending,
    Running,
    Complete,
    Stopped,
    Error,
    Disconnected,
}

impl std::fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Running => write!(f, "running"),
            Self::Complete => write!(f, "complete"),
            Self::Stopped => write!(f, "stopped"),
            Self::Error => write!(f, "error"),
            Self::Disconnected => write!(f, "disconnected"),
        }
    }
}

impl std::str::FromStr for SessionStatus {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "complete" => Ok(Self::Complete),
            "stopped" => Ok(Self::Stopped),
            "error" => Ok(Self::Error),
            "disconnected" => Ok(Self::Disconnected),
            _ => Err(anyhow::anyhow!("unknown session status: {s}")),
        }
    }
}

/// A persisted agent session row.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Session {
    pub id: String,
    pub project_dir: String,
    pub agent: String,
    pub model: String,
    pub task: String,
    pub status: SessionStatus,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub auto_mode: bool,
    pub ctx_size: u32,
    pub exit_code: Option<i32>,
    pub handoff_json: Option<String>,
    pub error_message: Option<String>,
}

/// A single event line within a session.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionEvent {
    pub id: i64,
    pub session_id: String,
    pub timestamp: i64,
    pub kind: String,
    pub stream: String,
    pub text: String,
}

pub fn ensure_sessions_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            project_dir TEXT NOT NULL DEFAULT '',
            agent TEXT NOT NULL DEFAULT '',
            model TEXT NOT NULL DEFAULT '',
            task TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL DEFAULT 'pending',
            created_at INTEGER NOT NULL,
            started_at INTEGER,
            completed_at INTEGER,
            auto_mode INTEGER NOT NULL DEFAULT 0,
            ctx_size INTEGER NOT NULL DEFAULT 32768,
            exit_code INTEGER,
            handoff_json TEXT,
            error_message TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status);
        CREATE INDEX IF NOT EXISTS idx_sessions_created ON sessions(created_at DESC);

        CREATE TABLE IF NOT EXISTS session_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            kind TEXT NOT NULL,
            stream TEXT NOT NULL DEFAULT 'stdout',
            text TEXT NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS idx_session_events_session ON session_events(session_id, id);",
    )?;
    Ok(())
}

/// Insert a new session (idempotent — replaces on conflict).
pub fn insert_session(conn: &Connection, s: &Session) -> Result<()> {
    ensure_sessions_schema(conn)?;
    conn.execute(
        "INSERT OR REPLACE INTO sessions
            (id, project_dir, agent, model, task, status, created_at,
             started_at, completed_at, auto_mode, ctx_size, exit_code,
             handoff_json, error_message)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
        rusqlite::params![
            s.id,
            s.project_dir,
            s.agent,
            s.model,
            s.task,
            s.status.to_string(),
            s.created_at,
            s.started_at,
            s.completed_at,
            s.auto_mode as i32,
            s.ctx_size as i64,
            s.exit_code,
            s.handoff_json,
            s.error_message,
        ],
    )?;
    Ok(())
}

/// Update only the mutable fields of a session (status, timestamps, exit code,
/// handoff, error). Does NOT touch immutable fields (id, project, model, task).
///
/// Uses parameterized queries — no string interpolation for user-supplied values.
pub fn update_session(
    conn: &Connection,
    id: &str,
    status: Option<SessionStatus>,
    started_at: Option<Option<i64>>,
    completed_at: Option<Option<i64>>,
    exit_code: Option<Option<i32>>,
    handoff: Option<Option<String>>,
    error: Option<Option<String>>,
) -> Result<()> {
    ensure_sessions_schema(conn)?;
    let mut sets = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut param_idx = 2; // ?1 is reserved for the WHERE id = ?1

    if let Some(s) = status {
        sets.push(format!("status = ?{param_idx}"));
        params.push(Box::new(s.to_string()));
        param_idx += 1;
    }
    if let Some(v) = started_at {
        match v {
            Some(ts) => {
                sets.push(format!("started_at = ?{param_idx}"));
                params.push(Box::new(ts));
                param_idx += 1;
            }
            None => sets.push("started_at = NULL".into()),
        }
    }
    if let Some(v) = completed_at {
        match v {
            Some(ts) => {
                sets.push(format!("completed_at = ?{param_idx}"));
                params.push(Box::new(ts));
                param_idx += 1;
            }
            None => sets.push("completed_at = NULL".into()),
        }
    }
    if let Some(v) = exit_code {
        match v {
            Some(c) => {
                sets.push(format!("exit_code = ?{param_idx}"));
                params.push(Box::new(c));
                param_idx += 1;
            }
            None => sets.push("exit_code = NULL".into()),
        }
    }
    if let Some(v) = handoff {
        match v {
            Some(h) => {
                sets.push(format!("handoff_json = ?{param_idx}"));
                params.push(Box::new(h));
                param_idx += 1;
            }
            None => sets.push("handoff_json = NULL".into()),
        }
    }
    if let Some(v) = error {
        match v {
            Some(e) => {
                sets.push(format!("error_message = ?{param_idx}"));
                params.push(Box::new(e));
            }
            None => sets.push("error_message = NULL".into()),
        }
    }
    if sets.is_empty() {
        return Ok(());
    }
    let sql = format!("UPDATE sessions SET {} WHERE id = ?1", sets.join(", "));
    // Build the final params: id first (?1), then the dynamic params.
    let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(id.to_string())];
    all_params.extend(params);
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = all_params.iter().map(|p| p.as_ref()).collect();
    conn.execute(&sql, param_refs.as_slice())?;
    Ok(())
}

/// Append an event to a session's log.
pub fn insert_session_event(conn: &Connection, e: &SessionEvent) -> Result<()> {
    ensure_sessions_schema(conn)?;
    conn.execute(
        "INSERT INTO session_events (session_id, timestamp, kind, stream, text)
         VALUES (?1,?2,?3,?4,?5)",
        rusqlite::params![e.session_id, e.timestamp, e.kind, e.stream, e.text],
    )?;
    Ok(())
}

/// Get a single session by id.
pub fn get_session(conn: &Connection, id: &str) -> Result<Option<Session>> {
    ensure_sessions_schema(conn)?;
    let mut stmt = conn.prepare(
        "SELECT id, project_dir, agent, model, task, status, created_at,
                started_at, completed_at, auto_mode, ctx_size, exit_code,
                handoff_json, error_message
         FROM sessions WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map([id], |r| session_from_row(r))?;
    rows.next().transpose().context("query session by id")
}

/// List sessions, most recent first. Optional status filter.
pub fn list_sessions(conn: &Connection, status_filter: Option<&str>, limit: usize) -> Result<Vec<Session>> {
    ensure_sessions_schema(conn)?;
    let mut stmt = if let Some(_s) = status_filter {
        conn.prepare(
            "SELECT id, project_dir, agent, model, task, status, created_at,
                    started_at, completed_at, auto_mode, ctx_size, exit_code,
                    handoff_json, error_message
             FROM sessions WHERE status = ?1 ORDER BY created_at DESC LIMIT ?2",
        )?
    } else {
        conn.prepare(
            "SELECT id, project_dir, agent, model, task, status, created_at,
                    started_at, completed_at, auto_mode, ctx_size, exit_code,
                    handoff_json, error_message
             FROM sessions ORDER BY created_at DESC LIMIT ?1",
        )?
    };
    let mut out = Vec::new();
    if let Some(s) = status_filter {
        let rows = stmt.query_map(rusqlite::params![s, limit as i64], |r| session_from_row(r))?;
        for row in rows {
            out.push(row?);
        }
    } else {
        let rows = stmt.query_map([limit as i64], |r| session_from_row(r))?;
        for row in rows {
            out.push(row?);
        }
    }
    Ok(out)
}

fn session_from_row(r: &rusqlite::Row) -> rusqlite::Result<Session> {
    Ok(Session {
        id: r.get(0)?,
        project_dir: r.get(1)?,
        agent: r.get(2)?,
        model: r.get(3)?,
        task: r.get(4)?,
        status: r.get::<_, String>(5)?.parse().unwrap_or(SessionStatus::Error),
        created_at: r.get(6)?,
        started_at: r.get(7)?,
        completed_at: r.get(8)?,
        auto_mode: r.get::<_, i32>(9)? != 0,
        ctx_size: r.get::<_, i64>(10)? as u32,
        exit_code: r.get(11)?,
        handoff_json: r.get(12)?,
        error_message: r.get(13)?,
    })
}

/// Get all events for a session, in order.
pub fn get_session_events(conn: &Connection, session_id: &str) -> Result<Vec<SessionEvent>> {
    ensure_sessions_schema(conn)?;
    let mut stmt = conn.prepare(
        "SELECT id, session_id, timestamp, kind, stream, text
         FROM session_events WHERE session_id = ?1 ORDER BY id ASC",
    )?;
    let rows = stmt.query_map([session_id], |r| {
        Ok(SessionEvent {
            id: r.get(0)?,
            session_id: r.get(1)?,
            timestamp: r.get(2)?,
            kind: r.get(3)?,
            stream: r.get(4)?,
            text: r.get(5)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Generate a structured handoff document from session data.
pub fn generate_handoff(conn: &Connection, session_id: &str) -> Result<String> {
    let s = get_session(conn, session_id)?
        .ok_or_else(|| anyhow::anyhow!("session '{session_id}' not found"))?;
    let events = get_session_events(conn, session_id)?;

    let mut handoff = String::new();

    // Objective
    handoff.push_str("## Objective\n\n");
    if s.task.is_empty() {
        handoff.push_str("_No task specified._\n\n");
    } else {
        handoff.push_str(&format!("{}\n\n", s.task));
    }

    // Configuration
    handoff.push_str("## Configuration\n\n");
    handoff.push_str(&format!("- **Project:** `{}`\n", s.project_dir));
    if !s.agent.is_empty() {
        handoff.push_str(&format!("- **Agent:** {}\n", s.agent));
    }
    if !s.model.is_empty() {
        handoff.push_str(&format!("- **Model:** `{}`\n", s.model));
    }
    handoff.push_str(&format!("- **Auto mode:** {}\n", s.auto_mode));
    handoff.push_str(&format!("- **Context size:** {}\n", s.ctx_size));
    handoff.push('\n');

    // Status
    handoff.push_str("## Status\n\n");
    let status_label = match s.status {
        SessionStatus::Complete => "COMPLETE",
        SessionStatus::Stopped => "STOPPED (by user)",
        SessionStatus::Error => "ERROR",
        SessionStatus::Running => "INTERRUPTED (was running at disconnect)",
        SessionStatus::Pending => "PENDING (never started)",
        SessionStatus::Disconnected => "DISCONNECTED",
    };
    handoff.push_str(&format!("**{status_label}**\n\n"));
    if let Some(code) = s.exit_code {
        handoff.push_str(&format!("- Exit code: {code}\n"));
    }
    if let Some(ref err) = s.error_message {
        handoff.push_str(&format!("- Error: {err}\n"));
    }

    // Timing
    handoff.push_str("\n## Timing\n\n");
    if let Some(ts) = s.started_at {
        handoff.push_str(&format!("- Started: {}\n", format_timestamp(ts)));
    }
    if let Some(ts) = s.completed_at {
        handoff.push_str(&format!("- Completed: {}\n", format_timestamp(ts)));
    }
    if let (Some(start), Some(end)) = (s.started_at, s.completed_at) {
        let dur = end - start;
        handoff.push_str(&format!("- Duration: {dur}s\n"));
    }
    handoff.push('\n');

    // Work performed — extract from stdout events
    let stdout_lines: Vec<&str> = events
        .iter()
        .filter(|e| e.stream == "stdout" && e.kind == "line")
        .map(|e| e.text.as_str())
        .collect();

    if !stdout_lines.is_empty() {
        handoff.push_str("## Work Performed\n\n");
        handoff.push_str("```\n");
        for line in &stdout_lines {
            handoff.push_str(line);
            handoff.push('\n');
        }
        handoff.push_str("```\n\n");
    }

    // Concerns
    if s.status == SessionStatus::Error || s.status == SessionStatus::Stopped {
        handoff.push_str("## Concerns\n\n");
        if s.status == SessionStatus::Error {
            handoff.push_str("- Session ended with an error and may need retry.\n");
        }
        if s.status == SessionStatus::Stopped {
            handoff.push_str("- Session was stopped by the user — work may be incomplete.\n");
        }
        handoff.push('\n');
    }

    // Recommended next action
    handoff.push_str("## Recommended Next Action\n\n");
    match s.status {
        SessionStatus::Complete => {
            handoff.push_str("Task completed. Review the output above for accuracy.\n");
        }
        SessionStatus::Stopped => {
            handoff.push_str("Session was interrupted. Consider re-running with the same prompt to continue.\n");
        }
        SessionStatus::Error => {
            handoff.push_str("Session failed. Check the error output and retry with adjusted parameters.\n");
        }
        SessionStatus::Running | SessionStatus::Disconnected => {
            handoff.push_str("Session may still be running or crashed. Check process status before re-running.\n");
        }
        _ => {
            handoff.push_str("Review session state and decide next step.\n");
        }
    }
    handoff.push('\n');

    // Session metadata for resumption
    handoff.push_str("---\n\n");
    handoff.push_str("<!-- cyberdeck-session-meta\n");
    handoff.push_str(&format!("session_id: {}\n", s.id));
    handoff.push_str(&format!("project: {}\n", s.project_dir));
    handoff.push_str(&format!("model: {}\n", s.model));
    handoff.push_str(&format!("status: {}\n", s.status));
    handoff.push_str("-->\n");

    Ok(handoff)
}

/// Delete a session and all its events.
pub fn delete_session(conn: &Connection, id: &str) -> Result<()> {
    ensure_sessions_schema(conn)?;
    conn.execute("DELETE FROM session_events WHERE session_id = ?1", [id])?;
    conn.execute("DELETE FROM sessions WHERE id = ?1", [id])?;
    Ok(())
}

fn format_timestamp(ts: i64) -> String {
    use std::time::{Duration, UNIX_EPOCH};
    let dt = UNIX_EPOCH + Duration::from_secs(ts as u64);
    let secs = dt
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Simple YYYY-MM-DD HH:MM:SS formatting without pulling in chrono
    let days = secs / 86400;
    let time = secs % 86400;
    let h = time / 3600;
    let m = (time % 3600) / 60;
    let s = time % 60;
    // Days since 1970-01-01 to a rough date
    let mut y = 1970u64;
    let mut remaining = days;
    loop {
        let days_in_year = if is_leap(y) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        y += 1;
    }
    let mut mth = 1u64;
    let month_days = [31, if is_leap(y) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for (i, &d) in month_days.iter().enumerate() {
        if remaining < d {
            mth = (i + 1) as u64;
            break;
        }
        remaining -= d;
    }
    format!("{y:04}-{mth:02}-{:02} {h:02}:{m:02}:{s:02}", remaining + 1)
}

fn is_leap(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        ensure_sessions_schema(&conn).unwrap();
        conn
    }

    fn sample_session(id: &str) -> Session {
        Session {
            id: id.into(),
            project_dir: "/home/deviant/Projects/cyberdeck".into(),
            agent: "opencode".into(),
            model: "qwen3.8-27b".into(),
            task: "Fix the bug in main.rs".into(),
            status: SessionStatus::Pending,
            created_at: 1_700_000_000,
            started_at: None,
            completed_at: None,
            auto_mode: true,
            ctx_size: 32768,
            exit_code: None,
            handoff_json: None,
            error_message: None,
        }
    }

    #[test]
    fn insert_and_get_session() {
        let conn = test_conn();
        let s = sample_session("sess-1");
        insert_session(&conn, &s).unwrap();
        let got = get_session(&conn, "sess-1").unwrap().unwrap();
        assert_eq!(got.id, "sess-1");
        assert_eq!(got.model, "qwen3.8-27b");
        assert_eq!(got.status, SessionStatus::Pending);
    }

    #[test]
    fn update_session_status() {
        let conn = test_conn();
        let s = sample_session("sess-2");
        insert_session(&conn, &s).unwrap();
        update_session(&conn, "sess-2", Some(SessionStatus::Running), Some(Some(1_700_000_005)), None, None, None, None).unwrap();
        let got = get_session(&conn, "sess-2").unwrap().unwrap();
        assert_eq!(got.status, SessionStatus::Running);
        assert_eq!(got.started_at, Some(1_700_000_005));
    }

    #[test]
    fn insert_and_get_events() {
        let conn = test_conn();
        let s = sample_session("sess-3");
        insert_session(&conn, &s).unwrap();
        insert_session_event(&conn, &SessionEvent {
            id: 0, session_id: "sess-3".into(), timestamp: 1_700_000_010,
            kind: "line".into(), stream: "stdout".into(), text: "hello world".into(),
        }).unwrap();
        insert_session_event(&conn, &SessionEvent {
            id: 0, session_id: "sess-3".into(), timestamp: 1_700_000_011,
            kind: "line".into(), stream: "stderr".into(), text: "warning".into(),
        }).unwrap();
        let events = get_session_events(&conn, "sess-3").unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].text, "hello world");
        assert_eq!(events[1].stream, "stderr");
    }

    #[test]
    fn list_sessions_ordered() {
        let conn = test_conn();
        insert_session(&conn, &Session { created_at: 100, ..sample_session("a") }).unwrap();
        insert_session(&conn, &Session { created_at: 200, ..sample_session("b") }).unwrap();
        insert_session(&conn, &Session { created_at: 300, ..sample_session("c") }).unwrap();
        let all = list_sessions(&conn, None, 100).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].id, "c"); // most recent first
        assert_eq!(all[2].id, "a");

        let running = list_sessions(&conn, Some("running"), 100).unwrap();
        assert!(running.is_empty());

        let mut s2 = sample_session("b");
        s2.status = SessionStatus::Running;
        update_session(&conn, "b", Some(SessionStatus::Running), None, None, None, None, None).unwrap();
        let running = list_sessions(&conn, Some("running"), 100).unwrap();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].id, "b");
    }

    #[test]
    fn delete_session_cascades() {
        let conn = test_conn();
        insert_session(&conn, &sample_session("del-1")).unwrap();
        insert_session_event(&conn, &SessionEvent {
            id: 0, session_id: "del-1".into(), timestamp: 1,
            kind: "line".into(), stream: "stdout".into(), text: "x".into(),
        }).unwrap();
        delete_session(&conn, "del-1").unwrap();
        assert!(get_session(&conn, "del-1").unwrap().is_none());
        assert!(get_session_events(&conn, "del-1").unwrap().is_empty());
    }

    #[test]
    fn handoff_generation() {
        let conn = test_conn();
        let mut s = sample_session("ho-1");
        s.status = SessionStatus::Complete;
        s.started_at = Some(1_700_000_010);
        s.completed_at = Some(1_700_000_060);
        s.exit_code = Some(0);
        insert_session(&conn, &s).unwrap();
        insert_session_event(&conn, &SessionEvent {
            id: 0, session_id: "ho-1".into(), timestamp: 1_700_000_020,
            kind: "line".into(), stream: "stdout".into(), text: "Fixed the bug".into(),
        }).unwrap();
        let h = generate_handoff(&conn, "ho-1").unwrap();
        assert!(h.contains("## Objective"));
        assert!(h.contains("Fix the bug in main.rs"));
        assert!(h.contains("## Configuration"));
        assert!(h.contains("qwen3.8-27b"));
        assert!(h.contains("## Work Performed"));
        assert!(h.contains("Fixed the bug"));
        assert!(h.contains("## Status"));
        assert!(h.contains("COMPLETE"));
        assert!(h.contains("50s")); // duration
    }
}
