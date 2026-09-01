//! Provider/model catalog fetching.
//!
//! Every built-in provider exposes an OpenAI-compatible `GET /v1/models`
//! endpoint. This module shells out to system `curl` (same transport as
//! deck-feeds — no HTTP client dependency) and maps the response into
//! `ProviderModel` entries. Responses are tolerated across provider quirks:
//! the common shape is `{"data":[{"id":"...","owned_by":"..."}]}`.

use std::process::Command;

use anyhow::{Context, Result};

use crate::model::{CloudProvider, ProviderModel};

/// Fetch a provider's model catalog via `GET {base_url}/models`.
/// `api_key` is optional; providers differ on whether listing requires auth.
pub fn fetch_models(p: &CloudProvider, api_key: Option<&str>) -> Result<Vec<ProviderModel>> {
    let url = format!("{}/models", p.base_url.trim_end_matches('/'));
    match fetch_url(&url, api_key, 20) {
        Ok(body) => parse_models_response(&body, &p.id),
        Err(e) if p.id == "gemini" && e.to_string().contains("404") => {
            // Google AI Studio OpenAI-compatible endpoint doesn't support /v1/models.
            // Return a static list of known Gemini models.
            Ok(gemini_static_models())
        }
        Err(e) => Err(e),
    }
}

fn gemini_static_models() -> Vec<ProviderModel> {
    // Current generation as of 2026-09: Google retired the 1.5/2.5 family for
    // new API keys (404 "no longer available to new users"), so the no-key
    // fallback must list models new keys can actually call. Verified live
    // against the OpenAI-compatible endpoint; context figures 1M flash-class,
    // None where unverified. With a key stored, the live catalog supersedes
    // this list entirely (see fetch_models).
    vec![
        ProviderModel {
            id: "gemini-3.6-flash".into(),
            name: "Gemini 3.6 Flash".into(),
            context: Some(1_048_576),
            free: true,
        },
        ProviderModel {
            id: "gemini-3.5-flash".into(),
            name: "Gemini 3.5 Flash".into(),
            context: Some(1_048_576),
            free: true,
        },
        ProviderModel {
            id: "gemini-3.5-flash-lite".into(),
            name: "Gemini 3.5 Flash-Lite".into(),
            context: Some(1_048_576),
            free: true,
        },
        ProviderModel {
            id: "gemini-3.1-pro-preview".into(),
            name: "Gemini 3.1 Pro (Preview)".into(),
            context: None,
            free: false,
        },
        ProviderModel {
            id: "gemini-3.1-flash-lite".into(),
            name: "Gemini 3.1 Flash-Lite".into(),
            context: Some(1_048_576),
            free: true,
        },
    ]
}

/// Parse the OpenAI-compatible `/v1/models` JSON body into provider models.
/// Returns an empty list on misshapen payloads rather than erroring, so one
/// provider's schema change never takes down the whole fleet view.
pub fn parse_models_response(body: &str, provider_id: &str) -> Result<Vec<ProviderModel>> {
    let v: serde_json::Value = serde_json::from_str(body)
        .with_context(|| format!("bad models JSON from {provider_id}"))?;
    let empty: Vec<serde_json::Value> = Vec::new();
    let data = v
        .get("data")
        .and_then(|d| d.as_array())
        .unwrap_or(&empty);
    let mut out = Vec::new();
    for row in data {
        let id = match row.get("id").and_then(|x| x.as_str()) {
            Some(id) => id,
            None => continue,
        };
        let name = row
            .get("name")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| id.to_string());
        // Context is often absent from /v1/models; individual routes or
        // catalog extras carry it. Default to None when not reported.
        let context = row
            .get("context_length")
            .and_then(|x| x.as_u64())
            .or_else(|| row.get("context").and_then(|x| x.as_u64()));
        // Heuristic: models whose id carries a "-free"/"free" marker, or that
        // a provider tags as free, are surfaced as such.
        let free = marker_is_free(id, row);
        out.push(ProviderModel {
            id: id.to_string(),
            name,
            context,
            free,
        });
    }
    Ok(out)
}

fn marker_is_free(id: &str, row: &serde_json::Value) -> bool {
    if id.contains("-free") || id.contains("free") {
        return true;
    }
    row.get("free").and_then(|x| x.as_bool()).unwrap_or(false)
}

/// Shell out to `curl -sSL --fail` against a URL with an optional bearer key.
/// Mirrors deck-feeds' transport so the app keeps its no-HTTP-client rule.
fn fetch_url(url: &str, api_key: Option<&str>, timeout_secs: u64) -> Result<String> {
    let mut cmd = Command::new("curl");
    cmd.args(["-sSL", "--fail", "--show-error", "--max-time"])
        .arg(timeout_secs.to_string());
    if let Some(key) = api_key.filter(|k| !k.is_empty()) {
        cmd.arg("-H").arg(format!("Authorization: Bearer {key}"));
    }
    cmd.arg(url);
    let out = cmd
        .output()
        .with_context(|| format!("spawn curl for {url}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "curl {} failed ({}): {}",
            url,
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    String::from_utf8(out.stdout).context("curl returned non-UTF-8 body")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_openai_models_payload() {
        let body = r#"{"object":"list","data":[
            {"id":"deepseek-v4-pro","object":"model","created":1,"owned_by":"provider"},
            {"id":"llama-3.3-70b-free","object":"model","created":1,"owned_by":"provider"}
        ]}"#;
        let ms = parse_models_response(body, "nim").unwrap();
        assert_eq!(ms.len(), 2);
        assert_eq!(ms[0].id, "deepseek-v4-pro");
        assert!(!ms[0].free);
        assert!(ms[1].free, "-free marker should flag a free model");
        assert_eq!(ms[1].name, "llama-3.3-70b-free");
    }

    #[test]
    fn tolerates_misshapen_payload() {
        let ms = parse_models_response("{}", "nim").unwrap();
        assert!(ms.is_empty());
        let ms2 = parse_models_response("not json", "nim").unwrap_err();
        assert!(ms2.to_string().contains("bad models JSON"));
    }

    #[test]
    fn reads_context_when_present() {
        let body = r#"{"data":[{"id":"m","context_length":128000}]}"#;
        let ms = parse_models_response(body, "groq").unwrap();
        assert_eq!(ms[0].context, Some(128000));
    }
}
