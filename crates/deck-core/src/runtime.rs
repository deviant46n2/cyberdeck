//! Runtime adapter registry: the seam every backend plugs into.
//!
//! Every backend describes itself with a [`RuntimeManifest`]; builtins are
//! derived from the existing `Engine` registry, and experimental backends
//! add ONE JSON file under `~/.local/share/cyberdeck/runtimes/*.json` —
//! no core changes. JSON keeps serde_json as the only dep.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::ModelFormat;
use crate::profile::{EngineProtocol, ModelSource};

/// Maturity of a backend. Informational only — never gates execution.
/// Experimental/community/unknown runtimes run the same pipeline as stable.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeStatus {
    #[default]
    Unknown,
    Stable,
    Experimental,
    Community,
}

impl RuntimeStatus {
    pub fn tag(self) -> &'static str {
        match self {
            RuntimeStatus::Stable => "stable",
            RuntimeStatus::Experimental => "experimental",
            RuntimeStatus::Community => "community",
            RuntimeStatus::Unknown => "unknown",
        }
    }
}

/// One tunable a backend exposes. Values live in `Profile::params` (structured,
/// not an opaque CLI string); the argv/env templates reference them by `{key}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigParam {
    pub key: String,
    #[serde(default)]
    pub label: String,
    /// Value used when the profile carries no override. `None` + no override
    /// means the placeholder is dropped from argv (flag omitted).
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub help: String,
}

/// Builtin template placeholders always available to a manifest, independent
/// of `configuration`. Each maps to a value the core resolves at launch.
pub const TEMPLATE_BUILTINS: &[&str] = &["model", "host", "port", "ctx", "alias"];

/// Declarative description of one backend. Builtins are generated from
/// `Engine::descriptor()`; customs parse from JSON (unknown fields ignored).
/// Empty `formats`/`architectures` = accepts anything.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeManifest {
    pub id: String,
    pub display: String,
    #[serde(default)]
    pub version: Option<String>,
    /// "gguf" / "safetensors-dir". Empty = any.
    #[serde(default)]
    pub formats: Vec<String>,
    /// "qwen3", "llama", ... Empty = any.
    #[serde(default)]
    pub architectures: Vec<String>,
    /// Free-form tags: "cuda", "long-context", "offload", ... (`offload`
    /// switches the fit VRAM model to spill weights to RAM).
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub model_source: ModelSourceSer,
    #[serde(default)]
    pub protocol: ProtocolSer,
    pub default_port: u16,
    pub test_port: u16,
    #[serde(default)]
    pub unit_name: String,
    #[serde(default)]
    pub is_system_service: bool,
    #[serde(default)]
    pub status: RuntimeStatus,
    /// Binary lookup order (first existing path wins). Empty = engine default.
    #[serde(default)]
    pub bin_candidates: Vec<String>,
    /// Tunables this backend exposes; defaults seed `Profile::params`.
    #[serde(default)]
    pub configuration: Vec<ConfigParam>,
    /// Launcher argv template (`{model}` `{host}` `{port}` `{ctx}` `{alias}`
    /// plus any declared config key); empty falls back to
    /// `["--model", "{model}", "--port", "{port}"]`.
    #[serde(default)]
    pub argv_template: Vec<String>,
    /// Environment variables (values may use the same placeholders).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ModelSourceSer {
    #[default]
    LocalPath,
    OllamaStore,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ProtocolSer {
    #[default]
    OpenAiChat,
    OllamaChat,
}

impl RuntimeManifest {
    pub fn model_source(&self) -> ModelSource {
        match self.model_source {
            ModelSourceSer::LocalPath => ModelSource::LocalPath,
            ModelSourceSer::OllamaStore => ModelSource::OllamaStore,
        }
    }

    pub fn protocol(&self) -> EngineProtocol {
        match self.protocol {
            ProtocolSer::OpenAiChat => EngineProtocol::OpenAiChat,
            ProtocolSer::OllamaChat => EngineProtocol::OllamaChat,
        }
    }

    pub fn is_offload(&self) -> bool {
        self.capabilities.iter().any(|c| c == "offload")
    }

    /// Dynamic compatibility check: artifact format + architecture + model
    /// source. Hardware (VRAM) is the fit engine's job, not this function's.
    pub fn supports(&self, format: &ModelFormat, arch: Option<&str>) -> bool {
        if self.model_source() != ModelSource::LocalPath {
            return false;
        }
        if !self.formats.is_empty() {
            let want = match format {
                ModelFormat::Gguf => "gguf",
                ModelFormat::SafetensorsDir => "safetensors-dir",
            };
            if !self.formats.iter().any(|f| f == want) {
                return false;
            }
        }
        if !self.architectures.is_empty() {
            let arch = arch.unwrap_or("?");
            if !self.architectures.iter().any(|a| a == arch) {
                return false;
            }
        }
        true
    }

    /// Values the core always resolves for a launch, independent of the
    /// manifest's declared `configuration`.
    pub fn builtin_values(model: &str, host: &str, port: u16, ctx: u32, alias: &str) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("model".to_string(), model.to_string()),
            ("host".to_string(), host.to_string()),
            ("port".to_string(), port.to_string()),
            ("ctx".to_string(), ctx.to_string()),
            ("alias".to_string(), alias.to_string()),
        ])
    }

    /// Declared tunables seeded at their manifest defaults (no overrides).
    pub fn default_params(&self) -> BTreeMap<String, String> {
        self.configuration
            .iter()
            .filter_map(|c| c.default.clone().map(|d| (c.key.clone(), d)))
            .collect()
    }

    /// Manifest defaults with the profile's overrides layered on top.
    pub fn effective_params(&self, overrides: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        let mut vals = self.default_params();
        for (k, v) in overrides {
            vals.insert(k.clone(), v.clone());
        }
        vals
    }

    /// Render argv. Builtin placeholders + declared params are substituted; a
    /// bare `{key}` with no value drops itself AND a preceding `-flag` token,
    /// so optional flags disappear cleanly instead of emitting `--flag`.
    pub fn render_argv(
        &self,
        model: &str,
        host: &str,
        port: u16,
        ctx: u32,
        alias: &str,
        overrides: &BTreeMap<String, String>,
    ) -> Vec<String> {
        let mut vals = Self::builtin_values(model, host, port, ctx, alias);
        vals.extend(self.effective_params(overrides));
        let tmpl = if self.argv_template.is_empty() {
            vec!["--model".to_string(), "{model}".to_string(), "--port".to_string(), "{port}".to_string()]
        } else {
            self.argv_template.clone()
        };
        let mut out: Vec<String> = Vec::new();
        for t in tmpl {
            if let Some(key) = bare_placeholder(&t)
                && !vals.contains_key(key)
            {
                if out.last().map(|p| p.starts_with('-')).unwrap_or(false) {
                    out.pop();
                }
                continue;
            }
            out.push(subst(&t, &vals));
        }
        out
    }

    /// Render env vars (same placeholder rules; always emitted).
    pub fn render_env(
        &self,
        model: &str,
        host: &str,
        port: u16,
        ctx: u32,
        alias: &str,
        overrides: &BTreeMap<String, String>,
    ) -> Vec<(String, String)> {
        let mut vals = Self::builtin_values(model, host, port, ctx, alias);
        vals.extend(self.effective_params(overrides));
        self.env
            .iter()
            .map(|(k, v)| (k.clone(), subst(v, &vals)))
            .collect()
    }

    /// Static validation at load time: every `{placeholder}` in the templates
    /// must be a builtin or a declared config key. Catches manifest typos
    /// before a launch fails with a confusing argv.
    pub fn validate(&self) -> anyhow::Result<()> {
        let declared: std::collections::HashSet<&str> = self
            .configuration
            .iter()
            .map(|c| c.key.as_str())
            .chain(TEMPLATE_BUILTINS.iter().copied())
            .collect();
        for (i, t) in self.argv_template.iter().enumerate() {
            for name in placeholders(t) {
                if !declared.contains(name.as_str()) {
                    anyhow::bail!(
                        "argv_template[{i}] '{t}': unknown placeholder '{{{name}}}' (declare it in configuration or use {})",
                        TEMPLATE_BUILTINS.join("/")
                    );
                }
            }
        }
        for (k, v) in &self.env {
            for name in placeholders(v) {
                if !declared.contains(name.as_str()) {
                    anyhow::bail!("env {k}: unknown placeholder '{{{name}}}'");
                }
            }
        }
        Ok(())
    }
}

/// Substitute every `{name}` present in `vals`; unknown braces are left as-is
/// (validation rejects those before launch).
fn subst(t: &str, vals: &BTreeMap<String, String>) -> String {
    let mut out = t.to_string();
    for (k, v) in vals {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

/// `{key}` → Some("key"); anything else (including `--flag={key}`) → None.
fn bare_placeholder(t: &str) -> Option<&str> {
    t.strip_prefix('{')?.strip_suffix('}')
}

/// All `{name}` occurrences in a token.
fn placeholders(t: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = t;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        if let Some(close) = after.find('}') {
            out.push(after[..close].to_string());
            rest = &after[close + 1..];
        } else {
            break;
        }
    }
    out
}

/// One row of the merged registry (builtin or custom) as the UI consumes it.
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeRow {
    pub id: String,
    pub display: String,
    pub status: String,
    pub default_port: u16,
    pub test_port: u16,
    pub formats: Vec<String>,
    pub capabilities: Vec<String>,
    pub custom: bool,
}

impl From<&RuntimeManifest> for RuntimeRow {
    fn from(m: &RuntimeManifest) -> Self {
        Self {
            id: m.id.clone(),
            display: m.display.clone(),
            status: m.status.tag().to_string(),
            default_port: m.default_port,
            test_port: m.test_port,
            formats: m.formats.clone(),
            capabilities: m.capabilities.clone(),
            custom: false,
        }
    }
}

/// Registry assembly (builtins + custom discovery) lives in
/// [`crate::runtime_registry`]; re-exported here so `runtime::*` stays the
/// single import path for both schema and discovery.
pub use crate::runtime_registry::{
    all_manifests, builtin_manifests, custom_runtimes_dir, list_runtimes,
    load_custom_manifests, parse_manifest_file,
};
