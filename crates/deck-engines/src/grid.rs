//! Grid plumbing shared by both doors (CLI and Tauri): turns raw user input
//! (a model path, engine names, Ollama ids, `label=prompt` tasks) into the
//! flat `MatrixCell` list + task pairs that `matrix` / `compare` execute.
//!
//! This is the single source of truth for "what counts as a cell" so the CLI
//! and the app can never disagree about the grid they run.

use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;

use deck_core::profile::{Engine, ModelSource};

use crate::matrix::MatrixCell;

/// Merge explicitly-passed per-engine binaries with the per-engine store
/// config, so a machine configured once needs no repeated `--bin` flags. Explicit
/// values win; an engine with no explicit bin falls back to its configured path
/// (None = the engine's default resolution). Shared by the CLI and the Compare
/// UI tab so both doors resolve binaries identically.
pub fn resolve_bins(explicit: &std::collections::HashMap<Engine, PathBuf>) -> std::collections::HashMap<Engine, PathBuf> {
    let mut bins = explicit.clone();
    if let Ok(conn) = deck_core::store::open(&deck_core::store::default_db_path()) {
        for e in [Engine::LlamaCpp, Engine::FreeToken, Engine::Ollama] {
            if !bins.contains_key(&e)
                && let Ok(Some(b)) = deck_core::store::get_engine_bin(&conn, e.store_id())
            {
                bins.insert(e, PathBuf::from(b));
            }
        }
    }
    bins
}

/// Top-level `*.gguf` files of a directory, or the single file itself.
pub fn quant_files(model: &Path) -> Result<Vec<std::path::PathBuf>> {
    if model.is_dir() {
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(model)
            .map_err(|e| anyhow::anyhow!("reading {model:?}: {e}"))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "gguf"))
            .collect();
        if files.is_empty() {
            anyhow::bail!("no *.gguf files found in {model:?}");
        }
        files.sort();
        Ok(files)
    } else if model.extension().is_some_and(|x| x == "gguf") {
        Ok(vec![model.to_path_buf()])
    } else {
        anyhow::bail!("{model:?} is neither a directory nor a .gguf file")
    }
}

/// Parse repeatable `"label=prompt"` tasks.
pub fn parse_tasks(tasks: &[String]) -> Result<Vec<(String, String)>> {
    if tasks.is_empty() {
        anyhow::bail!("pass at least one task as \"label=prompt\"");
    }
    tasks
        .iter()
        .map(|t| {
            t.split_once('=')
                .map(|(l, p)| (l.trim().to_string(), p.trim().to_string()))
                .filter(|(l, p)| !l.is_empty() && !p.is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!("task must be \"label=prompt\", got {t:?}")
                })
        })
        .collect()
}

/// Build the flat grid: local quants × local-source engines, plus each
/// requested Ollama id × ollama. Shared by the CLI (`matrix` / `compare`) and
/// the Compare UI tab so both doors run the identical grid.
///
/// `engines` are `Engine`s already parsed by the caller (matching the CLI's
/// `--engines`). Local-path engines only: an engine that serves Ollama's own
/// store cannot load arbitrary `~/models` GGUFs — that constraint is enforced
/// here, structurally.
pub fn build_cells(
    model: &Path,
    engines: &[Engine],
    ollama: &[String],
) -> Result<Vec<MatrixCell>> {
    for e in engines {
        if e.model_source() != ModelSource::LocalPath {
            anyhow::bail!(
                "{} cannot serve arbitrary ~/models GGUFs — put Ollama models on --ollama",
                e.descriptor().display
            );
        }
    }

    let mut cells: Vec<MatrixCell> = Vec::new();
    for f in quant_files(model)? {
        let label = f
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| f.display().to_string());
        for e in engines {
            cells.push(MatrixCell {
                engine: *e,
                model_id: f.display().to_string(),
                display: label.clone(),
            });
        }
    }
    for oid in ollama {
        cells.push(MatrixCell {
            engine: Engine::Ollama,
            model_id: oid.clone(),
            display: oid.clone(),
        });
    }
    if cells.is_empty() {
        anyhow::bail!("nothing to grid — pass a model file/dir and/or Ollama ids");
    }
    Ok(cells)
}
