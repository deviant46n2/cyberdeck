//! Engine control: render systemd units from a Profile, install them with
//! timestamped backups of whatever was there before, supervise the live
//! service (start/stop/health-wait, context fallback ladder), and — the
//! scientific heart — boot engines headlessly and run generation probes.
//! Split across focused modules:
//!   - `unit`     — unit-file rendering (pure)
//!   - `systemd`  — install/backup/start/stop/apply lifecycle
//!   - `health`   — /health probing, /metrics fetch, headless bring-up verify
//!   - `inference`— per-protocol generation (OpenAI-compat, Ollama /api/chat)
//!   - `matrix`   — the model × quant × engine grid runner
//!   - `compare`  — blind A/B over the grid: opaque trial ids, explicit
//!     quality/throughput scoring, a verdict naming the best (model × engine)
//!
//! Safety discipline (from the cyberdeck contract):
//!   - never overwrite a unit without first writing `<unit>.bak.<timestamp>`
//!   - `use` preserves the alias+port contract so clients don't reconfigure
//!   - on a failed load, walk the profile's ctx ladder, then restore last-good

mod health;
mod inference;
mod systemd;
mod unit;

pub mod status;

pub mod compare;
pub mod evaluation;
pub mod grid;
pub mod matrix;
pub mod rewire;
pub mod scoring;
pub mod workflow;

pub use health::{
    BringupOutcome, OOM_MARKERS, fetch_metrics, health_ok, health_ok_any, health_wait,
    measure_generation_tps, parse_tps,
    verify_on_test_port,
};
pub use inference::run_prompt;
pub use systemd::{
    apply, backup_existing, backup_file, install, reload_daemon, restore_last_good, start, stop,
};
pub use unit::{build_args, render_unit};
pub use workflow::{AgenticRunner, ExecReport, NodeResult, NodeRunner, StatelessRunner, execute};

#[cfg(test)]
mod tests {
    use super::*;
    use deck_core::profile::{Engine, Profile};
    use std::path::PathBuf;

    fn sample_llamacpp() -> Profile {
        let mut p = Profile::default();
        p.name = "qwen".into();
        p.engine = Engine::LlamaCpp;
        p.bin = PathBuf::from("/opt/llama.cpp/build/bin/llama-server");
        p.model = "/home/deviant/models/qwen.gguf".into();
        p.alias = "qwen3.8-27b".into();
        p.port = 18000;
        p.ctx_size = 32768;
        p.n_gpu_layers = 64;
        p.draft_model = Some(PathBuf::from("/home/deviant/models/mtp.gguf"));
        p.mem_max_mb = Some(26_624);
        p
    }

    #[test]
    fn render_contains_flags() {
        let u = render_unit(&sample_llamacpp());
        assert!(u.contains("--ctx-size 32768"));
        assert!(u.contains("--n-gpu-layers 64"));
        assert!(u.contains("--alias qwen3.8-27b"));
        assert!(u.contains("--port 18000"));
        assert!(u.contains("LLAMACPP_API_KEY=llamacpp-local"));
        assert!(u.contains("MemoryMax=26624M"));
        // MTP draft companion
        assert!(u.contains("--draft-model"));
        assert!(u.contains("mtp.gguf"));
        // reasoning defaults present
        assert!(u.contains("--reasoning on"));
    }

    #[test]
    fn build_args_freetoken() {
        let mut p = Profile::default();
        p.engine = Engine::FreeToken;
        p.model = "nvidia/Qwen3.6-35B-A3B-NVFP4".into();
        p.port = 1919;
        p.ft_backend = Some("offload".into());
        p.ft_moe_cache_size = Some(3000);
        let a = build_args(&p);
        assert_eq!(a[0], "serve");
        assert!(a.contains(&"--moe-backend".into()));
        assert!(a.contains(&"offload".into()));
        assert!(a.contains(&"--moe-cache-size".into()));
        assert!(a.contains(&"3000".into()));
        assert!(a.contains(&"--port".into()));
        assert!(a.contains(&"1919".into()));
    }

    #[test]
    fn build_args_ollama_is_env_configured_daemon() {
        let p = Profile {
            engine: Engine::Ollama,
            bin: PathBuf::from("/usr/bin/ollama"),
            host: "127.0.0.1".into(),
            port: 18997,
            ..Profile::default()
        };
        let a = build_args(&p);
        assert_eq!(a, vec!["serve"], "ollama takes no model/host args at exec");
        let u = render_unit(&p);
        assert!(u.contains("OLLAMA_HOST=127.0.0.1:18997"));
        assert!(u.contains("ExecStart=/usr/bin/ollama serve"));
        assert!(!u.contains("LLAMACPP_API_KEY"), "no llama.cpp auth env");
    }

    #[test]
    fn backup_existing_writes_bak() {
        let tmp = std::env::temp_dir().join(format!("cyberdeck-bak-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let unit = tmp.join("llama-server.service");
        std::fs::write(&unit, "[Service]\nExecStart=/old\n").unwrap();
        let bak = backup_existing(&unit).unwrap().unwrap();
        assert!(bak.exists());
        assert!(bak.to_string_lossy().contains(".bak."));
        // second backup distinct
        let bak2 = backup_existing(&unit).unwrap().unwrap();
        assert_ne!(bak, bak2);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn parses_generation_tps() {
        let text = "# TYPE llamacpp:generation_tokens_per_second gauge
llamacpp:generation_tokens_per_second 42.7
llamacpp:prompt_processing_tokens_per_second 1234.5";
        assert_eq!(parse_tps(text), Some(42.7));
    }

    #[test]
    fn parses_tps_without_generation_line() {
        let text = "# HELP ft:tok_per_sec tokens/sec
ft:tok_per_sec 9.5";
        assert_eq!(parse_tps(text), Some(9.5));
    }

    #[test]
    fn no_tps_returns_none() {
        assert_eq!(parse_tps("up 1\ngo_process_cpu_seconds_total 0.1"), None);
    }
}
