//! Compare door for the UI: runs the same blind A/B grid as `deck bench
//! compare` (built from the shared grid plumbing in deck-engines) and returns
//! the serializable report so the Compare tab can render the scored ranking and
//! hand it to an agent for synthesis. Long-running — callers should run it off
//! the UI thread (spawn_blocking).

use std::path::Path;
use std::time::Duration;

use deck_core::profile::Engine;

pub use deck_engines::compare::{CandidateStanding, CompareReport, ScoredTrial};

fn parse_engine(s: &str) -> Result<Engine, String> {
    Engine::parse(s).ok_or_else(|| format!("unknown engine '{s}' (llamacpp|freetoken|ollama)"))
}

/// Run the blind compare grid and return the report.
///
/// `model` is a single GGUF file or a directory of top-level GGUFs; `engines`
/// are local-source engine ids; `ollama` are Ollama model ids; `tasks` are
/// `"label=prompt"` strings. Binaries resolve from the per-engine store config
/// (machine config, no UI input) just like the CLI.
pub fn compare_run(
    model: String,
    engines: Vec<String>,
    ollama: Vec<String>,
    tasks: Vec<String>,
    runs: u32,
    max_tokens: u32,
    seed: u64,
) -> Result<CompareReport, String> {
    let parsed: Vec<Engine> = engines
        .iter()
        .map(|e| parse_engine(e))
        .collect::<Result<_, _>>()?;
    let cells = deck_engines::grid::build_cells(Path::new(&model), &parsed, &ollama)
        .map_err(|e| e.to_string())?;
    let tasks = deck_engines::grid::parse_tasks(&tasks).map_err(|e| e.to_string())?;
    let bins = deck_engines::grid::resolve_bins(&std::collections::HashMap::new());
    let opts = deck_engines::compare::CompareOpts {
        tasks: &tasks,
        runs,
        max_tokens,
        boot_timeout: Duration::from_secs(240),
        bins: &bins,
    };
    Ok(deck_engines::compare::run_compare(&cells, &opts, seed))
}
