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
        /// FreeToken offload backend: weights spill to RAM, VRAM holds KV + buffers
        #[arg(long, default_value_t = false)]
        offload: bool,
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
        /// MANAGED: also repoint dsh + opencode at the applied engine's port
        #[arg(long)]
        managed: bool,
    },
    /// Benchmark a live engine: probe /metrics and record, or list history
    Bench {
        #[command(subcommand)]
        action: BenchCmd,
    },
    /// PLUG IN a model + engine and let cyberdeck derive the best-max-ctx
    /// loadout, verify it headlessly on a test port (never touching the live
    /// service), then install + start + bench it.
    Bringup {
        /// Path to a local GGUF model file
        #[arg(long)]
        model: PathBuf,
        /// engine to load it through (llamacpp|freetoken)
        #[arg(long, default_value = "llamacpp")]
        engine: String,
        /// SKIP the test-port verification and apply directly (faster, riskier)
        #[arg(long, default_value_t = false)]
        fast: bool,
        /// DERIVE + print the loadout only; do NOT verify, install, or start
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        /// Name to save the derived loadout under (default: from model name)
        #[arg(long)]
        name: Option<String>,
    },
}

#[derive(Subcommand)]
enum BenchCmd {
    /// Probe a live engine's /metrics and store the generation tok/s reading
    Record {
        #[arg(long, default_value = "llamacpp")]
        engine: String,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 18000)]
        port: u16,
        #[arg(long, default_value = "?")]
        model: String,
        #[arg(long, default_value_t = 0)]
        ctx: u32,
    },
    /// List recent benchmark readings
    List,
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
        Commands::Fit {
            model,
            ctx,
            kv_bytes,
            ngl,
            kv_layers,
            reserve,
            offload,
        } => cmd_fit(model, ctx, kv_bytes, ngl, kv_layers, reserve, offload),
        Commands::Profile { action } => match action {
            ProfileCmd::New {
                name,
                model,
                engine,
                bin,
                alias,
                port,
                ctx,
                ngl,
                draft,
            } => cmd_profile_new(name, model, engine, bin, alias, port, ctx, ngl, draft),
            ProfileCmd::Import {
                engine,
                script,
                name,
            } => cmd_profile_import(engine, script, name),
            ProfileCmd::List { json } => cmd_profile_list(json),
        },
        Commands::Use {
            name,
            dry_run,
            managed,
        } => cmd_use(name, dry_run, managed),
        Commands::Bench { action } => match action {
            BenchCmd::Record {
                engine,
                host,
                port,
                model,
                ctx,
            } => cmd_bench_record(engine, host, port, model, ctx),
            BenchCmd::List => cmd_bench_list(),
        },
        Commands::Bringup {
            model,
            engine,
            fast,
            name,
            dry_run,
        } => cmd_bringup(model, engine, fast, name, dry_run),
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
    println!(
        "saved loadout '{name}' ({engine}, alias={}, port={})",
        p.alias, p.port
    );
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
            let mark = if active.as_deref() == Some(&p.name) {
                "*"
            } else {
                " "
            };
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

fn cmd_use(name: String, dry_run: bool, managed: bool) -> Result<()> {
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
    if managed && !dry_run {
        println!("MANAGED rewiring clients:");
        for r in deck_engines::rewire::rewire_clients(p.port) {
            println!("  [{}] {} — {}", r.client, r.path, r.status);
        }
    }
    Ok(())
}

fn cmd_bench_record(
    engine: String,
    host: String,
    port: u16,
    model: String,
    ctx: u32,
) -> Result<()> {
    let text = deck_engines::fetch_metrics(&host, port).map_err(|e| {
        anyhow::anyhow!(
            "could not reach {host}:{port}/metrics — is the engine running with --metrics? ({e})"
        )
    })?;
    let tps = deck_engines::parse_tps(&text)
        .ok_or_else(|| anyhow::anyhow!("no tokens/sec gauge exposed by {host}:{port}"))?;
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_bench_schema(&conn)?;
    let row = deck_core::store::BenchRow {
        id: 0,
        engine: engine.clone(),
        host: host.clone(),
        port,
        model: model.clone(),
        ctx,
        tps,
        at,
    };
    let id = deck_core::store::insert_bench(&conn, &row)?;
    println!("recorded #{id}: {tps:.1} tok/s from {engine} {host}:{port}");
    Ok(())
}

fn cmd_bench_list() -> Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_bench_schema(&conn)?;
    let rows = deck_core::store::recent_bench(&conn, 50)?;
    if rows.is_empty() {
        println!("no benchmark readings yet — run `deck bench record` against a live engine");
        return Ok(());
    }
    for r in rows {
        let when = chrono_like(r.at);
        println!(
            "{:>4}  {:<10} {:<15} {:>7.1} tok/s  {}",
            r.id,
            r.engine,
            format!("{}:{}", r.host, r.port),
            r.tps,
            when
        );
    }
    Ok(())
}

fn cmd_bringup(
    model: PathBuf,
    engine: String,
    fast: bool,
    name: Option<String>,
    dry_run: bool,
) -> Result<()> {
    let eng = parse_engine(&engine)?;
    println!(
        "[bringup] deriving loadout for {:?} via {eng:?}…",
        model.file_name().unwrap_or_default()
    );
    let derived = deck_core::profile::derive_loadout(&model, eng).map_err(anyhow::Error::msg)?;
    let mut p = derived.profile;

    if let Some(n) = &name {
        p.name = n.clone();
    } else {
        p.name = p.alias.clone();
    }

    println!(
        "[bringup] derived: ctx={} (max {}) kv={}MiB weights(gpu={}MiB ram={}MiB) verdict={} port={}",
        p.ctx_size,
        derived.max_ctx,
        derived.kv_mb,
        derived.weights_gpu_mb,
        derived.weights_ram_mb,
        derived.verdict,
        p.port,
    );

    if dry_run {
        println!(
            "[bringup] --dry-run: would save loadout '{}' (engine={:?} port={}) and apply it. nothing changed.",
            p.name, p.engine, p.port
        );
        return Ok(());
    }

    // Option 1 (default): verify headlessly on a test port WITHOUT touching the
    // live service, walking the ctx ladder if the max OOMs. Only then install.
    if !fast {
        let test_port = eng.test_port();
        println!(
            "[bringup] verifying on test port :{test_port} (live :{} untouched)…",
            p.port
        );
        let outcome =
            deck_engines::verify_on_test_port(&p, test_port, std::time::Duration::from_secs(120));
        if outcome.verdict != "RUNNING" {
            anyhow::bail!(
                "[bringup] verification FAILED on the test port: {} ({}) — nothing was changed on the live service; use --fast to force",
                outcome.summary,
                outcome.verdict,
            );
        }
        if outcome.ctx != p.ctx_size {
            println!(
                "[bringup] max ctx {} OOM'd; settled on ctx={}",
                p.ctx_size, outcome.ctx
            );
            p.ctx_size = outcome.ctx;
        }
        println!(
            "[bringup] verify OK: ctx={} serving{}",
            outcome.ctx,
            outcome
                .tok_per_sec
                .map(|t| format!(", {t:.1} tok/s"))
                .unwrap_or_default(),
        );
    } else {
        println!("[bringup] --fast: skipping test-port verification");
    }

    // Save the derived loadout, then apply (install + start + health-wait).
    let (_db, mut conn) = with_profiles_db()?;
    deck_core::store::upsert_profile(&mut conn, &p)?;
    println!(
        "[bringup] saved loadout '{}' (engine={:?} port={})",
        p.name, p.engine, p.port
    );

    deck_engines::apply(&p, false)?;
    println!("[bringup] applied '{}' on :{} — live.", p.name, p.port);

    // Bench and record the result so the chat header has a fresh tok/s.
    let text = deck_engines::fetch_metrics(&p.host, p.port)?;
    if let Some(tps) = deck_engines::parse_tps(&text) {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let db = deck_core::store::default_db_path();
        let conn = deck_core::store::open(&db)?;
        deck_core::store::ensure_bench_schema(&conn)?;
        let row = deck_core::store::BenchRow {
            id: 0,
            engine: format!("{:?}", p.engine).to_lowercase(),
            host: p.host.clone(),
            port: p.port,
            model: p.model.clone(),
            ctx: p.ctx_size,
            tps,
            at,
        };
        let id = deck_core::store::insert_bench(&conn, &row)?;
        println!("[bringup] bench recorded #{id}: {tps:.1} tok/s");
    } else {
        println!("[bringup] note: no /metrics tok/s gauge exposed (is --metrics on?)");
    }

    Ok(())
}

fn chrono_like(at: i64) -> String {
    if at <= 0 {
        return "—".into();
    }
    // time-of-day in UTC (no chrono dependency)
    let rem = at.rem_euclid(86400);
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let s = rem % 60;
    format!("{h:02}:{m:02}:{s:02} UTC")
}

fn cmd_scan() -> Result<()> {
    let roots = deck_core::scanner::default_roots();
    let mut models = deck_core::scanner::scan(&roots)?;

    // Also index ollama models.
    if let Ok(ollama) = deck_feeds::ollama_models() {
        for o in &ollama {
            let existing: std::collections::HashSet<String> = models
                .iter()
                .map(|m| std::fs::canonicalize(&m.path).ok()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| m.path.display().to_string()))
                .collect();
            let canonical = std::fs::canonicalize(&o.path)
                .ok()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| o.path.clone());
            if !existing.contains(&canonical) {
                let meta = if let Ok(gguf_meta) = deck_core::gguf::GgufMeta::read(&o.path) {
                    gguf_meta.to_meta(&std::path::PathBuf::from(&o.path))
                } else {
                    deck_core::model::ModelMeta {
                        path: std::path::PathBuf::from(o.path.clone()),
                        format: deck_core::model::ModelFormat::Gguf,
                        name: o.name.clone(),
                        arch: None,
                        quant: None,
                        params: None,
                        n_layers: None,
                        n_embd: None,
                        ctx_train: None,
                        vocab: None,
                        weight_size: o.size,
                        footprint: o.size,
                    }
                };
                models.push(meta);
            }
        }
    }

    let db = deck_core::store::default_db_path();
    let mut conn = deck_core::store::open(&db)?;
    let n = deck_core::store::upsert_many(&mut conn, &models)?;
    let keep: Vec<String> = models
        .iter()
        .map(|m| m.path.display().to_string())
        .collect();
    let pruned = deck_core::store::prune(&conn, &keep)?;

    println!(
        "indexed {n} model(s), pruned {pruned} stale -> {}",
        db.display()
    );
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
    let p = path;
    if p.is_dir() {
        return Ok(deck_core::safetensors::open_dir(p)?);
    }
    let g = deck_core::gguf::GgufMeta::read(p)?;
    Ok(g.to_meta(p))
}
