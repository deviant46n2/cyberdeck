pub fn hardware_profile() -> Result<deck_core::hardware::HardwareProfile, String> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db).map_err(|e| e.to_string())?;
    let id = deck_core::store::capture_hardware_profile(&conn).map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare("SELECT gpu, vram_mb, cpu, ram_mb, os, driver, cuda, cyberdeck_ver, engines_json, captured_at, content_hash FROM hardware_profiles WHERE id=?1").map_err(|e| e.to_string())?;
    stmt.query_row([id], |r| {
        Ok(deck_core::hardware::HardwareProfile {
            id,
            gpu: r.get(0)?, vram_mb: r.get::<_, i64>(1)? as u64,
            cpu: r.get(2)?, ram_mb: r.get::<_, i64>(3)? as u64,
            os: r.get(4)?, driver: r.get(5)?, cuda: r.get(6)?,
            cyberdeck_ver: r.get(7)?, engines_json: r.get(8)?,
            captured_at: r.get(9)?, content_hash: r.get(10)?,
        })
    }).map_err(|e| e.to_string())
}

/// Live host telemetry for the companion widget. Fast (<300 ms): polls disk
/// bytes + nvidia-smi once, so it is safe to call on the poll interval.
pub fn host_metrics() -> deck_core::hardware::LiveMetrics {
    deck_core::hardware::live_metrics()
}
