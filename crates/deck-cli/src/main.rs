use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

mod cmd;

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
    /// List the runtime registry and configure per-engine executable paths.
    /// A bin set here is used by bringup / test / matrix whenever a profile's
    /// default binary doesn't exist on disk — set once per machine.
    Engines {
        #[command(subcommand)]
        action: EngineCmd,
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
        /// Per-engine binary override, "engine=path" (else the engine's default)
        #[arg(long)]
        bin: Option<String>,
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
    /// Run a headless model × engine grid (quant × runtime) and record every trial
    Matrix {
        /// GGUF file, or a directory whose top-level *.gguf files are the quants to grid
        #[arg(long)]
        model: PathBuf,
        /// local-source engines to run the GGUFs through (llamacpp|freetoken)
        #[arg(long, value_delimiter = ',', default_value = "llamacpp")]
        engines: Vec<String>,
        /// Ollama model ids to grid (each runs through ollama)
        #[arg(long, value_delimiter = ',')]
        ollama: Vec<String>,
        /// Repeatable: task to run as "label=prompt"
        #[arg(long)]
        task: Vec<String>,
        /// Repeats per cell (variance sampling)
        #[arg(long, default_value_t = 1)]
        runs: u32,
        /// Max generation tokens per request
        #[arg(long, default_value_t = 512)]
        max_tokens: u32,
        /// Per-engine binary override, "engine=path" (repeatable; else profile default)
        #[arg(long)]
        bin: Vec<String>,
        /// Write machine-readable results (all trials) to this JSON path
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Blind A/B compare over a grid: execute like `matrix`, score every trial
    /// offline (lexical quality + normalized throughput), and reveal the best
    /// (model × engine) under opaque trial ids
    Compare {
        /// GGUF file, or a directory whose top-level *.gguf files are the quants to grid
        #[arg(long)]
        model: PathBuf,
        /// local-source engines to run the GGUFs through (llamacpp|freetoken)
        #[arg(long, value_delimiter = ',', default_value = "llamacpp")]
        engines: Vec<String>,
        /// Ollama model ids to compare (each runs through ollama)
        #[arg(long, value_delimiter = ',')]
        ollama: Vec<String>,
        /// Repeatable: task to run as "label=prompt"
        #[arg(long)]
        task: Vec<String>,
        /// Repeats per candidate (variance sampling)
        #[arg(long, default_value_t = 1)]
        runs: u32,
        /// Max generation tokens per request
        #[arg(long, default_value_t = 512)]
        max_tokens: u32,
        /// Per-engine binary override, "engine=path" (repeatable; else profile default)
        #[arg(long)]
        bin: Vec<String>,
        /// PRNG seed for opaque trial-id assignment (tie-breaks re-runs)
        #[arg(long, default_value_t = 20260828)]
        seed: u64,
        /// Write the full blind report (candidates + scored trials) to this JSON path
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum EngineCmd {
    /// List registered runtimes and their configured binaries
    List,
    /// Show or set an engine's executable path
    Bin {
        /// engine id: llamacpp | freetoken | ollama
        engine: String,
        /// executable path to configure (omit to print the current value)
        path: Option<PathBuf>,
        /// remove the configured path (back to the engine's default resolution)
        #[arg(long)]
        clear: bool,
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Scan => cmd::scan::run(),
        Commands::List { json } => cmd::list::run(json),
        Commands::Fit {
            model,
            ctx,
            kv_bytes,
            ngl,
            kv_layers,
            reserve,
            offload,
        } => cmd::fit::run(model, ctx, kv_bytes, ngl, kv_layers, reserve, offload),
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
            } => cmd::profile::new(name, model, engine, bin, alias, port, ctx, ngl, draft),
            ProfileCmd::Import {
                engine,
                script,
                name,
            } => cmd::profile::import(engine, script, name),
            ProfileCmd::List { json } => cmd::profile::list(json),
        },
        Commands::Use {
            name,
            dry_run,
            managed,
        } => cmd::use_cmd::run(name, dry_run, managed),
        Commands::Bench { action } => match action {
            BenchCmd::Record {
                engine,
                host,
                port,
                model,
                ctx,
            } => cmd::bench::record(engine, host, port, model, ctx),
            BenchCmd::List => cmd::bench::list(),
            BenchCmd::Matrix {
                model,
                engines,
                ollama,
                task,
                runs,
                max_tokens,
                bin,
                out,
            } => {
                let opts = cmd::bench::GridOpts::parse(&task, runs, max_tokens, &bin)?;
                cmd::bench::matrix(model, engines, ollama, opts, out)
            }
            BenchCmd::Compare {
                model,
                engines,
                ollama,
                task,
                runs,
                max_tokens,
                bin,
                seed,
                out,
            } => {
                let opts = cmd::bench::GridOpts::parse(&task, runs, max_tokens, &bin)?;
                cmd::bench::compare(model, engines, ollama, opts, seed, out)
            }
        },
        Commands::Bringup {
            model,
            engine,
            fast,
            name,
            dry_run,
            bin,
        } => cmd::bringup::run(model, engine, fast, name, dry_run, bin),
        Commands::Engines { action } => match action {
            EngineCmd::List => cmd::engines::list(),
            EngineCmd::Bin {
                engine,
                path,
                clear,
            } => cmd::engines::bin(&engine, path, clear),
        },
    }
}
