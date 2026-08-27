//! Unified model descriptor produced by every format parser so the
//! scanner/store/fit/UI treat GGUF and safetensors-model-dirs identically.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum ModelFormat {
    Gguf,
    SafetensorsDir,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelMeta {
    pub path: PathBuf,
    pub format: ModelFormat,
    /// Human-facing identity: GGUF general.name or HF model_type + repo hint.
    pub name: String,
    pub arch: Option<String>,
    pub quant: Option<String>,
    /// Actual tensor-parameter count when known, else estimated from bytes.
    pub params: Option<u64>,
    pub n_layers: Option<u64>,
    pub n_embd: Option<u64>,
    pub ctx_train: Option<u64>,
    pub vocab: Option<u64>,
    /// Bytes of tensor weights (GGUF file size, or safetensors total_size).
    pub weight_size: u64,
    /// Total on-disk footprint incl. tokenizer/config overhead.
    pub footprint: u64,
}

impl ModelMeta {
    /// Logical-identity key for dedup grouping. Uses arch + weight-size bucket
    /// (rounded to 0.5 GiB) rather than the quant label, because the same
    /// model can be labelled differently across copies (e.g. `modelopt` in an
    /// HF hub snapshot vs `W4A16_NVFP4` in a local export).
    pub fn identity(&self) -> String {
        let bucket = (self.weight_size as f64 / (512.0 * 1_048_576.0)).round() as u64;
        format!(
            "{}|{:?}|{}",
            self.arch.as_deref().unwrap_or("?"),
            self.format,
            bucket
        )
    }
}
