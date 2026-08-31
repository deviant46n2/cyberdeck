//! Per-provider quota tracking.
//!
//! Each provider's free tier is the native allowance (the whole point of going
//! direct rather than through an aggregator). Providers report limits very
//! differently — requests/day, tokens/month, minute RPM — and most do not
//! expose "current usage" via `/v1/models` at all. So we model quota the honest
//! way: record whatever the provider states (`limit`, `window`), track an
//! `estimated_used` the user/agent can bump, and keep a `source` flag so the
//! UI can say "provider-reported" vs "estimate" without over-claiming accuracy.

use serde::{Deserialize, Serialize};

/// How a provider expresses its free-tier allowance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuotaWindow {
    RequestsPerDay,
    RequestsPerMinute,
    TokensPerMonth,
    TokensSignupTotal,
}

impl QuotaWindow {
    pub fn label(self) -> &'static str {
        match self {
            QuotaWindow::RequestsPerDay => "req/day",
            QuotaWindow::RequestsPerMinute => "req/min",
            QuotaWindow::TokensPerMonth => "tok/mo",
            QuotaWindow::TokensSignupTotal => "tok signup",
        }
    }
}

/// Truthfulness of a quota reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuotaSource {
    /// Reported directly by the provider's API/site.
    Provider,
    /// Estimate — the user or agent bumped it; may drift.
    Estimate,
    /// No limits recorded yet for this provider.
    Unknown,
}

/// A stored quota row for one provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaEntry {
    pub provider_id: String,
    pub window: Option<QuotaWindow>,
    pub limit: Option<u64>,
    pub used: u64,
    pub source: QuotaSource,
    /// Epoch-seconds when the window resets (None = rolling/unknown).
    pub resets_at: Option<i64>,
}

/// A read model for the UI: the fraction consumed + human window label.
#[derive(Debug, Clone, Serialize)]
pub struct QuotaSnapshot {
    pub provider_id: String,
    pub label: &'static str,
    pub used: u64,
    pub limit: Option<u64>,
    pub pct: Option<f64>, // used/limit when known
    pub source: QuotaSource,
}

impl QuotaSnapshot {
    /// Build the snapshot from a stored entry; `pct` is None when no limit.
    pub fn from_entry(e: &QuotaEntry) -> QuotaSnapshot {
        let label = e.window.map(|w| w.label()).unwrap_or("");
        let pct = e.limit.filter(|l| *l > 0).map(|l| e.used as f64 / l as f64);
        QuotaSnapshot {
            provider_id: e.provider_id.clone(),
            label,
            used: e.used,
            limit: e.limit,
            pct,
            source: e.source,
        }
    }

    /// Render the consumed share for display, clamped to a sane 0..=2 range.
    pub fn pct_display(&self) -> f64 {
        self.pct.unwrap_or(0.0).clamp(0.0, 2.0)
    }
}

/// Default quota entries for the built-in free tiers (best-known figures, as
/// of 2026-08). Verifiable figures live here; genuinely unknown providers are
/// `Unknown`. Treat as seed data the user can correct.
pub fn default_quota(provider_id: &str) -> QuotaEntry {
    let (window, limit, source) = match provider_id {
        "groq" => (Some(QuotaWindow::RequestsPerDay), Some(14_400), QuotaSource::Provider),
        "gemini" => (Some(QuotaWindow::RequestsPerDay), Some(1_500), QuotaSource::Provider),
        "nim" => (Some(QuotaWindow::TokensPerMonth), None, QuotaSource::Provider),
        "deepseek" => (Some(QuotaWindow::TokensSignupTotal), Some(5_000_000), QuotaSource::Provider),
        "openrouter" => (Some(QuotaWindow::RequestsPerDay), None, QuotaSource::Estimate),
        _ => (None, None, QuotaSource::Unknown),
    };
    QuotaEntry {
        provider_id: provider_id.to_string(),
        window,
        limit,
        used: 0,
        source,
        resets_at: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_computes_pct() {
        let mut e = default_quota("groq");
        e.used = 7_200;
        let s = QuotaSnapshot::from_entry(&e);
        assert_eq!(s.limit, Some(14_400));
        assert!((s.pct.unwrap() - 0.5).abs() < 1e-9);
        assert!((s.pct_display() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn snapshot_tolerates_missing_limit() {
        let e = default_quota("nim");
        assert_eq!(e.limit, None);
        let s = QuotaSnapshot::from_entry(&e);
        assert_eq!(s.pct, None);
        assert_eq!(s.pct_display(), 0.0);
    }

    #[test]
    fn labels_match_windows() {
        assert_eq!(QuotaWindow::RequestsPerDay.label(), "req/day");
        assert_eq!(QuotaWindow::TokensPerMonth.label(), "tok/mo");
    }
}
