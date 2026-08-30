//! Loadout persistence and the `deck use` apply/rewire flow.

use serde::Serialize;

use crate::Profile;

#[derive(Serialize)]
pub struct ProfileRow {
    pub name: String,
    pub engine: String,
    pub alias: String,
    pub port: u16,
    pub ctx: u32,
    pub model: String,
}

#[derive(Serialize)]
pub struct UseResult {
    pub name: String,
    pub applied: bool,
    pub dry_run: bool,
    pub unit: String,
    /// MANAGED-mode client rewiring outcomes (empty unless --managed).
    pub rewired: Vec<String>,
}

pub fn list_profiles() -> anyhow::Result<Vec<ProfileRow>> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_profile_schema(&conn)?;
    Ok(deck_core::store::list_profiles(&conn)?
        .into_iter()
        .map(|p| ProfileRow {
            name: p.name,
            engine: format!("{:?}", p.engine),
            alias: p.alias,
            port: p.port,
            ctx: p.ctx_size,
            model: p.model,
        })
        .collect())
}

/// Persist a loadout (created or edited in the UI) to the index.
pub fn save_profile(p: Profile) -> anyhow::Result<()> {
    let db = deck_core::store::default_db_path();
    let mut conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_profile_schema(&conn)?;
    deck_core::store::upsert_profile(&mut conn, &p)
}

/// Remove a saved loadout by name.
pub fn delete_profile(name: &str) -> anyhow::Result<()> {
    let db = deck_core::store::default_db_path();
    let mut conn = deck_core::store::open(&db)?;
    deck_core::store::delete_profile(&mut conn, name)
}

/// Render the systemd unit for an arbitrary (possibly unsaved) profile so the
/// editor can preview exactly what `apply` would write.
pub fn render_profile_unit(p: Profile) -> String {
    deck_engines::render_unit(&p)
}

/// Render (dry_run) or apply a loadout. `dry_run` returns the unit without
/// touching the live service.
///
/// `managed` (opt-in) additionally repoints dsh + opencode at the applied
/// engine's port so the rest of the stack follows the swap. Off by default:
/// the Advisory contract preserves the alias+port so clients don't reconfigure.
pub fn use_profile(name: &str, dry_run: bool, managed: bool) -> anyhow::Result<UseResult> {
    let db = deck_core::store::default_db_path();
    let mut conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_profile_schema(&conn)?;
    let p = deck_core::store::get_profile(&conn, name)?
        .ok_or_else(|| anyhow::anyhow!("no loadout named '{name}'"))?;
    deck_core::store::set_active(&mut conn, name)?;
    let unit = deck_engines::render_unit(&p);
    let mut rewired = Vec::new();
    if !dry_run {
        deck_engines::apply(&p, false)?;
        if managed {
            // per-slot: rewrite only this profile's engine provider block so a
            // managed bind never disturbs another resident's baseURL
            for r in deck_engines::rewire::rewire_clients_for(p.engine.store_id(), p.port) {
                rewired.push(format!("[{}] {} — {}", r.client, r.path, r.status));
            }
        }
    }
    Ok(UseResult {
        name: name.to_string(),
        applied: !dry_run,
        dry_run,
        unit,
        rewired,
    })
}
