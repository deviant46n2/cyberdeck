//! Model inventory: scanning roots, indexing the sqlite store, dedup, delete.

use serde::Serialize;

/// Check if a blob file starts with the GGUF magic bytes (first 4 bytes = "GGUF").
/// Ollama stores model files as content-addressable blobs without file extensions,
/// so we need to detect GGUF format by reading the magic header.
fn is_gguf_blob(path: &str) -> bool {
    if let Ok(mut f) = std::fs::File::open(path) {
        let mut header = [0u8; 4];
        if std::io::Read::read_exact(&mut f, &mut header).is_ok() {
            return header == *b"GGUF";
        }
    }
    false
}

#[derive(Serialize)]
pub struct ModelRow {
    pub name: String,
    pub quant: Option<String>,
    pub arch: Option<String>,
    pub ctx_train: u64,
    pub footprint_gib: f64,
    pub path: String,
}

#[derive(Serialize)]
pub struct DupRow {
    pub identity: String,
    pub wasted_gib: f64,
    pub members: Vec<String>,
}

#[derive(Serialize)]
pub struct ScanResult {
    pub indexed: usize,
    pub pruned: usize,
    pub models: Vec<ModelRow>,
    pub dups: Vec<DupRow>,
}

fn gib(bytes: u64) -> f64 {
    bytes as f64 / 1_073_741_824.0
}

/// Refresh the index from configured roots; returns the fresh inventory.
pub fn scan() -> anyhow::Result<ScanResult> {
    let roots = deck_core::scanner::default_roots();
    let mut models = deck_core::scanner::scan(&roots)?;

    // Also index ollama models — they live as blobs under /var/lib/ollama/blobs
    // which is not a standard scanner root.
    if let Ok(ollama) = deck_feeds::ollama_models() {
        for o in &ollama {
            // Check if this blob is actually a GGUF file (some ollama blobs
            // are just manifest JSON or config files, not model weights).
            if o.path.ends_with(".gguf") || is_gguf_blob(&o.path) {
                let existing: std::collections::HashSet<String> = models
                    .iter()
                    .map(|m| {
                        std::fs::canonicalize(&m.path)
                            .ok()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| m.path.display().to_string())
                    })
                    .collect();
                let canonical = std::fs::canonicalize(&o.path)
                    .ok()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| o.path.clone());
                if !existing.contains(&canonical) {
                    // Parse GGUF header to extract model metadata.
                    let meta = if let Ok(gguf_meta) = deck_core::gguf::GgufMeta::read(&o.path) {
                        gguf_meta.to_meta(&std::path::PathBuf::from(&o.path))
                    } else {
                        deck_core::model::ModelMeta {
                            path: std::path::PathBuf::from(o.path.clone()),
                            format: deck_core::model::ModelFormat::Gguf,
                            name: o.name.clone(),
                            arch: None,
                            quant: None,
                            params: None,
                            n_layers: None,
                            n_embd: None,
                            ctx_train: None,
                            vocab: None,
                            weight_size: o.size,
                            footprint: o.size,
                        }
                    };
                    models.push(meta);
                }
            }
        }
    }

    let db = deck_core::store::default_db_path();
    let mut conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_profile_schema(&conn)?;
    let indexed = deck_core::store::upsert_many(&mut conn, &models)?;
    let keep: Vec<String> = models
        .iter()
        .map(|m| m.path.display().to_string())
        .collect();
    let pruned = deck_core::store::prune(&conn, &keep)?;
    let dups = deck_core::store::duplicates(&conn)?
        .into_iter()
        .map(|d| DupRow {
            identity: d.identity,
            wasted_gib: gib(d.wasted_bytes),
            members: d
                .members
                .iter()
                .map(|m| m.path.display().to_string())
                .collect(),
        })
        .collect();
    let models = models
        .into_iter()
        .map(|m| ModelRow {
            name: m.name,
            quant: m.quant,
            arch: m.arch,
            ctx_train: m.ctx_train.unwrap_or(0),
            footprint_gib: gib(m.footprint),
            path: m.path.display().to_string(),
        })
        .collect();
    Ok(ScanResult {
        indexed,
        pruned,
        models,
        dups,
    })
}

/// Index a specific set of landed files (e.g. a completed download or whole
/// shard set) without a full-tree rescan. Deliberately skips `prune`: pruning
/// over a subset would delete unrelated models — that stays a full-scan
/// concern. Returns how many paths were newly indexed.
pub fn index_downloaded(paths: &[String]) -> anyhow::Result<usize> {
    let paths: Vec<&std::path::Path> = paths.iter().map(std::path::Path::new).collect();
    let models = deck_core::scanner::scan_paths(&paths)?;
    let db = deck_core::store::default_db_path();
    let mut conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_profile_schema(&conn)?;
    deck_core::store::upsert_many(&mut conn, &models)
}

pub fn list_models() -> anyhow::Result<Vec<ModelRow>> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    Ok(deck_core::store::list(&conn)?
        .into_iter()
        .map(|m| ModelRow {
            name: m.name,
            quant: m.quant,
            arch: m.arch,
            ctx_train: m.ctx_train.unwrap_or(0),
            footprint_gib: gib(m.footprint),
            path: m.path.display().to_string(),
        })
        .collect())
}

/// Scan the index for duplicate models (same arch + quant + param count).
/// Returns groups of duplicate models so the UI can display them.
pub fn dedup() -> anyhow::Result<Vec<DupRow>> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    Ok(deck_core::store::duplicates(&conn)?
        .into_iter()
        .map(|d| DupRow {
            identity: d.identity,
            wasted_gib: gib(d.wasted_bytes),
            members: d
                .members
                .iter()
                .map(|m| m.path.display().to_string())
                .collect(),
        })
        .collect())
}

/// Delete a model from the index. If `delete_file` is true the file is removed
/// from disk as well (for local GGUF/safetensors only — skip for ollama/hub
/// paths that the user should manage externally).
pub fn delete_model(path: &str, delete_file: bool) -> anyhow::Result<usize> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::delete_model(&conn, path, delete_file)
}

/// Delete all duplicate models in a group except the cheapest one.
/// Returns the number of models removed.
pub fn dedup_delete(identity: &str, delete_file: bool) -> anyhow::Result<usize> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::dedup_delete(&conn, identity, delete_file)
}
