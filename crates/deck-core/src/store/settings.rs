use anyhow::Result;
use rusqlite::Connection;

// ------------------------------------------------------------ settings + audit_log (O3)
pub fn ensure_settings_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value_json TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS audit_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ts INTEGER NOT NULL,
            actor TEXT NOT NULL,
            key TEXT NOT NULL,
            old_json TEXT,
            new_json TEXT,
            reason TEXT NOT NULL
        )",
    )?;
    Ok(())
}

pub fn settings_get(conn: &Connection, key: &str) -> Result<Option<String>> {
    ensure_settings_schema(conn)?;
    let mut stmt = conn.prepare("SELECT value_json FROM settings WHERE key=?1")?;
    let mut rows = stmt.query_map([key], |r| r.get::<_, String>(0))?;
    Ok(rows.next().transpose()?)
}

pub fn settings_set(conn: &Connection, key: &str, value_json: &str, actor: &str, reason: &str) -> Result<()> {
    ensure_settings_schema(conn)?;
    // validate JSON
    if serde_json::from_str::<serde_json::Value>(value_json).is_err() {
        anyhow::bail!("value must be valid JSON");
    }
    let old = settings_get(conn, key)?.unwrap_or_default();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    conn.execute("INSERT INTO settings(key, value_json, updated_at) VALUES (?1,?2,?3) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json, updated_at=excluded.updated_at", rusqlite::params![key, value_json, now])?;
    conn.execute("INSERT INTO audit_log(ts, actor, key, old_json, new_json, reason) VALUES (?1,?2,?3,?4,?5,?6)", rusqlite::params![now, actor, key, if old.is_empty() { None } else { Some(old) }, value_json, reason])?;
    Ok(())
}

pub fn settings_list(conn: &Connection) -> Result<Vec<(String, String, i64)>> {
    ensure_settings_schema(conn)?;
    let mut stmt = conn.prepare("SELECT key, value_json, updated_at FROM settings ORDER BY key")?;
    Ok(stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<Vec<_>, _>>()?)
}

pub fn audit_list(conn: &Connection, limit: usize) -> Result<Vec<(i64, i64, String, String, Option<String>, Option<String>, String)>> {
    ensure_settings_schema(conn)?;
    let mut stmt = conn.prepare("SELECT id, ts, actor, key, old_json, new_json, reason FROM audit_log ORDER BY id DESC LIMIT ?1")?;
    Ok(stmt.query_map([limit as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))?.collect::<Result<Vec<_>,_>>()?)
}

pub fn settings_undo(conn: &Connection, audit_id: i64) -> Result<()> {
    ensure_settings_schema(conn)?;
    let mut stmt = conn.prepare("SELECT key, old_json, new_json FROM audit_log WHERE id=?1")?;
    let (key, old, _new): (String, Option<String>, Option<String>) = stmt.query_row([audit_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    if let Some(v) = old {
        settings_set(conn, &key, &v, "undo", &format!("revert #{audit_id}"))?;
    } else {
        conn.execute("DELETE FROM settings WHERE key=?1", [&key])?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        conn.execute("INSERT INTO audit_log(ts, actor, key, old_json, new_json, reason) VALUES (?1,?2,?3,?4,?5,?6)", rusqlite::params![now, "undo", key, _new, Option::<String>::None, format!("undo #{audit_id}")])?;
    }
    Ok(())
}
