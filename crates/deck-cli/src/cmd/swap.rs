//! `deck swap`: transactionally move a model to another runtime/config.
//! Verifies the replacement on its test port first; the live instance is only
//! touched once the candidate has proven it can serve, and is restored on any
//! live failure.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;

use super::with_profiles_db;

pub(crate) fn run(
    model: PathBuf,
    to: String,
    ctx: Option<u32>,
    fast: bool,
    dry_run: bool,
) -> Result<()> {
    let (_db, conn) = with_profiles_db()?;
    let target = model.display().to_string();
    let old = deck_core::store::current_profile_for_model(&conn, &target)?
        .ok_or_else(|| anyhow::anyhow!("no saved config for {target} — run `deck bringup` first"))?;

    let (derived, test_port) =
        deck_core::fitplan::plan_for_runtime(&model, &to).map_err(anyhow::Error::msg)?;
    let mut candidate = derived.profile;
    if let Some(c) = ctx {
        candidate.ctx_size = c;
    }
    candidate.name = format!("{}-{}", candidate.alias, candidate.runtime_key());
    // Same machine-specific binary substitution bringup does — otherwise a
    // derived builtin bin can be a placeholder that isn't installed.
    if let Ok(p2) = deck_core::store::resolve_engine_bin(&conn, candidate.clone()) {
        if p2.bin != candidate.bin {
            println!("[swap] engine binary: {}", p2.bin.display());
        }
        candidate = p2;
    }

    println!(
        "[swap] {} ({} @ ctx {}) → {} ({} @ ctx {})",
        old.name,
        old.runtime_key(),
        old.ctx_size,
        candidate.name,
        candidate.runtime_key(),
        candidate.ctx_size
    );

    if dry_run {
        println!("[swap] --dry-run: verifying candidate on :{test_port}; nothing stopped.");
        let outcome =
            deck_engines::verify_on_test_port(&candidate, test_port, Duration::from_secs(180));
        println!("[swap] verify: {} ({})", outcome.summary, outcome.verdict);
        if outcome.verdict == "RUNNING" {
            println!(
                "[swap] would swap to {} at ctx={}",
                candidate.runtime_key(),
                outcome.ctx
            );
        }
        return Ok(());
    }

    let report = deck_engines::swap::swap(&old, &candidate, test_port, !fast, &|s| {
        println!("[swap] {s}")
    })?;
    println!("[swap] {}", report.summary);

    if report.swapped {
        candidate.ctx_size = report.ctx;
        deck_core::store::upsert_profile(&conn, &candidate)?;
        deck_core::store::ensure_resident_schema(&conn).ok();
        let new_rt = candidate.runtime_key();
        let _ = deck_core::store::set_resident(&conn, &new_rt, &candidate.name, Some(true));
        if old.runtime_key() != new_rt {
            let _ = deck_core::store::clear_resident(&conn, &old.runtime_key());
        }
        println!(
            "[swap] saved '{}' as the resident for {} (ctx {})",
            candidate.name, new_rt, candidate.ctx_size
        );
    } else if report.rolled_back {
        println!(
            "[swap] rolled back — live instance is still {}",
            report.from_runtime
        );
    } else {
        println!("[swap] aborted — live instance untouched");
    }
    Ok(())
}
