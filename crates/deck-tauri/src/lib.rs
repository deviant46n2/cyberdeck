//! Tauri command API for cyberdeck.
//!
//! This crate is the single bridge between the UI and the engine crates. Every
//! command returns a serializable DTO so the same logic is unit-tested headless
//! here and consumed by both the desktop app and (eventually) the CLI.
//!
//! Each command family lives in its own module; the root re-exports the public
//! surface so `src-tauri` can keep addressing commands as `deck_tauri::name`.

pub use deck_core::profile::{Engine, EngineDescriptor, ModelSource, Profile};

pub mod agent;
mod bench;
mod bringup;
mod compare;
mod console;
mod downloads;
mod feeds;
mod fit;
mod hardware;
mod market;
mod portmap;
mod profiles;
mod recommend;
mod scan;
mod settings;
mod test;
mod workloads;
mod tui;

/// The runtime registry for the engine menu: every runtime the app knows
/// (llama.cpp / FreeToken / Ollama). Pure read from the descriptor table.
pub fn engine_list() -> Vec<EngineDescriptor> {
    deck_core::profile::engine_descriptors()
}

/// Per-engine executable configuration for a UI card, plus the currently
/// configured bin for each runtime (None = engine default resolution).
#[derive(serde::Serialize)]
pub struct EngineBinRow {
    pub engine_id: String,
    pub display: String,
    pub bin: Option<String>,
}

pub fn engine_bin_list() -> Vec<EngineBinRow> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db).ok();
    deck_core::profile::engine_descriptors()
        .into_iter()
        .map(|d| EngineBinRow {
            engine_id: d.id.to_string(),
            display: d.display.to_string(),
            bin: conn
                .as_ref()
                .and_then(|c| deck_core::store::get_engine_bin(c, d.id).ok())
                .flatten(),
        })
        .collect()
}

pub fn engine_bin_set(store_id: &str, bin: &str) -> Result<(), String> {
    if bin.trim().is_empty() {
        return engine_bin_clear(store_id);
    }
    let path = std::path::Path::new(bin.trim());
    if !path.exists() {
        return Err(format!("binary not found at {bin}"));
    }
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db).map_err(|e| e.to_string())?;
    deck_core::store::set_engine_bin(&conn, store_id, path.display().to_string().as_str())
        .map_err(|e| e.to_string())
}

pub fn engine_bin_clear(store_id: &str) -> Result<(), String> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db).map_err(|e| e.to_string())?;
    deck_core::store::clear_engine_bin(&conn, store_id).map_err(|e| e.to_string())
}

pub use bench::{BenchRow, EngineStatus, bench_history, bench_now, engine_status};
pub use bringup::{
    BringupLine, BringupPhase, BringupResult, FitBreakdown, apply_cached_profile, bringup_reset,
    bringup_start, test_model_start,
};
pub use compare::{CandidateStanding, CompareReport, ScoredTrial, compare_run};
pub use console::{OpDone, OpLine, OpStarted, opencode_run, opencode_stop};
pub use tui::{tui_resize, tui_spawn, tui_stop, tui_write};
pub use downloads::{
    DownloadDone, DownloadErr, DownloadEvt, DownloadStarted, download_cancel, download_remove,
    download_start,
};
pub use fit::{BrowseFitResult, FitRow, HwInfo, browse_fit_remote, fit, hw_info};
pub use market::{
    MarketFileRow, MarketHit, SignalRow, browse_org, market_files, market_search, signals_check,
    watch_add, watch_remove, watchlist,
};
pub use feeds::{FeedsPollResult, RankedRelease, Release, feeds_list, feeds_poll, feeds_rank};
pub use portmap::{PortMapSlot, engine_start, engine_stop, port_map_status};
pub use agent::{analyze_relevance, agent_tools};
pub use hardware::hardware_profile;
pub use recommend::recommend;
pub use settings::{settings_get, settings_list, settings_set};
pub use workloads::{Workload, workloads_list};
pub use profiles::{
    ProfileRow, UseResult, delete_profile, list_profiles, render_profile_unit, save_profile,
    use_profile,
};
pub use scan::{
    DupRow, ModelRow, ScanResult, dedup, dedup_delete, delete_model, index_downloaded, list_models,
    scan,
};
pub use test::{
    TestLine, TestPhase, TestResult, TweakResult, test_profile, test_profile_tweaked, test_stop,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn scan_and_dedup_headless() {
        let r = scan().expect("scan");
        assert!(r.indexed > 0, "should find local models");
        // the known NVFP4 duplicate must surface
        let hit = r.dups.iter().any(|d| d.wasted_gib > 10.0);
        assert!(hit, "expected the real duplicate to be reported");
        let rows = list_models().expect("list");
        assert_eq!(rows.len(), r.models.len());
    }

    #[test]
    fn fit_reports_verdict() {
        let f = fit(
            PathBuf::from("/home/deviant/Qwen3.8-27B-UD-Q3_K_XL.gguf"),
            32768,
            0.5,
            0,
            None,
            1600,
            false,
        )
        .expect("fit");
        assert!(!f.verdict.is_empty());
        assert!(f.model_vram_mb > 0);
    }

    #[test]
    fn fit_offload_spills_weights_to_ram() {
        let dir = PathBuf::from("/home/deviant/Qwen3.6-35B-A3B-NVFP4");
        if dir.exists() {
            let f = fit(dir, 32768, 1.0, 0, None, 1600, true).expect("fit");
            assert!(
                f.weights_ram_mb > 0,
                "offload should report RAM-spilled weights"
            );
            assert!(
                f.model_vram_mb < f.weights_mb + f.weights_ram_mb,
                "offload VRAM should be far below total weights"
            );
        }
    }

    #[test]
    fn render_known_profile() {
        // import from the real wrapper, then render without applying
        let script =
            std::path::PathBuf::from("/home/deviant/.local/share/llama-server/run-llama-server.sh");
        if script.exists() {
            let p = deck_core::importer::import_llamacpp_script(&script, "qwen").unwrap();
            let unit = deck_engines::render_unit(&p);
            assert!(unit.contains("qwen3.8-27b"));
            assert!(unit.contains("/home/deviant/Qwen3.8-27B-UD-Q3_K_XL.gguf"));
        }
    }

    #[test]
    fn hw_info_returns_something() {
        let hw = hw_info();
        // Either nvidia-smi works (detected) or not — both are valid
        if hw.detected {
            assert!(hw.vram_mb.unwrap() > 0);
        }
    }

    #[test]
    fn browse_fit_remote_smoke() {
        // Test against a known GGUF on HF. Skip if offline.
        let result = browse_fit_remote(
            "unsloth/Qwen3.8-GGUF",
            "Qwen3.8-Q4_K_M.gguf",
            32768,
            0.5,
            0, // all layers on GPU
            None,
            1600,
            false,
        );
        match result {
            Ok(r) => {
                assert!(!r.verdict.is_empty());
                assert!(r.model_vram_mb > 0);
                assert!(r.arch.is_some(), "should parse arch from remote header");
                assert!(
                    r.n_layers.is_some(),
                    "should parse n_layers from remote header"
                );
            }
            Err(e) => {
                // Network failures in CI are expected
                eprintln!("browse_fit_remote skipped (offline?): {e}");
            }
        }
    }
}
