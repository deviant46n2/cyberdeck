//! `deck agents` — the online fleet door: cloud providers, agent harnesses,
//! and the per-provider quota tracker. This is the CLI half of "one truth,
//! two doors" for the fleet; the Tauri door mirrors these operations.

use anyhow::Result;
use deck_agents::model::HarnessId;

use super::with_profiles_db;

/// List built-in providers + their seed quota, and every harness.
pub(crate) fn list() -> Result<()> {
    let (_db, conn) = with_profiles_db()?;
    let s = deck_agents::ops::status(&conn)?;

    println!("PROVIDERS — online model sources (direct, native quota)");
    println!("{:<12} {:<26} {:<10} {:<10} {}", "id", "name", "kind", "quota", "note");
    for row in &s.providers {
        let q = &row.quota;
        let quota = match (q.limit, q.source) {
            (Some(l), _) => format!("{}/{}", q.used, l),
            (None, deck_agents::quota::QuotaSource::Unknown) => "unknown".into(),
            (None, _) => format!("{} used", q.used),
        };
        println!(
            "{:<12} {:<26} {:<10} {:<10} {}",
            row.provider.id,
            row.provider.display,
            format!("{:?}", row.provider.kind).to_lowercase(),
            quota,
            row.provider.free_note
        );
    }

    println!("\nHARNESSES — agent loops you can point at a provider/model");
    println!("{:<12} {:<10} {}", "id", "name", "binding");
    for h in &s.harnesses {
        let binding = h
            .binding
            .as_ref()
            .map(|b| format!("{}/{}", b.provider_id, b.model_id))
            .unwrap_or_else(|| "none".into());
        println!(
            "{:<12} {:<10} {}",
            h.harness.id.as_str(),
            h.harness.display,
            binding
        );
    }
    Ok(())
}

/// Show the current binding + quota per harness (mirrors `status`, terse).
pub(crate) fn status() -> Result<()> {
    let (_db, conn) = with_profiles_db()?;
    let s = deck_agents::ops::status(&conn)?;

    println!("FLEET STATUS");
    for h in &s.harnesses {
        let binding = h
            .binding
            .as_ref()
            .map(|b| {
                let q = s
                    .providers
                    .iter()
                    .find(|p| p.provider.id == b.provider_id)
                    .map(|p| &p.quota);
                match q {
                    Some(q) if q.pct.is_some() => format!(
                        "{}/{} ({:.0}%)",
                        b.provider_id,
                        b.model_id,
                        q.pct_display() * 100.0
                    ),
                    _ => format!("{}/{}", b.provider_id, b.model_id),
                }
            })
            .unwrap_or_else(|| "unbound".into());
        println!("  {:<10} {}", h.harness.id.as_str(), binding);
    }
    Ok(())
}

/// Fetch a provider's `/v1/models` catalog.
pub(crate) fn catalog(provider_id: &str, key: Option<String>) -> Result<()> {
    let ms = deck_agents::ops::catalog(provider_id, key.as_deref())?;
    println!("{} — {} models", provider_id, ms.len());
    for m in ms {
        let ctx = m
            .context
            .map(|c| format!(" {}k", c / 1000))
            .unwrap_or_default();
        let free = if m.free { " [free]" } else { "" };
        println!("  {}{}{}", m.id, ctx, free);
    }
    Ok(())
}

/// Bind a harness to (provider, model): rewrite its config + record binding.
pub(crate) fn use_harness(harness_id: &str, provider_id: &str, model_id: &str) -> Result<()> {
    let hid = parse_harness(harness_id)?;
    let (_db, conn) = with_profiles_db()?;
    let out = deck_agents::ops::use_harness(&conn, hid, provider_id, model_id)?;
    let r = &out.report;
    println!(
        "{}: {}",
        r.harness,
        if r.backed_up { "backed up + " } else { "" }
    );
    println!("  -> {}", r.status);
    Ok(())
}

/// Record quota usage seen for a provider.
pub(crate) fn quota_set(provider_id: &str, used: u64) -> Result<()> {
    let (_db, conn) = with_profiles_db()?;
    deck_agents::ops::record_quota_used(&conn, provider_id, used)?;
    println!("recorded {used} used for {provider_id}");
    Ok(())
}

fn parse_harness(s: &str) -> Result<HarnessId> {
    match s {
        "opencode" | "oc" => Ok(HarnessId::Opencode),
        "goose" => Ok(HarnessId::Goose),
        "deepseek" | "ds" => Ok(HarnessId::Deepseek),
        other => anyhow::bail!(
            "unknown harness '{other}' (expected opencode | goose | deepseek)"
        ),
    }
}
