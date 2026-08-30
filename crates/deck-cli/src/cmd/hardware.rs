use anyhow::Result;

pub fn profile(json: bool) -> Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    let id = deck_core::store::capture_hardware_profile(&conn)?;
    let mut stmt = conn.prepare("SELECT gpu, vram_mb, cpu, ram_mb, os, driver, cuda, cyberdeck_ver, engines_json, captured_at, content_hash FROM hardware_profiles WHERE id=?1")?;
    let row = stmt.query_row([id], |r| {
        Ok(serde_json::json!({
            "id": id,
            "gpu": r.get::<_, String>(0)?,
            "vram_mb": r.get::<_, i64>(1)?,
            "cpu": r.get::<_, String>(2)?,
            "ram_mb": r.get::<_, i64>(3)?,
            "os": r.get::<_, String>(4)?,
            "driver": r.get::<_, String>(5)?,
            "cuda": r.get::<_, String>(6)?,
            "cyberdeck_ver": r.get::<_, String>(7)?,
            "engines_json": r.get::<_, String>(8)?,
            "captured_at": r.get::<_, i64>(9)?,
            "content_hash": r.get::<_, String>(10)?,
        }))
    })?;
    if json {
        println!("{}", serde_json::to_string_pretty(&row)?);
    } else {
        println!("hw#{} hash={} | {} {}MB | {} | driver={} | {} | engines={}", id, row["content_hash"].as_str().unwrap_or(""), row["gpu"].as_str().unwrap_or(""), row["vram_mb"], row["cpu"].as_str().unwrap_or(""), row["driver"].as_str().unwrap_or(""), row["cyberdeck_ver"].as_str().unwrap_or(""), row["engines_json"].as_str().unwrap_or(""));
    }
    Ok(())
}
