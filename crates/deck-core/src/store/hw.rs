use anyhow::Result;
use rusqlite::Connection;

// ------------------------------------------------------------ hardware_profiles (Phase 3)
pub fn ensure_hardware_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS hardware_profiles (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            gpu TEXT NOT NULL, vram_mb INTEGER NOT NULL,
            cpu TEXT NOT NULL, ram_mb INTEGER NOT NULL,
            os TEXT NOT NULL, driver TEXT NOT NULL, cuda TEXT NOT NULL,
            cyberdeck_ver TEXT NOT NULL, engines_json TEXT NOT NULL,
            captured_at INTEGER NOT NULL, content_hash TEXT NOT NULL UNIQUE
        )",
    )?;
    Ok(())
}

pub fn upsert_hardware_profile(conn: &Connection, p: &crate::hardware::HardwareProfile) -> Result<i64> {
    ensure_hardware_schema(conn)?;
    conn.execute(
        "INSERT OR IGNORE INTO hardware_profiles (gpu, vram_mb, cpu, ram_mb, os, driver, cuda, cyberdeck_ver, engines_json, captured_at, content_hash)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        rusqlite::params![p.gpu, p.vram_mb as i64, p.cpu, p.ram_mb as i64, p.os, p.driver, p.cuda, p.cyberdeck_ver, p.engines_json, p.captured_at, p.content_hash],
    )?;
    let mut stmt = conn.prepare("SELECT id FROM hardware_profiles WHERE content_hash=?1")?;
    Ok(stmt.query_row([&p.content_hash], |r| r.get(0))?)
}

pub fn capture_hardware_profile(conn: &Connection) -> Result<i64> {
    upsert_hardware_profile(conn, &crate::hardware::capture())
}
