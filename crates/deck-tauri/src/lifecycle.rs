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
}

/// Merged runtime registry (builtins + `~/.local/share/cyberdeck/runtimes/*.json`).
pub fn runtime_list() -> Vec<RuntimeRow> {
    deck_core::runtime::list_runtimes()
        .into_iter()
        .map(|r| RuntimeRow {
            id: r.id,
            display: r.display,
            status: r.status,
            default_port: r.default_port,
            test_port: r.test_port,
            formats: r.formats,
            capabilities: r.capabilities,
            custom: r.custom,
        })
        .collect()
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
    Ok(deck_core::fitplan::candidates_for(
        &meta,
        &deck_core::runtime::all_manifests(),
        vram,
    ))
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
