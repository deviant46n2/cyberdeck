//! Full bring-up: derive loadout, verify headlessly on a test port, install,
//! start, and bench.

use std::path::PathBuf;

use anyhow::Result;

use super::{parse_engine, with_profiles_db};

pub(crate) fn run(
    model: PathBuf,
    engine: String,
    fast: bool,
    name: Option<String>,
    dry_run: bool,
    bin: Option<String>,
) -> Result<()> {
    let eng = parse_engine(&engine)?;
    println!(
        "[bringup] deriving loadout for {:?} via {eng:?}…",
        model.file_name().unwrap_or_default()
    );
    let derived = deck_core::profile::derive_loadout(&model, eng).map_err(anyhow::Error::msg)?;
    let mut p = derived.profile;
    if let Some(b) = &bin {
        p.bin = PathBuf::from(b);
    }

    if let Some(n) = &name {
        p.name = n.clone();
    } else {
        p.name = p.alias.clone();
    }

    println!(
        "[bringup] derived: ctx={} (max {}) kv={}MiB weights(gpu={}MiB ram={}MiB) verdict={} port={}",
        p.ctx_size,
        derived.max_ctx,
        derived.kv_mb,
        derived.weights_gpu_mb,
        derived.weights_ram_mb,
        derived.verdict,
        p.port,
    );

    if dry_run {
        println!(
            "[bringup] --dry-run: would save loadout '{}' (engine={:?} port={}) and apply it. nothing changed.",
            p.name, p.engine, p.port
        );
        return Ok(());
    }

    // Option 1 (default): verify headlessly on a test port WITHOUT touching the
    // live service, walking the ctx ladder if the max OOMs. Only then install.
    if !fast {
        let test_port = eng.test_port();
        println!(
            "[bringup] verifying on test port :{test_port} (live :{} untouched)…",
            p.port
        );
        let outcome =
            deck_engines::verify_on_test_port(&p, test_port, std::time::Duration::from_secs(120));
        if outcome.verdict != "RUNNING" {
            anyhow::bail!(
                "[bringup] verification FAILED on the test port: {} ({}) — nothing was changed on the live service; use --fast to force",
                outcome.summary,
                outcome.verdict,
            );
        }
        if outcome.ctx != p.ctx_size {
            println!(
                "[bringup] max ctx {} OOM'd; settled on ctx={}",
                p.ctx_size, outcome.ctx
            );
            p.ctx_size = outcome.ctx;
        }
        println!(
            "[bringup] verify OK: ctx={} serving{}",
            outcome.ctx,
            outcome
                .tok_per_sec
                .map(|t| format!(", {t:.1} tok/s"))
                .unwrap_or_default(),
        );
    } else {
        println!("[bringup] --fast: skipping test-port verification");
    }

    // Save the derived loadout, then apply (install + start + health-wait).
    let (_db, mut conn) = with_profiles_db()?;
    deck_core::store::upsert_profile(&mut conn, &p)?;
    println!(
        "[bringup] saved loadout '{}' (engine={:?} port={})",
        p.name, p.engine, p.port
    );

    deck_engines::apply(&p, false)?;
    println!("[bringup] applied '{}' on :{} — live.", p.name, p.port);

    // Bench and record the result so the chat header has a fresh tok/s.
    let text = deck_engines::fetch_metrics(&p.host, p.port)?;
    if let Some(tps) = deck_engines::parse_tps(&text) {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let db = deck_core::store::default_db_path();
        let conn = deck_core::store::open(&db)?;
        deck_core::store::ensure_bench_schema(&conn)?;
        let row = deck_core::store::BenchRow {
            id: 0,
            engine: format!("{:?}", p.engine).to_lowercase(),
            host: p.host.clone(),
            port: p.port,
            model: p.model.clone(),
            ctx: p.ctx_size,
            tps,
            at,
        };
        let id = deck_core::store::insert_bench(&conn, &row)?;
        println!("[bringup] bench recorded #{id}: {tps:.1} tok/s");
    } else {
        println!("[bringup] note: no /metrics tok/s gauge exposed (is --metrics on?)");
    }

    Ok(())
}
