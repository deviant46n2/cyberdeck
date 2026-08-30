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
        /// Resident: bind this loadout to its engine's fixed PORT MAP slot and
        /// run it *alongside* other engine slots instead of treating it as a
        /// one-at-a-time swap
        #[arg(long)]
        resident: bool,
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
    /// Online intelligence feeds: poll release catalog and list it
    Feeds {
        #[command(subcommand)]
        action: FeedsCmd,
    },
    /// Workload definitions (Phase 2): coding, reasoning, instruction, assistant, agent
    Workloads {
        #[command(subcommand)]
        action: WorkloadsCmd,
    },
    /// Infinite Agent Canvas workflows (Phase 8c): save, list, run, history
    Workflow {
        #[command(subcommand)]
        action: WorkflowCmd,
    },
    /// Hardware profile (Phase 3): capture and show this machine's profile
    Hardware {
        #[command(subcommand)]
        action: HardwareCmd,
    },
    /// Recommendation per workload (Phase 4)
    Recommend {
        /// Workload id (coding, reasoning, instruction, assistant, agent)
        #[arg(long)]
        workload: String,
        /// Objective: quality|speed|efficient
        #[arg(long, default_value = "quality")]
        objective: String,
        #[arg(long)]
        json: bool,
    },
    /// Typed settings + audit log (O3): get/set/log/undo
    Settings {
        #[command(subcommand)]
        action: SettingsCmd,
    },
    /// One-click experiment pipeline (Phase 6): fit → matrix --workload → recommend
    Experiment {
        /// GGUF file or dir whose top-level *.gguf are the quants to grid
        #[arg(long)]
        model: PathBuf,
        #[arg(long, default_value = "coding")]
        workload: String,
        #[arg(long, value_delimiter = ',', default_value = "llamacpp")]
        engines: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        ollama: Vec<String>,
        #[arg(long, default_value_t = 1)]
        runs: u32,
        #[arg(long, default_value_t = 512)]
        max_tokens: u32,
        #[arg(long)]
        bin: Vec<String>,
        #[arg(long, default_value = "quality")]
        objective: String,
    },
    /// Download a repo's model file into ~/models (resumable).
    ///
    /// Picks the largest .gguf (or one matching --file/--quant), then
    /// streams it into `~/models` with curl resume. Single-file — the
    /// bounded queue stays in the Tauri app.
    Download {
        /// HuggingFace repo id (e.g. `unsloth/Qwen3.8-GGUF`)
        #[arg()]
        repo: String,
        /// Pick this exact filename (or suffix-match) instead of auto-picking
        #[arg(long)]
        file: Option<String>,
        /// Filter by quant token in the filename (e.g. `Q3_K_XL`)
        #[arg(long)]
        quant: Option<String>,
        /// Resolve and print the pick without downloading
        #[arg(long, default_value_t = false)]
        dry_run: bool,
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
    /// Show the best tok/s per (model, engine) across stored bench history
    Best,
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
        /// Expand tasks from a workload (coding|reasoning|instruction|assistant|agent); merges with --task
        #[arg(long)]
        workload: Option<String>,
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
        /// Expand tasks from a workload; merges with --task
        #[arg(long)]
        workload: Option<String>,
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
enum WorkloadsCmd {
    /// List seeded workloads and their tasks
    List {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum WorkflowCmd {
    /// Persist a workflow document, or --seed the built-in Coding Review template
    Save {
        /// Save the built-in Coding Review workflow + roles
        #[arg(long)]
        seed: bool,
        /// A workflow JSON document to import
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// List saved workflows
    List {
        #[arg(long)]
        json: bool,
    },
    /// Execute a workflow against a model
    Run {
        /// Workflow id
        id: String,
        /// Runner: stateless (default) or agentic (needs opencode + --dir)
        #[arg(long, default_value = "stateless")]
        runner: String,
        /// Workspace dir for the agentic runner
        #[arg(long)]
        dir: Option<PathBuf>,
        /// opencode model override for the agentic runner
        #[arg(long)]
        model: Option<String>,
    },
    /// List past workflow runs, optionally filtered to one workflow
    History {
        /// Filter to a single workflow id
        workflow: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Per-role bench (8e): which model best at which node, from matrix_runs
    Bench {
        /// Workflow id
        id: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum HardwareCmd {
    /// Capture (or reuse) and show the hardware profile for this machine
    Profile {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum SettingsCmd {
    Get { key: Option<String>, #[arg(long)] json: bool },
    Set { key: String, value: String, #[arg(long, default_value = "cli")] reason: String, #[arg(long, default_value = "user")] actor: String },
    Log { #[arg(long, default_value_t = 20)] limit: usize },
    Undo { id: i64 },
}

#[derive(Subcommand)]
enum FeedsCmd {
    /// Poll configured sources (hf, github) and upsert into the release catalog
    Poll {
        /// Only poll these sources (repeatable): hf, github
        #[arg(long)]
        source: Vec<String>,
    },
    /// List recent releases from the catalog
    List {
        #[arg(long)]
        json: bool,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Rank releases by hardware-grounded relevance (O2 + workload hint)
    Rank {
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        workload: Option<String>,
    },
    /// Watch: periodically poll feeds (O5 background polling)
    Watch {
        #[arg(long, default_value_t = 3600)]
        interval: u64,
        #[arg(long)]
        once: bool,
    },
}

#[derive(Subcommand)]
enum EngineCmd {
    /// List registered runtimes and their configured binaries
    List,
    /// Show the live PORT MAP: each engine's fixed slot, bound profile,
    /// systemd/health state, and resident flag
    Status {
        /// Host used to probe engine health (default loopback)
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
    },
    /// Stop an engine's unit and clear its port-map binding (other residents stay up)
    Stop {
        /// engine id: llamacpp | freetoken | ollama
        engine: String,
    },
    /// Start the bound profile on that engine's port (LM Studio-style)
    Start {
        /// engine id: llamacpp | freetoken | ollama
        engine: String,
    },
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
            resident,
        } => cmd::use_cmd::run(name, dry_run, managed, resident),
        Commands::Bench { action } => match action {
            BenchCmd::Record {
                engine,
                host,
                port,
                model,
                ctx,
            } => cmd::bench::record(engine, host, port, model, ctx),
            BenchCmd::List => cmd::bench::list(),
            BenchCmd::Best => cmd::bench::best(),
            BenchCmd::Matrix {
                model,
                engines,
                ollama,
                task,
                workload,
                runs,
                max_tokens,
                bin,
                out,
            } => {
                let tasks = cmd::bench::resolve_tasks(&task, workload.as_deref())?;
                let opts = cmd::bench::GridOpts::parse_parts(tasks, runs, max_tokens, &bin)?;
                cmd::bench::matrix(model, engines, ollama, opts, out)
            }
            BenchCmd::Compare {
                model,
                engines,
                ollama,
                task,
                workload,
                runs,
                max_tokens,
                bin,
                seed,
                out,
            } => {
                let tasks = cmd::bench::resolve_tasks(&task, workload.as_deref())?;
                let opts = cmd::bench::GridOpts::parse_parts(tasks, runs, max_tokens, &bin)?;
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
            EngineCmd::Status { host } => cmd::engines::status(&host),
            EngineCmd::Stop { engine } => cmd::engines::stop(&engine),
            EngineCmd::Start { engine } => cmd::engines::start(&engine),
            EngineCmd::Bin {
                engine,
                path,
                clear,
            } => cmd::engines::bin(&engine, path, clear),
        },
        Commands::Workloads { action } => match action {
            WorkloadsCmd::List { json } => cmd::workloads::list(json),
        },
        Commands::Workflow { action } => match action {
            WorkflowCmd::Save { seed, file } => cmd::workflow::save(seed, file.as_deref()),
            WorkflowCmd::List { json } => cmd::workflow::list(json),
            WorkflowCmd::Run { id, runner, dir, model } => cmd::workflow::run(&id, &runner, dir.as_deref(), model.as_deref()),
            WorkflowCmd::History { workflow, json } => cmd::workflow::history(workflow.as_deref(), json),
            WorkflowCmd::Bench { id, json } => cmd::workflow::bench(&id, json),
        },
        Commands::Hardware { action } => match action {
            HardwareCmd::Profile { json } => cmd::hardware::profile(json),
        },
        Commands::Recommend { workload, objective, json } => cmd::recommend::run(workload, objective, json),
        Commands::Settings { action } => match action {
            SettingsCmd::Get { key, json } => cmd::settings::get(key, json),
            SettingsCmd::Set { key, value, reason, actor } => cmd::settings::set(key, value, reason, actor),
            SettingsCmd::Log { limit } => cmd::settings::log(limit),
            SettingsCmd::Undo { id } => cmd::settings::undo(id),
        },
        Commands::Experiment { model, workload, engines, ollama, runs, max_tokens, bin, objective } => cmd::experiment::run(model, workload, engines, ollama, runs, max_tokens, bin, objective),
        Commands::Feeds { action } => match action {
            FeedsCmd::Poll { source } => cmd::feeds::poll(source),
            FeedsCmd::List { json, limit } => cmd::feeds::list(json, limit),
            FeedsCmd::Rank { limit, json, workload } => cmd::feeds::rank(limit, json, workload),
            FeedsCmd::Watch { interval, once } => cmd::feeds::watch(interval, once),
        },
        Commands::Download { repo, file, quant, dry_run } => {
            cmd::download::run(&repo, file.as_deref(), quant.as_deref(), dry_run)
        }
    }
}
