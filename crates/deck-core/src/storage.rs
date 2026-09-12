//! Storage reconciliation: the filesystem is the source of truth.
//!
//! The DB must never claim a model is installed when the file is gone, and
//! deleting a DB row must never be confused with deleting bytes. This module
//! is the pure comparator both doors share:
//!
//!   - `registered`: what the DB index claims (path + bytes);
//!   - `found`: what the filesystem scan actually saw;
//!
//! and it reports `ok / missing / orphaned` plus aggregate disk usage so the
//! Storage view can answer "what is consuming disk, who owns it, what would
//! cleanup delete".

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::Serialize;

/// One file the DB claims but the filesystem does not have.
#[derive(Debug, Clone, Serialize)]
pub struct MissingArtifact {
    pub path: String,
    pub name: String,
    pub claimed_bytes: u64,
}

/// One file on disk that no DB row owns (deleted record, manual copy,
/// HF cache entry) — a candidate for import or cleanup.
#[derive(Debug, Clone, Serialize)]
pub struct OrphanedArtifact {
    pub path: String,
    pub name: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Reconciliation {
    /// DB rows whose file exists (healthy).
    pub ok_count: usize,
    pub ok_bytes: u64,
    /// DB rows with no file on disk.
    pub missing: Vec<MissingArtifact>,
    /// Files on disk with no DB row.
    pub orphaned: Vec<OrphanedArtifact>,
    /// Total bytes of `found` (actual disk consumption by scanned models).
    pub total_bytes: u64,
}

fn key(raw: &str, canon: &str) -> String {
    if canon.is_empty() { raw.to_string() } else { canon.to_string() }
}

/// Pure reconciliation over (path, bytes) pairs. Callers canonicalize;
/// both the raw and canonical spellings of each path are accepted as keys
/// so symlinked vaults (`~/models` → elsewhere) still match.
pub fn reconcile_registered_vs_found(
    registered: &[(String, String, u64, String)],
    found: &[(String, String, u64, String)],
) -> Reconciliation {
    // (raw, canonical, bytes, name)
    let mut found_keys: HashSet<String> = HashSet::new();
    let mut found_by_key: HashMap<String, (u64, String, String)> = HashMap::new();
    let mut total_bytes = 0u64;
    for (raw, canon, bytes, name) in found {
        total_bytes += *bytes;
        found_keys.insert(key(raw, canon));
        found_by_key.insert(key(raw, canon), (*bytes, name.clone(), raw.clone()));
    }
    let mut reg_keys: HashSet<String> = HashSet::new();
    for (raw, canon, _, _) in registered {
        reg_keys.insert(key(raw, canon));
    }

    let mut missing = Vec::new();
    let mut ok_count = 0usize;
    let mut ok_bytes = 0u64;
    for (raw, canon, bytes, name) in registered {
        if found_keys.contains(&key(raw, canon)) {
            ok_count += 1;
            ok_bytes += *bytes;
        } else {
            missing.push(MissingArtifact {
                path: raw.clone(),
                name: name.clone(),
                claimed_bytes: *bytes,
            });
        }
    }

    let mut orphaned = Vec::new();
    for (raw, canon, bytes, name) in found {
        if !reg_keys.contains(&key(raw, canon)) {
            orphaned.push(OrphanedArtifact {
                path: raw.clone(),
                name: name.clone(),
                bytes: *bytes,
            });
        }
    }
    missing.sort_by(|a, b| a.path.cmp(&b.path));
    orphaned.sort_by(|a, b| a.path.cmp(&b.path));

    Reconciliation {
        ok_count,
        ok_bytes,
        missing,
        orphaned,
        total_bytes,
    }
}

/// Explicit deletion plan for the "Remove local files?" dialog: exactly
/// which files, how many bytes each, and the total to be freed. No IO here —
/// the caller stats the paths first (missing files are reported, not deleted).
#[derive(Debug, Clone, Serialize)]
pub struct RemovalPlan {
    pub files: Vec<RemovalEntry>,
    pub total_bytes: u64,
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemovalEntry {
    pub path: String,
    pub bytes: u64,
}

pub fn removal_plan(paths: &[PathBuf]) -> RemovalPlan {
    let mut files = Vec::new();
    let mut missing = Vec::new();
    for p in paths {
        match std::fs::metadata(p) {
            Ok(md) => {
                let bytes = if md.is_dir() {
                    walkdir::WalkDir::new(p)
                        .into_iter()
                        .flatten()
                        .filter_map(|e| e.metadata().ok())
                        .filter(|m| m.is_file())
                        .map(|m| m.len())
                        .sum()
                } else {
                    md.len()
                };
                files.push(RemovalEntry {
                    path: p.display().to_string(),
                    bytes,
                });
            }
            Err(_) => missing.push(p.display().to_string()),
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let total_bytes = files.iter().map(|f| f.bytes).sum();
    RemovalPlan {
        files,
        total_bytes,
        missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg(raw: &str, bytes: u64) -> (String, String, u64, String) {
        (raw.into(), format!("c:{raw}"), bytes, raw.into())
    }
    fn found(raw: &str, bytes: u64) -> (String, String, u64, String) {
        (raw.into(), format!("c:{raw}"), bytes, raw.into())
    }

    #[test]
    fn db_claim_without_file_is_missing() {
        let r = reconcile_registered_vs_found(&[reg("/m/a.gguf", 10)], &[]);
        assert_eq!(r.ok_count, 0);
        assert_eq!(r.missing.len(), 1);
        assert_eq!(r.missing[0].claimed_bytes, 10);
        assert!(r.orphaned.is_empty());
    }

    #[test]
    fn file_without_row_is_orphaned() {
        let r = reconcile_registered_vs_found(&[], &[found("/m/stray.gguf", 7)]);
        assert!(r.missing.is_empty());
        assert_eq!(r.orphaned.len(), 1);
        assert_eq!(r.total_bytes, 7);
    }

    #[test]
    fn healthy_row_is_ok_and_counted() {
        let r = reconcile_registered_vs_found(
            &[reg("/m/a.gguf", 10)],
            &[found("/m/a.gguf", 10)],
        );
        assert_eq!(r.ok_count, 1);
        assert_eq!(r.ok_bytes, 10);
        assert!(r.missing.is_empty() && r.orphaned.is_empty());
    }

    #[test]
    fn multi_artifact_selection_plans_exact_bytes() {
        let dir = std::env::temp_dir().join(format!("deck-rm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("q3.gguf");
        let b = dir.join("q4.gguf");
        std::fs::write(&a, vec![0u8; 1024]).unwrap();
        std::fs::write(&b, vec![0u8; 2048]).unwrap();
        let plan = removal_plan(&[a.clone(), b.clone(), dir.join("gone.gguf")]);
        assert_eq!(plan.total_bytes, 3072);
        assert_eq!(plan.missing.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
