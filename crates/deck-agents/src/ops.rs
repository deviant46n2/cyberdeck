//! High-level fleet operations — the door-facing orchestration that ties the
//! model catalog, config rewrite, quota, and store together. Deck-cli and the
//! Tauri layer call into this module rather than poking the store directly.

use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::model::{CloudProvider, Harness, HarnessBinding, HarnessId};
use crate::quota::{default_quota, QuotaEntry, QuotaSnapshot};
use crate::rewrite::{apply_binding, RewriteReport};

/// All built-in providers plus each one's stored quota snapshot.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProviderRow {
    pub provider: CloudProvider,
    pub quota: QuotaSnapshot,
}

/// The full fleet view: every harness, its active binding (if any), and the
/// quota snapshot for the provider it currently points at.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FleetStatus {
    pub harnesses: Vec<HarnessStatus>,
    pub providers: Vec<ProviderRow>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HarnessStatus {
    pub harness: Harness,
    pub binding: Option<HarnessBinding>,
}

/// List built-in providers with quota snapshots from the store (seeding a
/// default entry the first time a provider is seen).
pub fn providers_with_quota(conn: &Connection) -> Result<Vec<ProviderRow>> {
    let mut rows = Vec::new();
    for p in crate::model::builtin_providers() {
        let q = read_quota(conn, &p.id)?;
        rows.push(ProviderRow {
            provider: p,
            quota: QuotaSnapshot::from_entry(&q),
        });
    }
    Ok(rows)
}

/// The fleet status: harnesses + their bindings + provider/quota rows.
pub fn status(conn: &Connection) -> Result<FleetStatus> {
    let harnesses = crate::model::Harness::all()
        .into_iter()
        .map(|harness| {
            let binding = read_binding(conn, &harness);
            HarnessStatus { harness, binding }
        })
        .collect();
    let providers = providers_with_quota(conn)?;
    Ok(FleetStatus { harnesses, providers })
}

/// Fetch a provider's `/v1/models` catalog. `api_key` optional.
pub fn catalog(provider_id: &str, api_key: Option<&str>) -> Result<Vec<crate::model::ProviderModel>> {
    let p = crate::model::get_provider(provider_id)
        .with_context(|| format!("unknown provider '{provider_id}'"))?;
    crate::providers::fetch_models(&p, api_key)
}

/// Bind a harness to (provider, model): rewrite its config file, record the
/// binding in the store, and seed the provider's quota entry if absent.
pub fn use_harness(
    conn: &Connection,
    harness_id: HarnessId,
    provider_id: &str,
    model_id: &str,
) -> Result<UseOutcome> {
    let harness = Harness::get(harness_id);
    // Validate the provider exists before touching any config.
    crate::model::get_provider(provider_id)
        .with_context(|| format!("unknown provider '{provider_id}'"))?;
    let binding = HarnessBinding {
        provider_id: provider_id.to_string(),
        model_id: model_id.to_string(),
    };
    let report = apply_binding(harness_id, &binding)?;
    persist_binding(conn, &harness, &binding)?;
    // Seed the provider's default quota entry if it has never been recorded.
    if read_quota(conn, provider_id)?.source == crate::quota::QuotaSource::Unknown {
        persist_quota(conn, &default_quota(provider_id))?;
    }
    Ok(UseOutcome { report, binding })
}

/// Result of a `use` (config rewire + persisted binding).
#[derive(Debug, Clone, serde::Serialize)]
pub struct UseOutcome {
    pub report: RewriteReport,
    pub binding: HarnessBinding,
}

/// Mark a provider's quota usage as seen (bump or set absolute).
pub fn record_quota_used(conn: &Connection, provider_id: &str, used: u64) -> Result<()> {
    let mut e = read_quota(conn, provider_id)?;
    e.used = used;
    e.source = crate::quota::QuotaSource::Estimate;
    persist_quota(conn, &e)
}

/// Read the stored (or default-seeded) quota entry for a provider.
pub fn read_quota(conn: &Connection, provider_id: &str) -> Result<QuotaEntry> {
    match deck_core::store::get_quota(conn, provider_id)? {
        Some(json) => serde_json::from_str(&json)
            .with_context(|| format!("corrupt stored quota for {provider_id}")),
        None => Ok(default_quota(provider_id)),
    }
}

fn persist_quota(conn: &Connection, e: &QuotaEntry) -> Result<()> {
    let json = serde_json::to_string(e).context("serialize quota")?;
    deck_core::store::set_quota(conn, &e.provider_id, &json)
}

fn persist_binding(conn: &Connection, harness: &Harness, b: &HarnessBinding) -> Result<()> {
    let json = serde_json::to_string(b).context("serialize binding")?;
    deck_core::store::set_harness_binding(conn, harness.setting_key, &json)
}

fn read_binding(conn: &Connection, harness: &Harness) -> Option<HarnessBinding> {
    deck_core::store::get_harness_binding(conn, harness.setting_key)
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_quota_seeds_default_for_unknown() {
        let conn = test_conn();
        let q = read_quota(&conn, "groq").unwrap();
        assert_eq!(q.limit, Some(14_400));
        // Unknown provider (not in builtin default table) → Unknown source.
        let q2 = read_quota(&conn, "zzz").unwrap();
        assert_eq!(q2.source, crate::quota::QuotaSource::Unknown);
    }

    #[test]
    fn record_quota_used_persists_estimate() {
        let conn = test_conn();
        record_quota_used(&conn, "groq", 900).unwrap();
        let q = read_quota(&conn, "groq").unwrap();
        assert_eq!(q.used, 900);
        assert_eq!(q.source, crate::quota::QuotaSource::Estimate);
    }

    #[test]
    fn providers_with_quota_covers_all_builtins() {
        let conn = test_conn();
        let rows = providers_with_quota(&conn).unwrap();
        assert_eq!(rows.len(), crate::model::builtin_providers().len());
    }

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        // settings schema is created lazily by settings_set; ensure it.
        deck_core::store::ensure_settings_schema(&conn).unwrap();
        conn
    }
}
