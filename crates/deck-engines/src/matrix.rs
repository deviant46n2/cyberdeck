//! The bench matrix: a headless **model × quant × engine** grid.
//!
//! The user-facing intent: "test one model against 8 quants, each quant through
//! llama.cpp or FreeToken (or whatever runtime comes next), and get parseable
//! numbers to find the best model→task assignment."
//!
//! The grid is a flat list of `MatrixCell`s — every runtime that can actually
//! serve a given model appears as one cell. A local GGUF quant therefore grids
//! across local-source engines (llama.cpp, FreeToken); an Ollama model id grids
//! only through Ollama, because Ollama cannot load an arbitrary ~/models quant.
//! That constraint is structural, not cosmetic.
//!
//! Each cell boots its engine headlessly on the engine's dedicated test port
//! (never touching any live resident), runs every task the requested number of
//! times, records each trial, then tears down. Cells run one at a time so VRAM
//! is never contended. Every trial keeps the RAW ingredients (prompt/gen token
//! counts, wall ms) so the consumer can recompute derived metrics.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use deck_core::profile::{Engine, Profile};
use deck_core::store::MatrixRow;

use crate::health::boot_on_test_port;
use crate::inference::{GenSample, run_prompt};

/// One (model × engine) cell of the grid.
#[derive(Debug, Clone)]
pub struct MatrixCell {
    pub engine: Engine,
    /// Model id for the request: a local GGUF path, or an Ollama model id.
    pub model_id: String,
    /// Human label: the quant filename or the Ollama id.
    pub display: String,
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn fail_row(
    cell: &MatrixCell,
    ctx: u32,
    task: &str,
    run: u32,
    verdict: &str,
    summary: &str,
) -> MatrixRow {
    MatrixRow {
        engine: cell.engine.store_id().to_string(),
        model: cell.display.clone(),
        ctx,
        task: task.to_string(),
        run,
        verdict: verdict.to_string(),
        summary: summary.to_string(),
        gen_tokens: None,
        prompt_tokens: None,
        tok_s: None,
        tok_s_kind: "native".into(),
        wall_ms: 0,
        output: String::new(),
        at: now_epoch(),
        workload_id: None,
        hardware_profile_id: None,
        engine_version: None,
        prompt_tps: None,
        ttft_ms: None,
        peak_vram_mb: None,
        model_rev: None,
        sampling_json: None,
        role_id: None,
    }
}

/// Plan the launch profile for a cell: local sources derive a best-max-ctx
/// loadout against real VRAM; Ollama is a daemon that doesn't take a model at
/// launch, so the profile just fixes where it binds. An explicit `--bin`
/// override wins over the derived default.
fn plan_profile(cell: &MatrixCell, bins: &HashMap<Engine, PathBuf>) -> Result<Profile, String> {
    match cell.engine.model_source() {
        deck_core::profile::ModelSource::LocalPath => {
            let mut d = deck_core::profile::derive_loadout(&cell.model_id, cell.engine)
                .map_err(|e| format!("derive failed for {}: {e}", cell.display))?;
            if let Some(b) = bins.get(&cell.engine) {
                d.profile.bin = b.clone();
            }
            Ok(d.profile)
        }
        deck_core::profile::ModelSource::OllamaStore => Ok(Profile {
            engine: Engine::Ollama,
            bin: bins
                .get(&Engine::Ollama)
                .cloned()
                .unwrap_or_else(|| PathBuf::from("ollama")),
            host: "127.0.0.1".into(),
            model: cell.model_id.clone(),
            alias: cell.model_id.clone(),
            ..Profile::default()
        }),
    }
}

/// Run a single cell (boot → sample all tasks/runs → teardown).
fn run_cell(
    cell: &MatrixCell,
    bins: &HashMap<Engine, PathBuf>,
    tasks: &[(String, String)],
    runs: u32,
    max_tokens: u32,
    boot_timeout: Duration,
    rows: &mut Vec<MatrixRow>,
) {
    let profile = match plan_profile(cell, bins) {
        Ok(p) => p,
        Err(e) => {
            for (task, _) in tasks {
                rows.push(fail_row(cell, 0, task, 0, "ERROR", &e));
            }
            return;
        }
    };
    let test_port = cell.engine.test_port();
    let ctx = profile.ctx_size;
    let mut child = match boot_on_test_port(&profile, test_port, boot_timeout) {
        Ok(c) => c,
        Err((v, s)) => {
            for (task, _) in tasks {
                rows.push(fail_row(cell, ctx, task, 0, &v, &s));
            }
            return;
        }
    };
    let host = profile.host.clone();
    for (task, prompt) in tasks {
        for run in 0..runs {
            let s: GenSample = run_prompt(
                cell.engine,
                &host,
                test_port,
                &cell.model_id,
                prompt,
                max_tokens,
            );
            let at = now_epoch();
            let verdict = if s.ok { "RUNNING" } else { "ERROR" };
            rows.push(MatrixRow {
                engine: cell.engine.store_id().to_string(),
                model: cell.display.clone(),
                ctx,
                task: task.clone(),
                run,
                verdict: verdict.to_string(),
                summary: s.error.clone().unwrap_or_default(),
                gen_tokens: s.gen_tokens,
                prompt_tokens: s.prompt_tokens,
                tok_s: s.tok_s,
                tok_s_kind: s.tok_s_kind.to_string(),
                wall_ms: s.wall_ms,
                output: s.text,
                at,
                workload_id: None,
                hardware_profile_id: None,
                engine_version: None,
                prompt_tps: s.prompt_tps,
                ttft_ms: s.ttft_ms,
                peak_vram_mb: None,
                model_rev: None,
                sampling_json: None,
                role_id: None,
            });
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Run the whole grid and persist every trial to the `matrix_runs` table.
pub fn run_matrix(
    cells: &[MatrixCell],
    tasks: &[(String, String)],
    runs: u32,
    max_tokens: u32,
    boot_timeout: Duration,
    bins: &HashMap<Engine, PathBuf>,
) -> Vec<MatrixRow> {
    let mut rows = Vec::new();
    for cell in cells {
        eprintln!(
            "[matrix] cell: {} × {} → test :{}",
            cell.display,
            cell.engine.descriptor().display,
            cell.engine.test_port()
        );
        run_cell(cell, bins, tasks, runs, max_tokens, boot_timeout, &mut rows);
    }
    if let Ok(conn) = deck_core::store::open(&deck_core::store::default_db_path()) {
        let _ = deck_core::store::ensure_matrix_schema(&conn);
        let _ = deck_core::store::ensure_evaluations_schema(&conn);
        let hw_id = deck_core::store::capture_hardware_profile(&conn).ok();
        // Build evaluator map from any workload that owns the task label
        let mut eval_map: std::collections::HashMap<String, (String, String)> = std::collections::HashMap::new();
        if let Ok(ws) = deck_core::store::list_workloads(&conn) {
            for w in ws {
                for t in w.tasks { eval_map.entry(t.label).or_insert((t.evaluator, t.evaluator_config)); }
            }
        }
        for r in &rows {
            let mut r2 = r.clone();
            r2.hardware_profile_id = hw_id;
            if let Ok(id) = deck_core::store::insert_matrix_run(&conn, &r2) {
                let (ev, cfg) = eval_map.get(&r.task).cloned().unwrap_or(("lexical-placeholder".into(), "".into()));
                let evaluator = crate::evaluation::evaluator_for(&ev, &cfg);
                if let Ok(ev) = evaluator.evaluate(&r.output, id) {
                    let _ = deck_core::store::insert_evaluation(&conn, &ev);
                }
            }
        }
    }
    rows
}
