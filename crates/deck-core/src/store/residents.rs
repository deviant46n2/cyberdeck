use anyhow::Result;
use rusqlite::Connection;

// ------------------------------------------------------------ residents (PORT MAP)

/// What is bound to a single engine port slot. `resident = true` means the
/// profile is meant to run *alongside* other engine slots (multi-model
/// residency); `false` is a plain single swap that still records reality so
/// `deck engines status` can show what's currently bound to each slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resident {
    pub engine_id: String,
    pub profile: String,
    pub resident: bool,
}

/// Keyed by engine id (`llamacpp` / `freetoken` / `ollama`) because each
/// engine owns exactly one fixed PORT MAP slot with one unit. Presence of a row
/// = a profile is bound to that slot; the `resident` flag marks it as a
/// coexisting resident rather than a plain swap.
pub fn ensure_resident_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS residents (
            engine_id TEXT PRIMARY KEY,
            profile_name TEXT NOT NULL,
            resident INTEGER NOT NULL DEFAULT 0
        )",
    )?;
    Ok(())
}

/// Bind `profile` to `engine_id`'s slot. Keeps the existing `resident` flag if
/// unspecified so re-applying a resident profile doesn't silently demote it;
/// pass `resident` to force.
pub fn set_resident(
    conn: &Connection,
    engine_id: &str,
    profile: &str,
    resident: Option<bool>,
) -> Result<()> {
    ensure_resident_schema(conn)?;
    if let Some(r) = resident {
        conn.execute(
            "INSERT INTO residents (engine_id, profile_name, resident) VALUES (?1,?2,?3)
             ON CONFLICT(engine_id) DO UPDATE SET profile_name = excluded.profile_name,
                                                  resident = excluded.resident",
            rusqlite::params![engine_id, profile, r as i64],
        )?;
    } else {
        conn.execute(
            "INSERT INTO residents (engine_id, profile_name, resident)
             VALUES (?1,?2, COALESCE((SELECT resident FROM residents WHERE engine_id = ?1), 0))
             ON CONFLICT(engine_id) DO UPDATE SET profile_name = excluded.profile_name",
            rusqlite::params![engine_id, profile],
        )?;
    }
    Ok(())
}

pub fn get_resident(conn: &Connection, engine_id: &str) -> Result<Option<Resident>> {
    ensure_resident_schema(conn)?;
    let mut stmt =
        conn.prepare("SELECT engine_id, profile_name, resident FROM residents WHERE engine_id = ?1")?;
    let mut rows = stmt.query_map([engine_id], |r| {
        Ok(Resident {
            engine_id: r.get(0)?,
            profile: r.get(1)?,
            resident: r.get::<_, i64>(2)? != 0,
        })
    })?;
    Ok(rows.next().transpose()?)
}

pub fn list_residents(conn: &Connection) -> Result<Vec<Resident>> {
    ensure_resident_schema(conn)?;
    let mut stmt = conn.prepare("SELECT engine_id, profile_name, resident FROM residents")?;
    let rows = stmt.query_map([], |r| {
        Ok(Resident {
            engine_id: r.get(0)?,
            profile: r.get(1)?,
            resident: r.get::<_, i64>(2)? != 0,
        })
    })?;
    Ok(rows.flatten().collect())
}

pub fn clear_resident(conn: &Connection, engine_id: &str) -> Result<()> {
    ensure_resident_schema(conn)?;
    conn.execute("DELETE FROM residents WHERE engine_id = ?1", [engine_id])?;
    Ok(())
}
