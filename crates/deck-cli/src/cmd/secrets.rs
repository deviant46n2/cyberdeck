//! `deck secrets` — the OS-keychain door for cloud-provider API keys.
//!
//! Keys are stored in the OS keychain (Secret Service on Linux, the
//! industry-standard store) via `deck-agents::keys`, and resolved keychain-
//! first, env-fallback by the catalog/harness paths. This module only ever
//! prints names and masked values — never raw secrets.

use anyhow::Result;
use std::io::{BufRead, Write};

/// List providers that have a stored key (names only).
pub(crate) fn list() -> Result<()> {
    let ids = deck_agents::keys::list();
    if ids.is_empty() {
        println!("no provider keys stored in the OS keychain");
        return Ok(());
    }
    println!("providers with a stored key:");
    for id in ids {
        let env_name = deck_agents::keys::env_var_name(&id);
        let has_env = std::env::var(&env_name)
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false);
        println!("  {id:<12} [keychain] {env_name}{}", if has_env { " (+ env override)" } else { "" });
    }
    Ok(())
}

/// Store a provider's key, reading the value from stdin so it never appears
/// in argv or shell history. Provide the secret on stdin, e.g.
/// `deck secrets set groq` then paste, or `deck secrets set openrouter < token`.
pub(crate) fn set(provider: &str, inline: Option<String>) -> Result<()> {
    let key = match inline {
        Some(k) => k.trim().to_string(),
        None => read_stdin_line("paste the API key (then Enter):")?,
    };
    if key.is_empty() {
        anyhow::bail!("empty key — nothing stored for {provider}");
    }
    deck_agents::keys::set(provider, &key)?;
    println!("stored key for {provider} in the OS keychain (masked: {})", deck_agents::keys::mask(&key));
    Ok(())
}

/// Delete a provider's stored key.
pub(crate) fn unset(provider: &str) -> Result<()> {
    deck_agents::keys::delete(provider)?;
    println!("removed stored key for {provider}");
    Ok(())
}

/// Show whether a provider has a resolvable key and where it comes from.
pub(crate) fn check(provider: &str) -> Result<()> {
    let stored = deck_agents::keys::read(provider)?;
    let env_name = deck_agents::keys::env_var_name(provider);
    let env = std::env::var(&env_name)
        .map(|v| if v.trim().is_empty() { None } else { Some(v) })
        .unwrap_or(None);
    match (stored, env) {
        (Some(s), Some(e)) => {
            println!("{provider}: keychain ({}) + env {env_name} ({})", deck_agents::keys::mask(&s), deck_agents::keys::mask(&e));
        }
        (Some(s), None) => {
            println!("{provider}: keychain ({}) — env not set", deck_agents::keys::mask(&s));
        }
        (None, Some(e)) => {
            println!("{provider}: no keychain entry — using env {env_name} ({})", deck_agents::keys::mask(&e));
        }
        (None, None) => {
            println!("{provider}: no key found (neither keychain nor {env_name})");
        }
    }
    Ok(())
}

fn read_stdin_line(prompt: &str) -> Result<String> {
    print!("{prompt} ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim().to_string())
}
