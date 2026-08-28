//! HuggingFace API probing over curl: org polling, model-list parsing, and
//! the partial (Range) GGUF header fetch used for fit estimation.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::Deserialize;

/// GET a URL as UTF-8 text (JSON APIs), failing loudly on non-2xx.
pub(crate) fn fetch_url(url: &str, timeout_secs: u64) -> Result<String> {
    let out = std::process::Command::new("curl")
        .args([
            "-sSL",
            "--fail",
            "--show-error",
            "--max-time",
            &timeout_secs.to_string(),
            url,
        ])
        .output()
        .context("failed to spawn curl (is it installed?)")?;
    if !out.status.success() {
        anyhow::bail!(
            "GET {url} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub(crate) fn simple_encode(s: &str) -> String {
    s.replace(' ', "%20").replace('#', "%23")
}

#[derive(Debug, Clone)]
pub struct HfModel {
    pub id: String,
    pub author: String,
    pub created_at: String,
    pub downloads: u64,
    pub likes: u64,
    pub pipeline_tag: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Deserialize)]
struct HfRow {
    id: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    likes: u64,
    #[serde(default)]
    pipeline_tag: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

/// Parse the HF `/api/models` JSON array into our model shape (pure, testable).
pub fn parse_models(json: &str) -> Result<Vec<HfModel>> {
    let rows: Vec<HfRow> = serde_json::from_str(json)?;
    Ok(rows
        .into_iter()
        .map(|r| HfModel {
            author: r
                .author
                .unwrap_or_else(|| r.id.split('/').next().unwrap_or("").to_string()),
            created_at: r.created_at.unwrap_or_default(),
            id: r.id,
            downloads: r.downloads,
            likes: r.likes,
            pipeline_tag: r.pipeline_tag,
            tags: r.tags,
        })
        .collect())
}

/// Keeps only models whose id is not in `seen` (pure, testable).
pub fn diff_new(all: &[HfModel], seen: &[String]) -> Vec<HfModel> {
    let seen_set: std::collections::HashSet<&str> = seen.iter().map(|s| s.as_str()).collect();
    all.iter()
        .filter(|m| !seen_set.contains(m.id.as_str()))
        .cloned()
        .collect()
}

/// Fetch the most recent `limit` models authored by `org` from the HF API.
pub fn fetch_org(org: &str, limit: usize) -> Result<Vec<HfModel>> {
    let url = format!(
        "https://huggingface.co/api/models?author={}&sort=createdAt&direction=-1&limit={}",
        simple_encode(org),
        limit
    );
    let body = fetch_url(&url, 20)
        .with_context(|| format!("HF API request for '{org}' failed (offline?)"))?;
    parse_models(&body)
}

/// Fetch the first `max_bytes` of a GGUF file via HTTP Range, returning the
/// bytes and the total file size (from Content-Range). Used to parse GGUF
/// header metadata for fit estimation without downloading the full file.
///
/// `max_bytes` defaults to 2 MiB which comfortably covers all scalar KVs
/// (arch, block_count, embedding_length, file_type) before large tokenizer
/// arrays that the parser gracefully truncates past.
pub fn fetch_gguf_bytes(repo_id: &str, rfilename: &str, max_bytes: u64) -> Result<(Vec<u8>, u64)> {
    let url = format!(
        "https://huggingface.co/{repo_id}/resolve/main/{name}",
        repo_id = repo_id,
        name = simple_encode(rfilename)
    );
    let range = format!("bytes=0-{}", max_bytes.saturating_sub(1));

    // Dump response headers to a temp file (-D) while streaming the body on
    // stdout, so we can read Content-Range/Content-Length after the fact.
    let hdr_path = std::env::temp_dir().join(format!(
        "deck-hdr-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    ));
    let out = std::process::Command::new("curl")
        .args([
            "-sSL",
            "--fail",
            "--show-error",
            "--max-time",
            "30",
            "-H",
            &range,
            "-D",
            hdr_path.to_str().context("temp dir not utf-8")?,
            &url,
        ])
        .output()
        .with_context(|| format!("GGUF header fetch for '{repo_id}/{rfilename}' failed"))?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&hdr_path);
        anyhow::bail!(
            "GGUF header fetch for '{repo_id}/{rfilename}' failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let hdr_text = String::from_utf8_lossy(&std::fs::read(&hdr_path)?).into_owned();
    let _ = std::fs::remove_file(&hdr_path);

    // Total size from the FINAL Content-Range ("bytes 0-N/TOTAL") of a 206, or
    // fall back to final Content-Length / our cap.
    let total_size = hdr_text
        .lines()
        .filter(|l| l.to_lowercase().starts_with("content-range:"))
        .last()
        .and_then(|l| l.rsplit('/').next()?.trim().parse::<u64>().ok())
        .or_else(|| {
            hdr_text
                .lines()
                .filter(|l| l.to_lowercase().starts_with("content-length:"))
                .last()
                .and_then(|l| l.split(':').nth(1)?.trim().parse::<u64>().ok())
        })
        .unwrap_or(max_bytes);

    let mut buf: Vec<u8> = Vec::with_capacity(out.stdout.len());
    buf.extend_from_slice(&out.stdout[..out.stdout.len().min(max_bytes as usize)]);
    Ok((buf, total_size))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[
      {"id":"unsloth/GLM-5.3-Flash-GGUF","author":"unsloth","createdAt":"2026-08-26T14:00:00Z","downloads":0,"likes":132,"pipeline_tag":"text-generation","tags":["transformers","gguf"]},
      {"id":"unsloth/Qwen3.8-GGUF","author":"unsloth","createdAt":"2026-08-20T10:00:00Z","downloads":40,"likes":12,"tags":["gguf"]}
    ]"#;

    #[test]
    fn parses_hf_shape() {
        let models = parse_models(SAMPLE).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "unsloth/GLM-5.3-Flash-GGUF");
        assert_eq!(models[0].author, "unsloth");
        assert_eq!(models[0].likes, 132);
        assert_eq!(models[0].pipeline_tag.as_deref(), Some("text-generation"));
    }

    #[test]
    fn diff_returns_only_unseen() {
        let models = parse_models(SAMPLE).unwrap();
        let seen = vec!["unsloth/Qwen3.8-GGUF".to_string()];
        let fresh = diff_new(&models, &seen);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].id, "unsloth/GLM-5.3-Flash-GGUF");
    }
}
