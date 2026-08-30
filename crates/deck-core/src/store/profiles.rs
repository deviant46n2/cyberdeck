use anyhow::Result;
use rusqlite::Connection;

// ---------------------------------------------------------------- profiles

pub fn ensure_profile_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS profiles (
            name TEXT PRIMARY KEY,
            engine TEXT NOT NULL,
            body TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )?;
    Ok(())
}

pub fn upsert_profile(conn: &Connection, profile: &crate::profile::Profile) -> Result<()> {
    let body = serde_json::to_string(profile)?;
    let engine = profile.engine.store_id();
    conn.execute(
        "INSERT INTO profiles (name, engine, body) VALUES (?1,?2,?3)
         ON CONFLICT(name) DO UPDATE SET engine=?2, body=?3",
        rusqlite::params![profile.name, engine, body],
    )?;
    Ok(())
}

pub fn list_profiles(conn: &Connection) -> Result<Vec<crate::profile::Profile>> {
    let mut stmt = conn.prepare("SELECT body FROM profiles ORDER BY name")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for body in rows.flatten() {
        if let Ok(p) = serde_json::from_str::<crate::profile::Profile>(&body) {
            out.push(p);
        }
    }
    Ok(out)
}

pub fn get_profile(conn: &Connection, name: &str) -> Result<Option<crate::profile::Profile>> {
    let mut stmt = conn.prepare("SELECT body FROM profiles WHERE name = ?1")?;
    let mut rows = stmt.query_map([name], |r| r.get::<_, String>(0))?;
    if let Some(body) = rows.next().transpose()? {
        return Ok(Some(serde_json::from_str::<crate::profile::Profile>(
            &body,
        )?));
    }
    Ok(None)
}

pub fn delete_profile(conn: &Connection, name: &str) -> Result<()> {
    conn.execute("DELETE FROM profiles WHERE name = ?1", [name])?;
    Ok(())
}

pub fn set_active(conn: &Connection, name: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('active_profile', ?1)
         ON CONFLICT(key) DO UPDATE SET value=?1",
        [name],
    )?;
    Ok(())
}

pub fn active_profile(conn: &Connection) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM meta WHERE key = 'active_profile'")?;
    let mut rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.next().transpose()?)
}
