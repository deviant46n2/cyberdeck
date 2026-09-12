use anyhow::Result;
use rusqlite::Connection;

// ------------------------------------------------------------ engine bins
//
// Optional per-engine executable paths, keyed by `Engine::store_id` (e.g.
// "llamacpp"). A configured bin is used by bringup/test/matrix when a profile's
// resolved bin does not exist on disk — the one machine-specific fact a profile
// (or the CLI) should not be forced to carry. Unset means "use the engine's
// default resolution". Schema is created on first use so older databases pick it
// up without migration.

pub fn ensure_engine_bin_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS engine_bin (
            engine_id TEXT PRIMARY KEY,
            bin TEXT NOT NULL
        )",
    )?;
    // Installed release tag, so update checks know what is running without
    // re-probing the binary. NULL for hand-registered bins and old rows.
    crate::store::ensure_column(conn, "engine_bin", "version", "TEXT")?;
    Ok(())
}

/// Record a completed recipe install: binary path plus the upstream version it
/// came from. Plain `set_engine_bin` (hand-registered paths) leaves version
/// NULL — unknown, not stale.
pub fn record_runtime_install(
    conn: &Connection,
    engine_id: &str,
    bin: &str,
    version: &str,
) -> Result<()> {
    ensure_engine_bin_schema(conn)?;
    conn.execute(
        "INSERT INTO engine_bin (engine_id, bin, version) VALUES (?1,?2,?3)
         ON CONFLICT(engine_id) DO UPDATE SET bin = excluded.bin, version = excluded.version",
        rusqlite::params![engine_id, bin, version],
    )?;
    Ok(())
}

/// Installed version tag for a runtime, if a recipe install recorded one.
pub fn installed_version(conn: &Connection, engine_id: &str) -> Result<Option<String>> {
    ensure_engine_bin_schema(conn)?;
    let mut stmt = conn.prepare("SELECT version FROM engine_bin WHERE engine_id = ?1")?;
    let mut rows = stmt.query_map([engine_id], |r| r.get::<_, Option<String>>(0))?;
    Ok(rows.next().transpose()?.flatten())
}

pub fn get_engine_bin(conn: &Connection, store_id: &str) -> Result<Option<String>> {
    ensure_engine_bin_schema(conn)?;
    let mut stmt = conn.prepare("SELECT bin FROM engine_bin WHERE engine_id = ?1")?;
    let mut rows = stmt.query_map([store_id], |r| r.get::<_, String>(0))?;
    match rows.next() {
        Some(Ok(b)) => Ok(Some(b)),
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}

pub fn set_engine_bin(conn: &Connection, store_id: &str, bin: &str) -> Result<()> {
    ensure_engine_bin_schema(conn)?;
    conn.execute(
        "INSERT INTO engine_bin (engine_id, bin) VALUES (?1,?2)
         ON CONFLICT(engine_id) DO UPDATE SET bin = excluded.bin",
        rusqlite::params![store_id, bin],
    )?;
    Ok(())
}

pub fn clear_engine_bin(conn: &Connection, store_id: &str) -> Result<()> {
    ensure_engine_bin_schema(conn)?;
    conn.execute("DELETE FROM engine_bin WHERE engine_id = ?1", [store_id])?;
    Ok(())
}

/// Confirm a bin value will actually run: bare command names resolve through
/// PATH, so only an explicit path that is missing on disk needs substituting.
fn bin_looks_resolvable(bin: &std::path::Path) -> bool {
    let s = bin.to_string_lossy();
    !s.contains(['/', '\\']) || bin.is_file()
}

/// Substitute the configured per-engine executable path into a profile.
/// When an engine_bin is configured, it is always used — the profile's default
/// bin is just a placeholder. This is the one machine-specific fact a profile
/// (or the CLI) should not be forced to carry.
pub fn resolve_engine_bin(
    conn: &Connection,
    mut p: crate::profile::Profile,
) -> Result<crate::profile::Profile> {
    // Keyed by runtime id so a custom manifest ("beellama") can have its own
    // configured executable, exactly like a builtin engine.
    if let Some(b) = get_engine_bin(conn, &p.runtime_key())?
        && b != p.bin.to_string_lossy()
    {
        p.bin = std::path::PathBuf::from(b);
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_record_roundtrips_version_and_hand_bins_stay_unknown() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(installed_version(&conn, "llamacpp").unwrap().is_none());
        record_runtime_install(&conn, "llamacpp", "/x/llama-server", "b10930").unwrap();
        assert_eq!(
            installed_version(&conn, "llamacpp").unwrap().as_deref(),
            Some("b10930")
        );
        // Re-install moves the version forward.
        record_runtime_install(&conn, "llamacpp", "/x/llama-server", "b10931").unwrap();
        assert_eq!(
            installed_version(&conn, "llamacpp").unwrap().as_deref(),
            Some("b10931")
        );
    }
}
