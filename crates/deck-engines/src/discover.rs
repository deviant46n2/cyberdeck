//! Configuration discovery: turn fit *estimates* into measured facts.
//!
//! `plan_trials` enumerates (runtime × ctx) configs for one artifact using the
//! same fit math as Make-it-work — one trial per compatible runtime at its max
//! ctx by default, plus a half-ctx point at `variants >= 2`. `run_trial` boots
//! each config on its own test port, takes a real probe generation, and builds
//! `matrix_runs` rows (task `discovery-probe`) so fit candidates can render
//! TESTED instead of estimated. Cells run one at a time; VRAM is never shared.

use std::path::{Path, PathBuf};
use std::time::Duration;

use deck_core::profile::{Engine, EngineProtocol, Profile};
use deck_core::store::MatrixRow;
use serde::Serialize;

use crate::health::{PROBE_PROMPT, boot_on_test_port, fetch_metrics, parse_tps};
use crate::inference::run_prompt_protocol;
use crate::unit::manifest_for;

/// The shared probe workload: fixed content, 192 gen tokens — the same ruler
/// bringup benches with, so a discovery number and a bringup number compare.
pub const PROBE_TASK: &str = "discovery-probe";

/// One (runtime × ctx) configuration to measure.
#[derive(Debug, Clone)]
pub struct TrialPlan {
    pub runtime_id: String,
    pub display: String,
    pub test_port: u16,
    pub profile: Profile,
    pub label: String,
}

/// Enumerate trial configs. `runtimes`: None = every compatible backend, Some =
/// only the named ids (unknown ids are skipped, not fatal). `variants`: 1 =
/// each runtime at its fit-max ctx; 2 = plus a half-ctx point (bounded at 2K).
/// `ctx_cap`: clamp every trial point at or below it (to ask "does it run at
/// 8K?" without re-deriving). Returns (plans, skipped).
#[allow(clippy::type_complexity)]
pub fn plan_trials(
    model_path: &Path,
    runtimes: Option<&[String]>,
    variants: u32,
    ctx_cap: Option<u32>,
) -> Result<(Vec<TrialPlan>, Vec<(String, String)>), String> {
    let meta = if model_path.is_dir() {
        deck_core::safetensors::open_dir(model_path)
            .map_err(|e| format!("read safetensors dir {model_path:?}: {e}"))?
    } else {
        deck_core::gguf::GgufMeta::read(model_path)
            .map_err(|e| format!("read GGUF {model_path:?}: {e}"))?
            .to_meta(model_path)
    };
    let all = deck_core::runtime::all_manifests();
    let mut skipped: Vec<(String, String)> = Vec::new();
    let mut manifests: Vec<deck_core::runtime::RuntimeManifest> = Vec::new();
    match runtimes {
        Some(ids) => {
            for id in ids {
                match all.iter().find(|m| &m.id == id) {
                    Some(m) => manifests.push(m.clone()),
                    None => skipped.push((id.clone(), "unknown runtime id".into())),
                }
            }
        }
        None => {
            manifests = all
                .into_iter()
                .filter(|m| m.supports(&meta.format, meta.arch.as_deref()))
                .collect();
        }
    }
    if manifests.is_empty() {
        return Err("no compatible runtime for this artifact (see skipped)".into());
    }
    let mut plans = Vec::new();
    for m in &manifests {
        let (derived, test_port) = match deck_core::fitplan::plan_for_runtime(model_path, &m.id) {
            Ok(v) => v,
            Err(e) => {
                skipped.push((m.id.clone(), e));
                continue;
            }
        };
        let max = derived.max_ctx;
        let mut points = vec![max];
        if variants >= 2 {
            let half = (max / 2 / 2048 * 2048).max(2048);
            if half < max {
                points.push(half);
            }
        }
        if let Some(cap) = ctx_cap {
            for c in &mut points {
                *c = (*c).min(cap).max(2048);
            }
            points.sort_unstable();
            points.dedup();
        }
        for ctx in points {
            let mut p = derived.profile.clone();
            p.ctx_size = ctx;
            p.name = format!("discover-{}-{ctx}", m.id);
            plans.push(TrialPlan {
                runtime_id: m.id.clone(),
                display: m.display.clone(),
                test_port,
                profile: p,
                label: format!("{} @ ctx {ctx}", m.display),
            });
        }
    }
    Ok((plans, skipped))
}

/// Substitute machine-specific binary paths (the `engine_bin` table) into
/// plans. The lookup is injected so planning stays DB-free; callers pass
/// `|p| get_engine_bin(&conn, &p.runtime_key()).ok().flatten().map(PathBuf::from)`.
pub fn apply_engine_bins(plans: &mut [TrialPlan], bin_for: &dyn Fn(&Profile) -> Option<PathBuf>) {
    for t in plans {
        if let Some(b) = bin_for(&t.profile) {
            t.profile.bin = b;
        }
    }
}

fn protocol_for(p: &Profile) -> EngineProtocol {
    if let Some(m) = manifest_for(p) {
        return m.protocol();
    }
    Engine::parse(&p.runtime_key())
        .map(|e| e.protocol())
        .unwrap_or(EngineProtocol::OpenAiChat)
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// One measured trial: boot verdict, samples, and persist-ready rows.
pub struct TrialResult {
    pub runtime_id: String,
    pub display: String,
    pub ctx: u32,
    pub boot_verdict: String,
    pub boot_summary: String,
    pub rows: Vec<MatrixRow>,
    pub best_tps: Option<f64>,
    pub best_kind: Option<String>,
    pub prompt_tps: Option<f64>,
}

fn fail_row(plan: &TrialPlan, verdict: &str, summary: &str) -> MatrixRow {
    MatrixRow {
        engine: plan.runtime_id.clone(),
        model: plan.profile.model.clone(),
        ctx: plan.profile.ctx_size,
        task: PROBE_TASK.into(),
        run: 0,
        verdict: verdict.into(),
        summary: summary.into(),
        gen_tokens: None,
        prompt_tokens: None,
        tok_s: None,
        tok_s_kind: "wall".into(),
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
        workflow_id: None,
    }
}

/// Boot one trial config on its test port and measure it with the shared
/// probe (`runs` repeats). A config that cannot boot is recorded as a boot
/// row (OOM/CRASH/TIMEOUT) — never sampled. The live service is untouched.
pub fn run_trial(
    plan: &TrialPlan,
    max_tokens: u32,
    runs: u32,
    boot_timeout: Duration,
    progress: Option<&dyn Fn(&str)>,
) -> TrialResult {
    let p = &plan.profile;
    let mut child = match boot_on_test_port(p, plan.test_port, boot_timeout) {
        Ok(c) => c,
        Err((v, s)) => {
            let row = fail_row(plan, &v, &s);
            return TrialResult {
                runtime_id: plan.runtime_id.clone(),
                display: plan.display.clone(),
                ctx: p.ctx_size,
                boot_verdict: v,
                boot_summary: s,
                rows: vec![row],
                best_tps: None,
                best_kind: None,
                prompt_tps: None,
            };
        }
    };
    let host = p.host.clone();
    let engine_version = match Engine::parse(&plan.runtime_id) {
        Some(e) => crate::detect_engine_version(e, &host, plan.test_port),
        // A custom backend's declared manifest version is the source of truth.
        None => manifest_for(p).and_then(|m| m.version.clone()),
    };
    // A native tok/s reading while the server is up (llama.cpp /metrics).
    let native = fetch_metrics(&host, plan.test_port).ok().and_then(|m| parse_tps(&m));
    let mut rows = Vec::new();
    let mut best: Option<(f64, String, Option<f64>)> = None;
    for run in 0..runs {
        let s = run_prompt_protocol(
            protocol_for(p),
            &host,
            plan.test_port,
            &p.alias,
            PROBE_PROMPT,
            max_tokens,
        );
        if let Some(emit) = progress {
            let t = s
                .tok_s
                .map(|t| format!("{t:.1} tok/s"))
                .unwrap_or_else(|| "no reading".into());
            emit(&format!("{} run {run}: {t}", plan.label));
        }
        let better = match (&best, s.tok_s) {
            (None, Some(_)) => true,
            (Some((b, _, _)), Some(v)) => v > *b,
            _ => false,
        };
        if better {
            best = s.tok_s.map(|v| (v, s.tok_s_kind.to_string(), s.prompt_tps));
        }
        rows.push(MatrixRow {
            engine: plan.runtime_id.clone(),
            model: p.model.clone(),
            ctx: p.ctx_size,
            task: PROBE_TASK.into(),
            run,
            verdict: if s.ok { "RUNNING".into() } else { "ERROR".into() },
            summary: s.error.clone().unwrap_or_default(),
            gen_tokens: s.gen_tokens,
            prompt_tokens: s.prompt_tokens,
            tok_s: s.tok_s,
            tok_s_kind: s.tok_s_kind.into(),
            wall_ms: s.wall_ms,
            output: s.text.clone(),
            at: now_epoch(),
            workload_id: None,
            hardware_profile_id: None,
            engine_version: engine_version.clone(),
            prompt_tps: s.prompt_tps,
            ttft_ms: s.ttft_ms,
            peak_vram_mb: None,
            model_rev: None,
            sampling_json: None,
            role_id: None,
            workflow_id: None,
        });
    }
    let _ = child.kill();
    let _ = child.wait();
    let native_note = native.map(|t| format!(" (metrics native {t:.1})")).unwrap_or_default();
    let (best_tps, best_kind, prompt_tps) = match best {
        Some((t, k, pp)) => (Some(t), Some(k), pp),
        None => (None, None, None),
    };
    TrialResult {
        runtime_id: plan.runtime_id.clone(),
        display: plan.display.clone(),
        ctx: p.ctx_size,
        boot_verdict: "RUNNING".into(),
        boot_summary: format!("serving on :{}{native_note}", plan.test_port),
        rows,
        best_tps,
        best_kind,
        prompt_tps,
    }
}

/// Ranked summary of one discovery batch. `best` is the runtime id with the
/// highest measured tok/s (None when nothing served).
#[derive(Debug, Clone, Serialize)]
pub struct TrialSummary {
    pub runtime_id: String,
    pub display: String,
    pub ctx: u32,
    pub boot_verdict: String,
    pub tps: Option<f64>,
    pub kind: Option<String>,
    pub prompt_tps: Option<f64>,
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveryReport {
    pub model: String,
    pub trials: Vec<TrialSummary>,
    pub skipped: Vec<(String, String)>,
    pub best: Option<String>,
    pub row_ids: Vec<i64>,
    pub summary: String,
}

pub fn summarize(
    model: &str,
    results: &[TrialResult],
    skipped: &[(String, String)],
    row_ids: &[i64],
) -> DiscoveryReport {
    let trials: Vec<TrialSummary> = results
        .iter()
        .map(|r| TrialSummary {
            runtime_id: r.runtime_id.clone(),
            display: r.display.clone(),
            ctx: r.ctx,
            boot_verdict: r.boot_verdict.clone(),
            tps: r.best_tps,
            kind: r.best_kind.clone(),
            prompt_tps: r.prompt_tps,
            ok: r.boot_verdict == "RUNNING" && r.best_tps.map(|t| t > 0.0).unwrap_or(false),
        })
        .collect();
    let best = trials
        .iter()
        .filter(|t| t.ok)
        .max_by(|a, b| a.tps.partial_cmp(&b.tps).unwrap_or(std::cmp::Ordering::Equal))
        .map(|t| t.runtime_id.clone());
    let ok_count = trials.iter().filter(|t| t.ok).count();
    DiscoveryReport {
        model: model.into(),
        trials,
        skipped: skipped.to_vec(),
        best: best.clone(),
        row_ids: row_ids.to_vec(),
        summary: format!(
            "discovery: {ok_count}/{} trial(s) served{}",
            results.len(),
            best.map(|b| format!(" — best measured: {b}")).unwrap_or_default()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bogus_llama(port: u16) -> TrialPlan {
        let p = Profile {
            engine: Engine::LlamaCpp,
            bin: "/nonexistent-llama-server-xyz".into(),
            model: "/x.gguf".into(),
            port,
            ctx_size: 2048,
            ..Default::default()
        };
        TrialPlan {
            runtime_id: "llamacpp".into(),
            display: "llama.cpp".into(),
            test_port: port,
            profile: p,
            label: "llama.cpp @ ctx 2048".into(),
        }
    }

    #[test]
    fn boot_failure_records_a_row_and_never_samples() {
        // `runs` is irrelevant when the server never comes up.
        let r = run_trial(&bogus_llama(38991), 32, 3, Duration::from_secs(2), None);
        assert_eq!(r.boot_verdict, "ERROR");
        assert!(r.best_tps.is_none());
        assert_eq!(r.rows.len(), 1);
        assert_eq!(r.rows[0].verdict, "ERROR");
        assert_eq!(r.rows[0].task, PROBE_TASK);
    }

    #[test]
    fn summarize_ranks_by_measured_tps_and_names_best() {
        let mk = |rt: &str, tps: Option<f64>| TrialResult {
            runtime_id: rt.into(),
            display: rt.into(),
            ctx: 8192,
            boot_verdict: "RUNNING".into(),
            boot_summary: String::new(),
            rows: vec![],
            best_tps: tps,
            best_kind: tps.map(|_| "wall".into()),
            prompt_tps: None,
        };
        let rep = summarize("/m.gguf", &[mk("a", Some(10.0)), mk("b", Some(44.0)), mk("c", None)], &[], &[]);
        assert_eq!(rep.best.as_deref(), Some("b"));
        assert!(rep.summary.contains("2/3"));
    }
}
