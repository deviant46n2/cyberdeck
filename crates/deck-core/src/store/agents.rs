//! Store rows for the online agent fleet: harness↔provider bindings and
//! per-provider quota. Both live in the `settings` KV table (JSON blobs) so
//! they inherit its audit + undo machinery — a binding is exactly the dot-
//! namespaced key (`agents.opencode`) the harness catalog defines.

use anyhow::Result;
use rusqlite::Connection;

use crate::store::settings::{settings_get, settings_set};

/// Persist a harness→(provider,model) binding as a JSON blob under
/// `agents.<harness>`.
pub fn set_harness_binding(
    conn: &Connection,
    harness_key: &str,
    binding_json: &str,
) -> Result<()> {
    settings_set(conn, harness_key, binding_json, "deck-agents", "harness binding")
}

/// Read a stored harness binding JSON blob by its setting key.
pub fn get_harness_binding(conn: &Connection, harness_key: &str) -> Result<Option<String>> {
    settings_get(conn, harness_key)
}

/// Persist a quota entry as a JSON blob under `agents.quota.<provider>`.
pub fn set_quota(conn: &Connection, provider_id: &str, quota_json: &str) -> Result<()> {
    let key = format!("agents.quota.{provider_id}");
    settings_set(conn, &key, quota_json, "deck-agents", "quota update")
}

/// Read the stored quota JSON blob for a provider.
pub fn get_quota(conn: &Connection, provider_id: &str) -> Result<Option<String>> {
    let key = format!("agents.quota.{provider_id}");
    settings_get(conn, &key)
}

/// List all stored quota JSON blobs under `agents.quota.*`.
pub fn list_quota(conn: &Connection) -> Result<Vec<(String, String)>> {
    use crate::store::settings::settings_list;
    let all = settings_list(conn)?;
    Ok(all
        .into_iter()
        .filter(|(k, _, _)| k.starts_with("agents.quota."))
        .map(|(k, v, _)| (k, v))
        .collect())
}
