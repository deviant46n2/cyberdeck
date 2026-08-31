//! Provider API-key storage.
//!
//! Keys live in the **OS keychain** (Secret Service on Linux via the `keyring`
//! crate; Keychain / Credential Manager transparently on macOS / Windows),
//! which is the industry-standard store desktop apps use — encrypted at rest,
//! unlocked by the login session. There is intentionally **no** application-
//! level obfuscation or master passphrase: that is the non-standard pattern
//! that usually makes things weaker, not stronger.
//!
//! Resolution is keychain-first, process-env fallback (the convention matching
//! `gh` / GitHub CLI): a provider without a stored key still works if the
//! conventional `<PROVIDER>_API_KEY` variable is set. Keys are never written
//! to the repo, the settings KV table, or logs, and never leak through the
//! Tauri/CLI read surfaces (list returns names only; get returns a masked
//! form).

use anyhow::{Context, Result};

/// The keyring service name partitioning this app's secrets from others'.
const SERVICE: &str = "cyberdeck-agent-key";

/// Standard env-var name for a provider's key, e.g. `groq` → `GROQ_API_KEY`.
/// Dashes/hyphens in a provider id become underscores so `open-code`-style
/// ids map to a legal shell var.
pub fn env_var_name(provider: &str) -> String {
    format!(
        "{}_API_KEY",
        provider.to_uppercase().replace('-', "_")
    )
}

/// Read the conventional env var for a provider (`GROQ_API_KEY`, …).
pub fn env_key(provider: &str) -> Option<String> {
    let v = std::env::var(env_var_name(provider)).ok()?;
    let v = v.trim().to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// Resolve a provider's key: OS keychain first, then the env var.
///
/// `store_get` is injected so the precedence logic is testable without a
/// keyring daemon; `env_get` too (defaults to [`env_key`] in production).
pub fn resolve_with(
    store_get: impl Fn(&str) -> Option<String>,
    env_get: impl Fn(&str) -> Option<String>,
    provider: &str,
) -> Option<String> {
    store_get(provider).or_else(|| env_get(provider))
}

/// Resolve a provider's key against the real keychain + env.
pub fn resolve(provider: &str) -> Option<String> {
    resolve_with(|p| read(p).ok().flatten(), env_key, provider)
}

/// A mask of a secret safe for UI/CLI display, e.g. `sk-or-…pBqX`.
/// Reveals only the first 4 chars and the last 4.
pub fn mask(secret: &str) -> String {
    let s = secret.trim();
    if s.len() <= 8 {
        return "••••".to_string();
    }
    let head: String = s.chars().take(4).collect();
    let tail: String = s.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{head}…{tail}")
}

/// Store a provider's key in the OS keychain (overwrites any existing).
pub fn set(provider: &str, key: &str) -> Result<()> {
    let entry = entry(provider)?;
    entry
        .set_password(key)
        .with_context(|| format!("failed to store key for provider '{provider}' in OS keychain"))
}

/// Read a provider's key from the OS keychain. `Ok(None)` = not stored.
pub fn read(provider: &str) -> Result<Option<String>> {
    let entry = entry(provider)?;
    match entry.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(anyhow::anyhow!(
            "failed to read key for provider '{provider}': {e}"
        )),
    }
}

/// Delete a provider's key from the OS keychain. Missing entry is a no-op.
pub fn delete(provider: &str) -> Result<()> {
    let entry = entry(provider)?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(anyhow::anyhow!(
            "failed to delete key for provider '{provider}': {e}"
        )),
    }
}

/// List provider ids that have a stored key in the keychain (names only).
/// Uses the known provider catalog so we only surface recognized ids.
pub fn list() -> Vec<String> {
    crate::model::builtin_providers()
        .iter()
        .filter(|p| read(&p.id).map(|v| v.is_some()).unwrap_or(false))
        .map(|p| p.id.clone())
        .collect()
}

fn entry(provider: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, provider)
        .with_context(|| format!("OS keychain unavailable for provider '{provider}' (is a keyring daemon like gnome-keyring / KWallet running?)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_var_name_is_uppercase_suffix() {
        assert_eq!(env_var_name("groq"), "GROQ_API_KEY");
        assert_eq!(env_var_name("openrouter"), "OPENROUTER_API_KEY");
        assert_eq!(env_var_name("open-code"), "OPEN_CODE_API_KEY");
    }

    #[test]
    fn resolve_prefers_store_over_env() {
        let store = |p: &str| -> Option<String> {
            (p == "groq").then(|| "store-key".to_string())
        };
        let env = |p: &str| -> Option<String> {
            (p == "groq").then(|| "env-key".to_string())
        };
        assert_eq!(resolve_with(store, env, "groq").as_deref(), Some("store-key"));
    }

    #[test]
    fn resolve_falls_back_to_env_when_not_stored() {
        let store = |_p: &str| -> Option<String> { None };
        let env = |p: &str| -> Option<String> {
            (p == "deepseek").then(|| "ds-env".to_string())
        };
        assert_eq!(
            resolve_with(store, env, "deepseek").as_deref(),
            Some("ds-env")
        );
        assert_eq!(resolve_with(store, env, "unknown").as_deref(), None);
    }

    #[test]
    fn mask_shortens_and_never_leaks_middle() {
        let m = mask("sk-or-v1-abcdefghijklmnop");
        assert!(m.starts_with("sk-o"));
        assert!(m.ends_with("mnop"));
        let short = mask("short");
        assert_eq!(short, "••••");
    }
}
