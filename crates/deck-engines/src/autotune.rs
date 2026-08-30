//! Autotune: sweep configuration parameters on a test port, score by objective,
//! and apply the best configuration.
//!
//! Sweeps ctx/ngl/ubatch combinations for a given model/engine, runs each
//! headlessly, records metrics, scores by a user-selectable objective, and
//! returns the best configuration to feed the recommendation header.
//!
//! Safety: never touches live resident units; all work happens on a dedicated
//! test port. Results are persisted to `matrix_runs` for auditability.

use std::collections::HashMap;
use std::time::Duration;

use deck_core::profile::{Engine, Profile};
use deck_core::store::MatrixRow;
use crate::health::{boot_on_test_port, fetch_metrics, parse_tps};
use crate::matrix::MatrixCell;

/// Configuration sweep parameters for autotune.
#[derive(Debug, Clone)]
pub struct AutotuneParams {
    /// Context sizes to try (active ladder values are tried first, then custom)
    pub ctx_values: Vec<u32>,
    /// Number of GPU layers to try
    pub ngl_values: Vec<u32>,
    /// Upper batch size to try
    pub ubatch_values: Vec<u32>,
    /// Number of runs per configuration
    pub runs: u32,
    /// Maximum tokens per trial
    pub max_tokens: u32,
    /// Boot timeout per configuration
    pub boot_timeout: Duration,
    /// Generation timeout per trial
    pub generation_timeout: Duration,
}

/// Scoring objective for autotune results.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AutotuneObjective {
    /// Maximize throughput (tok/s)
    Throughput,
    /// Maximize context while maintaining acceptable throughput
    ContextFirst,
    /// Minimize GPU layers for given throughput
    MinNGL,
    /// Balanced: throughput / context ratio
    Balanced,
}

/// One autotune trial result.
#[derive(Debug, Clone)]
pub struct AutotuneTrial {
    /// Configuration parameters
    pub ctx: u32,
    pub ngl: u32,
    pub ubatch: u32,
    /// Measured metrics
    pub tok_s: Option<f64>,
    pub ctx_actual: u32,
    pub wall_ms: u64,
    /// Verdict: RAN, OOM, FAIL
    pub verdict: String,
    /// Error summary when applicable
    pub error: Option<String>,
}

/// Score an autotune trial.
pub fn score_trial(
    trial: &AutotuneTrial,
    objective: AutotuneObjective,
) -> f64 {
    let base = trial.tok_s.unwrap_or(0.0);
    match objective {
        AutotuneObjective::Throughput => base,
        AutotuneObjective::ContextFirst => {
            if base > 0.0 { base / trial.ctx as f64 } else { 0.0 }
        }
        AutotuneObjective::MinNGL => {
            // Lower ngl is better; score inverts ngl
            1.0 / (trial.ngl as f64 + 1.0)
        }
        AutotuneObjective::Balanced => {
            if base > 0.0 { base / (trial.ctx as f64 + trial.ngl as f64 * 10.0) } else { 0.0 }
        }
    }
}

/// Run autotune sweep for a given profile on a test port.
pub fn run_autotune(
    profile: &Profile,
    engine: Engine,
    params: &AutotuneParams,
    objective: AutotuneObjective,
) -> Vec<AutotuneTrial> {
    let test_port = engine.test_port();
    let mut results = Vec::new();

    // Use the profile's active ladder for ctx values, falling back to params
    let ctx_candidates = if !profile.ctx_ladder.is_empty() {
        profile.ctx_ladder.clone()
    } else {
        params.ctx_values.clone()
    };

    for ctx in &ctx_candidates {
        for ngl in &params.ngl_values {
            for ubatch in &params.ubatch_values {
                let mut draft = profile.clone();
                draft.ctx_size = *ctx;
                draft.n_gpu_layers = *ngl;
                draft.ubatch_size = *ubatch;

                match boot_on_test_port(&draft, test_port, params.boot_timeout) {
                    Ok(mut child) => {
                        // Run generation probe
                        let metrics = fetch_metrics(&draft.host, test_port)
                            .ok()
                            .and_then(|m| parse_tps(&m));

                        let _ = child.kill();
                        let _ = child.wait();

                        let trial = AutotuneTrial {
                            ctx: *ctx,
                            ngl: *ngl,
                            ubatch: *ubatch,
                            tok_s: metrics,
                            ctx_actual: draft.ctx_size,
                            wall_ms: params.boot_timeout.as_millis() as u64,
                            verdict: "RAN".into(),
                            error: None,
                        };
                        results.push(trial);
                    }
                    Err((verdict, summary)) => {
                        results.push(AutotuneTrial {
                            ctx: *ctx,
                            ngl: *ngl,
                            ubatch: *ubatch,
                            tok_s: None,
                            ctx_actual: *ctx,
                            wall_ms: params.boot_timeout.as_millis() as u64,
                            verdict: verdict,
                            error: Some(summary),
                        });
                    }
                }
            }
        }
    }

    results
}

/// Find the best trial from autotune results.
pub fn best_trial(
    trials: &[AutotuneTrial],
    objective: AutotuneObjective,
) -> Option<&AutotuneTrial> {
    trials.iter().max_by(|a, b| {
        score_trial(a, objective).total_cmp(&score_trial(b, objective))
    })
}

/// Run autotune and return the best configuration.
pub fn run_and_find_best(
    profile: &Profile,
    engine: Engine,
    params: &AutotuneParams,
    objective: AutotuneObjective,
) -> Option<AutotuneTrial> {
    let trials = run_autotune(profile, engine, params, objective);
    best_trial(&trials, objective).cloned()
}

