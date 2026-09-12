//! Runtime discovery: builtins + custom manifests → merged registry.
//!
//! The schema lives in [`crate::runtime`]; this module assembles it.
//! Builtins derive from the existing `Engine` registry (same ports/units,
//! so behavior is unchanged); customs load from
//! `~/.local/share/cyberdeck/runtimes/*.json`, skipping broken files and
//! builtin-id collisions instead of failing.

use std::collections::BTreeMap;

use crate::profile::{Engine, EngineProtocol, ModelSource};
use crate::runtime::{ModelSourceSer, ProtocolSer, RuntimeManifest, RuntimeRow, RuntimeStatus};

fn builtin_status(engine: Engine) -> RuntimeStatus {
    match engine {
        Engine::LlamaCpp | Engine::FreeToken | Engine::Ollama => RuntimeStatus::Stable,
        Engine::UncensoredLlamaCpp => RuntimeStatus::Community,
    }
}

/// The existing engines expressed as manifests.
pub fn builtin_manifests() -> Vec<RuntimeManifest> {
    Engine::all()
        .into_iter()
        .map(|e| {
            let d = e.descriptor();
            let formats = match d.model_source {
                ModelSource::LocalPath => vec!["gguf".to_string(), "safetensors-dir".to_string()],
                ModelSource::OllamaStore => vec![],
            };
            let capabilities = if e == Engine::FreeToken {
                vec!["offload".to_string(), "cuda".to_string()]
            } else if e == Engine::Ollama {
                vec!["daemon".to_string()]
            } else {
                vec!["cuda".to_string(), "long-context".to_string()]
            };
            RuntimeManifest {
                id: d.id.to_string(),
                display: d.display.to_string(),
                version: None,
                formats,
                architectures: vec![],
                capabilities,
                model_source: match d.model_source {
                    ModelSource::LocalPath => ModelSourceSer::LocalPath,
                    ModelSource::OllamaStore => ModelSourceSer::OllamaStore,
                },
                protocol: match d.protocol {
                    EngineProtocol::OpenAiChat => ProtocolSer::OpenAiChat,
                    EngineProtocol::OllamaChat => ProtocolSer::OllamaChat,
                },
                default_port: d.default_port,
                test_port: d.test_port,
                unit_name: d.unit_name.to_string(),
                is_system_service: d.is_system_service,
                status: builtin_status(e),
                bin_candidates: vec![],
                configuration: vec![],
                argv_template: vec![],
                env: BTreeMap::new(),
            }
        })
        .collect()
}

/// Directory holding custom backend manifests (`*.json`).
pub fn custom_runtimes_dir() -> std::path::PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| std::path::PathBuf::from(".local/share"))
        .join("cyberdeck/runtimes")
}

/// Parse + validate one manifest file. Broken files Err (never poison the registry).
pub fn parse_manifest_file(path: &std::path::Path) -> anyhow::Result<RuntimeManifest> {
    let text = std::fs::read_to_string(path)?;
    let mut m: RuntimeManifest = serde_json::from_str(&text)?;
    if m.id.trim().is_empty() {
        anyhow::bail!("{}: manifest id is empty", path.display());
    }
    if m.default_port == 0 || m.test_port == 0 {
        anyhow::bail!("{}: ports must be nonzero", path.display());
    }
    if m.default_port == m.test_port {
        anyhow::bail!("{}: default_port == test_port", path.display());
    }
    if m.status == RuntimeStatus::Unknown {
        m.status = RuntimeStatus::Experimental;
    }
    if m.unit_name.trim().is_empty() {
        m.unit_name = format!("cyberdeck-{}.service", m.id);
    }
    m.validate()
        .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    Ok(m)
}

/// Load all custom manifests, skipping (not failing on) broken files.
/// A custom id that collides with a builtin is skipped — builtins win.
pub fn load_custom_manifests() -> Vec<RuntimeManifest> {
    let dir = custom_runtimes_dir();
    let entries: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .map(|r| r.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    let builtin_ids: std::collections::HashSet<String> =
        builtin_manifests().into_iter().map(|m| m.id).collect();
    let mut out = Vec::new();
    for p in entries {
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        match parse_manifest_file(&p) {
            Ok(m) if !builtin_ids.contains(&m.id) => out.push(m),
            Ok(_) => eprintln!("runtime manifest {} collides with a builtin id — skipped", p.display()),
            Err(e) => eprintln!("runtime manifest skipped: {e}"),
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Merged registry: builtins first, then customs. Custom rows are flagged.
pub fn list_runtimes() -> Vec<RuntimeRow> {
    let mut rows: Vec<RuntimeRow> = builtin_manifests().iter().map(RuntimeRow::from).collect();
    for m in load_custom_manifests() {
        rows.push(RuntimeRow {
            custom: true,
            ..RuntimeRow::from(&m)
        });
    }
    rows
}

/// Merged manifests (builtins + customs) for fit/launch planning.
pub fn all_manifests() -> Vec<RuntimeManifest> {
    let mut all = builtin_manifests();
    all.extend(load_custom_manifests());
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelFormat;

    #[test]
    fn builtins_cover_existing_engines_with_stable_ports() {
        let m = builtin_manifests();
        assert_eq!(m.len(), 4);
        let llamacpp = m.iter().find(|x| x.id == "llamacpp").unwrap();
        assert_eq!(llamacpp.default_port, 18000);
        assert_eq!(llamacpp.status, RuntimeStatus::Stable);
        assert!(llamacpp.supports(&ModelFormat::Gguf, Some("qwen3")));
        let ollama = m.iter().find(|x| x.id == "ollama").unwrap();
        assert!(!ollama.supports(&ModelFormat::Gguf, Some("qwen3")));
    }

    fn tmp_manifest(name: &str, body: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("deck-rt-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join(format!("{name}.json"));
        std::fs::write(&f, body).unwrap();
        f
    }

    #[test]
    fn custom_manifest_parses_and_renders_argv() {
        let f = tmp_manifest("beellama", r#"{"id":"beellama","display":"Beellama","formats":["gguf"],
            "capabilities":["cuda","long-context"],"default_port":18222,"test_port":18991,
            "argv_template":["serve","-m","{model}","--port","{port}","--ctx-size","{ctx}"]}"#);
        let m = parse_manifest_file(&f).unwrap();
        assert_eq!(m.id, "beellama");
        assert_eq!(m.status, RuntimeStatus::Experimental); // unknown -> experimental
        assert!(m.supports(&ModelFormat::Gguf, Some("qwen3")));
        assert!(!m.supports(&ModelFormat::SafetensorsDir, Some("qwen3")));
        let argv = m.render_argv("/models/q.gguf", "127.0.0.1", 18222, 49152, "q", &BTreeMap::new());
        assert_eq!(argv, vec!["serve", "-m", "/models/q.gguf", "--port", "18222", "--ctx-size", "49152"]);
        // Defaulted unit name keeps the manifest terse.
        assert_eq!(m.unit_name, "cyberdeck-beellama.service");
        // Empty id and live==test ports both reject.
        let bad = tmp_manifest("bad", r#"{"id":"","display":"x","default_port":1,"test_port":2}"#);
        assert!(parse_manifest_file(&bad).is_err());
        std::fs::write(&bad, r#"{"id":"x","display":"x","default_port":5,"test_port":5}"#).unwrap();
        assert!(parse_manifest_file(&bad).is_err());
        let _ = std::fs::remove_dir_all(f.parent().unwrap());
        let _ = std::fs::remove_dir_all(bad.parent().unwrap());
    }

    #[test]
    fn declared_param_overrides_and_optional_flag_drop() {
        let f = tmp_manifest(
            "param",
            r#"{"id":"param","display":"P","default_port":18230,"test_port":18990,
                "configuration":[{"key":"gpu_layers","default":"999"},{"key":"kv_cache"}],
                "argv_template":["--model","{model}","--n-gpu-layers","{gpu_layers}","--cache-type-k","{kv_cache}"]}"#,
        );
        let m = parse_manifest_file(&f).unwrap();
        // Default gpu_layers=999; kv_cache has no default/override -> flag+value dropped.
        let argv = m.render_argv("/m.gguf", "h", 1, 2048, "a", &BTreeMap::new());
        assert_eq!(argv, vec!["--model", "/m.gguf", "--n-gpu-layers", "999"]);
        // Override wins; kv_cache now set -> flag reappears.
        let over = BTreeMap::from([("gpu_layers".to_string(), "0".to_string()), ("kv_cache".to_string(), "q4_0".to_string())]);
        let argv = m.render_argv("/m.gguf", "h", 1, 2048, "a", &over);
        assert_eq!(argv, vec!["--model", "/m.gguf", "--n-gpu-layers", "0", "--cache-type-k", "q4_0"]);
        let _ = std::fs::remove_dir_all(f.parent().unwrap());
    }

    #[test]
    fn unknown_placeholder_is_rejected_at_load() {
        let f = tmp_manifest(
            "typo",
            r#"{"id":"typo","display":"T","default_port":18231,"test_port":18989,
                "argv_template":["--model","{model}","--ctx","{contxt}"]}"#,
        );
        let err = parse_manifest_file(&f).unwrap_err().to_string();
        assert!(err.contains("contxt"), "error should name the bad placeholder: {err}");
        let _ = std::fs::remove_dir_all(f.parent().unwrap());
    }
}
