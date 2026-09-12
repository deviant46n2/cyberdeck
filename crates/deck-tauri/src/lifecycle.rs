//! Model lifecycle doors: unambiguous forget vs remove, storage
//! reconciliation, runtime registry, and fit candidates.
//!
//! Naming is the contract (§6, §23 of the redesign brief):
//!   - `forget_model` = drop the library record, FILES STAY on disk;
//!   - `remove_model_files` = delete explicit artifact files, report bytes;
//!   - `storage_reconcile` = DB-vs-disk truth (ok / missing / orphaned).

use serde::Serialize;

use deck_core::storage::removal_plan;

/// Forget a model: remove the index row, NEVER touch the file.
/// The next scan will surface the file as orphaned (re-importable).
pub fn forget_model(path: &str) -> anyhow::Result<crate::scan::DeleteResult> {
    crate::scan::delete_model(path, false)
}

#[derive(Serialize)]
pub struct RemovalReport {
    pub files: Vec<RemovalFile>,
    pub total_bytes: u64,
    pub total_gib: f64,
    pub missing: Vec<String>,
    pub rows_removed: usize,
}

#[derive(Serialize)]
pub struct RemovalFile {
    pub path: String,
    pub bytes: u64,
    pub gib: f64,
}

/// Remove explicit local files: shows exact bytes, then deletes.
/// Missing paths are reported, never fatal. DB rows for deleted files are
/// dropped; rows for missing paths are left for `forget_model`.
pub fn remove_model_files(paths: &[String]) -> anyhow::Result<RemovalReport> {
    let owned: Vec<std::path::PathBuf> = paths.iter().map(std::path::PathBuf::from).collect();
    let plan = removal_plan(&owned);
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    let mut rows_removed = 0usize;
    for f in &plan.files {
        rows_removed += deck_core::store::delete_model(&conn, &f.path, false)?;
        // Delete bytes AFTER the row drop is staged — a crash between the two
        // leaves an orphaned file (re-importable), never a phantom DB row.
        if std::path::Path::new(&f.path).is_dir() {
            let _ = std::fs::remove_dir_all(&f.path);
        } else {
            let _ = std::fs::remove_file(&f.path);
        }
    }
    Ok(RemovalReport {
        files: plan
            .files
            .into_iter()
            .map(|f| RemovalFile {
                gib: f.bytes as f64 / 1_073_741_824.0,
                path: f.path,
                bytes: f.bytes,
            })
            .collect(),
        total_bytes: plan.total_bytes,
        total_gib: plan.total_bytes as f64 / 1_073_741_824.0,
        missing: plan.missing,
        rows_removed,
    })
}

#[derive(Serialize)]
pub struct StorageReport {
    pub ok_count: usize,
    pub ok_gib: f64,
    pub total_gib: f64,
    pub missing: Vec<deck_core::storage::MissingArtifact>,
    pub orphaned: Vec<deck_core::storage::OrphanedArtifact>,
}

/// Filesystem-as-truth: full rescan vs DB index → ok/missing/orphaned.
pub fn storage_reconcile() -> anyhow::Result<StorageReport> {
    let mut roots = deck_core::scanner::default_roots();
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    roots.extend(deck_core::store::scan_dirs(&conn)?);
    let found = deck_core::scanner::scan(&roots)?;
    let registered = deck_core::store::list(&conn)?;
    drop(conn);

    let canon_of = |p: &std::path::Path| {
        std::fs::canonicalize(p)
            .map(|c| c.display().to_string())
            .unwrap_or_else(|_| p.display().to_string())
    };
    let reg: Vec<(String, String, u64, String)> = registered
        .iter()
        .map(|m| {
            let raw = m.path.display().to_string();
            (raw.clone(), canon_of(&m.path), m.footprint, m.name.clone())
        })
        .collect();
    let fnd: Vec<(String, String, u64, String)> = found
        .iter()
        .map(|m| {
            let raw = m.path.display().to_string();
            (raw.clone(), canon_of(&m.path), m.footprint, m.name.clone())
        })
        .collect();
    let r = deck_core::storage::reconcile_registered_vs_found(&reg, &fnd);
    Ok(StorageReport {
        ok_count: r.ok_count,
        ok_gib: r.ok_bytes as f64 / 1_073_741_824.0,
        total_gib: r.total_bytes as f64 / 1_073_741_824.0,
        missing: r.missing,
        orphaned: r.orphaned,
    })
}

#[derive(Serialize)]
pub struct RuntimeRow {
    pub id: String,
    pub display: String,
    pub status: String,
    pub default_port: u16,
    pub test_port: u16,
    pub formats: Vec<String>,
    pub capabilities: Vec<String>,
    pub custom: bool,
    /// A binary resolves on this machine (no spawning — filesystem/PATH only).
    pub installed: bool,
    pub bin: Option<String>,
}

/// Merged runtime registry (builtins + `~/.local/share/cyberdeck/runtimes/*.json`),
/// each row annotated with whether it is actually installed.
pub fn runtime_list() -> Vec<RuntimeRow> {
    let builtin: std::collections::HashSet<String> = deck_core::profile::Engine::all()
        .into_iter()
        .map(|e| e.store_id().to_string())
        .collect();
    let dirs = deck_engines::availability::system_path_dirs();
    let conn = deck_core::store::open(&deck_core::store::default_db_path()).ok();
    deck_core::runtime::all_manifests()
        .into_iter()
        .map(|m| {
            let ov = conn
                .as_ref()
                .and_then(|c| deck_core::store::get_engine_bin(c, &m.id).ok().flatten());
            let a = deck_engines::availability::availability_for(&m, ov.as_deref(), &dirs);
            RuntimeRow {
                id: m.id.clone(),
                display: m.display.clone(),
                status: m.status.tag().to_string(),
                default_port: m.default_port,
                test_port: m.test_port,
                formats: m.formats.clone(),
                capabilities: m.capabilities.clone(),
                custom: !builtin.contains(&m.id),
                installed: a.installed,
                bin: a.bin.map(|p| p.display().to_string()),
            }
        })
        .collect()
}

/// Explicit `<bin> --version` probe for one runtime. Opt-in only: never called
/// implicitly, because a manifest pointing at a daemon could start serving.
pub fn runtime_probe_version(id: &str) -> anyhow::Result<Option<String>> {
    let m = deck_core::runtime::all_manifests()
        .into_iter()
        .find(|x| x.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown runtime '{id}'"))?;
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    let ov = deck_core::store::get_engine_bin(&conn, &m.id).ok().flatten();
    let dirs = deck_engines::availability::system_path_dirs();
    let Some(bin) = deck_engines::availability::find_bin(&m.bin_candidates, &dirs, ov.as_deref())
    else {
        return Ok(None);
    };
    Ok(deck_engines::availability::probe_version(&bin))
}

/// Fit candidates for one artifact across all compatible runtimes, best first.
/// Pure planning — launches nothing.
pub fn fit_candidates(model_path: &str) -> anyhow::Result<Vec<deck_core::fitplan::FitCandidate>> {
    let p = std::path::Path::new(model_path);
    let meta = if p.is_dir() {
        deck_core::safetensors::open_dir(p)?
    } else {
        deck_core::gguf::GgufMeta::read(p)?.to_meta(p)
    };
    let vram = deck_core::fit::hw_vram().unwrap_or(12 * 1024);
    let mut cands = deck_core::fitplan::candidates_for(
        &meta,
        &deck_core::runtime::all_manifests(),
        vram,
    );
    // Empirical beats estimated: attach the freshest successful measurement
    // per (artifact, runtime) so the UI can show TESTED instead of a guess —
    // and mark what is actually installed so a recommendation can never point
    // at a missing binary.
    if let Ok(conn) = deck_core::store::open(&deck_core::store::default_db_path()) {
        for c in &mut cands {
            c.tested = deck_core::store::latest_tested(&conn, model_path, &c.runtime_id)
                .ok()
                .flatten();
        }
        deck_core::fitplan::attach_availability(
            &conn,
            &deck_core::runtime::system_path_dirs(),
            &mut cands,
        );
    }
    Ok(cands)
}

/// Measure candidate configs for one model and persist the trials, so fit
/// candidates can render TESTED. Long-running (one model load per trial) —
/// callers run this off the UI thread.
pub fn discover(
    model_path: &str,
    runtimes: Option<Vec<String>>,
    variants: u32,
    ctx: Option<u32>,
    runs: u32,
    max_tokens: u32,
) -> anyhow::Result<deck_engines::discover::DiscoveryReport> {
    if !std::path::Path::new(model_path).exists() {
        anyhow::bail!("model not found on disk: {model_path}");
    }
    let (mut plans, skipped) = deck_engines::discover::plan_trials(
        std::path::Path::new(model_path),
        runtimes.as_deref(),
        variants,
        ctx,
    )
    .map_err(anyhow::Error::msg)?;
    if plans.is_empty() {
        anyhow::bail!("nothing measurable — every candidate was skipped");
    }
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_engines::discover::apply_engine_bins(&mut plans, &|p| {
        deck_core::store::get_engine_bin(&conn, &p.runtime_key())
            .ok()
            .flatten()
            .map(std::path::PathBuf::from)
    });
    let mut all_rows = Vec::new();
    let mut results = Vec::new();
    for plan in &plans {
        let res = deck_engines::discover::run_trial(
            plan,
            max_tokens,
            runs,
            std::time::Duration::from_secs(180),
            None,
        );
        all_rows.extend(res.rows.iter().cloned());
        results.push(res);
    }
    let ids = deck_core::store::persist_matrix_batch(&conn, &all_rows)?;
    Ok(deck_engines::discover::summarize(
        model_path,
        &results,
        &skipped,
        &ids,
    ))
}

#[derive(Serialize)]
pub struct InstallReport {
    pub runtime: String,
    /// True when the recipe is manual guidance (nothing downloaded).
    pub manual: bool,
    pub url: Option<String>,
    pub version: Option<String>,
    pub bin: Option<String>,
    pub summary: String,
}

/// Install a runtime backend from its manifest recipe, or return manual
/// guidance. Long-running (downloads) — callers run this off the UI thread.
pub fn runtime_install(
    id: &str,
    tag: Option<&str>,
    dry_run: bool,
) -> anyhow::Result<InstallReport> {
    use deck_engines::install::{Resolved, execute, resolve_recipe};
    let m = deck_core::runtime::all_manifests()
        .into_iter()
        .find(|x| x.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown runtime '{id}'"))?;
    match resolve_recipe(&m, tag)? {
        Resolved::Manual { url, instructions } => Ok(InstallReport {
            runtime: id.into(),
            manual: true,
            url: Some(url.clone()),
            version: None,
            bin: None,
            summary: if instructions.is_empty() {
                format!("manual install: {url}")
            } else {
                format!("manual install: {url} — {instructions}")
            },
        }),
        Resolved::Download(plan) => {
            if dry_run {
                return Ok(InstallReport {
                    runtime: id.into(),
                    manual: false,
                    url: Some(plan.url.clone()),
                    version: Some(plan.version.clone()),
                    bin: None,
                    summary: format!(
                        "dry-run: would fetch {} → {}",
                        plan.url, plan.bin_path
                    ),
                });
            }
            let dest = deck_core::runtime::runtime_install_dir(id);
            let bin = execute(&plan, &dest, &|s| eprintln!("[install] {s}"))?;
            let db = deck_core::store::default_db_path();
            let conn = deck_core::store::open(&db)?;
            deck_core::store::set_engine_bin(&conn, id, &bin.display().to_string())?;
            Ok(InstallReport {
                runtime: id.into(),
                manual: false,
                url: Some(plan.url),
                version: Some(plan.version),
                bin: Some(bin.display().to_string()),
                summary: format!("installed {id} → {}", bin.display()),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lists_four_builtins_without_customs() {
        let rows = runtime_list();
        assert!(rows.len() >= 4);
        assert!(rows.iter().any(|r| r.id == "llamacpp" && r.status == "stable"));
        assert!(rows.iter().any(|r| r.id == "freetoken"));
    }

    #[test]
    fn candidates_require_a_real_file() {
        // Missing path must error, never return a phantom plan.
        assert!(fit_candidates("/no/such/model.gguf").is_err());
    }
}
