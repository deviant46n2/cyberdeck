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
    Ok(())
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

/// Substitute the configured per-engine executable path into a profile when
/// its resolved bin does not exist on disk. Leaves explicit, existing binaries
/// (and bare PATH-resolvable names like `ollama`) untouched. This is the one
/// piece of machine-specific config a profile should not carry.
pub fn resolve_engine_bin(
    conn: &Connection,
    mut p: crate::profile::Profile,
) -> Result<crate::profile::Profile> {
    if !bin_looks_resolvable(&p.bin)
        && let Some(b) = get_engine_bin(conn, p.engine.store_id())?
        && b != p.bin.to_string_lossy()
    {
        p.bin = std::path::PathBuf::from(b);
    }
    Ok(p)
}
