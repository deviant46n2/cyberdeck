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
    let mut roots = deck_core::scanner::default_roots();
    // Merge user-configured extra scan directories.
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    let extra = deck_core::store::scan_dirs(&conn)?;
    drop(conn);
    roots.extend(extra);
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
                            n_head: None,
                            n_head_kv: None,
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

/// Outcome of a delete request, for the UI/CLI to surface honestly.
#[derive(Serialize)]
pub struct DeleteResult {
    pub rows: usize,
    pub file_deleted: bool,
    pub message: String,
}

/// True when a path lives in ollama's content-addressed blob store, whose
/// files are root-owned (`ollama` user) and cannot be removed by the app user.
fn is_ollama_blob(path: &str) -> bool {
    path.contains("/ollama/blobs/")
}

/// Delete a model from the index. If `delete_file` is true the file is removed
/// from disk as well. Ollama blobs are deleted through the daemon (`ollama rm
/// <tag>`), which owns the files; local GGUFs are unlinked directly. Whatever
/// cannot be removed is reported back instead of swallowed, because a survivor
/// on disk is re-indexed by the next scan and only an honest message explains
/// why it came back.
pub fn delete_model(path: &str, delete_file: bool) -> anyhow::Result<DeleteResult> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    delete_model_with(&conn, path, delete_file)
}

/// Testable core of [`delete_model`]: file work against a caller-provided
/// connection. The row is dropped regardless of file outcome.
fn delete_model_with(
    conn: &rusqlite::Connection,
    path: &str,
    delete_file: bool,
) -> anyhow::Result<DeleteResult> {
    let (file_deleted, message) = if !delete_file {
        (false, "index entry removed; file kept on disk".to_string())
    } else if is_ollama_blob(path) {
        match deck_feeds::ollama_delete_blob(path) {
            deck_feeds::OllamaDeleteOutcome::Removed { blob_gone: true } => {
                (true, "ollama model and its blob are gone".to_string())
            }
            deck_feeds::OllamaDeleteOutcome::Removed { blob_gone: false } => (
                false,
                "ollama tag removed, but the shared blob is still on disk and stays vaulted until every referencing model is removed".to_string(),
            ),
            deck_feeds::OllamaDeleteOutcome::NoTag => (
                false,
                format!(
                    "no installed ollama model references this blob; delete the root-owned file once as root: sudo rm {path}"
                ),
            ),
            deck_feeds::OllamaDeleteOutcome::DaemonUnavailable => (
                false,
                format!("ollama is not running (or rejected the delete); start it, or remove the file once as root: sudo rm {path}"),
            ),
        }
    } else {
        let p = std::path::Path::new(path);
        let result = if p.is_file() {
            std::fs::remove_file(p)
        } else if p.is_dir() {
            std::fs::remove_dir_all(p)
        } else {
            Ok(())
        };
        match result {
            Ok(_) => (true, "file removed".to_string()),
            Err(e) => (
                false,
                format!("file could not be removed: {e}; it is gone from the Vault but will be re-indexed by the next scan until removed manually"),
            ),
        }
    };

    let rows = deck_core::store::delete_model(conn, path, false)?;

    Ok(DeleteResult {
        rows,
        file_deleted,
        message,
    })
}

/// Delete all duplicate models in a group except the cheapest one.
/// Returns the number of models removed.
pub fn dedup_delete(identity: &str, delete_file: bool) -> anyhow::Result<usize> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::dedup_delete(&conn, identity, delete_file)
}

// ---------------------------------------------------- extra scan directories

/// List user-configured extra scan directories.
pub fn list_scan_dirs() -> anyhow::Result<Vec<String>> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    Ok(deck_core::store::scan_dirs(&conn)?
        .iter()
        .map(|p| p.display().to_string())
        .collect())
}

/// Add a directory to the extra scan list.
pub fn add_scan_dir(path: &str) -> anyhow::Result<()> {
    let p = std::path::Path::new(path);
    if !p.is_absolute() {
        anyhow::bail!("Path must be absolute");
    }
    if !p.is_dir() {
        anyhow::bail!("Not a directory: {path}");
    }
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::add_scan_dir(&conn, path)
}

/// Remove a directory from the extra scan list.
pub fn remove_scan_dir(path: &str) -> anyhow::Result<bool> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::remove_scan_dir(&conn, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_gguf(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "deck-tauri-delete-test-{}-{tag}.gguf",
            std::process::id()
        ));
        std::fs::write(&p, b"GGUF").unwrap();
        p
    }

    fn db_with_model(path: &str) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        deck_core::store::ensure_models_table(&conn).unwrap();
        conn.execute(
            "INSERT INTO models (path, format) VALUES (?1, 'gguf')",
            [path],
        )
        .unwrap();
        conn
    }

    #[test]
    fn local_delete_unlinks_file_and_drops_row() {
        let f = temp_gguf("local");
        let conn = db_with_model(f.to_str().unwrap());
        let r = delete_model_with(&conn, f.to_str().unwrap(), true).unwrap();
        assert!(r.file_deleted);
        assert_eq!(r.rows, 1);
        assert!(!f.exists(), "file must be gone");
    }

    #[test]
    fn delete_without_file_keeps_disk_and_drops_row() {
        let f = temp_gguf("keep");
        let conn = db_with_model(f.to_str().unwrap());
        let r = delete_model_with(&conn, f.to_str().unwrap(), false).unwrap();
        assert!(!r.file_deleted);
        assert_eq!(r.rows, 1);
        assert!(f.exists(), "file stays when delete_file is false");
        std::fs::remove_file(&f).unwrap();
    }

    #[test]
    fn unknown_path_returns_zero_rows() {
        let conn = db_with_model("/nope/missing.gguf");
        let r = delete_model_with(
            &conn,
            std::env::temp_dir().join("no-such-file.gguf").to_str().unwrap(),
            false,
        )
        .unwrap();
        assert_eq!(r.rows, 0);
        assert!(!r.file_deleted);
    }

    #[test]
    fn ollama_blob_paths_are_routed_away_from_local_unlink() {
        assert!(is_ollama_blob(
            "/var/lib/ollama/blobs/sha256-abc123"
        ));
        assert!(!is_ollama_blob("/home/me/models/qwen.gguf"));
    }
}
