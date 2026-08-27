use anyhow::Result;
use clap::{Parser, Subcommand};

/// cyberdeck — local LLM fleet manager.
///
/// Inventory what you have, judge what fits before loading,
/// swap loadouts without hand-editing units.
#[derive(Parser)]
#[command(name = "deck", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Scan configured roots and refresh the local model index
    Scan,
    /// List indexed models with arch/quant/size
    List {
        #[arg(long)]
        json: bool,
    },
    /// Estimate VRAM fit for a model at a given context
    Fit {
        /// Path to a local model file (GGUF) or dir (safetensors)
        #[arg(long)]
        model: PathBuf,
        #[arg(long)]
        ctx: u32,
        /// Bytes per KV element: 0.5 = q4_0, 1.0 = fp16, 2.0 = fp32
        #[arg(long, default_value_t = 0.5)]
        kv_bytes: f64,
        /// Fraction of weights kept on GPU (1.0 = all layers)
        #[arg(long, default_value_t = 1.0)]
        ngl: f64,
        /// Layers whose KV is cached (hybrid attention). Default: all.
        #[arg(long)]
        kv_layers: Option<u64>,
        /// Desktop VRAM reserve (compositor + ckb-next etc), MiB
        #[arg(long, default_value_t = 1600)]
        reserve: u64,
    },
}

use std::path::PathBuf;

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Scan => cmd_scan(),
        Commands::List { json } => cmd_list(json),
        Commands::Fit { model, ctx, kv_bytes, ngl, kv_layers, reserve } => {
            cmd_fit(model, ctx, kv_bytes, ngl, kv_layers, reserve)
        }
    }
}

fn cmd_scan() -> Result<()> {
    let roots = deck_core::scanner::default_roots();
    let models = deck_core::scanner::scan(&roots)?;

    let db = deck_core::store::default_db_path();
    let mut conn = deck_core::store::open(&db)?;
    let n = deck_core::store::upsert_many(&mut conn, &models)?;
    let keep: Vec<String> = models.iter().map(|m| m.path.display().to_string()).collect();
    let pruned = deck_core::store::prune(&conn, &keep)?;

    println!("indexed {n} model(s), pruned {pruned} stale -> {}", db.display());
    for m in &models {
        println!(
            "  {:<10} {:<18} {:<8} {:.2} GiB  {}",
            format!("{:?}", m.format),
            m.arch.as_deref().unwrap_or("?"),
            m.quant.as_deref().unwrap_or("?"),
            m.footprint as f64 / 1_073_741_824.0,
            m.path.display(),
        );
    }

    let dups = deck_core::store::duplicates(&conn)?;
    if !dups.is_empty() {
        println!("\nDUPLICATES (wasted space):");
        for d in &dups {
            println!(
                "  {:<14} wasted {:.2} GiB across {} copies",
                d.identity,
                d.wasted_bytes as f64 / 1_073_741_824.0,
                d.members.len()
            );
            for m in &d.members {
                println!("      {}", m.path.display());
            }
        }
    }
    Ok(())
}

fn cmd_list(json: bool) -> Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    let models = deck_core::store::list(&conn)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&models)?);
    } else {
        for m in &models {
            println!(
                "{:<16} {:<8} arch={:<10} ctx={:<8} {:.2} GiB  {}",
                m.name,
                m.quant.as_deref().unwrap_or("?"),
                m.arch.as_deref().unwrap_or("?"),
                m.ctx_train.unwrap_or(0),
                m.footprint as f64 / 1_073_741_824.0,
                m.path.display(),
            );
        }
    }
    Ok(())
}

fn cmd_fit(
    model: PathBuf,
    ctx: u32,
    kv_bytes: f64,
    ngl: f64,
    kv_layers: Option<u64>,
    reserve: u64,
) -> Result<()> {
    let meta = load_model(&model)?;
    let req = deck_core::fit::FitRequest {
        ctx: ctx as u64,
        kv_bytes,
        ngl_frac: ngl,
        kv_layers,
        reserved_mb: reserve,
    };
    let available = deck_core::fit::available_vram_mb(16303);
    let b = deck_core::fit::estimate(&meta, &req, available);

    println!("model : {}", meta.path.display());
    println!(
        "ctx   : {}  kv_bytes={}  ngl={}  kv_layers={:?}",
        req.ctx, req.kv_bytes, req.ngl_frac, req.kv_layers
    );
    println!("--------------------------------------------------");
    println!("weights            {:>6} MiB", b.weights_mb);
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
    let p = path;
    if p.is_dir() {
        return Ok(deck_core::safetensors::open_dir(p)?);
    }
    let g = deck_core::gguf::GgufMeta::read(p)?;
    Ok(g.to_meta(p))
}
