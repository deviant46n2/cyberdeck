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
