//! Hot-swap door: move a model to another runtime/config transactionally.
//! Same logic as `deck swap` — verify on the test port, commit, roll back on
//! live failure. Shared by the app and CLI through `deck-engines::swap`.

use std::path::Path;
use std::time::Duration;

use serde::Serialize;

#[derive(Serialize)]
pub struct HotSwapReport {
    pub from_runtime: String,
    pub to_runtime: String,
    /// Candidate passed test-port verification.
    pub verified: bool,
    pub swapped: bool,
    pub rolled_back: bool,
    pub ctx: u32,
    pub tps: Option<f64>,
    pub summary: String,
}

/// Plan (and optionally commit) a swap. `dry_run` verifies but never stops or
/// starts anything. Long-running (model load) — callers run this off the UI
/// thread via the async command wrapper.
pub fn hot_swap(
    model_path: &str,
    to: &str,
    ctx: Option<u32>,
    fast: bool,
    dry_run: bool,
) -> anyhow::Result<HotSwapReport> {
    if !Path::new(model_path).exists() {
        anyhow::bail!("model not found on disk: {model_path}");
    }
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_profile_schema(&conn)?;
    let old = deck_core::store::current_profile_for_model(&conn, model_path)?
        .ok_or_else(|| anyhow::anyhow!("no saved config for {model_path} — bring it up first"))?;

    let (derived, test_port) =
        deck_core::fitplan::plan_for_runtime(Path::new(model_path), to).map_err(anyhow::Error::msg)?;
    let mut candidate = derived.profile;
    if let Some(c) = ctx {
        candidate.ctx_size = c;
    }
    candidate.name = format!("{}-{}", candidate.alias, candidate.runtime_key());
    if let Ok(p2) = deck_core::store::resolve_engine_bin(&conn, candidate.clone()) {
        candidate = p2;
    }

    let from_runtime = old.runtime_key();
    let to_runtime = candidate.runtime_key();

    if dry_run {
        let outcome =
            deck_engines::verify_on_test_port(&candidate, test_port, Duration::from_secs(180));
        return Ok(HotSwapReport {
            from_runtime,
            to_runtime,
            verified: outcome.verdict == "RUNNING",
            swapped: false,
            rolled_back: false,
            ctx: outcome.ctx,
            tps: outcome.tok_per_sec,
            summary: format!("dry-run: {} ({})", outcome.summary, outcome.verdict),
        });
    }

    let report = deck_engines::swap::swap(&old, &candidate, test_port, !fast, &|s| {
        eprintln!("[swap] {s}")
    })?;

    if report.swapped {
        candidate.ctx_size = report.ctx;
        deck_core::store::upsert_profile(&conn, &candidate)?;
        deck_core::store::ensure_resident_schema(&conn).ok();
        let new_rt = candidate.runtime_key();
        let _ = deck_core::store::set_resident(&conn, &new_rt, &candidate.name, Some(true));
        if old.runtime_key() != new_rt {
            let _ = deck_core::store::clear_resident(&conn, &old.runtime_key());
        }
    }

    Ok(HotSwapReport {
        from_runtime: report.from_runtime,
        to_runtime: report.to_runtime,
        verified: report.verified,
        swapped: report.swapped,
        rolled_back: report.rolled_back,
        ctx: report.ctx,
        tps: report.tps,
        summary: report.summary,
    })
}
