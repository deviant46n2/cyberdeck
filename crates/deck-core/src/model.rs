//! Unified model descriptor produced by every format parser so the
//! scanner/store/fit/UI treat GGUF and safetensors-model-dirs identically.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum ModelFormat {
    Gguf,
    SafetensorsDir,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelMeta {
    pub path: PathBuf,
    pub format: ModelFormat,
    /// Human-facing identity: GGUF general.name or HF model_type + repo hint.
    pub name: String,
    /// Publisher-declared model family, GGUF `general.basename` (e.g.
    /// "Qwen3.8-27B" — quant-free by spec). None when the format carries no
    /// family signal (safetensors dirs, unparseable headers).
    pub basename: Option<String>,
    pub arch: Option<String>,
    pub quant: Option<String>,
    /// Actual tensor-parameter count when known, else estimated from bytes.
    pub params: Option<u64>,
    pub n_layers: Option<u64>,
    pub n_embd: Option<u64>,
    /// Attention head counts (GGUF `attention.head_count[_kv]`). Their ratio
    /// gives the GQA KV width — the number that actually sizes the KV cache.
    pub n_head: Option<u64>,
    pub n_head_kv: Option<u64>,
    pub ctx_train: Option<u64>,
    pub vocab: Option<u64>,
    /// Bytes of tensor weights (GGUF file size, or safetensors total_size).
    pub weight_size: u64,
    /// Total on-disk footprint incl. tokenizer/config overhead.
    pub footprint: u64,
}

impl ModelMeta {
    /// Logical-identity key for dedup grouping. Uses arch + weight-size bucket
    /// (rounded to 0.5 GiB) rather than the quant label, because the same
    /// model can be labelled differently across copies (e.g. `modelopt` in an
    /// HF hub snapshot vs `W4A16_NVFP4` in a local export).
    pub fn identity(&self) -> String {
        let bucket = (self.weight_size as f64 / (512.0 * 1_048_576.0)).round() as u64;
        format!(
            "{}|{:?}|{}",
            self.arch.as_deref().unwrap_or("?"),
            self.format,
            bucket
        )
    }

    /// Vault grouping key: one listing per model family, every variant inside
    /// individually selectable. A declared `basename` groups by arch + model
    /// line: when it carries a `major.minor` token, that token IS the family
    /// ("Qwen3.8-27B", "Qwen3.8", "Huihui-Qwen3.8" all list as Qwen 3.8 while
    /// Qwen3.6 stays apart). Without a version token the full basename is the
    /// key; without a basename it falls back to [`ModelMeta::identity`] —
    /// today's rows, byte for byte. Dedup keeps using `identity()` on purpose:
    /// broadening it could delete quants the user wants to keep.
    pub fn group_key(&self) -> String {
        let arch = self.arch.as_deref().unwrap_or("?").to_lowercase();
        match self.basename.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(b) => {
                let lower = b.to_lowercase();
                match version_token(&lower) {
                    Some(v) => format!("{arch}|{v}"),
                    None => format!("{arch}|{lower}"),
                }
            }
            None => self.identity(),
        }
    }
}

/// First `major.minor` token in a family name ("huihui-qwen3.8" → "3.8").
/// Bare sizes ("27b") and single numbers ("v2") are not versions.
fn version_token(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let mut j = i;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            let mut k = j;
            if k < bytes.len() && bytes[k] == b'.' {
                k += 1;
            }
            let mut l = k;
            while l < bytes.len() && bytes[l].is_ascii_digit() {
                l += 1;
            }
            if k > j && l > k {
                return Some(name[i..l].to_string());
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(basename: Option<&str>, arch: Option<&str>, bytes: u64) -> ModelMeta {
        ModelMeta {
            path: std::path::PathBuf::from(format!("/m/{}.gguf", basename.unwrap_or("x"))),
            format: ModelFormat::Gguf,
            name: "n".into(),
            basename: basename.map(str::to_string),
            arch: arch.map(str::to_string),
            quant: Some("Q4_K_M".into()),
            params: None,
            n_layers: None,
            n_embd: None,
            n_head: None,
            n_head_kv: None,
            ctx_train: None,
            vocab: None,
            weight_size: bytes,
            footprint: bytes,
        }
    }

    #[test]
    fn same_basename_groups_across_quants_and_sizes() {
        let gib = 1024 * 1024 * 1024u64;
        let a = meta(Some("Qwen3.8-27B"), Some("qwen35"), 10 * gib);
        let b = meta(Some("qwen3.8-27b"), Some("QWEN35"), 13 * gib);
        assert_eq!(a.group_key(), b.group_key(), "case-insensitive family key");
    }

    #[test]
    fn different_families_or_arches_stay_apart() {
        let gib = 1024 * 1024 * 1024u64;
        let a = meta(Some("Qwen3.8-27B"), Some("qwen35"), 10 * gib);
        let b = meta(Some("Muse-Glimmer-30B"), Some("muse-glimmer"), 10 * gib);
        let c = meta(Some("Qwen3.8-27B"), Some("other-arch"), 10 * gib);
        assert_ne!(a.group_key(), b.group_key());
        assert_ne!(a.group_key(), c.group_key());
    }

    #[test]
    fn abliterated_finetunes_share_the_38_listing_not_36() {
        let gib = 1024 * 1024 * 1024u64;
        let base = meta(Some("Qwen3.8-27B"), Some("qwen35"), 12 * gib);
        let abit = meta(Some("Qwen3.8"), Some("qwen35"), 13 * gib);
        let huihui = meta(Some("Huihui-Qwen3.8"), Some("qwen35"), 16 * gib);
        let q36 = meta(Some("qwen3.6"), Some("qwen35"), 12 * gib);
        assert_eq!(base.group_key(), abit.group_key());
        assert_eq!(base.group_key(), huihui.group_key());
        assert_ne!(base.group_key(), q36.group_key());
    }

    #[test]
    fn version_token_ignores_bare_sizes_and_singles() {
        assert_eq!(version_token("qwen3.8-27b"), Some("3.8".to_string()));
        assert_eq!(version_token("huihui-qwen3.8"), Some("3.8".to_string()));
        assert_eq!(version_token("gemma-4-26b-a4b-it"), None);
        assert_eq!(version_token("mistral"), None);
        assert_eq!(version_token("model-v2"), None);
    }

    #[test]
    fn no_basename_falls_back_to_identity_rows() {
        let gib = 1024 * 1024 * 1024u64;
        let a = meta(None, Some("qwen3"), 10 * gib);
        let b = meta(None, Some("qwen3"), 10 * gib + 100);
        assert_eq!(a.group_key(), b.group_key());
        assert_eq!(a.group_key(), a.identity(), "fallback is today's grouping");
        let c = meta(None, Some("qwen3"), 20 * gib);
        assert_ne!(a.group_key(), c.group_key(), "different buckets stay apart");
    }
}
