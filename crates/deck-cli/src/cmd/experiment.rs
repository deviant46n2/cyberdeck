use anyhow::Result;
use std::path::PathBuf;
use std::time::Duration;

pub fn run(model: PathBuf, workload: String, engines: Vec<String>, ollama: Vec<String>, runs: u32, max_tokens: u32, bin: Vec<String>, objective: String) -> Result<()> {
    println!("[experiment] model={:?} workload={workload} objective={objective}", model);
    let tasks = super::bench::resolve_tasks(&[], Some(&workload))?;
    let opts = super::bench::GridOpts::parse_parts(tasks, runs, max_tokens, &bin)?;
    // derive fit preview (optional, not blocking)
    if let Some(path) = model.to_str() {
        if std::path::Path::new(path).is_file() {
            if let Ok(eng) = super::parse_engine(&engines.first().cloned().unwrap_or_else(|| "llamacpp".into())) {
                if let Ok(d) = deck_core::profile::derive_loadout(path, eng) {
                    println!("[experiment] fit: ctx={} verdict={} kv={}MB gpu={}MB", d.max_ctx, d.verdict, d.kv_mb, d.weights_gpu_mb);
                }
            }
        }
    }
    let cells = {
        let parsed: Vec<deck_core::profile::Engine> = engines.iter().map(|e| super::parse_engine(e)).collect::<Result<Vec<_>, _>>()?;
        deck_engines::grid::build_cells(&model, &parsed, &ollama)?
    };
    let rows = deck_engines::matrix::run_matrix(&cells, &opts.tasks, opts.runs, opts.max_tokens, Duration::from_secs(240), &opts.bins);
    println!("[experiment] matrix: {} trial(s) recorded", rows.len());
    // recommend
    match deck_core::recommend::recommend(&workload, &objective) {
        Ok(r) if !r.is_empty() => {
            println!("[experiment] recommendation for {workload}/{objective}:");
            for (i, c) in r.iter().take(3).enumerate() {
                println!("  {}. {} — {}", i+1, c.model, c.explain);
            }
        }
        Ok(_) => println!("[experiment] no ranking yet"),
        Err(e) => println!("[experiment] recommend pending: {e}"),
    }
    Ok(())
}
