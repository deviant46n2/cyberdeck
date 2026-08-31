//! deck-agents — the online (cloud) fleet: coding-agent harnesses and their
//! free/paid model providers, quota tracking, and config rewriting.
//!
//! This is the "always-online intelligence" half of cyberdeck. The core fleet
//! (llama.cpp / FreeToken / Ollama) runs hardware-bound models on loopback
//! ports; the agent fleet fronts *cloud* providers (NVIDIA NIM, Groq, Gemini,
//! OpenRouter, OpenCode Go/Zen, ...) over remote OpenAI-compatible endpoints.
//! A harness (OpenCode, Goose, DeepSeek) is the agent UI/loop; it points at a
//! provider and one of that provider's models. Quotas are tracked per provider
//! so each harness keeps its native free-tier allowance.
//!
//! Layering: this crate is a domain crate — it depends only on deck-core (the
//! store) and does its own transport (system `curl`, the same way deck-feeds
//! does). No UI or engine code lives here.

pub mod model;
pub mod providers;
pub mod rewrite;
pub mod quota;
pub mod ops;

/// Human label for an OpenAI-compatible provider category intended to surface
/// whether an endpoint is a single service or an aggregator/router.
pub use model::ProviderKind;

/// A single provider's model entry from its `/v1/models` catalog.
pub use model::ProviderModel;

/// A free/paid online model source.
pub use model::CloudProvider;

/// A coding-agent harness (the agent UI/loop that talks to a provider).
pub use model::Harness;

/// Which provider+model a harness is currently bound to.
pub use model::HarnessBinding;

/// A measured (or provider-reported) quota reading for one provider.
pub use quota::QuotaSnapshot;
