//! Benchmark: probe a live engine, list history, or run the model × quant ×
//! engine matrix grid.

use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use deck_core::profile::Engine;

use super::parse_engine;

pub(crate) fn record(
    engine: String,
    host: String,
    port: u16,
    model: String,
    ctx: u32,
) -> Result<()> {
    let eng = super::parse_engine(&engine)?;
    let tps = deck_engines::measure_generation_tps(eng, &host, port, &model)
        .map_err(anyhow::Error::msg)?;
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_bench_schema(&conn)?;
    let hw_id = deck_core::store::capture_hardware_profile(&conn).ok();
    let row = deck_core::store::BenchRow {
        id: 0,
        engine: engine.clone(),
        host: host.clone(),
        port,
        model: model.clone(),
        ctx,
        tps,
        at,
        hardware_profile_id: hw_id,
        engine_version: None,
        prompt_tps: None,
        ttft_ms: None,
    };
    let id = deck_core::store::insert_bench(&conn, &row)?;
    println!("recorded #{id}: {tps:.1} tok/s from {engine} {host}:{port}");
    Ok(())
}

pub(crate) fn list() -> Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_bench_schema(&conn)?;
    let rows = deck_core::store::recent_bench(&conn, 50)?;
    if rows.is_empty() {
        println!("no benchmark readings yet — run `deck bench record` against a live engine");
        return Ok(());
    }
    for r in rows {
        println!(
            "{:>4}  {:<10} {:<15} {:>7.1} tok/s  {}",
            r.id,
            r.engine,
            format!("{}:{}", r.host, r.port),
            r.tps,
            chrono_like(r.at)
        );
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

/// Parse repeatable `--task "label=prompt"` flags.
fn parse_tasks(tasks: &[String]) -> Result<Vec<(String, String)>> {
    deck_engines::grid::parse_tasks(tasks)
}

pub(crate) fn resolve_tasks(cli_tasks: &[String], workload: Option<&str>) -> Result<Vec<(String, String)>> {
    let mut tasks = Vec::new();
    if let Some(wid) = workload {
        let db = deck_core::store::default_db_path();
        let conn = deck_core::store::open(&db)?;
        deck_core::store::ensure_seeded_workloads(&conn)?;
        if let Some(w) = deck_core::store::get_workload(&conn, wid)? {
            for t in w.tasks {
                tasks.push((t.label, t.prompt));
            }
        } else {
            anyhow::bail!("unknown workload '{wid}' — known: coding, reasoning, instruction, assistant, agent");
        }
    }
    if !cli_tasks.is_empty() {
        let mut cli = parse_tasks(cli_tasks)?;
        tasks.append(&mut cli);
    }
    if tasks.is_empty() {
        anyhow::bail!("pass --task \"label=prompt\" or --workload <id>");
    }
    Ok(tasks)
}

/// Parsed matrix knobs shared down to the grid runner.
pub(crate) struct GridOpts {
    pub(crate) tasks: Vec<(String, String)>,
    pub(crate) runs: u32,
    pub(crate) max_tokens: u32,
    pub(crate) bins: std::collections::HashMap<Engine, PathBuf>,
}

impl GridOpts {
    pub(crate) fn parse_parts(
        tasks: Vec<(String, String)>,
        runs: u32,
        max_tokens: u32,
        bin: &[String],
    ) -> Result<Self> {
        let mut bins: std::collections::HashMap<Engine, PathBuf> = std::collections::HashMap::new();
        for b in bin {
            let (e, path) = b
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("--bin must be \"engine=path\", got {b:?}"))?;
            bins.insert(parse_engine(e)?, PathBuf::from(path));
        }
        // Seed any engine without an explicit --bin from the per-engine DB
        // config, so a machine configured once needs no repeated flags.
        bins = deck_engines::grid::resolve_bins(&bins);
        Ok(Self {
            tasks,
            runs,
            max_tokens,
            bins,
        })
    }
}

/// Build the flat grid: local quants × local-source engines, plus each
/// requested Ollama id × ollama. Shared by `matrix` and `compare`.
fn build_cells(
    model: &Path,
    engines: &[String],
    ollama: &[String],
) -> Result<Vec<deck_engines::matrix::MatrixCell>> {
    let parsed: Vec<Engine> = engines
        .iter()
        .map(|e| parse_engine(e))
        .collect::<Result<_>>()?;
    deck_engines::grid::build_cells(model, &parsed, ollama)
}

/// The scientific grid: local quants × local-source engines, plus any requested
/// Ollama ids (each × ollama). Every cell is booted headlessly on the engine's
/// dedicated test port, run through each task × run, recorded, then torn down.
pub(crate) fn matrix(
    model: PathBuf,
    engines: Vec<String>,
    ollama: Vec<String>,
    opts: GridOpts,
    out: Option<PathBuf>,
) -> Result<()> {
    let cells = build_cells(&model, &engines, &ollama)?;

    eprintln!(
        "[matrix] grid: {} cell(s), {} task(s) × {} run(s), max_tokens={}",
        cells.len(),
        opts.tasks.len(),
        opts.runs,
        opts.max_tokens
    );

    let rows = deck_engines::matrix::run_matrix(
        &cells,
        &opts.tasks,
        opts.runs,
        opts.max_tokens,
        Duration::from_secs(240),
        &opts.bins,
    );

    for r in &rows {
        let tps = r
            .tok_s
            .map(|t| format!("{t:>8.1} tok/s"))
            .unwrap_or_else(|| "       —   ".into());
        let gen_count = r
            .gen_tokens
            .map(|g| g.to_string())
            .unwrap_or_else(|| "-".into());
        println!(
            "{:<10} {:<22} {:<14} {:<8} {tps} ({} {gen_count} tok) {:>7}ms  {}",
            r.engine,
            r.model,
            r.task,
            r.verdict,
            r.tok_s_kind,
            r.wall_ms,
            chrono_like(r.at)
        );
    }
    if let Some(p) = out {
        let json = serde_json::to_string_pretty(&rows)?;
        std::fs::write(&p, json)?;
        eprintln!("[matrix] wrote {} trial(s) to {p:?}", rows.len());
    }
    Ok(())
}

/// Blind A/B over the grid: same execution as `matrix` (every trial persists to
/// `matrix_runs`), scored offline and re-revealed under opaque trial ids. The
/// printed table is the scored ranking; the JSON (--out) carries every output
/// grouped by trial id so the operator can evaluate before trusting the score.
pub(crate) fn compare(
    model: PathBuf,
    engines: Vec<String>,
    ollama: Vec<String>,
    opts: GridOpts,
    seed: u64,
    out: Option<PathBuf>,
) -> Result<()> {
    let cells = build_cells(&model, &engines, &ollama)?;
    let compare_opts = deck_engines::compare::CompareOpts {
        tasks: &opts.tasks,
        runs: opts.runs,
        max_tokens: opts.max_tokens,
        boot_timeout: Duration::from_secs(240),
        bins: &opts.bins,
    };
    eprintln!(
        "[compare] grid: {} candidate(s), {} task(s) × {} run(s), seed={seed}",
        cells.len(),
        opts.tasks.len(),
        opts.runs
    );

    let report = deck_engines::compare::run_compare(&cells, &compare_opts, seed);

    println!(
        "trial        {:>6}  {:>7}  {:<5} {:>8}   candidate",
        "avg", "avg", "verdict", "score"
    );
    for c in &report.candidates {
        let avg_tok = c
            .mean_tok_s
            .map(|t| format!("{t:>7.1}"))
            .unwrap_or_else(|| "     —".into());
        let v = c.verdict.as_deref().unwrap_or("");
        println!(
            "{:<12} {avg_tok}  {:>7.3}  {:<5} {:<10} × {:<16} (ctx {})",
            c.trial, c.mean_score, v, c.engine, c.model, c.ctx
        );
        if let Some(f) = &c.failure {
            println!("  {:<24} ⚠ {f}", "");
        }
    }
    println!("verdict: {}", report.verdict);
    if let Some(p) = out {
        let json = serde_json::to_string_pretty(&report)?;
        std::fs::write(&p, json)?;
        eprintln!(
            "[compare] wrote {} scored trial(s) to {p:?}",
            report.trials.len()
        );
    }
    Ok(())
}

/// Show the best tok/s per (model, engine) across stored bench history.
pub(crate) fn best() -> Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_bench_schema(&conn)?;
    let rows = deck_core::store::recent_bench(&conn, 500)?;
    if rows.is_empty() {
        println!("no benchmark readings yet — run `deck bench record` against a live engine");
        return Ok(());
    }
    let mut groups: HashMap<(String, String), Vec<f64>> = HashMap::new();
    let mut latest: HashMap<(String, String), (f64, i64)> = HashMap::new();
    for r in &rows {
        let key = (r.model.clone(), r.engine.clone());
        groups.entry(key.clone()).or_default().push(r.tps);
        let entry = latest.entry(key).or_insert((r.tps, r.at));
        if r.at > entry.1 {
            entry.1 = r.at;
            entry.0 = r.tps;
        }
    }
    let mut summary: Vec<_> = groups
        .into_iter()
        .map(|(k, tps)| {
            let best = tps.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let avg = tps.iter().sum::<f64>() / tps.len() as f64;
            let latest_tps = latest.get(&k).map(|(t, _)| *t).unwrap_or(best);
            (k, best, avg, latest_tps, tps.len())
        })
        .collect();
    summary.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    println!(
        "{:<32} {:<12} {:>7} {:>7} {:>7} {}",
        "model", "engine", "best", "latest", "avg", "runs"
    );
    for ((model, engine), best, _avg, latest, count) in summary {
        let short = std::path::Path::new(&model)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_else(|| model.as_str());
        println!(
            "{:<32} {:<12} {:>7.1} {:>7.1} {:>7.1} {}",
            short, engine, best, latest, _avg, count
        );
    }
    Ok(())
}
