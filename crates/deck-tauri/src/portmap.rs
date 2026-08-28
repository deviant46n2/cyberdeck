//! PORT MAP status for the UI: the fixed per-engine slots, what profile is
//! bound to each (from the residents table), and a live up/down probe. This is
//! the Tauri door to the same residency state `deck engines status` reads — the
//! single truth lives in the store's `residents` table + the descriptor registry.

use crate::fit;
use deck_core::profile::Profile;
use serde::Serialize;

#[derive(Serialize)]
pub struct PortMapSlot {
    pub engine: String,
    pub display: String,
    pub port: u16,
    pub profile: Option<String>,
    pub resident: bool,
    /// "up" (answers on port), "starting" (unit active, port not up yet), "down".
    pub state: String,
    /// Fit verdict for the bound profile (PASS/WARN/OOM) — computed from the
    /// profile's model + ctx_size so the chat header can show where to type.
    pub fit_verdict: Option<String>,
}

/// Build the full PORT MAP status. Probes are one-shot and non-blocking so a
/// down engine fails fast instead of hanging the render.
pub fn port_map_status(host: &str) -> Vec<PortMapSlot> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db).ok();
    let residents = conn
        .as_ref()
        .and_then(|c| deck_core::store::list_residents(c).ok())
        .unwrap_or_default();
    let by_engine: std::collections::HashMap<String, deck_core::store::Resident> = residents
        .into_iter()
        .map(|r| (r.engine_id.clone(), r))
        .collect();

    // Cache profiles by name for fit computation
    let profiles: Vec<Profile> = conn
        .as_ref()
        .and_then(|c| deck_core::store::list_profiles(c).ok())
        .unwrap_or_default();
    let profile_by_name: std::collections::HashMap<String, Profile> = profiles
        .into_iter()
        .map(|p| (p.name.clone(), p))
        .collect();

    deck_core::profile::Engine::all()
        .into_iter()
        .map(|e| {
            let d = e.descriptor();
            let probe = deck_engines::status::probe_slot(e, host);
            let state = if probe.port_up {
                "up".to_string()
            } else if probe.unit_active {
                "starting".to_string()
            } else {
                "down".to_string()
            };
            let r = by_engine.get(d.id);
            let profile_name = r.map(|r| r.profile.clone());
            let fit_verdict = profile_name.as_ref().and_then(|name| {
                profile_by_name.get(name).and_then(|p| {
                    // Run fit estimate for this profile's model + ctx
                    let model_path = std::path::PathBuf::from(&p.model);
                    fit(
                        model_path,
                        p.ctx_size,
                        0.5,   // kv_bytes default
                        p.n_gpu_layers,
                        None,  // kv_layers
                        1600,  // reserve
                        p.ft_backend.as_deref() == Some("offload"),
                    )
                    .ok()
                    .map(|f| f.verdict)
                })
            });
            PortMapSlot {
                engine: d.id.to_string(),
                display: d.display.to_string(),
                port: d.default_port,
                profile: profile_name,
                resident: r.map(|r| r.resident).unwrap_or(false),
                state,
                fit_verdict,
            }
        })
        .collect()
}

/// Stop one engine's unit and clear its port-map binding — the UI door to
/// `deck engines stop`. Other residents are untouched; that is the essence of
/// multi-model residency.
pub fn engine_stop(engine_id: &str) -> anyhow::Result<()> {
    let eng = deck_core::profile::Engine::parse(engine_id)
        .ok_or_else(|| anyhow::anyhow!("unknown engine '{engine_id}'"))?;
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_resident_schema(&conn)?;
    deck_engines::stop(eng.systemd_unit())?;
    deck_core::store::clear_resident(&conn, eng.store_id())?;
    Ok(())
}
