//! Core type definitions for the online agent fleet.
//!
//! `CloudProvider` and `Harness` are the two appwide selectors: you pick a
//! harness (the agent UI/loop) and a provider model (the online source). Both
//! are static catalogs here, like `deck_core::profile::Engine::all()`, plus
//! the DTOs the doors (CLI / Tauri) hand to the UI.

use serde::{Deserialize, Serialize};

/// Category of an online model endpoint. A service is a single provider's
/// API; an aggregator (OpenRouter) fronts many providers behind one key. This
/// drives quota semantics: aggregator quotas are router-level and can differ
/// from each underlying provider's native free tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// A single provider's native API (NIM, Groq, Gemini, DeepSeek, ...).
    Service,
    /// A router/aggregator fronting multiple providers (OpenRouter, ...).
    Aggregator,
}

/// One model entry in a provider's catalog (`/v1/models`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProviderModel {
    pub id: String,
    /// Human-friendly name; falls back to `id`.
    pub name: String,
    /// Context window in tokens, when the provider reports it.
    pub context: Option<u64>,
    /// Free-tier flag — surfaced so the fleet view can rank free sources.
    pub free: bool,
}

/// A free/paid online model source (NVIDIA NIM, Groq, Gemini, OpenRouter, ...).
#[derive(Debug, Clone, Serialize)]
pub struct CloudProvider {
    /// Stable id, e.g. `"nim"`, `"groq"`, `"gemini"`, `"openrouter"`.
    pub id: String,
    pub display: &'static str,
    /// OpenAI-compatible base URL (`…/v1`).
    pub base_url: String,
    pub kind: ProviderKind,
    /// True when the provider has a usable free tier.
    pub has_free_tier: bool,
    /// Free-tier note (e.g. "120+ models", "1,500 req/day"), for the UI.
    pub free_note: &'static str,
}

/// A coding-agent harness — the agent UI/loop that talks to providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HarnessId {
    Opencode,
    Goose,
    Deepseek,
}

impl HarnessId {
    pub fn as_str(self) -> &'static str {
        match self {
            HarnessId::Opencode => "opencode",
            HarnessId::Goose => "goose",
            HarnessId::Deepseek => "deepseek",
        }
    }
    pub fn all() -> [HarnessId; 3] {
        [HarnessId::Opencode, HarnessId::Goose, HarnessId::Deepseek]
    }
}

/// A coding-agent harness: id, display name, and where its config lives.
#[derive(Debug, Clone, Serialize)]
pub struct Harness {
    pub id: HarnessId,
    pub display: &'static str,
    /// Dot-namespaced key holding the active `HarnessBinding` in the settings
    /// store (e.g. `agents.opencode`).
    pub setting_key: &'static str,
}

impl Harness {
    pub fn all() -> Vec<Harness> {
        HarnessId::all()
            .iter()
            .map(|id| match id {
                HarnessId::Opencode => Harness {
                    id: *id,
                    display: "OpenCode",
                    setting_key: "agents.opencode",
                },
                HarnessId::Goose => Harness {
                    id: *id,
                    display: "Goose",
                    setting_key: "agents.goose",
                },
                HarnessId::Deepseek => Harness {
                    id: *id,
                    display: "DeepSeek",
                    setting_key: "agents.deepseek",
                },
            })
            .collect()
    }

    pub fn get(id: HarnessId) -> Harness {
        Harness::all().into_iter().find(|h| h.id == id).expect("harness catalog complete")
    }
}

/// The active provider+model for a harness — what you pick appwide.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HarnessBinding {
    pub provider_id: String,
    pub model_id: String,
}

/// The built-in provider catalog. Extend here to add a source; the fetch,
/// quota, and rewire layers all key off `CloudProvider.id`.
pub fn builtin_providers() -> Vec<CloudProvider> {
    vec![
        CloudProvider {
            id: "nim".into(),
            display: "NVIDIA NIM",
            base_url: "https://integrate.api.nvidia.com/v1".into(),
            kind: ProviderKind::Service,
            has_free_tier: true,
            free_note: "120+ open models · free tier",
        },
        CloudProvider {
            id: "groq".into(),
            display: "Groq",
            base_url: "https://api.groq.com/openai/v1".into(),
            kind: ProviderKind::Service,
            has_free_tier: true,
            free_note: "~14,400 req/day · very fast",
        },
        CloudProvider {
            id: "gemini".into(),
            display: "Google AI Studio (Gemini)",
            base_url: "https://generativelanguage.googleapis.com/v1beta/openai".into(),
            kind: ProviderKind::Service,
            has_free_tier: true,
            free_note: "~1,500 req/day Flash/Lite",
        },
        CloudProvider {
            id: "openrouter".into(),
            display: "OpenRouter",
            base_url: "https://openrouter.ai/api/v1".into(),
            kind: ProviderKind::Aggregator,
            has_free_tier: true,
            free_note: "many providers, one key",
        },
        CloudProvider {
            id: "go".into(),
            display: "OpenCode Go",
            base_url: "https://opencode.ai/zen/go/v1".into(),
            kind: ProviderKind::Service,
            has_free_tier: false,
            free_note: "$10/mo · 24 models",
        },
        CloudProvider {
            id: "zen".into(),
            display: "OpenCode Zen",
            base_url: "https://opencode.ai/zen/v1".into(),
            kind: ProviderKind::Service,
            has_free_tier: true,
            free_note: "75+ models · free tier",
        },
        CloudProvider {
            id: "deepseek".into(),
            display: "DeepSeek",
            base_url: "https://api.deepseek.com/v1".into(),
            kind: ProviderKind::Service,
            has_free_tier: true,
            free_note: "5M signup tokens · cheap paid",
        },
    ]
}

pub fn get_provider(id: &str) -> Option<CloudProvider> {
    builtin_providers().into_iter().find(|p| p.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_catalog_is_complete_and_unique() {
        let ps = builtin_providers();
        assert!(!ps.is_empty());
        let mut ids: Vec<&str> = ps.iter().map(|p| p.id.as_str()).collect();
        ids.sort();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "provider ids must be unique");
    }

    #[test]
    fn harness_catalog_matches_all_ids() {
        let hs = Harness::all();
        assert_eq!(hs.len(), HarnessId::all().len());
        for h in &hs {
            assert_eq!(Harness::get(h.id).id, h.id);
        }
    }

    #[test]
    fn get_provider_finds_builtins() {
        assert!(get_provider("nim").is_some());
        assert!(get_provider("openrouter").is_some());
        assert!(get_provider("does-not-exist").is_none());
    }
}
