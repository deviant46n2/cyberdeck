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
        home.join("models"),
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
