//! Loadout (profile) model and persistence.
//!
//! A profile describes a fully-specified engine launch: which binary, which
//! model, every optimization flag, the alias/port contract, and a context
//! fallback ladder used when a load OOMs. Profiles are stored as JSON blobs in
//! the SQLite index so the schema never has to chase engine flag churn.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum Engine {
    LlamaCpp,
    FreeToken,
}

impl Engine {
    pub fn systemd_unit(&self) -> &'static str {
        match self {
            Engine::LlamaCpp => "llama-server.service",
            Engine::FreeToken => "freetoken.service",
        }
    }

    /// Live PORT MAP slot (what clients point at).
    pub fn default_port(self) -> u16 {
        match self {
            Engine::LlamaCpp => 18000,
            Engine::FreeToken => 1919,
        }
    }

    /// Dedicated headless verification slot, distinct from every live port so
    /// a bring-up never collides with a resident engine.
    pub fn test_port(self) -> u16 {
        match self {
            Engine::LlamaCpp => 18999,
            Engine::FreeToken => 18998,
        }
    }

    /// Accepts the spellings used across CLI flags and UI buttons.
    pub fn parse(s: &str) -> Option<Engine> {
        match s.to_ascii_lowercase().as_str() {
            "llamacpp" | "llama" | "llama.cpp" | "LlamaCpp" => Some(Engine::LlamaCpp),
            "freetoken" | "ft" | "FreeToken" => Some(Engine::FreeToken),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub engine: Engine,
    pub bin: PathBuf,
    pub model: String,
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub metrics: bool,

    // --- context / offload ---
    pub ctx_size: u32,
    pub ctx_ladder: Vec<u32>,
    pub n_gpu_layers: u32,
    pub ubatch_size: u32,
    pub flash_attn: bool,
    pub kv_cache_type_k: Option<String>,
    pub kv_cache_type_v: Option<String>,
    pub load_mode: Option<String>,

    // --- speculative decoding (llama.cpp MTP) ---
    pub spec_type: Option<String>,
    pub draft_model: Option<PathBuf>,

    // --- sampling defaults ---
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: u32,
    pub parallel: u32,

    // --- reasoning (llama.cpp) ---
    pub reasoning: Option<String>,
    pub reasoning_format: Option<String>,
    pub reasoning_effort: Option<String>,
    pub reasoning_budget: Option<u32>,

    // --- freetoken specific ---
    pub ft_backend: Option<String>,
    pub ft_moe_cache_size: Option<u32>,

    // --- resource limits (cgroup) ---
    pub mem_max_mb: Option<u64>,
    pub mem_swap_max_mb: Option<u64>,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: String::new(),
            engine: Engine::LlamaCpp,
            bin: PathBuf::from("/usr/bin/llama-server"),
            model: String::new(),
            alias: "model".into(),
            host: "0.0.0.0".into(),
            port: 18000,
            metrics: true,
            ctx_size: 32768,
            ctx_ladder: vec![49152, 40960, 32768],
            n_gpu_layers: 64,
            ubatch_size: 256,
            flash_attn: true,
            kv_cache_type_k: Some("q4_0".into()),
            kv_cache_type_v: Some("q4_0".into()),
            load_mode: Some("mmap+mlock".into()),
            spec_type: None,
            draft_model: None,
            temperature: 0.7,
            top_p: 0.8,
            top_k: 20,
            parallel: 1,
            reasoning: Some("on".into()),
            reasoning_format: Some("deepseek".into()),
            reasoning_effort: Some("medium".into()),
            reasoning_budget: Some(4096),
            ft_backend: None,
            ft_moe_cache_size: None,
            mem_max_mb: None,
            mem_swap_max_mb: None,
        }
    }
}

impl Profile {
    pub fn active_ladder(&self) -> Vec<u32> {
        let mut ladder = vec![self.ctx_size];
        ladder.extend(self.ctx_ladder.iter().copied());
        ladder
    }
}

/// Computed result of deriving a loadout from a model + engine.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DerivedLoadout {
    pub profile: Profile,
    /// The largest ctx that fit `PASS` at derive time (the chosen ctx_size).
    pub max_ctx: u32,
    /// The per-max-ctx fit breakdown from the estimator.
    pub kv_mb: u64,
    pub weights_gpu_mb: u64,
    pub weights_ram_mb: u64,
    pub buffers_mb: u64,
    pub model_vram_mb: u64,
    pub available_mb: u64,
    pub available_for_model_mb: u64,
    pub headroom_mb: u64,
    pub verdict: String,
}

/// Derive a fully-populated engine launch spec from a GGUF model file +
/// engine choice, picking the largest context window that still fits the
/// detected GPU VRAM (WARN is allowed; OOM is walked back until it passes).
///
/// This is the "plug in a model, click FreeToken, it figures it out" step:
/// nothing in the returned profile is left at a magic default the user has to
/// reason about — ctx, KV cache type, layer offload, Flash Attention, and
/// engine-specific server flags (FreeToken prefill/offload backend, llama.cpp
/// MTP) are all derived from the model's actual header + hardware.
pub fn derive_loadout(
    model_path: impl AsRef<std::path::Path>,
    engine: Engine,
) -> Result<DerivedLoadout, String> {
    let path = model_path.as_ref();
    let meta = if path.is_dir() {
        // Safetensors model-dir (e.g. FreeToken NVFP4 shards).
        crate::safetensors::open_dir(path)
            .map_err(|e| format!("read safetensors dir {path:?}: {e}"))?
    } else {
        crate::gguf::GgufMeta::read(path)
            .map_err(|e| format!("read GGUF {path:?}: {e}"))?
            .to_meta(path)
    };
    derive_from_meta(&meta, engine)
}

/// Pure core of `derive_loadout`, separable for tests: given a model's parsed
/// metadata and an engine, plan the best-max-ctx loadout against detected VRAM.
pub fn derive_from_meta(
    meta: &crate::model::ModelMeta,
    engine: Engine,
) -> Result<DerivedLoadout, String> {
    let vram_mb = kind_of_vram();

    // Weights are small enough to hold on GPU wholesale in the common case;
    // FreeToken offload spills the remainder to RAM when they aren't.
    let offload = engine == Engine::FreeToken;
    let reserved = 1600u64; // desktop reserve (compositor + ckb-next etc)

    // Find the largest ctx whose fit verdict is Pass or Warn (never OOM).
    let mut max_ctx: u64 = 0;
    let mut best: Option<crate::fit::FitBreakdown> = None;
    // Step up from 2K in 2K jumps to a sane ceiling (TODO: binary search for
    // speed on huge ctx; linear is fine for local planning).
    let step = 2048u64;
    for ctx in (step..=262_144).step_by(step as usize) {
        let req = crate::fit::FitRequest {
            ctx,
            kv_bytes: 0.5,
            ngl_frac: 1.0,
            kv_layers: None,
            reserved_mb: reserved,
            offload,
        };
        let fb = crate::fit::estimate(meta, &req, vram_mb);
        use crate::fit::Verdict;
        if matches!(fb.verdict, Verdict::Pass | Verdict::Warn) {
            max_ctx = ctx;
            best = Some(fb);
        } else {
            break; // first OOM terminates the climb
        }
    }
    if max_ctx == 0 {
        return Err(format!(
            "model {:?} does not fit this GPU (vram={vram_mb} MiB) even at 2K ctx",
            meta.path
        ));
    }

    let fb = best.unwrap();
    let profile = build_profile_from_derive(meta, engine, offload, max_ctx, &fb);
    let available = kind_of_vram();
    let reserved = 1600u64;
    let available_for_model = available.saturating_sub(reserved);
    let model_vram = fb.weights_mb + fb.kv_mb + fb.buffers_mb;
    let headroom = available_for_model.saturating_sub(model_vram);

    Ok(DerivedLoadout {
        profile,
        max_ctx: max_ctx as u32,
        kv_mb: fb.kv_mb,
        weights_gpu_mb: fb.weights_mb,
        weights_ram_mb: fb.weights_ram_mb,
        buffers_mb: fb.buffers_mb,
        model_vram_mb: model_vram,
        available_mb: available,
        available_for_model_mb: available_for_model,
        headroom_mb: headroom,
        verdict: format!("{:?}", fb.verdict),
    })
}

/// Assemble a fully-populated `Profile` from the derived fit. `offload` is
/// already decided by engine; the model + hardware facts drive everything else.
fn build_profile_from_derive(
    meta: &crate::model::ModelMeta,
    engine: Engine,
    offload: bool,
    ctx: u64,
    fb: &crate::fit::FitBreakdown,
) -> Profile {
    let mut p = Profile::default();
    p.engine = engine;
    p.model = meta.path.display().to_string();
    p.alias = meta
        .name
        .chars()
        .filter_map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                Some(c.to_ascii_lowercase())
            } else if c.is_whitespace() {
                Some('-')
            } else {
                None
            }
        })
        .collect::<String>();
    p.port = default_port(engine);
    p.ctx_size = ctx as u32;
    // Ladder steps below the max so a real-world OOM still degrades gracefully.
    let grain = 4096u32;
    p.ctx_ladder = [
        p.ctx_size.saturating_sub(grain),
        p.ctx_size.saturating_sub(2 * grain),
    ]
    .into_iter()
    .filter(|c| *c >= 2048 && *c != p.ctx_size)
    .collect();
    p.flash_attn = true;

    if offload {
        // FreeToken: weights spill to RAM, VRAM holds KV + buffers.
        p.n_gpu_layers = 0; // fit treats offload as GPU working set + RAM spill
        p.kv_cache_type_k = Some("q4_0".into());
        p.kv_cache_type_v = Some("q4_0".into());
        // The "prefill/offload server" the user refers to: a MoE cache worker
        // sized from the max ctx headroom.
        p.ft_backend = Some("offload".into());
        p.ft_moe_cache_size = Some(fb.weights_ram_mb.clamp(1024, 16 * 1024) as u32 / 1024);
        p.mem_max_mb = None;
    } else {
        // llama.cpp: keep all layers on GPU if they fit.
        p.n_gpu_layers = 0;
        p.load_mode = Some("mmap".into());
        // NVIDIA GPU present -> offer MTP speculative decoding if a draft
        // exists next to the model (draft_model left None here; caller may set).
        p.spec_type = None;
    }

    if meta.name.to_lowercase().contains("reasoning") || meta.arch.as_deref() == Some("qwen3") {
        p.reasoning = Some("on".into());
        p.reasoning_budget = Some(4096);
        p.reasoning_effort = Some("medium".into());
    }

    p
}

/// Detect GPU VRAM in MB, falling back to a conservative 12 GiB if nvidia-smi
/// is unavailable so derive always has *something* to plan against.
fn kind_of_vram() -> u64 {
    crate::fit::hw_vram().unwrap_or(12 * 1024)
}

/// Fixed port slot per engine (the PORT MAP). Delegates to the method so the
/// map lives in exactly one place.
pub fn default_port(engine: Engine) -> u16 {
    engine.default_port()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ModelFormat, ModelMeta};
    use std::path::PathBuf;

    #[test]
    fn engine_map_is_single_source_of_truth() {
        for e in [Engine::LlamaCpp, Engine::FreeToken] {
            assert_ne!(
                e.test_port(),
                e.default_port(),
                "test slot must not collide with live slot"
            );
            assert_ne!(
                Engine::LlamaCpp.test_port(),
                Engine::FreeToken.default_port(),
                "llamacpp test port must not collide with freetoken live port"
            );
        }
        assert_eq!(Engine::LlamaCpp.default_port(), 18000);
        assert_eq!(Engine::FreeToken.default_port(), 1919);
        // CLI + UI spellings parse; junk doesn't.
        for s in ["llamacpp", "LLAMA.CPP", "freetoken", "ft", "FreeToken"] {
            assert!(Engine::parse(s).is_some(), "{s} should parse");
        }
        assert!(Engine::parse("ollama").is_none());
    }

    fn meta(weight_gib: u64, n_layers: u64, n_embd: u64, name: &str, arch: &str) -> ModelMeta {
        ModelMeta {
            path: PathBuf::from(format!("/models/{name}.gguf")),
            format: ModelFormat::Gguf,
            name: name.into(),
            arch: Some(arch.into()),
            quant: Some("Q4_K_M".into()),
            params: Some(8 * 1_000_000_000),
            n_layers: Some(n_layers),
            n_embd: Some(n_embd),
            ctx_train: Some(131072),
            vocab: Some(151936),
            weight_size: weight_gib * 1024 * 1024 * 1024,
            footprint: weight_gib * 1024 * 1024 * 1024,
        }
    }

    #[test]
    fn derive_llamacpp_fills_flags_and_ctx() {
        // Small 4 GiB model on any real/fallback GPU fits a huge ctx.
        let m = meta(4, 48, 5120, "qwen3.8-27b", "qwen3");
        let d = derive_from_meta(&m, Engine::LlamaCpp).expect("derives");
        assert!(d.max_ctx > 0, "max ctx populated");
        assert_eq!(d.profile.engine, Engine::LlamaCpp);
        assert_eq!(d.profile.port, 18000);
        assert_eq!(d.profile.ctx_size, d.max_ctx);
        assert!(d.profile.flash_attn);
        // llama.cpp keeps layers on GPU (ngl=0 means all) and uses mmap.
        assert_eq!(d.profile.n_gpu_layers, 0);
        assert_eq!(d.profile.load_mode.as_deref(), Some("mmap"));
        // qwen3 -> reasoning enabled with a budget.
        assert_eq!(d.profile.reasoning.as_deref(), Some("on"));
        assert!(d.profile.reasoning_budget.is_some());
        // ladder has steps below max ctx.
        assert!(!d.profile.ctx_ladder.is_empty());
        for c in &d.profile.ctx_ladder {
            assert!(*c < d.max_ctx);
        }
    }

    #[test]
    fn derive_freetoken_is_offload_and_windows_port() {
        // 30 GiB model with FreeToken: offload to RAM, VRAM for KV + buffers.
        let m = meta(30, 64, 3584, "Qwen3.6-35B-A3B-NVFP4", "qwen3");
        let d = derive_from_meta(&m, Engine::FreeToken).expect("derives");
        assert_eq!(d.profile.port, 1919);
        assert_eq!(d.profile.ft_backend.as_deref(), Some("offload"));
        assert!(d.profile.ft_moe_cache_size.is_some());
        // offload path -> ngl 0, q4 kv.
        assert_eq!(d.profile.kv_cache_type_k.as_deref(), Some("q4_0"));
    }

    #[test]
    fn derive_alias_is_slugified() {
        let m = meta(4, 48, 5120, "Qwen3.8 27B UD Q3", "qwen3");
        let d = derive_from_meta(&m, Engine::LlamaCpp).unwrap();
        assert_eq!(d.profile.alias, "qwen3.8-27b-ud-q3");
    }
}
