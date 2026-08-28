//! HF marketplace lookups: full-text search, repo model-file listing with
//! HEAD-probed sizes, and remote size resolution.

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::probe::fetch_url;

fn simple_encode(s: &str) -> String {
    s.replace(' ', "%20").replace('#', "%23")
}

/// Size of a remote file via a HEAD-style probe (follows redirects, reads the
/// final `content-length` header). `None` when the server hides it or we're
/// offline.
fn head_size(url: &str) -> Option<u64> {
    let out = std::process::Command::new("curl")
        .args(["-sIL", "--max-time", "20", url])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.to_lowercase().starts_with("content-length:"))
        .last()
        .and_then(|l| l.split(':').nth(1)?.trim().parse().ok())
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub id: String,
    pub downloads: u64,
    pub likes: u64,
    pub pipeline_tag: Option<String>,
    pub tags: Vec<String>,
    pub created_at: String,
}

#[derive(Deserialize)]
struct SearchRow {
    id: String,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    likes: u64,
    #[serde(default)]
    pipeline_tag: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    created_at: Option<String>,
}

/// Parse the HF `/api/models?search=` JSON array (pure, testable).
pub fn parse_search(json: &str) -> Result<Vec<SearchHit>> {
    let rows: Vec<SearchRow> = serde_json::from_str(json)?;
    Ok(rows
        .into_iter()
        .map(|r| SearchHit {
            id: r.id,
            downloads: r.downloads,
            likes: r.likes,
            pipeline_tag: r.pipeline_tag,
            tags: r.tags,
            created_at: r.created_at.unwrap_or_default(),
        })
        .collect())
}

pub fn search_models(query: &str, limit: usize) -> Result<Vec<SearchHit>> {
    let url = format!(
        "https://huggingface.co/api/models?search={}&sort=downloads&direction=-1&limit={}",
        simple_encode(query),
        limit
    );
    let body = fetch_url(&url, 20)
        .with_context(|| format!("HF search for '{query}' failed (offline?)"))?;
    parse_search(&body)
}

#[derive(Debug, Clone)]
pub struct MarketFile {
    pub rfilename: String,
    pub size: Option<u64>,
}

#[derive(Deserialize)]
struct ModelDetail {
    #[serde(default)]
    siblings: Vec<Sibling>,
}

#[derive(Deserialize)]
struct Sibling {
    rfilename: String,
}

/// Parse a single-model detail JSON into its sibling filenames (pure).
pub fn parse_siblings(json: &str) -> Result<Vec<String>> {
    let d: ModelDetail = serde_json::from_str(json)?;
    Ok(d.siblings.into_iter().map(|s| s.rfilename).collect())
}

/// List downloadable model files in a repo, resolving each file's size via a
/// HEAD request (probed 8-at-a-time so a 30-shard repo costs ~4 rounds of
/// latency instead of 30 sequential ones). Covers GGUF *and* safetensors
/// repos; other assets are skipped.
pub fn model_files(repo_id: &str) -> Result<Vec<MarketFile>> {
    let url = format!("https://huggingface.co/api/models/{repo_id}");
    let body = fetch_url(&url, 20)
        .with_context(|| format!("HF model lookup for '{repo_id}' failed (offline?)"))?;
    let names = parse_siblings(&body)?;

    let targets: Vec<String> = names
        .into_iter()
        .filter(|n| {
            let lower = n.to_lowercase();
            lower.ends_with(".gguf") || lower.ends_with(".safetensors")
        })
        .collect();

    let urls: Vec<String> = targets
        .iter()
        .map(|n| format!("https://huggingface.co/{repo_id}/resolve/main/{n}"))
        .collect();

    const PROBE_CONCURRENCY: usize = 8;
    let mut sizes: Vec<Option<u64>> = vec![None; urls.len()];
    std::thread::scope(|s| {
        for (slots, chunk_urls) in sizes
            .chunks_mut(PROBE_CONCURRENCY)
            .zip(urls.chunks(PROBE_CONCURRENCY))
        {
            s.spawn(move || {
                for (slot, u) in slots.iter_mut().zip(chunk_urls.iter()) {
                    *slot = head_size(u);
                }
            });
        }
    });

    Ok(targets
        .into_iter()
        .zip(sizes)
        .map(|(rfilename, size)| MarketFile { rfilename, size })
        .collect())
}

/// Resolve a repo file's size via HEAD without downloading it.
/// `None` when the server hides Content-Length.
pub fn remote_file_size(repo_id: &str, rfilename: &str) -> Option<u64> {
    head_size(&format!(
        "https://huggingface.co/{repo_id}/resolve/main/{name}",
        repo_id = repo_id,
        name = simple_encode(rfilename)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = r#"[
      {"id":"unsloth/Qwen3.8-27B-GGUF","likes":3010,"downloads":7638591,"pipeline_tag":"text-generation","tags":["gguf","qwen3_5"],"createdAt":"2026-08-13T08:28:40Z"},
      {"id":"Qwen/Qwen3.8-27B-FP8","likes":705,"downloads":3797538,"tags":["transformers"]}
    ]"#;

    #[test]
    fn parses_search() {
        let hits = parse_search(SEARCH).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "unsloth/Qwen3.8-27B-GGUF");
        assert_eq!(hits[0].downloads, 7_638_591);
        assert_eq!(hits[0].pipeline_tag.as_deref(), Some("text-generation"));
        assert!(hits[1].pipeline_tag.is_none());
    }

    const DETAIL: &str = r#"{"id":"x/Y-GGUF","siblings":[
      {"rfilename":"README.md"},
      {"rfilename":"UD-IQ1_M/Y-UD-IQ1_M-00001-of-00003.gguf","size":123},
      {"rfilename":"UD-IQ1_M/Y-UD-IQ1_M-00002-of-00003.gguf"}
    ]}"#;

    #[test]
    fn parses_siblings_and_filters_gguf() {
        let names = parse_siblings(DETAIL).unwrap();
        let gguf: Vec<_> = names
            .iter()
            .filter(|n| n.to_lowercase().ends_with(".gguf"))
            .cloned()
            .collect();
        assert_eq!(gguf.len(), 2);
        assert!(gguf[0].contains("00001"));
    }
}
