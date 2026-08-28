//! Blind A/B comparison on top of the bench matrix grid.
//!
//! `run_compare` executes the same grid as `run_matrix` (every raw trial is
//! persisted to `matrix_runs` exactly as before), then re-presents the results
//! blind: each candidate is assigned an opaque `trial-NNN` id, outputs are
//! grouped under those ids, and only the final verdict maps id → candidate.
//!
//! The scorecard itself — `quality`, `normalized_throughput`, the PRNG order —
//! lives in [`crate::scoring`]; this module assembles rows into a report and
//! names the winner, keeping the science and the pipeline separate.

use std::collections::HashMap;
use std::time::Duration;

use deck_core::profile::Engine;
use deck_core::store::MatrixRow;

use crate::scoring::{normalized_throughput, quality, shuffled_indices};

pub use crate::matrix::MatrixCell;

/// Knobs shared with the CLI's bench matrix parser.
pub struct CompareOpts<'a> {
    pub tasks: &'a [(String, String)],
    pub runs: u32,
    pub max_tokens: u32,
    pub boot_timeout: Duration,
    pub bins: &'a HashMap<Engine, std::path::PathBuf>,
}

/// One scored trial — the report's leaf row, shown under an opaque trial id.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScoredTrial {
    pub trial: String,
    pub task: String,
    pub run: u32,
    pub ok: bool,
    /// Engine error summary when the trial failed (boot failure, etc.).
    pub error: Option<String>,
    pub gen_tokens: Option<u64>,
    pub prompt_tokens: Option<u64>,
    pub tok_s: Option<f64>,
    pub tok_s_kind: String,
    pub wall_ms: u64,
    pub score: f64,
    pub output: String,
}

/// A candidate's aggregate standing (trial id stays opaque until the verdict).
#[derive(Debug, Clone, serde::Serialize)]
pub struct CandidateStanding {
    pub trial: String,
    pub engine: String,
    pub model: String,
    pub ctx: u32,
    pub ok_runs: u32,
    pub trials: u32,
    pub mean_tok_s: Option<f64>,
    pub mean_score: f64,
    /// First failure summary (boot crash, task error) when no run succeeded.
    pub failure: Option<String>,
    pub verdict: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CompareReport {
    pub procedure: String,
    pub tasks: Vec<String>,
    /// Opaque trial id → candidate mapping, REVEALED here (the verdict also
    /// names the winner in prose).
    pub candidates: Vec<CandidateStanding>,
    pub trials: Vec<ScoredTrial>,
    pub verdict: String,
}

/// Group rows into candidates (engine × model), assign blind trial ids, score
/// every trial, and name the verdict. Rows come from `run_matrix`, so they are
/// already persisted to `matrix_runs`.
pub fn report_from_rows(rows: Vec<MatrixRow>, seed: u64) -> CompareReport {
    // Group trial rows by candidate key (engine × model).
    let mut by_candidate: Vec<(String, Vec<MatrixRow>)> = Vec::new();
    {
        let mut order: Vec<String> = Vec::new();
        let mut map: HashMap<String, Vec<MatrixRow>> = HashMap::new();
        for r in rows {
            let k = format!("{} × {}", r.engine, r.model);
            if !map.contains_key(&k) {
                order.push(k.clone());
            }
            map.entry(k.clone()).or_default().push(r);
        }
        for k in order {
            let v = map.remove(&k).unwrap_or_default();
            by_candidate.push((k, v));
        }
    }

    // Opaque trial id per candidate (order randomized by seed).
    let perm = shuffled_indices(by_candidate.len(), seed);
    let mut trial_id: HashMap<String, String> = HashMap::new();
    for (pos, &cix) in perm.iter().enumerate() {
        trial_id.insert(by_candidate[cix].0.clone(), format!("trial-{:03}", pos + 1));
    }

    // Per-task tok/s max for normalization (tasks in first-seen order).
    let mut task_max: HashMap<String, f64> = HashMap::new();
    let mut task_order: Vec<String> = Vec::new();
    for (_, cand_rows) in &by_candidate {
        for r in cand_rows {
            if !task_max.contains_key(&r.task) {
                task_order.push(r.task.clone());
            }
            if let Some(t) = r.tok_s {
                let m = task_max.entry(r.task.clone()).or_insert(0.0);
                *m = m.max(t);
            }
        }
    }

    let mut trials: Vec<ScoredTrial> = Vec::new();
    let mut standings: Vec<CandidateStanding> = Vec::new();
    for (key, cand_rows) in &by_candidate {
        let trial = trial_id[key].clone();
        let mut scored = 0u32;
        let mut ok_runs = 0u32;
        let mut score_sum = 0.0;
        let mut tok_s_sum = 0.0;
        let mut ctx = 0u32;
        let mut engine = String::new();
        let mut model = String::new();
        let mut failure: Option<String> = None;
        for r in cand_rows {
            ctx = r.ctx;
            engine = r.engine.clone();
            model = r.model.clone();
            if r.verdict != "RUNNING" && failure.is_none() {
                let s = r.summary.trim();
                failure = Some(if s.is_empty() {
                    r.verdict.clone()
                } else {
                    format!("{}: {}", r.verdict, s)
                });
            }
            let gmax = task_max.get(&r.task).copied().unwrap_or(0.0);
            let sc = if r.verdict == "RUNNING" {
                let q = quality(&r.output);
                let t = normalized_throughput(r.tok_s, gmax);
                0.6 * q + 0.4 * t
            } else {
                0.0
            };
            scored += 1;
            if r.verdict == "RUNNING" {
                ok_runs += 1;
                if let Some(t) = r.tok_s {
                    tok_s_sum += t;
                }
            }
            score_sum += sc;
            trials.push(ScoredTrial {
                trial: trial.clone(),
                task: r.task.clone(),
                run: r.run,
                ok: r.verdict == "RUNNING",
                error: if r.verdict == "RUNNING" {
                    None
                } else {
                    Some(r.summary.clone())
                },
                gen_tokens: r.gen_tokens,
                prompt_tokens: r.prompt_tokens,
                tok_s: r.tok_s,
                tok_s_kind: r.tok_s_kind.clone(),
                wall_ms: r.wall_ms,
                score: sc,
                output: r.output.clone(),
            });
        }
        standings.push(CandidateStanding {
            trial: trial.clone(),
            engine,
            model,
            ctx,
            ok_runs,
            trials: scored,
            mean_tok_s: if ok_runs > 0 {
                Some(tok_s_sum / ok_runs as f64)
            } else {
                None
            },
            mean_score: if scored > 0 {
                score_sum / scored as f64
            } else {
                0.0
            },
            failure,
            verdict: None,
        });
    }

    let verdict = choose_verdict(&mut standings);

    CompareReport {
        procedure: concat!(
            "score = 0.6·quality + 0.4·throughput; ",
            "quality = 0.25·variety + 0.75·(1 − bigram repetition) on the output; ",
            "throughput = native/wall tok/s normalized to the task group's max; ",
            "a candidate's standing is the mean of its trial scores; the verdict ",
            "names the top mean score (tie broken by mean tok/s)."
        )
        .to_string(),
        tasks: task_order,
        candidates: standings,
        trials,
        verdict,
    }
}

/// Name the winner by mean score (tie → mean tok/s), annotate both, prose.
fn choose_verdict(standings: &mut [CandidateStanding]) -> String {
    if standings.is_empty() {
        return "no trials to compare".into();
    }
    let mut sorted = standings.to_vec();
    sorted.sort_by(|a, b| {
        b.mean_score
            .partial_cmp(&a.mean_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.mean_tok_s
                    .unwrap_or(0.0)
                    .partial_cmp(&a.mean_tok_s.unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    for s in standings.iter_mut() {
        s.verdict = if s.trial == sorted[0].trial {
            Some("BEST".into())
        } else if sorted.get(1).is_some_and(|w| s.trial == w.trial) {
            Some("runner-up".into())
        } else {
            None
        };
    }
    let top = &sorted[0];
    let rest = if sorted.len() > 1 {
        format!(
            " runner-up {} ({:.3})",
            sorted[1].trial, sorted[1].mean_score
        )
    } else {
        String::new()
    };
    format!(
        "{} wins this grid with a mean score of {:.3} ({:.1} tok/s avg, {}/{} runs OK).{}",
        top.trial,
        top.mean_score,
        top.mean_tok_s.unwrap_or(0.0),
        top.ok_runs,
        top.trials,
        rest
    )
}

/// Execute the grid blind: run everything through `run_matrix` (persisting to
/// `matrix_runs`), then score and blind.
pub fn run_compare(cells: &[MatrixCell], opts: &CompareOpts<'_>, seed: u64) -> CompareReport {
    let rows = crate::matrix::run_matrix(
        cells,
        opts.tasks,
        opts.runs,
        opts.max_tokens,
        opts.boot_timeout,
        opts.bins,
    );
    report_from_rows(rows, seed)
}
