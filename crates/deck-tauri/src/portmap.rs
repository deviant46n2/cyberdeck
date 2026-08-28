//! PORT MAP status for the UI: the fixed per-engine slots, what profile is
//! bound to each (from the residents table), and a live up/down probe. This is
//! the Tauri door to the same residency state `deck engines status` reads — the
//! single truth lives in the store's `residents` table + the descriptor registry.

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
            PortMapSlot {
                engine: d.id.to_string(),
                display: d.display.to_string(),
                port: d.default_port,
                profile: r.map(|r| r.profile.clone()),
                resident: r.map(|r| r.resident).unwrap_or(false),
                state,
            }
        })
        .collect()
}
