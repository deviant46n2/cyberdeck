//! Filesystem scanner. Walks configured roots, classifies each entry as a
//! GGUF model or a safetensors model-dir, and parses metadata without ever
//! loading weights.

use std::collections::HashSet;
use std::path::PathBuf;

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
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
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

pub fn scan(roots: &[PathBuf]) -> Result<Vec<ModelMeta>> {
    let mut found = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

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
            let path = entry.path();
            if entry.file_type().is_dir() {
                if path.join("config.json").exists() && safetensors::is_model_dir(path) {
                    if let Ok(meta) = safetensors::open_dir(path) {
                        found.push(meta);
                    }
                }
            } else if path.extension().and_then(|e| e.to_str()) == Some("gguf") {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.contains("ggml-vocab") {
                    continue;
                }
                if let Some(meta) = entry.metadata().ok().filter(|m| m.len() > 1_000_000) {
                    if meta.is_file() {
                        if let Ok(g) = crate::gguf::GgufMeta::read(path) {
                            found.push(g.to_meta(path));
                        }
                    }
                }
            }
        }
    }

    // Deduplicate by canonical path: roots can overlap (HOME covers ~/models,
    // ~/.cache/huggingface/hub), which would otherwise double-list models.
    found.retain(|m| {
        let key = std::fs::canonicalize(&m.path)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| m.path.display().to_string());
        seen.insert(key)
    });
    found.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(found)
}
