//! Benchmark: probe a live engine, list history, or run the model × quant ×
//! engine matrix grid.

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::Duration;

use deck_core::profile::{Engine, ModelSource};

use super::parse_engine;

pub(crate) fn record(
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

/// Top-level `*.gguf` files of a directory, or the single file itself.
fn quant_files(model: &Path) -> Result<Vec<PathBuf>> {
    if model.is_dir() {
        let mut files: Vec<PathBuf> = std::fs::read_dir(model)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "gguf"))
            .collect();
        if files.is_empty() {
            anyhow::bail!("no *.gguf files found in {model:?}");
        }
        files.sort();
        Ok(files)
    } else if model.extension().is_some_and(|x| x == "gguf") {
        Ok(vec![model.to_path_buf()])
    } else {
        anyhow::bail!("{model:?} is neither a directory nor a .gguf file")
    }
}

/// Parse repeatable `--task "label=prompt"` flags.
fn parse_tasks(tasks: &[String]) -> Result<Vec<(String, String)>> {
    if tasks.is_empty() {
        anyhow::bail!("pass at least one --task \"label=prompt\"");
    }
    tasks
        .iter()
        .map(|t| {
            t.split_once('=')
                .map(|(l, p)| (l.trim().to_string(), p.trim().to_string()))
                .filter(|(l, p)| !l.is_empty() && !p.is_empty())
                .ok_or_else(|| anyhow::anyhow!("--task must be \"label=prompt\", got {t:?}"))
        })
        .collect()
}

/// Parsed matrix knobs shared down to the grid runner.
pub(crate) struct GridOpts {
    pub(crate) tasks: Vec<(String, String)>,
    pub(crate) runs: u32,
    pub(crate) max_tokens: u32,
    pub(crate) bins: std::collections::HashMap<Engine, PathBuf>,
}

impl GridOpts {
    pub(crate) fn parse(
        tasks: &[String],
        runs: u32,
        max_tokens: u32,
        bin: &[String],
    ) -> Result<Self> {
        let tasks = parse_tasks(tasks)?;
        let mut bins: std::collections::HashMap<Engine, PathBuf> = std::collections::HashMap::new();
        for b in bin {
            let (e, path) = b
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("--bin must be \"engine=path\", got {b:?}"))?;
            bins.insert(parse_engine(e)?, PathBuf::from(path));
        }
        // Seed any engine without an explicit --bin from the per-engine DB
        // config, so a machine configured once needs no repeated flags.
        if let Ok(conn) = deck_core::store::open(&deck_core::store::default_db_path()) {
            for e in [Engine::LlamaCpp, Engine::FreeToken, Engine::Ollama] {
                if !bins.contains_key(&e)
                    && let Ok(Some(b)) = deck_core::store::get_engine_bin(&conn, e.store_id())
                {
                    bins.insert(e, PathBuf::from(b));
                }
            }
        }
        Ok(Self {
            tasks,
            runs,
            max_tokens,
            bins,
        })
    }
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
    let parsed_engines: Vec<Engine> = engines
        .iter()
        .map(|e| parse_engine(e))
        .collect::<Result<_>>()?;
    for e in &parsed_engines {
        if e.model_source() != ModelSource::LocalPath {
            anyhow::bail!(
                "{} cannot serve arbitrary ~/models GGUFs — put Ollama models on --ollama",
                e.descriptor().display
            );
        }
    }

    let mut cells: Vec<deck_engines::matrix::MatrixCell> = Vec::new();
    for f in quant_files(&model)? {
        let label = f
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| f.display().to_string());
        for e in &parsed_engines {
            cells.push(deck_engines::matrix::MatrixCell {
                engine: *e,
                model_id: f.display().to_string(),
                display: label.clone(),
            });
        }
    }
    for oid in ollama {
        cells.push(deck_engines::matrix::MatrixCell {
            engine: Engine::Ollama,
            model_id: oid.clone(),
            display: oid,
        });
    }
    if cells.is_empty() {
        anyhow::bail!("nothing to grid — pass --model <gguf|dir> and/or --ollama ids");
    }

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
