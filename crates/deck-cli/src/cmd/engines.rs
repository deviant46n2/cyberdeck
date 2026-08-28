//! `deck engines` — runtime registry: list runtimes and configure the
//! per-engine executable path used by bringup / test / matrix whenever a
//! profile's default binary doesn't exist on disk.

use std::path::PathBuf;

use anyhow::Result;

use super::parse_engine;

pub(crate) fn list() -> Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_engine_bin_schema(&conn)?;
    for d in deck_core::profile::engine_descriptors() {
        let bin = deck_core::store::get_engine_bin(&conn, d.id)?;
        match bin {
            Some(b) => println!("{:<10} {}  ->  {}", d.id, d.display, b),
            None => println!("{:<10} {}  ->  (engine default)", d.id, d.display),
        }
    }
    Ok(())
}

pub(crate) fn bin(engine: &str, path: Option<PathBuf>, clear: bool) -> Result<()> {
    let eng = parse_engine(engine)?;
    let store_id = eng.store_id();
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;

    if clear {
        deck_core::store::clear_engine_bin(&conn, store_id)?;
        println!("[{store_id}] cleared — using engine default resolution");
        return Ok(());
    }
    if let Some(path) = &path {
        if !path.exists() {
            anyhow::bail!("[{store_id}] binary not found on disk: {}", path.display());
        }
        deck_core::store::set_engine_bin(&conn, store_id, path.display().to_string().as_str())?;
        println!("[{store_id}] bin = {}", path.display());
        return Ok(());
    }
    // No path: print the current value.
    match deck_core::store::get_engine_bin(&conn, store_id)? {
        Some(b) => println!("[{store_id}] bin = {b}"),
        None => println!("[{store_id}] no configured bin (engine default resolution)"),
    }
    Ok(())
}
