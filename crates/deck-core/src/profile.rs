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
