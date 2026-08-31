//! Tauri bridge for the online agent fleet (providers, harnesses, quota).
//!
//! Each function returns a serializable DTO consumed by the frontend's
//! `api.ts` fleet door. Logic lives in `deck_agents::ops`; this module only
//! adapts results to the flat shapes the UI renders.

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

/// A provider row with its quota snapshot, as the UI's provider table.
#[derive(Serialize)]
pub struct ProviderView {
    pub id: String,
    pub display: String,
    pub kind: String,
    pub free_note: String,
    pub quota_used: u64,
    pub quota_limit: Option<u64>,
    pub quota_label: String,
    pub quota_pct: Option<f64>,
    pub quota_source: String,
}

/// A harness row with its active binding (if any).
#[derive(Serialize)]
pub struct HarnessView {
    pub id: String,
    pub display: String,
    pub binding_provider: Option<String>,
    pub binding_model: Option<String>,
}

/// The full fleet read-model the picker + quota cards render.
#[derive(Serialize)]
pub struct FleetView {
    pub providers: Vec<ProviderView>,
    pub harnesses: Vec<HarnessView>,
}

fn conn() -> Result<Connection> {
    let db = deck_core::store::default_db_path();
    deck_core::store::open(&db).map_err(anyhow::Error::from)
}

/// Full fleet read: providers + quota + harnesses + bindings.
pub fn agents_fleet() -> Result<FleetView> {
    let c = conn()?;
    let s = deck_agents::ops::status(&c)?;
    Ok(FleetView {
        providers: s
            .providers
            .into_iter()
            .map(|row| {
                let q = &row.quota;
                ProviderView {
                    id: row.provider.id.clone(),
                    display: row.provider.display.to_string(),
                    kind: match row.provider.kind {
                        deck_agents::model::ProviderKind::Service => "service".into(),
                        deck_agents::model::ProviderKind::Aggregator => "aggregator".into(),
                    },
                    free_note: row.provider.free_note.to_string(),
                    quota_used: q.used,
                    quota_limit: q.limit,
                    quota_label: q.label.to_string(),
                    quota_pct: q.pct,
                    quota_source: format!("{:?}", q.source).to_lowercase(),
                }
            })
            .collect(),
        harnesses: s
            .harnesses
            .into_iter()
            .map(|h| HarnessView {
                id: h.harness.id.as_str().to_string(),
                display: h.harness.display.to_string(),
                binding_provider: h.binding.as_ref().map(|b| b.provider_id.clone()),
                binding_model: h.binding.as_ref().map(|b| b.model_id.clone()),
            })
            .collect(),
    })
}

/// Fetch a provider's `/v1/models` catalog (network — callers should run on a
/// blocking thread).
pub fn agents_catalog(provider_id: &str, key: Option<String>) -> Result<Vec<deck_agents::model::ProviderModel>> {
    deck_agents::ops::catalog(provider_id, key.as_deref())
}

/// Bind a harness to (provider, model): rewrite config + persist binding.
pub fn agents_use(harness_id: &str, provider_id: &str, model_id: &str) -> Result<String> {
    let hid = match harness_id {
        "opencode" => deck_agents::model::HarnessId::Opencode,
        "goose" => deck_agents::model::HarnessId::Goose,
        "deepseek" => deck_agents::model::HarnessId::Deepseek,
        other => anyhow::bail!("unknown harness '{other}'"),
    };
    let c = conn()?;
    let out = deck_agents::ops::use_harness(&c, hid, provider_id, model_id)?;
    Ok(format!("{}: {}", out.report.harness, out.report.status))
}

/// Record quota usage for a provider.
pub fn agents_quota_set(provider_id: &str, used: u64) -> Result<()> {
    let c = conn()?;
    deck_agents::ops::record_quota_used(&c, provider_id, used)
}

// --- provider-key storage (OS keychain) ---
//
// These mirror `deck agents secrets` for the Tauri door. Only names and masked
// values ever cross to the frontend; raw keys are set via `secret_set` and
// never returned.

/// Resolution read-model for one provider's key: where it comes from + a
/// masked preview (safe to render).
#[derive(Serialize)]
pub struct SecretView {
    pub provider: String,
    pub from_keychain: bool,
    pub from_env: bool,
    pub env_var: String,
    pub masked: Option<String>,
}

/// List providers that have a stored key (names only, grouped with env var).
pub fn secret_list() -> Result<Vec<String>> {
    Ok(deck_agents::keys::list())
}

/// Store a provider's key in the OS keychain. Returns a masked confirmation.
pub fn secret_set(provider_id: &str, key: &str) -> Result<String> {
    deck_agents::keys::set(provider_id, key)?;
    Ok(format!(
        "stored key for {provider_id} (masked: {})",
        deck_agents::keys::mask(key)
    ))
}

/// Delete a provider's stored key.
pub fn secret_unset(provider_id: &str) -> Result<()> {
    deck_agents::keys::delete(provider_id)
}

/// Where a provider's key resolves from (keychain/env) + a masked preview.
pub fn secret_check(provider_id: &str) -> Result<SecretView> {
    let stored = deck_agents::keys::read(provider_id)?.filter(|k| !k.is_empty());
    let env_var = deck_agents::keys::env_var_name(provider_id);
    let env = std::env::var(&env_var)
        .ok()
        .filter(|v| !v.trim().is_empty());
    let shown = stored
        .as_deref()
        .or(env.as_deref())
        .map(|k| deck_agents::keys::mask(k));
    Ok(SecretView {
        provider: provider_id.to_string(),
        from_keychain: stored.is_some(),
        from_env: env.is_some(),
        env_var,
        masked: shown,
    })
}
