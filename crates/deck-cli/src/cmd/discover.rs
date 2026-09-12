//! `deck discover`: measure candidate configs so the fit UI can show TESTED,
//! not just estimated. One trial per compatible runtime at its fit-max ctx by
//! default (plus a half-ctx point at `--variants 2`); each trial boots on its
//! own test port, takes the shared probe, and persists a `matrix_runs` row.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;

use super::with_profiles_db;

pub(crate) fn run(
    model: PathBuf,
    runtimes: Vec<String>,
    variants: u32,
    ctx: Option<u32>,
    runs: u32,
    max_tokens: u32,
    json: bool,
) -> Result<()> {
    let runtimes_opt = if runtimes.is_empty() {
        None
    } else {
        Some(runtimes.as_slice())
    };
    let (mut plans, skipped) =
        deck_engines::discover::plan_trials(&model, runtimes_opt, variants, ctx)
            .map_err(anyhow::Error::msg)?;
    println!(
        "[discover] {} trial(s) for {}",
        plans.len(),
        model.display()
    );
    for (r, why) in &skipped {
        println!("[discover] skip {r}: {why}");
    }
    if plans.is_empty() {
        anyhow::bail!("nothing measurable — every candidate was skipped");
    }

    let (_db, conn) = with_profiles_db()?;
    deck_engines::discover::apply_engine_bins(&mut plans, &|p| {
        deck_core::store::get_engine_bin(&conn, &p.runtime_key())
            .ok()
            .flatten()
            .map(PathBuf::from)
    });

    let mut all_rows = Vec::new();
    let mut results = Vec::new();
    for plan in &plans {
        println!(
            "[discover] trial: {} (bin {})",
            plan.label,
            plan.profile.bin.display()
        );
        let res = deck_engines::discover::run_trial(
            plan,
            max_tokens,
            runs,
            Duration::from_secs(180),
            Some(&|s: &str| println!("[discover] {s}")),
        );
        println!(
            "[discover] {} → boot {} · {}",
            plan.label,
            res.boot_verdict,
            res.best_tps
                .map(|t| format!("{t:.1} tok/s"))
                .unwrap_or_else(|| res.boot_summary.clone())
        );
        all_rows.extend(res.rows.iter().cloned());
        results.push(res);
    }

    let ids = deck_core::store::persist_matrix_batch(&conn, &all_rows)?;
    let report = deck_engines::discover::summarize(
        &model.display().to_string(),
        &results,
        &skipped,
        &ids,
    );
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("\n{}", report.summary);
        for t in &report.trials {
            println!(
                "  {:<12} ctx {:>6} boot {:<8} {}",
                t.runtime_id,
                t.ctx,
                t.boot_verdict,
                t.tps
                    .map(|v| format!(
                        "{v:.1} tok/s ({})",
                        t.kind.as_deref().unwrap_or("?")
                    ))
                    .unwrap_or_else(|| "no reading".into())
            );
        }
    }
    Ok(())
}
