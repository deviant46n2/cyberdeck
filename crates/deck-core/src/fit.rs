//! VRAM fit estimator.
//!
//! Produces a per-load breakdown so the UI can show exactly where VRAM goes
//! and why a load will or won't fit. The "lore" your wrapper script captured
//! (desktop reserve, hybrid attention caching only some layers, etc.) lives
//! here as tunable inputs, not comments.

use crate::model::ModelMeta;

#[derive(Debug, Clone)]
pub struct FitRequest {
    /// Target context window.
    pub ctx: u64,
    /// Bytes per KV element (0.5 = q4_0, 1.0 = fp16, 2.0 = fp32).
    pub kv_bytes: f64,
    /// Fraction of weight tensors kept on the GPU (1.0 = all layers).
    pub ngl_frac: f64,
    /// Layers whose KV is actually cached. None = all layers. This is the
    /// hybrid-attention lever your qwen35 build uses (16/48 cached).
    pub kv_layers: Option<u64>,
    /// Fixed reservation for the desktop (compositor + ckb-next GUI, etc).
    pub reserved_mb: u64,
    /// FreeToken offload backend: weights are split across VRAM + RAM, spilling
    /// the remainder to system memory. When set, the estimator treats the bulk
    /// of the weights as living in RAM and only charges VRAM for the KV cache,
    /// activation buffers, and a small on-GPU working set.
    pub offload: bool,
}

impl Default for FitRequest {
    fn default() -> Self {
        Self {
            ctx: 32768,
            kv_bytes: 0.5,
            ngl_frac: 1.0,
            kv_layers: None,
            reserved_mb: 1600,
            offload: false,
        }
    }
}

#[derive(Debug)]
pub struct FitBreakdown {
    pub weights_mb: u64,
    pub kv_mb: u64,
    pub buffers_mb: u64,
    /// Fixed reservation for the desktop (compositor + ckb-next etc).
    pub overhead_mb: u64,
    /// weights + kv + buffers, i.e. what the engine actually maps to VRAM.
    pub model_vram_mb: u64,
    /// For offload backends, the weight bytes that spilled to system RAM
    /// instead of VRAM. 0 when not offloading.
    pub weights_ram_mb: u64,
    pub available_mb: u64,
    /// available minus overhead: headroom the engine is allowed to use.
    pub available_for_model_mb: u64,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Warn,
    Oom,
}

impl Verdict {
    pub fn tag(&self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Warn => "WARN",
            Verdict::Oom => "OOM",
        }
    }
}

pub fn estimate(model: &ModelMeta, req: &FitRequest, available_mb: u64) -> FitBreakdown {
    let n_layers = req.kv_layers.or(model.n_layers).unwrap_or(0);
    let n_embd = model.n_embd.unwrap_or(0);
    let kv_elements = req.ctx.saturating_mul(n_layers).saturating_mul(n_embd);
    let kv_mb = (kv_elements as f64 * req.kv_bytes * 2.0 / 1_048_576.0) as u64;

    let buffers_mb = ((req.ctx * 2 * n_embd) as f64 / 1_048_576.0).max(32.0) as u64;
    let overhead_mb = req.reserved_mb;

    let (weights_mb, weights_ram_mb) = if req.offload {
        let total = (model.weight_size as f64 / 1_048_576.0) as u64;
        // FreeToken's offload backend keeps an on-GPU working set (the layers
        // currently resident) and spills the rest to system RAM. We can't see
        // the exact layer split without runtime, so approximate the GPU-side
        // working set at ~10% of the weights (floor 512 MiB) and charge the
        // remainder to RAM. VRAM pressure is therefore dominated by KV + buffers.
        let on_gpu = ((total as f64 * 0.10).max(512.0)) as u64;
        (on_gpu, total.saturating_sub(on_gpu))
    } else {
        (
            (model.weight_size as f64 * req.ngl_frac / 1_048_576.0) as u64,
            0,
        )
    };

    let model_vram_mb = weights_mb + kv_mb + buffers_mb;
    let available_for_model_mb = available_mb.saturating_sub(overhead_mb);

    let verdict = if model_vram_mb > available_for_model_mb {
        Verdict::Oom
    } else if available_for_model_mb - model_vram_mb < 600 {
        Verdict::Warn
    } else {
        Verdict::Pass
    };

    FitBreakdown {
        weights_mb,
        kv_mb,
        buffers_mb,
        overhead_mb,
        model_vram_mb,
        weights_ram_mb,
        available_mb,
        available_for_model_mb,
        verdict,
    }
}

/// Total VRAM of the primary GPU, or `fallback_mb` if nvml/query fails.
pub fn available_vram_mb(fallback_mb: u64) -> u64 {
    let out = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output();
    if let Ok(o) = out {
        if let Ok(s) = String::from_utf8(o.stdout) {
            if let Some(line) = s.lines().next() {
                if let Ok(v) = line.trim().parse::<u64>() {
                    return v;
                }
            }
        }
    }
    fallback_mb
}
