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
/// size when no real GGUF header is available. If the release payload looks
/// like a HF model with tags, we approximate; otherwise we degrade gracefully.
/// Real fit uses `fit::estimate` when GGUF meta is fetchable — that path is
/// exercised by MARKET's `browse_fit_remote`; here we need a fast offline rank.
fn hw_term(release: &Release, vram_mb: u64) -> (f64, bool, String) {
    // GitHub releases always fit (they're engines, not models)
    if release.source == "github" {
        return (1.0, true, "engine release".into());
    }
    // HF: guess from payload downloads/likes + name; if repo contains "27b"/"70b" etc
    let repo = release.repo.to_lowercase();
    let guess_gb: f64 = if repo.contains("70b") || repo.contains("72b") { 40.0 }
    else if repo.contains("32b") || repo.contains("27b") { 16.0 }
    else if repo.contains("14b") { 9.0 }
    else if repo.contains("8b") || repo.contains("7b") { 5.0 }
    else if repo.contains("3b") || repo.contains("1b") { 2.0 }
    else { 8.0 }; // middle default
    let guess_mb = (guess_gb * 1024.0) as u64;
    let fits = guess_mb + 1600 < vram_mb; // reserve 1.6G like fit.rs
    let score = if fits { 1.0 } else if guess_mb < vram_mb + 4000 { 0.5 } else { 0.0 };
    let reason = if fits { format!("~{guess_gb:.0}GB fits {vram_mb}MB") } else { format!("~{guess_gb:.0}GB tight on {vram_mb}MB") };
    (score, fits, reason)
}

pub fn score_one(
    release: &Release,
    installed: &[Installed],
    bench: &BenchBest,
    vram_mb: u64,
    recency_days: f64,
    w: &Weights,
) -> Score {
    // hw
    let (hw, fits, hw_reason) = hw_term(release, vram_mb);
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

    Score { total, hw, family, novelty, bench: bench_s, recency, fits, reasons }
}

/// Score all releases; sort descending. `now` = unix secs for recency.
pub fn rank(
    releases: Vec<Release>,
    installed: &[Installed],
    bench: &BenchBest,
    vram_mb: u64,
    now: i64,
    w: &Weights,
) -> Vec<(Release, Score)> {
    let mut out: Vec<(Release, Score)> = releases
        .into_iter()
        .map(|r| {
            let age_days = if r.fetched_at > 0 { (now - r.fetched_at) as f64 / 86400.0 } else { 30.0 };
            // also consider published_at if ISO
            let sc = score_one(&r, installed, bench, vram_mb, age_days, w);
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
        let s = score_one(&r, &[], &BenchBest::default(), 16000, 0.0, &Weights::default());
        assert!(s.fits);
        assert_eq!(s.hw, 1.0);
    }
    #[test]
    fn family_hit_scores_higher() {
        let r = rel("hf", "unsloth/Qwen3-8B-GGUF");
        let installed = vec![Installed { name: "qwen3".into(), arch: Some("qwen3".into()), quant: None }];
        let s_hit = score_one(&r, &installed, &BenchBest::default(), 16000, 0.0, &Weights::default());
        let s_miss = score_one(&r, &[], &BenchBest::default(), 16000, 0.0, &Weights::default());
        assert!(s_hit.family > s_miss.family);
        assert!(s_hit.total > s_miss.total);
    }
    #[test]
    fn rank_sorts_desc() {
        let a = rel("hf", "unsloth/Qwen3-70B-GGUF");
        let b = rel("hf", "unsloth/Qwen3-7B-GGUF");
        let ranked = rank(vec![a, b], &[], &BenchBest::default(), 16000, 0, &Weights::default());
        // 7B fits, 70B doesn't → 7B first
        assert!(ranked[0].0.repo.contains("7B"));
    }
}
