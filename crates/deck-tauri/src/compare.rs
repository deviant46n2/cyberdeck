//! Compare door for the UI: runs the same blind A/B grid as `deck bench
//! compare` (built from the shared grid plumbing in deck-engines) and returns
//! the serializable report so the Compare tab can render the scored ranking and
//! hand it to an agent for synthesis. Long-running — callers should run it off
//! the UI thread (spawn_blocking).

use std::path::Path;
use std::time::Duration;

use deck_core::profile::Engine;
use tauri::Emitter;

pub use deck_engines::compare::{CandidateStanding, CompareReport, ScoredTrial};

fn parse_engine(s: &str) -> Result<Engine, String> {
    Engine::parse(s).ok_or_else(|| format!("unknown engine '{s}' (llamacpp|freetoken|ollama)"))
}

/// Run the blind compare grid and return the report.
///
/// `models` are GGUF files and/or directories of top-level GGUFs (one cell
/// per file × engine); `engines` are local-source engine ids; `ollama` are
/// Ollama model ids. `tasks` are `"label=prompt"` strings merged with the
/// `workload`'s tasks when given. Binaries resolve from the per-engine store
/// config (machine config, no UI input) just like the CLI.
///
/// `live` selects the VRAM-safe path (no headless boots):
/// - `Some("auto")` — each engine runs against its own UP slot from the port
///   map (errors naming any engine whose slot is down);
/// - `Some("host:port")` — every cell samples that server;
/// - `None` — boot headless cells on the test ports.
#[allow(clippy::too_many_arguments)]
pub fn compare_run(
    app: &tauri::AppHandle,
    models: Vec<String>,
    engines: Vec<String>,
    ollama: Vec<String>,
    tasks: Vec<String>,
    runs: u32,
    max_tokens: u32,
    seed: u64,
    live: Option<String>,
    workload: Option<String>,
) -> Result<CompareReport, String> {
    if models.is_empty() && ollama.is_empty() {
        return Err("no candidates — pick at least one model".into());
    }
    if runs == 0 {
        return Err("runs must be at least 1".into());
    }
    if max_tokens == 0 {
        return Err("max_tokens must be at least 1".into());
    }
    let parsed: Vec<Engine> = engines
        .iter()
        .map(|e| parse_engine(e))
        .collect::<Result<_, _>>()?;
    if parsed.is_empty() && ollama.is_empty() {
        return Err("no engines — pick at least one engine".into());
    }
    let mut cells = Vec::new();
    for m in &models {
        let mut c = deck_engines::grid::build_cells(Path::new(m), &parsed, &[])
            .map_err(|e| format!("{m}: {e}"))?;
        cells.append(&mut c);
    }
    // Ollama ids grid without touching the GGUF path logic (build_cells
    // requires a real model path for its quant scan, so construct directly).
    for oid in &ollama {
        cells.push(deck_engines::matrix::MatrixCell {
            engine: Engine::Ollama,
            model_id: oid.clone(),
            display: oid.clone(),
        });
    }
    // Dedupe identical (engine, model) cells from overlapping picks.
    cells.sort_by(|a, b| {
        (a.engine.store_id(), &a.model_id).cmp(&(b.engine.store_id(), &b.model_id))
    });
    cells.dedup_by(|a, b| {
        a.engine.store_id() == b.engine.store_id() && a.model_id == b.model_id
    });
    if cells.is_empty() {
        return Err("no runnable cells — check the model paths exist".into());
    }
    let mut all_tasks = expand_workload_tasks(workload.as_deref())?;
    let mut extra = deck_engines::grid::parse_tasks(&tasks).map_err(|e| e.to_string())?;
    all_tasks.append(&mut extra);
    if all_tasks.is_empty() {
        return Err("no tasks — pick a workload or add label=prompt tasks".into());
    }
    // Per-trial heartbeat → "compare-step" events so the UI streams the run
    // instead of going dark for minutes.
    let progress = |s: &str| {
        let _ = app.emit("compare-step", serde_json::json!({ "step": s }));
    };
    match live.as_deref() {
        Some("auto") => run_live_auto(app, &cells, &all_tasks, runs, max_tokens, workload.as_deref(), seed),
        Some(addr) => {
            let (host, port) = parse_host_port(addr)?;
            Ok(deck_engines::compare::run_compare_live(
                &cells,
                &host,
                port,
                &all_tasks,
                runs,
                max_tokens,
                workload.as_deref(),
                seed,
                Some(&progress),
            ))
        }
        None => {
            let bins = deck_engines::grid::resolve_bins(&std::collections::HashMap::new());
            let opts = deck_engines::compare::CompareOpts {
                tasks: &all_tasks,
                runs,
                max_tokens,
                boot_timeout: Duration::from_secs(240),
                bins: &bins,
            };
            Ok(deck_engines::compare::run_compare(&cells, &opts, seed, Some(&progress)))
        }
    }
}

/// Expand a workload id into its (label, prompt) tasks. Unknown ids are an
/// error, not silence — a typo'd workload must not run an empty grid.
fn expand_workload_tasks(workload: Option<&str>) -> Result<Vec<(String, String)>, String> {
    let Some(wid) = workload.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(Vec::new());
    };
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db).map_err(|e| e.to_string())?;
    deck_core::store::ensure_seeded_workloads(&conn).map_err(|e| e.to_string())?;
    let w = deck_core::store::get_workload(&conn, wid)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("unknown workload '{wid}'"))?;
    Ok(w.tasks.into_iter().map(|t| (t.label, t.prompt)).collect())
}

/// Live-auto: group cells by engine, sample each group against that engine's
/// UP slot, then blind once over the merged rows (trial ids stay unique).
fn run_live_auto(
    app: &tauri::AppHandle,
    cells: &[deck_engines::matrix::MatrixCell],
    tasks: &[(String, String)],
    runs: u32,
    max_tokens: u32,
    workload: Option<&str>,
    seed: u64,
) -> Result<CompareReport, String> {
    let slots = crate::portmap::port_map_status("127.0.0.1");
    let mut by_engine: std::collections::HashMap<&str, Vec<deck_engines::matrix::MatrixCell>> =
        std::collections::HashMap::new();
    for c in cells {
        by_engine.entry(c.engine.store_id()).or_default().push(c.clone());
    }
    // Deterministic engine order so reports are stable across runs.
    let mut engines: Vec<&str> = by_engine.keys().copied().collect();
    engines.sort();
    let mut rows = Vec::new();
    for eng in engines {
        let slot = slots
            .iter()
            .find(|s| s.engine == eng)
            .ok_or_else(|| format!("no port-map slot for engine '{eng}'"))?;
        if slot.state != "up" {
            return Err(format!(
                "live compare needs the {eng} slot UP (it's {}) — LOAD a flavor first or uncheck live mode",
                slot.state
            ));
        }
        let progress = |s: &str| {
            let _ = app.emit("compare-step", serde_json::json!({ "step": s }));
        };
        let mut r = deck_engines::matrix::run_matrix_live(
            &by_engine[eng],
            "127.0.0.1",
            slot.port,
            tasks,
            runs,
            max_tokens,
            workload,
            Some(&progress),
        );
        rows.append(&mut r);
    }
    Ok(deck_engines::compare::report_from_rows(rows, seed))
}

/// Parse a "host:port" address (bare "port" defaults to 127.0.0.1).
fn parse_host_port(addr: &str) -> Result<(String, u16), String> {
    let (host, port_s) = match addr.rsplit_once(':') {
        Some(("", p)) => ("127.0.0.1", p),
        Some((h, p)) if !h.is_empty() => (h, p),
        _ => ("127.0.0.1", addr.trim_start_matches(':')),
    };
    if host.contains(':') || port_s.is_empty() {
        return Err(format!("live must be \"host:port\", got {addr:?}"));
    }
    let port: u16 = port_s
        .parse()
        .map_err(|_| format!("live must be \"host:port\", got {addr:?}"))?;
    Ok((host.to_string(), port))
}
