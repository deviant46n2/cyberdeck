//! VRAM fit estimation for a model at a given context.

use std::path::PathBuf;

use anyhow::Result;

pub(crate) fn run(
    model: PathBuf,
    ctx: u32,
    kv_bytes: f64,
    ngl: f64,
    kv_layers: Option<u64>,
    reserve: u64,
    offload: bool,
) -> Result<()> {
    let meta = load_model(&model)?;
    let req = deck_core::fit::FitRequest {
        ctx: ctx as u64,
        kv_bytes,
        ngl_frac: ngl,
        kv_layers,
        reserved_mb: reserve,
        offload,
    };
    let available = deck_core::fit::available_vram_mb(16303);
    let b = deck_core::fit::estimate(&meta, &req, available);

    println!("model : {}", meta.path.display());
    println!(
        "ctx   : {}  kv_bytes={}  ngl={}  kv_layers={:?}  offload={}",
        req.ctx, req.kv_bytes, req.ngl_frac, req.kv_layers, req.offload
    );
    println!("--------------------------------------------------");
    println!("weights            {:>6} MiB", b.weights_mb);
    if b.weights_ram_mb > 0 {
        println!("  (offload: {} MiB spilled to RAM)", b.weights_ram_mb);
    }
    println!("kv cache           {:>6} MiB", b.kv_mb);
    println!("buffers            {:>6} MiB", b.buffers_mb);
    println!("model VRAM         {:>6} MiB", b.model_vram_mb);
    println!("desktop reserve    {:>6} MiB", b.overhead_mb);
    println!("--------------------------------------------------");
    println!(
        "VERDICT  [{}]   model {}/{} MiB available-for-model",
        b.verdict.tag(),
        b.model_vram_mb,
        b.available_for_model_mb
    );
    Ok(())
}

fn load_model(path: &PathBuf) -> Result<deck_core::model::ModelMeta> {
    if path.is_dir() {
        return Ok(deck_core::safetensors::open_dir(path)?);
    }
    let g = deck_core::gguf::GgufMeta::read(path)?;
    Ok(g.to_meta(path))
}
