//! Model lifecycle CLI doors: forget vs remove-files, storage, runtimes,
//! make-it-work (dry-run fit ranking). Thin wrappers over deck-core so the
//! CLI and the Tauri app share one truth.

use std::path::PathBuf;

use anyhow::{Context, Result};

pub(crate) fn forget(path: &str) -> Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    let rows = deck_core::store::delete_model(&conn, path, false)?;
    println!("forgot {path} ({rows} record(s) removed; files left on disk)");
    Ok(())
}

pub(crate) fn remove_files(paths: &[String]) -> Result<()> {
    let owned: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let plan = deck_core::storage::removal_plan(&owned);
    if plan.files.is_empty() && plan.missing.is_empty() {
        println!("nothing to remove");
        return Ok(());
    }
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    let mut freed = 0u64;
    for f in &plan.files {
        deck_core::store::delete_model(&conn, &f.path, false)?;
        let p = PathBuf::from(&f.path);
        if p.is_dir() {
            std::fs::remove_dir_all(&p)
                .with_context(|| format!("removing dir {}", f.path))?;
        } else {
            std::fs::remove_file(&p)
                .with_context(|| format!("removing file {}", f.path))?;
        }
        freed += f.bytes;
        println!(
            "removed {} ({:.2} GiB)",
            f.path,
            f.bytes as f64 / 1_073_741_824.0
        );
    }
    for m in &plan.missing {
        println!("missing (not on disk, record kept): {m}");
    }
    println!("freed {:.2} GiB", freed as f64 / 1_073_741_824.0);
    Ok(())
}

pub(crate) fn storage(json: bool) -> Result<()> {
    let mut roots = deck_core::scanner::default_roots();
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    roots.extend(deck_core::store::scan_dirs(&conn)?);
    let found = deck_core::scanner::scan(&roots)?;
    let registered = deck_core::store::list(&conn)?;
    drop(conn);
    let canon = |p: &std::path::Path| {
        std::fs::canonicalize(p)
            .map(|c| c.display().to_string())
            .unwrap_or_else(|_| p.display().to_string())
    };
    let reg: Vec<(String, String, u64, String)> = registered
        .iter()
        .map(|m| {
            let raw = m.path.display().to_string();
            (raw.clone(), canon(&m.path), m.footprint, m.name.clone())
        })
        .collect();
    let fnd: Vec<(String, String, u64, String)> = found
        .iter()
        .map(|m| {
            let raw = m.path.display().to_string();
            (raw.clone(), canon(&m.path), m.footprint, m.name.clone())
        })
        .collect();
    let r = deck_core::storage::reconcile_registered_vs_found(&reg, &fnd);
    if json {
        println!("{}", serde_json::to_string_pretty(&r)?);
        return Ok(());
    }
    println!(
        "ok: {} ({:.2} GiB)  total on disk: {:.2} GiB",
        r.ok_count,
        r.ok_bytes as f64 / 1_073_741_824.0,
        r.total_bytes as f64 / 1_073_741_824.0
    );
    for m in &r.missing {
        println!("MISSING (db claims, file gone): {} [{:.2} GiB]", m.path, m.claimed_bytes as f64 / 1_073_741_824.0);
    }
    for o in &r.orphaned {
        println!("ORPHANED (on disk, unregistered): {} [{:.2} GiB]", o.path, o.bytes as f64 / 1_073_741_824.0);
    }
    Ok(())
}

pub(crate) fn runtimes(json: bool) -> Result<()> {
    let rows = deck_core::runtime::list_runtimes();
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    for r in &rows {
        println!(
            "{}{}  :{} / test :{}  [{}]{}",
            r.id,
            if r.custom { " (custom)" } else { "" },
            r.default_port,
            r.test_port,
            r.status,
            if r.capabilities.is_empty() {
                String::new()
            } else {
                format!("  {}", r.capabilities.join(","))
            }
        );
    }
    Ok(())
}

pub(crate) fn make_it_work(model: PathBuf, json: bool) -> Result<()> {
    let meta = if model.is_dir() {
        deck_core::safetensors::open_dir(&model)?
    } else {
        deck_core::gguf::GgufMeta::read(&model)?.to_meta(&model)
    };
    let vram = deck_core::fit::hw_vram().unwrap_or(12 * 1024);
    let cands = deck_core::fitplan::candidates_for(&meta, &deck_core::runtime::all_manifests(), vram);
    if json {
        println!("{}", serde_json::to_string_pretty(&cands)?);
        return Ok(());
    }
    if cands.is_empty() {
        println!("no viable runtime/configuration on this machine (vram={vram} MiB)");
        return Ok(());
    }
    for c in &cands {
        println!("{} [{}]: ctx {} — {}", c.display, c.status, c.max_ctx, c.why);
    }
    println!("\nrecommended: {} (ctx {})", cands[0].display, cands[0].max_ctx);
    Ok(())
}
