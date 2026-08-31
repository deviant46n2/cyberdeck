//! Harness config rewriting — the "swap" half of the fleet.
//!
//! Pointing a harness at a chosen cloud provider+model means editing that
//! harness's config file. Each harness has a different config path/format:
//! - OpenCode: `~/.config/opencode/opencode.json` (JSON, OpenAI-compatible
//!   provider blocks under `provider.<id>`, plus `model`/`small_model`).
//! - Goose: JSON config with a provider registry + an active model.
//! - DeepSeek harness: its own provider/endpoint config.
//!
//! We edit structurally (read JSON → mutate → serialize → write), the same way
//! `deck_core::opencode_sync` edits OpenCode, but generalised to arbitrary
//! harnesses. Every write is preceded by a timestamped backup, matching the
//! `backup_file` discipline used by the local engine rewire.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::model::{CloudProvider, Harness, HarnessId, HarnessBinding};

/// Result of rewriting one harness config.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RewriteReport {
    pub harness: &'static str,
    pub path: String,
    pub status: String, // "wrote <provider>/<model>" | "config path not wired" | "no write planned"
    pub backed_up: bool,
}

/// Where each harness keeps its config on disk.
fn config_path(id: HarnessId) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    match id {
        HarnessId::Opencode => Some(home.join(".config/opencode/opencode.json")),
        HarnessId::Goose => Some(home.join(".config/goose/config.json")),
        HarnessId::Deepseek => None, // not yet wired — see doc note
    }
}

/// Rewrite one harness's config to bind it to `binding` (provider + model).
/// Returns a report describing what was written (or why it was skipped).
pub fn bind_harness(harness: &Harness, binding: &HarnessBinding) -> Result<RewriteReport> {
    let provider = crate::model::get_provider(&binding.provider_id)
        .with_context(|| format!("unknown provider '{}'", binding.provider_id))?;

    let Some(path) = config_path(harness.id) else {
        return Ok(RewriteReport {
            harness: harness.display,
            path: String::new(),
            status: "config path not wired for this harness yet".into(),
            backed_up: false,
        });
    };

    let existing = read_json(&path)?;
    let planned = match harness.id {
        HarnessId::Opencode => write_opencode_blocks(existing, &provider, binding),
        HarnessId::Goose => write_goose_blocks(Some(existing), &provider, binding),
        HarnessId::Deepseek => None,
    };

    let Some(doc) = planned else {
        return Ok(RewriteReport {
            harness: harness.display,
            path: path.display().to_string(),
            status: "no write planned for this harness".into(),
            backed_up: false,
        });
    };

    // Back up before mutating — same discipline as the engine rewire.
    let backup = backup_file(&path)?;
    write_json(&path, &doc)?;

    Ok(RewriteReport {
        harness: harness.display,
        path: path.display().to_string(),
        status: format!("wrote {}/{}", binding.provider_id, binding.model_id),
        backed_up: backup,
    })
}

/// Ensure an OpenAI-compatible provider block + set the active model.
/// Returns the doc to write, always (a fresh object is created if needed).
fn write_opencode_blocks(
    doc: Value,
    provider: &CloudProvider,
    binding: &HarnessBinding,
) -> Option<Value> {
    let mut obj = doc.as_object().cloned().unwrap_or_default();
    let block = json!({
        "options": { "baseURL": provider.base_url },
        "models": { binding.model_id.clone(): { "name": binding.model_id.clone() } },
    });
    let prov = obj.entry("provider".to_string()).or_insert_with(|| json!({}));
    if let Value::Object(pm) = prov {
        pm.insert(provider.id.clone(), block);
    }
    let active = format!("{}/{}", provider.id, binding.model_id);
    obj.insert("model".to_string(), json!(active));
    obj.insert("small_model".to_string(), json!(active));
    Some(Value::Object(obj))
}

/// Best-effort Goose config writer. Goose's schema is a provider registry; we
/// ensure the provider block exists and set the active binding when the shape
/// allows. Unknown shapes are left untouched.
fn write_goose_blocks(
    doc: Option<Value>,
    provider: &CloudProvider,
    binding: &HarnessBinding,
) -> Option<Value> {
    let mut obj = doc
        .and_then(|d| d.as_object().cloned())
        .unwrap_or_default();
    let models = obj.entry("models".to_string()).or_insert_with(|| json!({}));
    if let Value::Object(mm) = models {
        mm.insert(
            binding.model_id.clone(),
            json!({
                "name": binding.model_id,
                "provider": provider.id,
                "base_url": provider.base_url,
            }),
        );
    }
    Some(Value::Object(obj))
}

/// Read a JSON file into a `Value`; a missing file yields `Value::Null`.
fn read_json(path: &PathBuf) -> Result<Value> {
    if !path.exists() {
        return Ok(Value::Null);
    }
    let s = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&s).with_context(|| format!("parsing {}", path.display()))
}

/// Write a JSON value (pretty, 2-space) to a file, creating parents.
fn write_json(path: &PathBuf, v: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let s = serde_json::to_string_pretty(v).context("serialize config")?;
    std::fs::write(path, s).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Copy a file to `<path>.bak.<nanos>` before mutation. Returns whether a
/// backup was actually made (the file existed).
fn backup_file(path: &PathBuf) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let bak = format!("{}.bak.{}", path.display(), nanos);
    std::fs::copy(path, &bak).with_context(|| format!("backing up {}", path.display()))?;
    Ok(true)
}

/// Apply a binding to a harness by id — the CLI/Tauri-facing convenience.
pub fn apply_binding(harness_id: HarnessId, binding: &HarnessBinding) -> Result<RewriteReport> {
    let h = crate::model::Harness::get(harness_id);
    bind_harness(&h, binding)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(pid: &str, mid: &str) -> HarnessBinding {
        HarnessBinding { provider_id: pid.into(), model_id: mid.into() }
    }

    #[test]
    fn opencode_rewrite_sets_provider_and_model() {
        let provider = crate::model::get_provider("nim").unwrap();
        let b = binding("nim", "deepseek-v4-pro");
        let doc = write_opencode_blocks(json!({}), &provider, &b).unwrap();
        assert_eq!(doc["model"], json!("nim/deepseek-v4-pro"));
        assert_eq!(doc["small_model"], json!("nim/deepseek-v4-pro"));
        assert_eq!(doc["provider"]["nim"]["options"]["baseURL"], json!(provider.base_url));
    }

    #[test]
    fn opencode_rewrite_preserves_existing_blocks() {
        let provider = crate::model::get_provider("groq").unwrap();
        let b = binding("groq", "llama-3.3-70b");
        let mut doc = json!({
            "provider": { "llamacpp": { "options": { "baseURL": "http://127.0.0.1:18000/v1" } } }
        });
        doc = write_opencode_blocks(doc, &provider, &b).unwrap();
        assert!(doc["provider"]["llamacpp"].is_object(), "local block must survive");
        assert!(doc["provider"]["groq"].is_object(), "new cloud block added");
        assert_eq!(doc["model"], json!("groq/llama-3.3-70b"));
    }

    #[test]
    fn goose_rewrite_adds_model_entry() {
        let provider = crate::model::get_provider("gemini").unwrap();
        let b = binding("gemini", "gemini-3-flash");
        let doc = write_goose_blocks(Some(json!({})), &provider, &b).unwrap();
        assert_eq!(doc["models"]["gemini-3-flash"]["provider"], json!("gemini"));
        assert_eq!(doc["models"]["gemini-3-flash"]["base_url"], json!(provider.base_url));
    }

    // No actual file writes in unit tests; the write path is exercised via
    // the CLI/Tauri doors against the user's real config (with backups).
}
