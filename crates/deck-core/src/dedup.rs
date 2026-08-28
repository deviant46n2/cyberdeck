//! Shard-dedup detector.
//!
//! Groups indexed models by logical identity (arch + quant + param count).
//! Two distinct files/dirs sharing one identity are a duplicate — usually a
//! `~/models` copy sitting beside the HuggingFace hub cache. Reports wasted
//! bytes so the UI can surface "this costs you 45GB twice".

use std::collections::HashMap;

use crate::model::ModelMeta;

#[derive(Debug, Clone)]
pub struct DupGroup {
    pub identity: String,
    pub members: Vec<ModelMeta>,
    /// Sum of extra footprint beyond the first (cheapest) copy.
    pub wasted_bytes: u64,
}

pub fn find_duplicates(models: &[ModelMeta]) -> Vec<DupGroup> {
    let mut by_identity: HashMap<String, Vec<ModelMeta>> = HashMap::new();
    for m in models {
        by_identity.entry(m.identity()).or_default().push(m.clone());
    }

    let mut groups = Vec::new();
    for (identity, mut members) in by_identity {
        if members.len() < 2 {
            continue;
        }
        members.sort_by_key(|m| m.footprint);
        let kept = members[0].footprint;
        // Reclaimable space = everything minus the single cheapest copy.
        let wasted: u64 = members.iter().map(|m| m.footprint).sum::<u64>() - kept;
        groups.push(DupGroup {
            identity,
            members,
            wasted_bytes: wasted,
        });
    }
    groups.sort_by(|a, b| b.wasted_bytes.cmp(&a.wasted_bytes));
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ModelFormat, ModelMeta};
    use std::path::PathBuf;

    fn fake(identity: &str, footprint: u64) -> ModelMeta {
        ModelMeta {
            path: PathBuf::from(format!("/x/{identity}/{footprint}")),
            format: ModelFormat::Gguf,
            name: identity.into(),
            arch: Some(identity.into()),
            quant: Some("Q4_0".into()),
            params: None,
            n_layers: None,
            n_embd: None,
            n_head: None,
            n_head_kv: None,
            ctx_train: None,
            vocab: None,
            weight_size: footprint,
            footprint,
        }
    }

    #[test]
    fn equal_copies_report_full_reclaim() {
        let models = vec![fake("dup", 100), fake("dup", 100)];
        let groups = find_duplicates(&models);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].wasted_bytes, 100);
    }

    #[test]
    fn mixed_sizes_keep_cheapest() {
        let models = vec![fake("dup", 100), fake("dup", 50), fake("dup", 100)];
        let groups = find_duplicates(&models);
        assert_eq!(groups[0].wasted_bytes, 200);
    }
}
