//! `deck engines` — runtime registry: list runtimes and configure the
//! per-engine executable path used by bringup / test / matrix whenever a
//! profile's default binary doesn't exist on disk.

use std::path::PathBuf;

use anyhow::Result;

use super::parse_engine;

/// Live PORT MAP: for every engine slot, what's bound to it (from the
/// residents table), whether its systemd unit is active, and whether it
/// answers on its fixed port. Host is loopback by default — engines serve on
/// 0.0.0.0 but are probed locally.
pub(crate) fn status(host: &str) -> Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_resident_schema(&conn)?;
    let resid_by_engine: std::collections::HashMap<String, deck_core::store::Resident> =
        deck_core::store::list_residents(&conn)?
            .into_iter()
            .map(|r| (r.engine_id.clone(), r))
            .collect();

    println!("PORT MAP — fixed slots per engine. Resident = runs alongside others.");
    println!("engine     port  state    profile      resident");
    for e in deck_core::profile::Engine::all() {
        let d = e.descriptor();
        let probe = deck_engines::status::probe_slot(e, host);
        let state = if probe.port_up {
            "UP"
        } else if probe.unit_active {
            "starting"
        } else {
            "down"
        };
        let profile = resid_by_engine
            .get(d.id)
            .map(|r| (r.profile.as_str(), if r.resident { "resident" } else { "-" }))
            .unwrap_or(("-", "-"));
        println!(
            "{:<10} {:>6}  {:<8} {:<12}  {}",
            d.id, d.default_port, state, profile.0, profile.1
        );
    }
    Ok(())
}

/// Stop an engine's unit and clear its port-map binding. Leaves the other
/// residents untouched — the essence of multi-model residency.
pub(crate) fn stop(engine: &str) -> Result<()> {
    let eng = parse_engine(engine)?;
    let store_id = eng.store_id();
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_resident_schema(&conn)?;
    deck_engines::stop(eng.systemd_unit())?;
    deck_core::store::clear_resident(&conn, store_id)?;
    println!("[{store_id}] unit stopped; port-map binding cleared");
    Ok(())
}

pub(crate) fn start(engine: &str) -> Result<()> {
    let eng = parse_engine(engine)?;
    let store_id = eng.store_id();
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    let r = deck_core::store::get_resident(&conn, store_id)?.ok_or_else(|| anyhow::anyhow!("no profile bound to {store_id} — load one via `deck use <profile> --resident` first"))?;
    let p = deck_core::store::get_profile(&conn, &r.profile)?.ok_or_else(|| anyhow::anyhow!("bound profile '{}' not found", r.profile))?;
    deck_engines::apply(&p, false)?;
    println!("[{store_id}] started '{}' on :{}", p.name, p.port);
    Ok(())
}

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
