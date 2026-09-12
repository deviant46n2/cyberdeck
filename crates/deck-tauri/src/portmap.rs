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

/// Fit verdict for the profile bound to a slot, if any.
fn fit_for(
    profile_name: Option<&String>,
    profile_by_name: &std::collections::HashMap<String, Profile>,
) -> Option<String> {
    profile_name.and_then(|name| {
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
    })
}

/// Build the full PORT MAP status. Probes are one-shot and non-blocking so a
/// down engine fails fast instead of hanging the render. Builtin slots always
/// appear; custom manifests appear when bound or installed, so a weird backend
/// that is actually running is visible instead of invisible.
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

    let mut slots: Vec<PortMapSlot> = deck_core::profile::Engine::all()
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
            PortMapSlot {
                engine: d.id.to_string(),
                display: d.display.to_string(),
                port: d.default_port,
                profile: profile_name.clone(),
                resident: r.map(|r| r.resident).unwrap_or(false),
                state,
                fit_verdict: fit_for(profile_name.as_ref(), &profile_by_name),
            }
        })
        .collect();

    // Custom backends: visible when bound to a profile or installed on disk.
    let builtin_ids: std::collections::HashSet<String> = deck_core::profile::Engine::all()
        .into_iter()
        .map(|e| e.store_id().to_string())
        .collect();
    let dirs = deck_engines::availability::system_path_dirs();
    for m in deck_core::runtime::all_manifests()
        .into_iter()
        .filter(|m| !builtin_ids.contains(&m.id))
    {
        let r = by_engine.get(&m.id);
        let ov = conn
            .as_ref()
            .and_then(|c| deck_core::store::get_engine_bin(c, &m.id).ok().flatten());
        let installed =
            deck_engines::availability::availability_for(&m, ov.as_deref(), &dirs).installed;
        if r.is_none() && !installed {
            continue;
        }
        let profile_name = r.map(|r| r.profile.clone());
        let (unit_name, port) = match profile_name
            .as_ref()
            .and_then(|n| profile_by_name.get(n))
        {
            Some(p) => (deck_engines::unit_name_for(p), p.port),
            None => (m.unit_name.clone(), m.default_port),
        };
        let unit_active = deck_engines::is_active(&unit_name, m.is_system_service);
        let port_up = deck_engines::health_ok_any(host, port);
        let state = if port_up {
            "up".to_string()
        } else if unit_active {
            "starting".to_string()
        } else {
            "down".to_string()
        };
        slots.push(PortMapSlot {
            engine: m.id.clone(),
            display: m.display.clone(),
            port,
            profile: profile_name.clone(),
            resident: r.map(|r| r.resident).unwrap_or(false),
            state,
            fit_verdict: fit_for(profile_name.as_ref(), &profile_by_name),
        });
    }
    slots
}

/// Resolve the systemd unit for a runtime id: the bound profile's resolved
/// unit when one exists (custom-aware), else the builtin slot or the custom
/// manifest's unit name.
fn unit_for(conn: &rusqlite::Connection, runtime_id: &str) -> anyhow::Result<String> {
    if let Ok(Some(r)) = deck_core::store::get_resident(conn, runtime_id)
        && let Ok(Some(p)) = deck_core::store::get_profile(conn, &r.profile)
    {
        return Ok(deck_engines::unit_name_for(&p));
    }
    if let Some(e) = deck_core::profile::Engine::parse(runtime_id) {
        return Ok(e.systemd_unit().to_string());
    }
    if let Some(m) = deck_core::runtime::all_manifests()
        .into_iter()
        .find(|m| m.id == runtime_id)
    {
        return Ok(m.unit_name);
    }
    anyhow::bail!("unknown engine/runtime '{runtime_id}'")
}

/// Stop one runtime's unit and clear its port-map binding — the UI door to
/// `deck engines stop`. Other residents are untouched; that is the essence of
/// multi-model residency. Works for custom manifest ids, not just builtins.
pub fn engine_stop(engine_id: &str) -> anyhow::Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_resident_schema(&conn)?;
    let unit = unit_for(&conn, engine_id)?;
    deck_engines::stop(&unit)?;
    deck_core::store::clear_resident(&conn, engine_id)?;
    Ok(())
}

/// Start the bound resident for one runtime (LM-Studio-style one-click start).
/// Uses the profile already bound in `residents`; fails if none. The id is a
/// builtin store id or a custom manifest id — both key residents alike.
pub fn engine_start(engine_id: &str) -> anyhow::Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    let r = deck_core::store::get_resident(&conn, engine_id)?.ok_or_else(|| anyhow::anyhow!("no profile bound to {engine_id} — use `deck use <profile> --resident` or load one first"))?;
    let p = deck_core::store::get_profile(&conn, &r.profile)?.ok_or_else(|| anyhow::anyhow!("bound profile '{}' not found", r.profile))?;
    deck_engines::apply(&p, false)?;
    Ok(())
}

// ---- ollama daemon control (system service) ----

/// Check if the ollama daemon is currently running.
pub fn ollama_is_running() -> bool {
    deck_engines::is_active("ollama.service", true)
}

/// Start the ollama system service. Returns an error if it fails.
pub fn ollama_start() -> anyhow::Result<()> {
    deck_engines::start_system("ollama.service")?;
    Ok(())
}

/// Stop the ollama system service.
pub fn ollama_stop() -> anyhow::Result<()> {
    deck_engines::stop_system("ollama.service")?;
    Ok(())
}

// ---- unmanaged llama-server processes (ad-hoc, not via systemd) ----

/// Re-export the unmanaged process type for the Tauri command.
pub type UnmanagedProcess = deck_engines::unmanaged::UnmanagedProcess;

/// List all running llama-server processes NOT managed by systemd.
pub fn unmanaged_engines() -> Vec<UnmanagedProcess> {
    deck_engines::unmanaged::discover_unmanaged().unwrap_or_default()
}

/// Start (or restart) an unmanaged process via its systemd unit. Only works
/// for processes that live inside a systemd user unit — truly ad-hoc
/// processes cannot be started because we don't store their command line.
pub fn unmanaged_start(pid: u32) -> anyhow::Result<()> {
    deck_engines::unmanaged::start_unit(pid)
}

/// Kill an unmanaged llama-server process by PID (SIGTERM).
pub fn unmanaged_stop(pid: u32) -> anyhow::Result<()> {
    deck_engines::unmanaged::stop_pid(pid)
}

// ---- external systemd services (hand-rolled, not cyberdeck-managed) ----

/// Re-export the external service type for the Tauri command.
pub type ExternalService = deck_engines::external::ExternalService;

/// List all external systemd user services running llama-server.
pub fn external_services() -> Vec<ExternalService> {
    deck_engines::external::discover_external().unwrap_or_default()
}

/// Start an external service by its systemd unit name.
pub fn external_service_start(unit: &str) -> anyhow::Result<()> {
    deck_engines::external::start_service(unit)
}

/// Stop an external service by its systemd unit name.
pub fn external_service_stop(unit: &str) -> anyhow::Result<()> {
    deck_engines::external::stop_service(unit)
}
