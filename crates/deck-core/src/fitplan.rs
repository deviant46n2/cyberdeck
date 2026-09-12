//! Fit Engine v2: model × runtime × hardware → ranked candidate configurations.
//!
//! The v1 estimator (`fit::estimate`) answers one (model, ctx, kv) point.
//! This module climbs the context ladder per COMPATIBLE runtime and returns
//! an explainable shortlist: which runtimes can run this artifact, at what
//! max context, at what VRAM cost, and why the winner won. The UI's
//! "Make it work" consumes `recommend()`; Tune consumes the full list.

use serde::Serialize;

use crate::fit::{FitRequest, Verdict};
use crate::model::ModelMeta;
use crate::runtime::RuntimeManifest;

/// One viable configuration for one runtime.
#[derive(Debug, Clone, Serialize)]
pub struct FitCandidate {
    pub runtime_id: String,
    pub display: String,
    pub status: String,
    pub max_ctx: u64,
    pub kv_label: String,
    pub model_vram_mb: u64,
    pub weights_ram_mb: u64,
    pub available_mb: u64,
    pub headroom_mb: u64,
    pub verdict: String,
    /// Human-readable "why" (see `library::explain_fit`).
    pub why: String,
}

/// Climb 2K→256K in 2K steps and keep the largest ctx whose verdict is
/// Pass or Warn. Returns None when even 2K OOMs. Shared with the builtin
/// loadout deriver so both paths agree on what "fits".
pub fn max_ctx_for(meta: &ModelMeta, offload: bool, vram_mb: u64) -> Option<(u64, crate::fit::FitBreakdown)> {
    let mut best: Option<(u64, crate::fit::FitBreakdown)> = None;
    for ctx in (2048u64..=262_144).step_by(2048) {
        let req = FitRequest {
            ctx,
            kv_bytes: 0.5,
            ngl_frac: 1.0,
            kv_layers: None,
            reserved_mb: 1600,
            offload,
        };
        let fb = crate::fit::estimate(meta, &req, vram_mb);
        if matches!(fb.verdict, Verdict::Pass | Verdict::Warn) {
            best = Some((ctx, fb));
        } else {
            break;
        }
    }
    best
}

/// Ranked candidates for every compatible runtime. Incompatible runtimes
/// (wrong format/arch, Ollama-style daemon stores) are skipped silently —
/// the caller asked "what can run THIS artifact", not for an error list.
pub fn candidates_for(
    meta: &ModelMeta,
    runtimes: &[RuntimeManifest],
    vram_mb: u64,
) -> Vec<FitCandidate> {
    let mut out = Vec::new();
    for rt in runtimes {
        if !rt.supports(&meta.format, meta.arch.as_deref()) {
            continue;
        }
        let offload = rt.is_offload();
        let Some((ctx, fb)) = max_ctx_for(meta, offload, vram_mb) else {
            continue;
        };
        let headroom = fb.available_for_model_mb.saturating_sub(fb.model_vram_mb);
        out.push(FitCandidate {
            runtime_id: rt.id.clone(),
            display: rt.display.clone(),
            status: rt.status.tag().to_string(),
            max_ctx: ctx,
            kv_label: "Q4".to_string(),
            model_vram_mb: fb.model_vram_mb,
            weights_ram_mb: fb.weights_ram_mb,
            available_mb: fb.available_mb,
            headroom_mb: headroom,
            verdict: fb.verdict.tag().to_string(),
            why: crate::library::explain_fit(
                &rt.id,
                ctx,
                "Q4",
                fb.model_vram_mb,
                fb.available_mb,
                offload,
            ),
        });
    }
    // Prefer the largest viable context; tie-break toward stable runtimes
    // (explicit rank — alphabetical order would prefer "community").
    fn rank(status: &str) -> u8 {
        match status {
            "stable" => 0,
            "community" => 1,
            "experimental" => 2,
            _ => 3,
        }
    }
    out.sort_by(|a, b| {
        b.max_ctx
            .cmp(&a.max_ctx)
            .then_with(|| rank(&a.status).cmp(&rank(&b.status)))
    });
    out
}

/// The "Make it work" default: best candidate, if any.
pub fn recommend(meta: &ModelMeta, runtimes: &[RuntimeManifest], vram_mb: u64) -> Option<FitCandidate> {
    candidates_for(meta, runtimes, vram_mb).into_iter().next()
}

/// Resolve a runtime id (builtin engine name or custom manifest id) into a
/// derived loadout + its dedicated test port. The single derive dispatcher the
/// app and CLI bringup/swap doors share — no door re-implements the dispatch.
pub fn plan_for_runtime(
    model: &std::path::Path,
    runtime_id: &str,
) -> Result<(crate::profile::DerivedLoadout, u16), String> {
    if let Some(e) = crate::profile::Engine::parse(runtime_id) {
        let d = crate::profile::derive_loadout(model, e)?;
        return Ok((d, e.test_port()));
    }
    let m = crate::runtime::all_manifests()
        .into_iter()
        .find(|m| m.id == runtime_id)
        .ok_or_else(|| {
            format!(
                "unknown engine/runtime '{runtime_id}' (builtins: llamacpp|freetoken|ollama; or a manifest in ~/.local/share/cyberdeck/runtimes)"
            )
        })?;
    let test_port = m.test_port;
    let d = crate::profile::derive_custom_loadout(model, &m)?;
    Ok((d, test_port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelFormat;
    use crate::runtime::builtin_manifests;
    use std::path::PathBuf;

    fn meta(weight_gib: u64) -> ModelMeta {
        ModelMeta {
            path: PathBuf::from("/models/q.gguf"),
            format: ModelFormat::Gguf,
            name: "qwen".into(),
            arch: Some("qwen3".into()),
            quant: Some("Q4_K_M".into()),
            params: None,
            n_layers: Some(48),
            n_embd: Some(5120),
            n_head: None,
            n_head_kv: None,
            ctx_train: None,
            vocab: None,
            weight_size: weight_gib * 1024 * 1024 * 1024,
            footprint: weight_gib * 1024 * 1024 * 1024,
        }
    }

    #[test]
    fn small_model_yields_llamacpp_candidate_with_explanation() {
        let c = candidates_for(&meta(4), &builtin_manifests(), 16 * 1024);
        assert!(!c.is_empty());
        let top = &c[0];
        assert!(top.max_ctx >= 32768);
        assert!(!top.why.is_empty());
        // Ollama (daemon store) never appears for a local GGUF.
        assert!(c.iter().all(|x| x.runtime_id != "ollama"));
    }

    #[test]
    fn absurdly_huge_model_yields_no_candidate() {
        // 1 TiB weights: even the offload working set + 2K KV cannot fit.
        let c = candidates_for(&meta(1024), &builtin_manifests(), 16 * 1024);
        assert!(c.is_empty());
        assert!(recommend(&meta(1024), &builtin_manifests(), 16 * 1024).is_none());
    }
}
