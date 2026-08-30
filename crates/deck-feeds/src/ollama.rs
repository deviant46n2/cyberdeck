//! Ollama integration: enumerate installed models via the local API and map
//! each to its on-disk GGUF blob path.

use anyhow::Result;

/// Scan ollama's installed models: query the local API to get model names,
/// then parse `ollama show --modelfile <name>` to find the `FROM` blob path.
/// Only returns models that have an on-disk GGUF file (no cloud models).
pub fn ollama_models() -> Result<Vec<OllamaModelInfo>> {
    let text = crate::probe::fetch_url("http://localhost:11434/api/tags", 5)?;

    #[derive(serde::Deserialize)]
    struct OllamaList {
        models: Vec<OllamaEntry>,
    }
    #[derive(serde::Deserialize)]
    struct OllamaEntry {
        name: String,
    }

    let list: OllamaList = serde_json::from_str(&text)?;
    let mut results = Vec::new();

    for entry in list.models {
        // Parse modelfile to find the FROM blob path.
        let output = std::process::Command::new("ollama")
            .args(["show", "--modelfile", &entry.name])
            .output();

        if let Ok(o) = output {
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                if let Some(path_str) = line.strip_prefix("FROM ") {
                    let path_str = path_str.trim();
                    if path_str.is_empty() {
                        continue;
                    }
                    let p = std::path::Path::new(path_str);
                    if p.is_file() {
                        if let Ok(meta) = p.metadata() {
                            results.push(OllamaModelInfo {
                                name: entry.name.clone(),
                                path: path_str.to_string(),
                                size: meta.len(),
                            });
                        }
                    }
                    break;
                }
            }
        }
    }

    Ok(results)
}

#[derive(Debug, Clone)]
pub struct OllamaModelInfo {
    pub name: String,
    pub path: String,
    pub size: u64,
}

/// Outcome of an ollama-model delete by blob path.
#[derive(Debug, Clone, PartialEq)]
pub enum OllamaDeleteOutcome {
    /// `ollama rm` removed the tag referencing the blob; whether the blob
    /// itself is gone from disk (it survives while another tag shares it).
    Removed { blob_gone: bool },
    /// The blob is on disk but no installed ollama model references it.
    NoTag,
    /// The ollama daemon/CLI is unreachable or rejected the delete.
    DaemonUnavailable,
}

/// Delete an ollama model by its on-disk blob path. Ollama owns the blobs it
/// manages, so the only reliable door is the daemon itself (`ollama rm
/// <tag>`); a user-level `remove_file` hits Permission denied under
/// /var/lib/ollama/blobs. Multiple tags can share one blob, so the blob
/// survives while any other installed model still references it.
pub fn ollama_delete_blob(path: &str) -> OllamaDeleteOutcome {
    let models = match ollama_models() {
        Ok(m) => m,
        Err(_) => return OllamaDeleteOutcome::DaemonUnavailable,
    };
    let want = std::fs::canonicalize(path).unwrap_or_else(|_| std::path::PathBuf::from(path));
    let tag = models.iter().find(|m| {
        std::fs::canonicalize(&m.path).unwrap_or_else(|_| std::path::PathBuf::from(&m.path)) == want
    });
    let Some(tag) = tag.map(|m| m.name.clone()) else {
        return OllamaDeleteOutcome::NoTag;
    };

    match std::process::Command::new("ollama").args(["rm", &tag]).output() {
        Ok(o) if o.status.success() => {
            // Give the daemon a moment to reap the blob before checking disk.
            std::thread::sleep(std::time::Duration::from_millis(300));
            OllamaDeleteOutcome::Removed {
                blob_gone: !std::path::Path::new(path).exists(),
            }
        }
        _ => OllamaDeleteOutcome::DaemonUnavailable,
    }
}
