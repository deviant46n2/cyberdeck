//! Filesystem scanner. Walks configured roots, classifies each entry as a
//! GGUF model or a safetensors model-dir, and parses metadata without ever
//! loading weights.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::model::ModelMeta;
use crate::safetensors;

const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    "build-dir",
    "dist",
    "vendor",
    "Clone",
];

pub fn default_roots() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    vec![
        home.clone(),
        crate::store::models_dir(),
        home.join(".cache/huggingface/hub"),
    ]
}

/// True if a walked entry should be descended into / reported.
fn keep(entry: &walkdir::DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    if name.starts_with('.') && name != ".cache" {
        return false;
    }
    if SKIP_DIRS.contains(&name.as_ref()) {
        return false;
    }
    let p = entry.path().to_string_lossy();
    if p.contains("/Trash/") || p.contains(".Trash") {
        return false;
    }
    true
}

/// Classify a single filesystem entry as a model: a safetensors model dir (a
/// dir carrying config.json + weights) or a GGUF file larger than 1 MiB.
fn classify(path: &Path, metadata: &std::fs::Metadata) -> Option<ModelMeta> {
    if metadata.is_dir() {
        if path.join("config.json").exists() && safetensors::is_model_dir(path) {
            return safetensors::open_dir(path).ok();
        }
        return None;
    }
    if !metadata.is_file() {
        return None;
    }
    if path.extension().and_then(|e| e.to_str()) == Some("gguf") {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.contains("ggml-vocab") {
            return None;
        }
        if metadata.len() > 1_000_000 {
            // Parses the GGUF header only — never loads weights.
            if let Ok(g) = crate::gguf::GgufMeta::read(path) {
                return Some(g.to_meta(path));
            }
        }
    }
    None
}

fn dedup_and_sort(mut found: Vec<ModelMeta>) -> Vec<ModelMeta> {
    let mut seen: HashSet<String> = HashSet::new();
    // Deduplicate by canonical path: roots can overlap (HOME covers ~/models,
    // ~/.cache/huggingface/hub), which would otherwise double-list models.
    found.retain(|m| {
        let key = std::fs::canonicalize(&m.path)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| m.path.display().to_string());
        seen.insert(key)
    });
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

pub fn scan(roots: &[PathBuf]) -> Result<Vec<ModelMeta>> {
    let mut found = Vec::new();

    for root in roots {
        if !root.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(root)
            .max_depth(10)
            .into_iter()
            .filter_entry(keep)
            .flatten()
        {
            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if let Some(meta) = classify(entry.path(), &metadata) {
                found.push(meta);
            }
        }
    }

    Ok(dedup_and_sort(found))
}

/// Index an explicit set of paths (files that just landed on disk) without
/// walking any roots. Used by the downloader so a finished transfer shows up
/// in the vault immediately, with no full-tree rescan.
pub fn scan_paths(paths: &[&Path]) -> Result<Vec<ModelMeta>> {
    let mut found = Vec::new();
    for path in paths {
        let metadata = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if let Some(meta) = classify(path, &metadata) {
            found.push(meta);
        }
    }
    Ok(dedup_and_sort(found))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Apparent size of each sparse duplicate — large so the wasted-bytes
    /// assertion (>10 GiB) is meaningful without actually writing gigabytes.
    const DUP_SIZE: u64 = 11 * 1024 * 1024 * 1024; // 11 GiB
    /// GGUF `general.file_type` code for Q4_K_M (see gguf::file_type_name).
    const FILE_TYPE_Q4_K_M: u32 = 15;

    /// Write a minimal, valid GGUF header onto `path` then extend it to
    /// `apparent_len` with a sparse tail — `metadata.len()` reports `apparent_len`
    /// (so classify + footprint see a big model) while ~0 disk is used.
    fn write_mini_gguf(path: &Path, arch: &str, apparent_len: u64) {
        fn wu32(b: &mut Vec<u8>, x: u32) {
            b.extend_from_slice(&x.to_le_bytes());
        }
        fn wu64(b: &mut Vec<u8>, x: u64) {
            b.extend_from_slice(&x.to_le_bytes());
        }
        fn wstr(b: &mut Vec<u8>, s: &str) {
            wu64(b, s.len() as u64);
            b.extend_from_slice(s.as_bytes());
        }
        // magic + version 3 + tensor_count 0 + 3 KVs (arch, file_type, params)
        let mut b = Vec::new();
        wu32(&mut b, 0x4655_4747);
        wu32(&mut b, 3);
        wu64(&mut b, 0);
        wu64(&mut b, 3);
        wstr(&mut b, "general.architecture");
        wu32(&mut b, 8 /* String */);
        wstr(&mut b, arch);
        wstr(&mut b, "general.file_type");
        wu32(&mut b, 4 /* U32 */);
        wu32(&mut b, FILE_TYPE_Q4_K_M);
        wstr(&mut b, "general.parameter_count");
        wu32(&mut b, 5 /* I32 */);
        wu32(&mut b, 1_800_000_000); // ~1.8B params
        std::fs::write(path, &b).unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_len(apparent_len).unwrap();
    }

    /// Scan a temp root containing a duplicated GGUF pair and assert the
    /// dedup pass surfaces the duplicate with >10 GiB wasted. The whole
    /// pipeline is injected (explicit root + temp DB) so it needs no host
    /// state and runs in CI.
    #[test]
    fn scan_dup_surfaces_wasted_over_10gib() {
        let root = std::env::temp_dir().join(format!("deck-core-scan-dup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // Two same-identity models in different dirs — the ~/models-vs-hub-cache
        // shape the original host test was pointing at, now synthetic.
        let a = root.join("copy-a");
        let b = root.join("copy-b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        write_mini_gguf(&a.join("model.gguf"), "qwen3", DUP_SIZE);
        write_mini_gguf(&b.join("model.gguf"), "qwen3", DUP_SIZE);

        let models = scan(std::slice::from_ref(&root)).expect("scan fixture");
        assert_eq!(models.len(), 2, "both duplicate copies should be indexed");

        // Inject a temp DB; dedup groups by identity (arch+format+size-bucket).
        let db_path = root.join("dup.sqlite");
        let mut conn = crate::store::open(&db_path).expect("open temp db");
        crate::store::ensure_profile_schema(&conn).unwrap();
        crate::store::upsert_many(&mut conn, &models).expect("upsert");
        let dups = crate::store::duplicates(&conn).expect("duplicates");

        let gib = |bytes: u64| bytes as f64 / 1_073_741_824.0;
        assert_eq!(dups.len(), 1, "one duplicate identity expected");
        assert!(
            gib(dups[0].wasted_bytes) > 10.0,
            "wasted {:.1} GiB should exceed the >10 GiB bar",
            gib(dups[0].wasted_bytes)
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
