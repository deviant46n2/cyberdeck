//! VRAM/RAM fit estimation for local models and remote GGUF headers.

use std::io::Cursor;
use std::path::PathBuf;

use serde::Serialize;

#[derive(Serialize)]
pub struct FitRow {
    pub model: String,
    pub ctx: u64,
    pub weights_mb: u64,
    pub kv_mb: u64,
    pub buffers_mb: u64,
    pub model_vram_mb: u64,
    pub weights_ram_mb: u64,
    pub overhead_mb: u64,
    pub available_for_model_mb: u64,
    pub verdict: String,
}

#[derive(Serialize)]
pub struct HwInfo {
    pub vram_mb: Option<u64>,
    pub detected: bool,
}

/// Result of a remote fit estimate: the GGUF metadata we could parse plus
/// the full fit breakdown for a given context size.
#[derive(Serialize)]
pub struct BrowseFitResult {
    // GGUF metadata (from the header parse — may be partial if truncated)
    pub arch: Option<String>,
    pub quant: Option<String>,
    pub params: Option<u64>,
    pub n_layers: Option<u64>,
    pub n_embd: Option<u64>,
    pub truncated: bool,
    // fit breakdown
    pub weights_mb: u64,
    pub kv_mb: u64,
    pub buffers_mb: u64,
    pub model_vram_mb: u64,
    pub weights_ram_mb: u64,
    pub overhead_mb: u64,
    pub available_for_model_mb: u64,
    pub verdict: String,
}

/// The numeric fit outcome shared by the local and remote fit commands.
struct Estimate {
    weights_mb: u64,
    kv_mb: u64,
    buffers_mb: u64,
    model_vram_mb: u64,
    weights_ram_mb: u64,
    overhead_mb: u64,
    available_for_model_mb: u64,
    verdict: String,
}

/// Translate an absolute layer count into the estimator's fraction. 0 means
/// "all layers on GPU" — used by the quick HUD estimate.
fn ngl_fraction(n_gpu_layers: u32, n_layers: Option<u64>) -> f64 {
    if n_gpu_layers == 0 {
        1.0
    } else {
        let total = n_layers.unwrap_or(0).max(1) as f64;
        (n_gpu_layers as f64 / total).clamp(0.0, 1.0)
    }
}

fn estimate(
    meta: &deck_core::model::ModelMeta,
    ctx: u32,
    kv_bytes: f64,
    n_gpu_layers: u32,
    kv_layers: Option<u64>,
    reserve: u64,
    offload: bool,
) -> Estimate {
    let req = deck_core::fit::FitRequest {
        ctx: ctx as u64,
        kv_bytes,
        ngl_frac: ngl_fraction(n_gpu_layers, meta.n_layers),
        kv_layers,
        reserved_mb: reserve,
        offload,
    };
    let available = deck_core::fit::available_vram_mb(16_303);
    let b = deck_core::fit::estimate(meta, &req, available);
    Estimate {
        weights_mb: b.weights_mb,
        kv_mb: b.kv_mb,
        buffers_mb: b.buffers_mb,
        model_vram_mb: b.model_vram_mb,
        weights_ram_mb: b.weights_ram_mb,
        overhead_mb: b.overhead_mb,
        available_for_model_mb: b.available_for_model_mb,
        verdict: b.verdict.tag().to_string(),
    }
}

pub fn fit(
    model: PathBuf,
    ctx: u32,
    kv_bytes: f64,
    n_gpu_layers: u32,
    kv_layers: Option<u64>,
    reserve: u64,
    offload: bool,
) -> anyhow::Result<FitRow> {
    let meta = if model.is_dir() {
        deck_core::safetensors::open_dir(&model)?
    } else {
        deck_core::gguf::GgufMeta::read(&model)?.to_meta(&model)
    };
    let b = estimate(
        &meta,
        ctx,
        kv_bytes,
        n_gpu_layers,
        kv_layers,
        reserve,
        offload,
    );
    Ok(FitRow {
        model: meta.path.display().to_string(),
        ctx: ctx as u64,
        weights_mb: b.weights_mb,
        kv_mb: b.kv_mb,
        buffers_mb: b.buffers_mb,
        model_vram_mb: b.model_vram_mb,
        weights_ram_mb: b.weights_ram_mb,
        overhead_mb: b.overhead_mb,
        available_for_model_mb: b.available_for_model_mb,
        verdict: b.verdict,
    })
}

/// Detect GPU VRAM via nvidia-smi. The frontend uses this to display the
/// hardware baseline and for fit calculations against remote models.
pub fn hw_info() -> HwInfo {
    let vram = deck_core::fit::hw_vram();
    HwInfo {
        vram_mb: vram,
        detected: vram.is_some(),
    }
}

/// Fetch the first 2 MiB of a remote GGUF file via HTTP Range, parse its
/// header metadata, and compute a VRAM fit estimate. This is the core
/// operation behind the BROWSE view: exact fit without downloading the file.
pub fn browse_fit_remote(
    repo_id: &str,
    rfilename: &str,
    ctx: u32,
    kv_bytes: f64,
    n_gpu_layers: u32,
    kv_layers: Option<u64>,
    reserve: u64,
    offload: bool,
) -> anyhow::Result<BrowseFitResult> {
    // 1. Fetch the GGUF header (first 2 MiB — covers all scalar KVs)
    let (bytes, total_size) = deck_feeds::fetch_gguf_bytes(repo_id, rfilename, 2 * 1024 * 1024)?;

    // 2. Parse GGUF metadata from the buffer
    let mut cursor = Cursor::new(bytes);
    let gguf_meta = deck_core::gguf::GgufMeta::from_reader(&mut cursor, total_size)?;

    // 3. Build a ModelMeta for the fit estimator. weight_size = file_size
    //    (GGUF files store all tensor weights; close enough for VRAM calc).
    let meta = deck_core::model::ModelMeta {
        path: PathBuf::from(format!("{repo_id}/{rfilename}")),
        format: deck_core::model::ModelFormat::Gguf,
        name: gguf_meta.name().unwrap_or(rfilename).to_string(),
        arch: gguf_meta.arch().map(str::to_string),
        quant: gguf_meta.quant_name(),
        params: gguf_meta.params().map(|v| v as u64),
        n_layers: gguf_meta.n_layers(),
        n_embd: gguf_meta.n_embd(),
        ctx_train: gguf_meta.ctx_train().map(|v| v as u64),
        vocab: gguf_meta.vocab_size(),
        weight_size: total_size,
        footprint: total_size,
    };

    // 4. Run the fit estimator
    let b = estimate(
        &meta,
        ctx,
        kv_bytes,
        n_gpu_layers,
        kv_layers,
        reserve,
        offload,
    );

    Ok(BrowseFitResult {
        arch: meta.arch,
        quant: meta.quant,
        params: meta.params,
        n_layers: meta.n_layers,
        n_embd: meta.n_embd,
        truncated: gguf_meta.truncated,
        weights_mb: b.weights_mb,
        kv_mb: b.kv_mb,
        buffers_mb: b.buffers_mb,
        model_vram_mb: b.model_vram_mb,
        weights_ram_mb: b.weights_ram_mb,
        overhead_mb: b.overhead_mb,
        available_for_model_mb: b.available_for_model_mb,
        verdict: b.verdict,
    })
}
