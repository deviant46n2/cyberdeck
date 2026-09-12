//! Run history: live execution instances of configurations.
//!
//! A run binds a named profile to a runtime + endpoint with start/stop
//! timestamps, so the UI can answer "what is running, since when, as what".

use anyhow::Result;
use rusqlite::Connection;

/// Additive table; old DBs start with no run history.
pub fn ensure_runs_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS runs (
            id INTEGER PRIMARY KEY,
            config_name TEXT NOT NULL,
            runtime_id TEXT NOT NULL,
            endpoint TEXT NOT NULL,
            started_at INTEGER NOT NULL,
            stopped_at INTEGER,
            verdict TEXT
        );",
    )?;
    Ok(())
}

/// Record a run start; returns the run id. Stopping fills `stopped_at`.
pub fn record_run_start(conn: &Connection, config: &str, runtime: &str, endpoint: &str, at: i64) -> Result<i64> {
    conn.execute(
        "INSERT INTO runs (config_name, runtime_id, endpoint, started_at) VALUES (?1,?2,?3,?4)",
        rusqlite::params![config, runtime, endpoint, at],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn record_run_stop(conn: &Connection, id: i64, at: i64, verdict: &str) -> Result<()> {
    conn.execute(
        "UPDATE runs SET stopped_at = ?2, verdict = ?3 WHERE id = ?1",
        rusqlite::params![id, at, verdict],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_stop_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        ensure_runs_schema(&conn).unwrap();
        let id = record_run_start(&conn, "qwen-49k", "llamacpp", "127.0.0.1:18000", 100).unwrap();
        record_run_stop(&conn, id, 200, "ok").unwrap();
        let v: String = conn
            .prepare("SELECT verdict FROM runs WHERE id = ?1")
            .unwrap()
            .query_row([id], |r| r.get(0))
            .unwrap();
        assert_eq!(v, "ok");
    }
}
