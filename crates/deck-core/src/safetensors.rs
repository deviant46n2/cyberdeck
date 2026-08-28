//! Safetensors model-directory parser (e.g. FreeToken NVFP4 layouts).
//!
//! A model "dir" is identified by `config.json` + at least one `*.safetensors`
//! file (or `model.safetensors.index.json`). Quant info comes from
//! `hf_quant_config.json` when present.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::model::{ModelFormat, ModelMeta};

#[derive(Debug)]
pub struct SafetensorsModel {
    dir: PathBuf,
    config: serde_json::Value,
    quant_config: Option<serde_json::Value>,
    index: Option<serde_json::Value>,
}

impl SafetensorsModel {
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        let config_path = dir.join("config.json");
        let config = read_json(&config_path)
            .with_context(|| format!("reading {}", config_path.display()))?;

        let quant_config = match read_json(&dir.join("hf_quant_config.json")) {
            Ok(v) => Some(v),
            Err(_) => None,
        };
        let index = match read_json(&dir.join("model.safetensors.index.json")) {
            Ok(v) => Some(v),
            Err(_) => None,
        };

        Ok(Self {
            dir,
            config,
            quant_config,
            index,
        })
    }

    fn architecture(&self) -> Option<String> {
        self.config
            .get("architectures")
            .and_then(|a| a.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| {
                self.config
                    .get("model_type")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
    }

    fn n_layers(&self) -> Option<u64> {
        int(&self.config, "num_hidden_layers")
            .or_else(|| int_nested_text(&self.config, "num_hidden_layers"))
    }

    fn n_embd(&self) -> Option<u64> {
        int(&self.config, "hidden_size").or_else(|| int_nested_text(&self.config, "hidden_size"))
    }

    fn ctx_train(&self) -> Option<u64> {
        int(&self.config, "max_position_embeddings")
            .or_else(|| int_nested_text(&self.config, "max_position_embeddings"))
            .or_else(|| int(&self.config, "max_sequence_length"))
    }

    fn vocab(&self) -> Option<u64> {
        int(&self.config, "vocab_size").or_else(|| int_nested_text(&self.config, "vocab_size"))
    }

    fn quant(&self) -> Option<String> {
        if let Some(qc) = &self.quant_config {
            // NVFP4 is expressed per-layer under quantization.quantized_layers.*;
            // surface it as the canonical label when present.
            if let Some(found) = search_value(qc, "NVFP4") {
                return Some(found.to_uppercase());
            }
            if let Some(q) = qc
                .get("quantization")
                .and_then(|q| q.get("quant_algo"))
                .and_then(|v| v.as_str())
            {
                if q != "MIXED_PRECISION" {
                    return Some(q.to_string());
                }
            }
            if let Some(p) = qc
                .get("producer")
                .and_then(|p| p.get("name"))
                .and_then(|v| v.as_str())
            {
                return Some(p.to_string());
            }
        }
        self.config
            .get("quantization_config")
            .and_then(|c| c.get("quant_method"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    }

    fn weight_size(&self) -> u64 {
        if let Some(idx) = &self.index {
            if let Some(total) = idx
                .get("metadata")
                .and_then(|m| m.get("total_size"))
                .and_then(|v| v.as_u64())
            {
                return total;
            }
        }
        let mut total = 0u64;
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) == Some("safetensors") {
                    // Follow symlinks: HF hub stores safetensors as symlinks
                    // into blobs/, and DirEntry::metadata reports the link size.
                    if let Ok(m) = std::fs::metadata(&p) {
                        total += m.len();
                    }
                }
            }
        }
        total
    }

    fn footprint(&self) -> u64 {
        let mut total = 0u64;
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for e in entries.flatten() {
                let p = e.path();
                if let Ok(m) = std::fs::metadata(&p) {
                    total += m.len();
                }
            }
        }
        total.max(self.weight_size())
    }

    pub fn into_meta(self) -> ModelMeta {
        let arch = self.architecture();
        let weight_size = self.weight_size();
        let name = self.pretty_name();

        ModelMeta {
            path: self.dir.clone(),
            format: ModelFormat::SafetensorsDir,
            name,
            arch,
            quant: self.quant(),
            params: None,
            n_layers: self.n_layers(),
            n_embd: self.n_embd(),
            n_head: None,
            n_head_kv: None,
            ctx_train: self.ctx_train(),
            vocab: self.vocab(),
            weight_size,
            footprint: self.footprint(),
        }
    }

    /// Prefer a human-facing name over a raw `model_type` token. Sources, in
    /// order: `_name_or_path` basename, the directory's last component (for
    /// e.g. `Qwen3.6-35B-A3B-NVFP4`), then `model_type`/arch as a last resort.
    fn pretty_name(&self) -> String {
        let mt = || {
            self.config
                .get("model_type")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };
        if let Some(n) = self
            .config
            .get("_name_or_path")
            .and_then(|v| v.as_str())
            .map(str::to_string)
        {
            if let Some(base) = n.rsplit('/').next() {
                let base = base.trim().to_string();
                if !base.is_empty() && base.chars().any(|c| c.is_ascii_alphabetic()) {
                    return base;
                }
            }
        }
        if let Some(dir) = self.dir.file_name().and_then(|s| s.to_str()) {
            let dir = dir.trim().to_string();
            if !dir.is_empty() && dir.chars().any(|c| c.is_ascii_alphabetic()) {
                return dir;
            }
        }
        mt().or_else(|| self.architecture())
            .unwrap_or_else(|| "unknown".into())
    }
}

pub fn open_dir(dir: impl AsRef<Path>) -> Result<ModelMeta> {
    SafetensorsModel::open(dir).map(|m| m.into_meta())
}

pub fn is_model_dir(dir: impl AsRef<Path>) -> bool {
    let dir = dir.as_ref();
    dir.join("config.json").exists()
        && (dir.join("model.safetensors.index.json").exists()
            || std::fs::read_dir(dir)
                .map(|e| {
                    e.flatten().any(|x| {
                        x.path().extension().and_then(|s| s.to_str()) == Some("safetensors")
                    })
                })
                .unwrap_or(false))
}

fn read_json(path: &Path) -> Result<serde_json::Value> {
    let text = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

/// Recursively searches for a string value containing `needle` anywhere in the
/// JSON tree. Used to surface nested quant labels like `W4A16_NVFP4`.
fn search_value(node: &serde_json::Value, needle: &str) -> Option<String> {
    match node {
        serde_json::Value::String(s) => {
            if s.contains(needle) {
                Some(s.clone())
            } else {
                None
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values() {
                if let Some(found) = search_value(v, needle) {
                    return Some(found);
                }
            }
            None
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                if let Some(found) = search_value(v, needle) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_nvfp4_if_present() {
        let path = std::path::Path::new("/home/deviant/models/Qwen36-35B-A3B-NVFP4");
        let path = if path.exists() {
            path.to_path_buf()
        } else {
            std::path::Path::new("/home/deviant/models/Qwen3.6-35B-A3B-NVFP4").to_path_buf()
        };
        if !path.exists() {
            eprintln!("NVFP4 fixture absent, skipping");
            return;
        }
        let meta = open_dir(&path).unwrap();
        assert!(
            meta.quant.map(|q| q.contains("NVFP4")).unwrap_or(false),
            "should detect NVFP4"
        );
        assert!(
            meta.weight_size > 20_000_000_000,
            "should read ~22GB from index"
        );
        assert!(
            meta.footprint >= meta.weight_size,
            "footprint must include weights"
        );
    }
}

fn int(value: &serde_json::Value, key: &str) -> Option<u64> {
    value
        .get(key)
        .and_then(|v| v.as_u64())
        .or_else(|| value.get(key).and_then(|v| v.as_f64()).map(|f| f as u64))
}

/// Multimodal / MoE configs nest the text-LLM parameters under a `text_config`
/// sub-object (e.g. `Qwen3_5MoeForConditionalGeneration`). Look there when the
/// top-level key is absent so fit/KV arithmetic still has layers×embd.
fn int_nested_text(value: &serde_json::Value, key: &str) -> Option<u64> {
    value.get("text_config").and_then(|t| int(t, key))
}
