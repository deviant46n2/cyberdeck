//! Model library: identity vs artifact vs configuration vs run.
//!
//! The old model treated `Model = downloaded file = runtime = configuration`.
//! This module separates the concepts without touching the existing `models`
//! table (each row is one ARTIFACT — one file/dir on disk):
//!
//!   - identity: "Qwen 3.8 27B" — groups artifacts via [`ModelMeta::identity`]
//!     (arch + format + size bucket; tolerant of relabelled quants).
//!   - artifact: one `ModelMeta` row (one GGUF file or safetensors dir).
//!   - configuration: a named `Profile` (engine + ctx + KV + offload + ...).
//!   - run: a live execution instance of a configuration (portmap state).
//!
//! Lifecycle states (DISCOVERED / INSTALLED / RUNNING) are derived, not
//! stored: `classify()` combines file existence with live-run membership.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::ModelMeta;

/// Lifecycle of one artifact on this machine.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Lifecycle {
    /// Known to cyberdeck (remote candidate, watchlist entry) but no local file.
    Discovered,
    /// Local file exists, nothing running.
    Installed,
    /// A live runtime instance is executing this artifact now.
    Running,
}

/// Derive lifecycle from ground truth: file on disk + live-run membership.
/// `running_paths` is the set of model paths backing live portmap slots.
pub fn classify(exists_on_disk: bool, is_running: bool) -> Lifecycle {
    match (exists_on_disk, is_running) {
        (_, true) => Lifecycle::Running,
        (true, false) => Lifecycle::Installed,
        (false, false) => Lifecycle::Discovered,
    }
}

/// One identity group: every local artifact that is (probably) the same model
/// at different quants, plus aggregate disk usage for the card header.
#[derive(Debug, Clone, Serialize)]
pub struct IdentityGroup {
    pub identity: String,
    pub arch: Option<String>,
    pub display_name: String,
    pub artifacts: Vec<ArtifactView>,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactView {
    pub path: String,
    pub name: String,
    pub quant: Option<String>,
    pub bytes: u64,
    pub lifecycle: Lifecycle,
}

fn display_name_for(members: &[&ModelMeta]) -> String {
    // Shortest shared stem: strip quant-ish suffixes and extensions.
    let first = members
        .first()
        .map(|m| m.name.clone())
        .unwrap_or_else(|| "unknown model".to_string());
    first
        .trim_end_matches(".gguf")
        .rsplit_once(['-', '_', ' '])
        .map(|(stem, _)| {
            // Keep the stem only when siblings share it (same model, diff quant).
            let shared = members.iter().all(|m| {
                m.name.trim_end_matches(".gguf").starts_with(stem) || stem.len() < 4
            });
            if shared && stem.len() >= 4 {
                stem.to_string()
            } else {
                first.clone()
            }
        })
        .unwrap_or(first)
}

/// Group artifacts by [`ModelMeta::identity`], annotating lifecycle from
/// `running_paths` (canonical-or-raw model paths behind live slots).
/// Pure — no IO, so the UI and tests share it.
pub fn group_identities(models: &[ModelMeta], running_paths: &std::collections::HashSet<String>) -> Vec<IdentityGroup> {
    let mut by_id: BTreeMap<String, Vec<&ModelMeta>> = BTreeMap::new();
    for m in models {
        by_id.entry(m.identity()).or_default().push(m);
    }
    by_id
        .into_iter()
        .map(|(identity, members)| {
            let arch = members.iter().find_map(|m| m.arch.clone());
            let display_name = display_name_for(&members);
            let total_bytes = members.iter().map(|m| m.footprint).sum();
            let mut artifacts: Vec<ArtifactView> = members
                .iter()
                .map(|m| {
                    let raw = m.path.display().to_string();
                    let canon = std::fs::canonicalize(&m.path)
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| raw.clone());
                    let running = running_paths.contains(&raw) || running_paths.contains(&canon);
                    ArtifactView {
                        path: raw,
                        name: m.name.clone(),
                        quant: m.quant.clone(),
                        bytes: m.footprint,
                        lifecycle: classify(m.path.exists(), running),
                    }
                })
                .collect();
            artifacts.sort_by(|a, b| a.path.cmp(&b.path));
            IdentityGroup {
                identity,
                arch,
                display_name,
                artifacts,
                total_bytes,
            }
        })
        .collect()
}

/// Origin of one configuration value: recommended by the fit engine vs set
/// by the user. Stored per-profile (`origin`) and per-field overrides
/// (`overrides_json`: list of field names the user touched).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ConfigOrigin {
    #[default]
    Auto,
    Override,
}

impl ConfigOrigin {
    pub fn tag(self) -> &'static str {
        match self {
            ConfigOrigin::Auto => "auto",
            ConfigOrigin::Override => "override",
        }
    }
}

/// Which artifact + runtime + hardware facts produced a configuration, plus
/// the exact auto baseline it was derived from. Persisted as JSON in the
/// profile's `provenance` column so Tune can show, per field, AUTO vs
/// OVERRIDE and why the automatic value was chosen. `baseline` is what makes
/// the distinction survive edits: overrides are the diff against it.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Provenance {
    pub runtime_id: String,
    pub artifact_path: String,
    pub vram_mb: u64,
    pub why: String,
    #[serde(default)]
    pub baseline: crate::profile::Profile,
    #[serde(default)]
    pub overridden_fields: Vec<String>,
}

/// Top-level `Profile` fields that differ between the auto baseline and an
/// edited config. Field names match the serialized keys, so the UI can render
/// an AUTO/OVERRIDE marker per field without a hand-maintained list.
pub fn diff_fields(baseline: &crate::profile::Profile, edited: &crate::profile::Profile) -> Vec<String> {
    let (a, b) = match (
        serde_json::to_value(baseline).ok(),
        serde_json::to_value(edited).ok(),
    ) {
        (Some(serde_json::Value::Object(a)), Some(serde_json::Value::Object(b))) => (a, b),
        _ => return Vec::new(),
    };
    let mut out: Vec<String> = a
        .iter()
        .filter(|(k, v)| b.get(*k).map(|bv| bv != *v).unwrap_or(true))
        .map(|(k, _)| k.clone())
        .chain(b.keys().filter(|k| !a.contains_key(*k)).cloned())
        .collect();
    out.sort();
    out.dedup();
    out
}

pub fn explain_fit(
    runtime_id: &str,
    ctx: u64,
    kv_label: &str,
    model_vram_mb: u64,
    available_mb: u64,
    offload: bool,
) -> String {
    if offload {
        format!(
            "{runtime_id}: ctx {ctx} with {kv_label} KV; weights spill to RAM, VRAM holds KV+buffers (~{model_vram_mb} MiB of {available_mb} MiB)."
        )
    } else {
        format!(
            "{runtime_id}: ctx {ctx} with {kv_label} KV; estimated VRAM ~{model_vram_mb} MiB of {available_mb} MiB."
        )
    }
}

/// Canonical path helper shared by storage + UI: canonicalize when possible,
/// else the raw display string (missing files have nothing to canonicalize).
pub fn canon(path: &std::path::Path) -> String {
    std::fs::canonicalize(path)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

pub fn models_dir() -> PathBuf {
    crate::store::models_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelFormat;

    fn meta(path: &str, arch: &str, bytes: u64) -> ModelMeta {
        ModelMeta {
            path: PathBuf::from(path),
            format: ModelFormat::Gguf,
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            basename: None,
            arch: Some(arch.into()),
            quant: Some("Q4_K_M".into()),
            params: None,
            n_layers: Some(48),
            n_embd: Some(5120),
            n_head: None,
            n_head_kv: None,
            ctx_train: None,
            vocab: None,
            weight_size: bytes,
            footprint: bytes,
        }
    }

    #[test]
    fn lifecycle_truth_table() {
        assert_eq!(classify(false, false), Lifecycle::Discovered);
        assert_eq!(classify(true, false), Lifecycle::Installed);
        assert_eq!(classify(true, true), Lifecycle::Running);
        // A running flag wins even if the file check raced a delete.
        assert_eq!(classify(false, true), Lifecycle::Running);
    }

    #[test]
    fn diff_reports_only_changed_fields() {
        let a = crate::profile::Profile::default();
        let mut b = a.clone();
        assert!(diff_fields(&a, &b).is_empty());
        b.ctx_size += 4096;
        b.n_gpu_layers = 0;
        assert_eq!(diff_fields(&a, &b), vec!["ctx_size".to_string(), "n_gpu_layers".to_string()]);
    }

    #[test]
    fn groups_share_identity_and_sum_bytes() {
        // Same arch + same 0.5GiB bucket => one identity (see ModelMeta::identity).
        let gib = 1024 * 1024 * 1024u64;
        let a = meta("/m/qwen-Q3.gguf", "qwen3", 10 * gib);
        let b = meta("/m/qwen-Q4.gguf", "qwen3", 10 * gib + 100);
        let groups = group_identities(&[a, b], &std::collections::HashSet::new());
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].artifacts.len(), 2);
        assert_eq!(groups[0].total_bytes, 20 * gib + 100);
    }
}
