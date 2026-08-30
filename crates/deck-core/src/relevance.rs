//! O2 relevance scoring — pure, deterministic, hardware-grounded.
//!
//! Score = w1·fits_hardware + w2·family_overlap + w3·quant_novelty
//!       + w4·bench_delta + w5·recency
//!
//! No ML. Explainable: each term is 0..1, weights sum to 1.

use crate::store::Release;

/// Installed model inventory (from `store::list`).
#[derive(Debug, Clone)]
pub struct Installed {
    pub name: String,
    pub arch: Option<String>,
    pub quant: Option<String>,
}

/// Minimal bench best signal.
#[derive(Debug, Clone, Default)]
pub struct BenchBest {
    /// best tok/s for a family hint (e.g. qwen3); None = no history
    pub tok_s: Option<f64>,
}

/// Weights — caller can tune; sum to 1.
#[derive(Debug, Clone)]
pub struct Weights {
    pub hw: f64,
    pub family: f64,
    pub novelty: f64,
    pub bench: f64,
    pub recency: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Self { hw: 0.30, family: 0.25, novelty: 0.15, bench: 0.20, recency: 0.10 }
    }
}

/// Explainable breakdown.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Score {
    pub total: f64,
    pub hw: f64,
    pub family: f64,
    pub novelty: f64,
    pub bench: f64,
    pub recency: f64,
    pub fits: bool,
    /// Estimated GGUF download size (GB) for a model; `None` for engine
    /// releases or names we cannot size offline (O4 DISK enrichment).
    pub disk_gb: Option<f64>,
    /// Largest context (tokens) the candidate still fits VRAM at, given the
    /// offline weight guess; `None` when the size is uncertain — never invent
    /// a ctx for an un-sizable model (O4 fit-at-ctx enrichment).
    pub max_ctx: Option<u64>,
    /// Whether the estimated download fits the free disk at rank time.
    pub disk_fits: bool,
    pub reasons: Vec<String>,
}

/// Naive family token from repo id / arch: "unsloth/Qwen3.8-GGUF" → "qwen"
fn family_of(s: &str) -> String {
    let lower = s.to_lowercase();
    for tok in ["qwen", "llama", "mistral", "gemma", "phi", "deepseek", "glm", "codellama"] {
        if lower.contains(tok) {
            return tok.to_string();
        }
    }
    // fallback: first path component
    lower.split('/').next().unwrap_or(&lower).to_string()
}

fn quant_token(s: &str) -> Option<String> {
    let lower = s.to_lowercase();
    // common quant suffixes
    for q in ["q4_k_m", "q4_0", "q5_k_m", "q3_k", "iq4_xs", "q8_0", "f16", "bf16", "q6_k"] {
        if lower.contains(q) {
            return Some(q.to_string());
        }
    }
    None
}

/// Hardware fit 0/0.5/1 based on cheap heuristic: repo name length as proxy
/// Total-params (billions) parsed from a model repo name, used as the DDG-free
/// offline size guess. Returns `None` when we can't name a reliable total — a
/// decimal/composite MoE name like `Qwen3.8-Flash-Next` or `1.5b` whose params
/// token is NOT the total, or a name with no recognizable `NNb`/`N.Nb` marker.
///
/// # Safety invariant (hardware is ground truth)
/// The caller MUST NOT turn a `None` into a "fits" verdict. A name we cannot
/// size offline must be reported as *uncertain* (fit pending a real GGUF
/// header probe), never as a confident "~N GB fits" — otherwise an un-sizable
/// flagship like Flash-Next (125B-total MoE) ranks as if it fit this 16 GB box.
fn params_total_b(repo: &str) -> Option<f64> {
    let lower = repo.to_lowercase();
    let bytes = lower.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        // consume `NN` or `N.N` digit run
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let mut decimal = false;
        if i + 1 < bytes.len() && bytes[i] == b'.' && bytes[i + 1].is_ascii_digit() {
            decimal = true;
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
        }
        // the digit run must be immediately followed by `b` to be a params marker
        if i < bytes.len() && bytes[i] == b'b' {
            let slice = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
            let v = slice.parse::<f64>().ok().filter(|&v| v > 0.0);
            if let Some(v) = v {
                // decimal params (3.8, 1.5, 3.1) are MoE/composite names where
                // the visible number is NOT the total → uncertain
                if decimal {
                    return None;
                }
                return Some(v);
            }
        }
    }
    None
}

/// Map total-params (B) to an approximate GGUF size in GB (f16‑ish ballpark).
fn params_to_gb(p: f64) -> f64 {
    if p >= 60.0 {
        40.0
    } else if p >= 30.0 {
        16.0
    } else if p >= 13.0 {
        9.0
    } else if p >= 6.0 {
        5.0
    } else {
        2.0
    }
}

/// KV-cache bytes per one billion params per one thousand context tokens.
///
/// Derived from `fit::estimate`'s real math (fp16, no GQA): a 32B model at
/// 32k ctx → ~4 GiB of KV, an 8B at 32k → ~1 GiB. So KV ≈ 4 MiB per B-param
/// per 1000 tokens. Conservative (no GQA discount) — a model that fits under
/// this rule genuinely fits.
const KV_MB_PER_B_PARAM_PER_1K: f64 = 4.0;

/// Largest context (tokens) whose KV still fits in VRAM after the estimated
/// weights + a small buffer, given the desktop reservation. Returns `None`
/// when weights alone already exceed available-for-model (can't fit at any
/// ctx). Deterministic, pure, and reuses the same reservation fit.rs uses.
fn kv_ctx_at(params_b: f64, vram_mb: u64, reserved_mb: u64) -> Option<u64> {
    let weights_mb = params_to_gb(params_b) * 1024.0;
    let available_for_model = vram_mb as f64 - reserved_mb as f64;
    let headroom = available_for_model - weights_mb - 64.0; // 64 MiB buffers
    if headroom <= 0.0 {
        return None;
    }
    // headroom_mb = KV_MB/1k * params_b * (ctx/1000) → ctx = headroom*1000 / (kv*params)
    let ctx = (headroom * 1000.0 / (KV_MB_PER_B_PARAM_PER_1K * params_b)) as u64;
    Some(ctx.clamp(1024, 131_072))
}

/// size when no real GGUF header is available. If the release payload looks
/// like a HF model with tags, we approximate; otherwise we degrade gracefully.
/// Real fit uses `fit::estimate` when GGUF meta is fetchable — that path is
/// exercised by MARKET's `browse_fit_remote`; here we need a fast offline rank.
fn hw_term(release: &Release, vram_mb: u64, reserved_mb: u64) -> (f64, bool, String, Option<f64>, Option<u64>) {
    // GitHub releases always fit (they're engines, not models) — no size/ctx
    if release.source == "github" {
        return (1.0, true, "engine release".into(), None, None);
    }
    // HF: guess total size from the params marker in the repo name. Names we
    // cannot size (decimal/composite MoE, or no marker) are UNCERTAIN — we must
    // not claim a fit for an un-sizable flagship (see params_total_b).
    let repo = release.repo.to_lowercase();
    let (guess_gb, total_b): (f64, f64) = match params_total_b(&repo) {
        Some(total_b) => (params_to_gb(total_b), total_b),
        None => {
            return (
                0.0,
                false,
                "size unknown (composite/MoE) — probe GGUF in MARKET before testing".into(),
                None,
                None,
            );
        }
    };
    let guess_mb = (guess_gb * 1024.0) as u64;
    let fits = guess_mb + reserved_mb < vram_mb;
    let score = if fits { 1.0 } else if guess_mb < vram_mb + 4000 { 0.5 } else { 0.0 };
    let reason = if fits { format!("~{guess_gb:.0}GB fits {vram_mb}MB") } else { format!("~{guess_gb:.0}GB tight on {vram_mb}MB") };
    let max_ctx = if fits { kv_ctx_at(total_b, vram_mb, reserved_mb) } else { None };
    (score, fits, reason, Some(guess_gb), max_ctx)
}

pub fn score_one(
    release: &Release,
    installed: &[Installed],
    bench: &BenchBest,
    vram_mb: u64,
    recency_days: f64,
    w: &Weights,
    disk_free_mb: u64,
) -> Score {
    // hw
    let (hw, fits, hw_reason, disk_gb, max_ctx) = hw_term(release, vram_mb, 1600);
    // family overlap: does installed contain same family?
    let fam = family_of(&release.repo);
    let family_hit = installed.iter().any(|m| {
        let a = m.arch.as_deref().unwrap_or(&m.name).to_lowercase();
        a.contains(&fam) || m.name.to_lowercase().contains(&fam)
    });
    let family = if family_hit { 1.0 } else { 0.3 };
    // quant novelty: new quant not installed?
    let q = quant_token(&release.repo);
    let novelty = match &q {
        Some(tok) => {
            let have = installed.iter().any(|m| m.quant.as_deref().unwrap_or("").to_lowercase().contains(tok));
            if have { 0.3 } else { 1.0 }
        }
        None => 0.5,
    };
    // bench delta: if we have a best, slightly boost when release is recent & fits
    let bench_s = if bench.tok_s.is_some() && fits { 0.7 } else if bench.tok_s.is_some() { 0.4 } else { 0.5 };
    // recency: 0 days =1, 30 days=0.5, 90 days=0
    let recency = (1.0 - (recency_days / 90.0)).clamp(0.0, 1.0);

    let total = w.hw * hw + w.family * family + w.novelty * novelty + w.bench * bench_s + w.recency * recency;

    let mut reasons = vec![hw_reason];
    if family_hit { reasons.push(format!("family {fam} you use")); }
    if let Some(tok) = q { if novelty > 0.5 { reasons.push(format!("new quant {tok}")); } }
    if bench.tok_s.is_some() && fits { reasons.push("may beat current best".into()); }

    let disk_fits = match disk_gb {
        Some(gb) => (gb * 1024.0) as u64 + 512 < disk_free_mb,
        // engine releases and un-sizable models can't be judged against disk
        None => true,
    };
    if !disk_fits {
        if let Some(gb) = disk_gb {
            reasons.push(format!("~{gb:.0}GB won't fit free disk"));
        }
    }

    Score {
        total,
        hw,
        family,
        novelty,
        bench: bench_s,
        recency,
        fits,
        disk_gb,
        max_ctx,
        disk_fits,
        reasons,
    }
}

/// Score all releases; sort descending. `now` = unix secs for recency.
pub fn rank(
    releases: Vec<Release>,
    installed: &[Installed],
    bench: &BenchBest,
    vram_mb: u64,
    now: i64,
    w: &Weights,
    disk_free_mb: u64,
) -> Vec<(Release, Score)> {
    let mut out: Vec<(Release, Score)> = releases
        .into_iter()
        .map(|r| {
            let age_days = if r.fetched_at > 0 { (now - r.fetched_at) as f64 / 86400.0 } else { 30.0 };
            // also consider published_at if ISO
            let sc = score_one(&r, installed, bench, vram_mb, age_days, w, disk_free_mb);
            (r, sc)
        })
        .collect();
    out.sort_by(|a, b| b.1.total.partial_cmp(&a.1.total).unwrap_or(std::cmp::Ordering::Equal));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rel(source: &str, repo: &str) -> Release {
        Release { source: source.into(), repo: repo.into(), rev: "r1".into(), kind: "model".into(), title: repo.into(), url: "".into(), published_at: "".into(), payload_json: "{}".into(), fetched_at: 0 }
    }
    #[test]
    fn github_always_fits() {
        let r = rel("github", "ggml-org/llama.cpp");
        let s = score_one(&r, &[], &BenchBest::default(), 16000, 0.0, &Weights::default(), 268_000);
        assert!(s.fits);
        assert_eq!(s.hw, 1.0);
    }
    #[test]
    fn family_hit_scores_higher() {
        let r = rel("hf", "unsloth/Qwen3-8B-GGUF");
        let installed = vec![Installed { name: "qwen3".into(), arch: Some("qwen3".into()), quant: None }];
        let s_hit = score_one(&r, &installed, &BenchBest::default(), 16000, 0.0, &Weights::default(), 268_000);
        let s_miss = score_one(&r, &[], &BenchBest::default(), 16000, 0.0, &Weights::default(), 268_000);
        assert!(s_hit.family > s_miss.family);
        assert!(s_hit.total > s_miss.total);
    }
    #[test]
    fn rank_sorts_desc() {
        let a = rel("hf", "unsloth/Qwen3-70B-GGUF");
        let b = rel("hf", "unsloth/Qwen3-7B-GGUF");
        let ranked = rank(vec![a, b], &[], &BenchBest::default(), 16000, 0, &Weights::default(), 268_000);
        // 7B fits, 70B doesn't → 7B first
        assert!(ranked[0].0.repo.contains("7B"));
    }
    #[test]
    fn moe_flagship_never_overclaims_fit() {
        // Flash-Next is a 125B-total MoE, AGENTS.md's explicit hardware
        // non-starter. Its decimal/composite name must NOT rank as "fits".
        let r = rel("hf", "bartowski/Qwen3.8-Flash-Next-GGUF");
        let s = score_one(&r, &[], &BenchBest::default(), 16000, 0.0, &Weights::default(), 268_000);
        assert!(!s.fits, "Flash-Next must not be declared fittable offline");
        assert_eq!(s.hw, 0.0);
        // O4 enrichment: un-sizable moons must not invent a DISK or ctx number
        assert_eq!(s.disk_gb, None);
        assert_eq!(s.max_ctx, None);
    }
    #[test]
    fn decimal_params_is_uncertain_not_tiny() {
        // Qwen3.6-35b-a3b: active 3B but 35B total. The 35B integer must win,
        // not a 2GB "3b" guess.
        assert_eq!(params_total_b("bartowski/Qwen3.6-35b-a3b-GGUF"), Some(35.0));
        // decimal-only names are un-sizable → uncertain
        assert_eq!(params_total_b("bartowski/Qwen3.8-Flash-Next-GGUF"), None);
        // plain integers still resolve
        assert_eq!(params_total_b("unsloth/Qwen3-8B-GGUF"), Some(8.0));
    }
    #[test]
    fn o4_disk_and_ctx_enrichment_for_sizable_model() {
        // 8B GGUF: ~5GB disk (params_to_gb band), fits 16k VRAM → ctx derived.
        let r = rel("hf", "unsloth/Qwen3-8B-GGUF");
        let s = score_one(&r, &[], &BenchBest::default(), 16000, 0.0, &Weights::default(), 268_000);
        assert!(s.fits);
        assert_eq!(s.disk_gb, Some(5.0));
        assert!(s.disk_fits);
        // 8B: weights ≈ 5120 MiB → headroom ≈ 16000-1600-5120-64 = 9216 MiB
        // ctx = 9216*1000 / (4*8) = 288000 → clamped to 131072
        assert_eq!(s.max_ctx, Some(131_072));
    }
    #[test]
    fn o4_ctx_scales_down_when_read_only_headroom() {
        // 70B GGUF ~40GB does not fit 16 GiB VRAM → no ctx, disk still reported.
        let r = rel("hf", "unsloth/Qwen3-70B-GGUF");
        let s = score_one(&r, &[], &BenchBest::default(), 16000, 0.0, &Weights::default(), 268_000);
        assert!(!s.fits);
        assert_eq!(s.disk_gb, Some(40.0));
        assert_eq!(s.max_ctx, None);
    }
    #[test]
    fn o4_disk_fits_flips_when_free_disk_is_tiny() {
        let r = rel("hf", "unsloth/Qwen3-8B-GGUF");
        // only ~1 GB free disk: the ~2GB download cannot fit
        let s = score_one(&r, &[], &BenchBest::default(), 16000, 0.0, &Weights::default(), 1024);
        assert!(s.fits, "weights fit VRAM but the download is blocked by disk");
        assert!(!s.disk_fits);
    }
}
