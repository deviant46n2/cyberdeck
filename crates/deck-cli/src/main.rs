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
    /// Manage loadout profiles (engine launch specs)
    Profile {
        #[command(subcommand)]
        action: ProfileCmd,
    },
    /// Apply a loadout: render+install unit (with .bak), restart, health-wait
    Use {
        name: String,
        /// Render + show the unit but do NOT restart the live service
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum ProfileCmd {
    /// Create a new loadout from flags
    New {
        name: String,
        #[arg(long)]
        model: String,
        #[arg(long, default_value = "llamacpp")]
        engine: String,
        #[arg(long)]
        bin: Option<PathBuf>,
        #[arg(long, default_value = "qwen3.8-27b")]
        alias: String,
        #[arg(long, default_value_t = 18000)]
        port: u16,
        #[arg(long, default_value_t = 32768)]
        ctx: u32,
        #[arg(long, default_value_t = 64)]
        ngl: u32,
        #[arg(long)]
        draft: Option<PathBuf>,
    },
    /// Import an existing launch script into a loadout
    Import {
        #[arg(long, default_value = "llamacpp")]
        engine: String,
        #[arg(long)]
        script: PathBuf,
        #[arg(long, default_value = "imported")]
        name: String,
    },
    /// List saved loadouts
    List {
        #[arg(long)]
        json: bool,
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
        Commands::Profile { action } => match action {
            ProfileCmd::New { name, model, engine, bin, alias, port, ctx, ngl, draft } => {
                cmd_profile_new(name, model, engine, bin, alias, port, ctx, ngl, draft)
            }
            ProfileCmd::Import { engine, script, name } => cmd_profile_import(engine, script, name),
            ProfileCmd::List { json } => cmd_profile_list(json),
        },
        Commands::Use { name, dry_run } => cmd_use(name, dry_run),
    }
}

fn parse_engine(s: &str) -> anyhow::Result<deck_core::profile::Engine> {
    match s {
        "llamacpp" | "llama" | "llama.cpp" => Ok(deck_core::profile::Engine::LlamaCpp),
        "freetoken" | "ft" => Ok(deck_core::profile::Engine::FreeToken),
        other => anyhow::bail!("unknown engine '{other}' (llamacpp|freetoken)"),
    }
}

fn with_profiles_db() -> Result<(PathBuf, rusqlite::Connection)> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_profile_schema(&conn)?;
    Ok((db, conn))
}

fn cmd_profile_new(
    name: String,
    model: String,
    engine: String,
    bin: Option<PathBuf>,
    alias: String,
    port: u16,
    ctx: u32,
    ngl: u32,
    draft: Option<PathBuf>,
) -> Result<()> {
    let mut p = deck_core::profile::Profile::default();
    p.name = name.clone();
    p.engine = parse_engine(&engine)?;
    p.model = model;
    p.alias = alias;
    p.port = port;
    p.ctx_size = ctx;
    p.n_gpu_layers = ngl;
    p.draft_model = draft;
    if let Some(b) = bin {
        p.bin = b;
    } else if p.engine == deck_core::profile::Engine::FreeToken {
        p.bin = PathBuf::from("ft");
    }
    let (_db, mut conn) = with_profiles_db()?;
    deck_core::store::upsert_profile(&mut conn, &p)?;
    println!("saved loadout '{name}' ({engine}, alias={}, port={})", p.alias, p.port);
    Ok(())
}

fn cmd_profile_import(engine: String, script: PathBuf, name: String) -> Result<()> {
    let eng = parse_engine(&engine)?;
    let p = match eng {
        deck_core::profile::Engine::LlamaCpp => {
            deck_core::importer::import_llamacpp_script(&script, &name)?
        }
        deck_core::profile::Engine::FreeToken => {
            deck_core::importer::import_freetoken_script(&script, &name)?
        }
    };
    let (_db, mut conn) = with_profiles_db()?;
    deck_core::store::upsert_profile(&mut conn, &p)?;
    println!(
        "imported loadout '{}' from {} (alias={}, port={}, ctx={})",
        p.name,
        script.display(),
        p.alias,
        p.port,
        p.ctx_size
    );
    Ok(())
}

fn cmd_profile_list(json: bool) -> Result<()> {
    let (_db, conn) = with_profiles_db()?;
    let profiles = deck_core::store::list_profiles(&conn)?;
    let active = deck_core::store::active_profile(&conn)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&profiles)?);
    } else if profiles.is_empty() {
        println!("no loadouts saved. use `deck profile import` or `deck profile new`.");
    } else {
        for p in &profiles {
            let mark = if active.as_deref() == Some(&p.name) { "*" } else { " " };
            println!(
                "{mark} {:<14} {:<10} alias={:<12} port={:<6} ctx={}",
                p.name,
                format!("{:?}", p.engine),
                p.alias,
                p.port,
                p.ctx_size
            );
        }
    }
    Ok(())
}

fn cmd_use(name: String, dry_run: bool) -> Result<()> {
    let (_db, conn) = with_profiles_db()?;
    let p = deck_core::store::get_profile(&conn, &name)?
        .ok_or_else(|| anyhow::anyhow!("no loadout named '{name}'"))?;
    deck_core::store::set_active(&conn, &name)?;
    println!(
        "applying loadout '{}' (alias={}, port={}){}",
        name,
        p.alias,
        p.port,
        if dry_run { " [dry-run]" } else { "" }
    );
    deck_engines::apply(&p, dry_run)?;
    Ok(())
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
