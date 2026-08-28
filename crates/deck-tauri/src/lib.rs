//! Tauri command API for cyberdeck.
//!
//! This crate is the single bridge between the UI and the engine crates. Every
//! command returns a serializable DTO so the same logic is unit-tested headless
//! here and consumed by both the desktop app and (eventually) the CLI.
//!
//! Each command family lives in its own module; the root re-exports the public
//! surface so `src-tauri` can keep addressing commands as `deck_tauri::name`.

pub use deck_core::profile::{Engine, Profile};

mod bench;
mod bringup;
mod console;
mod downloads;
mod fit;
mod market;
mod profiles;
mod scan;
mod test;

pub use bench::{BenchRow, EngineStatus, bench_history, bench_now, engine_status};
pub use bringup::{
    BringupLine, BringupPhase, BringupResult, FitBreakdown, bringup_start, test_model_start,
};
pub use console::{OpDone, OpLine, OpStarted, opencode_run, opencode_stop};
pub use downloads::{
    DownloadDone, DownloadErr, DownloadEvt, DownloadStarted, download_cancel, download_remove,
    download_start,
};
pub use fit::{BrowseFitResult, FitRow, HwInfo, browse_fit_remote, fit, hw_info};
pub use market::{
    MarketFileRow, MarketHit, SignalRow, browse_org, market_files, market_search, signals_check,
    watch_add, watch_remove, watchlist,
};
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
