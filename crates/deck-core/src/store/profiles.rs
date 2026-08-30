use anyhow::Result;
use rusqlite::Connection;

// ---------------------------------------------------------------- profiles

/// A flavor = a named loadout bound to a vault model. `model_id` is the hinge:
/// one `models` row hosts N flavors (different ctx/engine variants of the same
/// file), and switching between them is `deck use <name>`. It is backfilled by
/// matching `body.model` against `models.path` on schema touch, so existing
/// databases link up without a destructive migration.
pub fn ensure_profile_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS profiles (
            name TEXT PRIMARY KEY,
            engine TEXT NOT NULL,
            body TEXT NOT NULL,
            model_id INTEGER REFERENCES models(id)
        );
        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )?;
    // Pre-upgrade DBs predate the column → add it, then link existing rows.
    let has_column = conn
        .prepare("PRAGMA table_info(profiles)")
        .and_then(|mut stmt| {
            Ok::<_, rusqlite::Error>(
                stmt.query_map([], |r| r.get::<_, String>(1))?
                    .flatten()
                    .any(|c| c == "model_id"),
            )
        })?;
    if !has_column {
        conn.execute(
            "ALTER TABLE profiles ADD COLUMN model_id INTEGER REFERENCES models(id)",
            [],
        )?;
    }
    backfill_model_ids(conn)?;
    Ok(())
}

/// Resolve the vault row for a profile's model string (real local file/dir
/// paths only — remote HF ids and empty drafts stay unlinked). Returns None
/// when there is nothing to link.
pub fn resolve_model_id(conn: &Connection, model: &str) -> Result<Option<i64>> {
    let p = std::path::Path::new(model);
    if !p.is_absolute() || !p.exists() {
        return Ok(None);
    }
    Ok(conn
        .prepare("SELECT id FROM models WHERE path = ?1")?
        .query_row([model], |r| r.get::<_, i64>(0))
        .ok())
}

/// Convergence rule: materialize a minimal vault row for a local model file so
/// an applied loadout is guaranteed to have a vault entry (the Ollama-blob case
/// stops living off-book). Returns the model_id, or None for non-local paths.
pub fn ensure_model_indexed(conn: &Connection, model: &str) -> Result<Option<i64>> {
    if let Some(id) = resolve_model_id(conn, model)? {
        return Ok(Some(id));
    }
    let p = std::path::Path::new(model);
    if !p.is_absolute() || !p.exists() {
        return Ok(None);
    }
    let format = if p.is_dir() {
        "safetensors-dir"
    } else {
        "gguf"
    };
    let name = p
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    conn.execute(
        "INSERT INTO models (path, format, name, scanned_at)
         VALUES (?1, ?2, ?3, ?4) ON CONFLICT(path) DO NOTHING",
        rusqlite::params![model, format, name, stamp],
    )?;
    resolve_model_id(conn, model)
}

/// Re-link a profile to its vault row after saving/applying (cheap idempotent —
/// a profile saved while the file was remote picks up the row once it exists).
pub fn ensure_profile_model(conn: &Connection, profile: &crate::profile::Profile) -> Result<()> {
    if profile.model.trim().is_empty() {
        return Ok(());
    }
    if let Some(id) = ensure_model_indexed(conn, &profile.model)? {
        conn.execute(
            "UPDATE profiles SET model_id = ?2 WHERE name = ?1",
            rusqlite::params![profile.name, id],
        )?;
    }
    Ok(())
}

/// One-time + idempotent link of existing profiles to their vault rows.
fn backfill_model_ids(conn: &Connection) -> Result<()> {
    let profiles = list_profiles(conn)?;
    for p in profiles {
        let _ = ensure_profile_model(conn, &p);
    }
    Ok(())
}

pub fn upsert_profile(conn: &Connection, profile: &crate::profile::Profile) -> Result<()> {
    let body = serde_json::to_string(profile)?;
    let engine = profile.engine.store_id();
    let model_id = ensure_model_indexed(conn, &profile.model)?;
    conn.execute(
        "INSERT INTO profiles (name, engine, body, model_id) VALUES (?1,?2,?3,?4)
         ON CONFLICT(name) DO UPDATE SET engine=?2, body=?3, model_id=?4",
        rusqlite::params![profile.name, engine, body, model_id],
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_gguf(tag: &str) -> (std::path::PathBuf, String) {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("cyberdeck-flavor-{tag}-{}.gguf", std::process::id()));
        std::fs::write(&path, b"dummy gguf").expect("temp file");
        let model = path.display().to_string();
        (path, model)
    }

    #[test]
    fn flavor_linkage_and_convergence() {
        let conn = Connection::open_in_memory().expect("mem");
        crate::store::ensure_models_table(&conn).unwrap();
        ensure_profile_schema(&conn).unwrap();
        let (path, model) = temp_gguf("a");

        let a = crate::profile::Profile {
            name: "qwen-14k".into(),
            model: model.clone(),
            ..Default::default()
        };
        upsert_profile(&conn, &a).unwrap();

        // saving a local model materializes its vault row and links the flavor
        let mid: Option<i64> = conn
            .query_row(
                "SELECT model_id FROM profiles WHERE name = 'qwen-14k'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mid = mid.expect("linked");
        let vault_rows: i64 = conn
            .query_row("SELECT count(*) FROM models WHERE path = ?1", [&model], |r| r.get(0))
            .unwrap();
        assert_eq!(vault_rows, 1);

        // second flavor of the SAME model shares the vault row
        let b = crate::profile::Profile {
            name: "qwen-32k".into(),
            ..a.clone()
        };
        upsert_profile(&conn, &b).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM profiles WHERE model_id = ?1",
                [mid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2);

        // remote HF ids and empty drafts stay unlinked
        let remote = crate::profile::Profile {
            name: "remote-1".into(),
            model: "Qwen/Qwen3.8-27B-Instruct".into(),
            ..a.clone()
        };
        upsert_profile(&conn, &remote).unwrap();
        let rmid: Option<i64> = conn
            .query_row("SELECT model_id FROM profiles WHERE name = 'remote-1'", [], |r| r.get(0))
            .unwrap();
        assert!(rmid.is_none());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn ensure_schema_migrates_and_backfills_model_ids() {
        let conn = Connection::open_in_memory().expect("mem");
        crate::store::ensure_models_table(&conn).unwrap();
        // simulate a pre-model_id DB (profiles without the FK column)
        conn.execute_batch(
            "CREATE TABLE profiles (name TEXT PRIMARY KEY, engine TEXT NOT NULL, body TEXT NOT NULL);
             CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .unwrap();
        let (path, model) = temp_gguf("b");
        conn.execute(
            "INSERT INTO models (path, format) VALUES (?1, 'gguf')",
            [&model],
        )
        .unwrap();
        let p = crate::profile::Profile {
            name: "old-flavor".into(),
            model: model.clone(),
            ..Default::default()
        };
        let body = serde_json::to_string(&p).unwrap();
        conn.execute(
            "INSERT INTO profiles (name, engine, body) VALUES ('old-flavor', 'llamacpp', ?1)",
            [&body],
        )
        .unwrap();

        ensure_profile_schema(&conn).unwrap();
        let mid: Option<i64> = conn
            .query_row("SELECT model_id FROM profiles WHERE name = 'old-flavor'", [], |r| r.get(0))
            .unwrap();
        assert!(mid.is_some(), "backfill should link the pre-existing profile");

        std::fs::remove_file(&path).ok();
    }
}
