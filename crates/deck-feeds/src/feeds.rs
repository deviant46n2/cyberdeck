//! Online intelligence feeds — O1 foundational slice.
//!
//! A `Source` is a small adapter that fetches `Vec<Release>` from one origin.
//! New origins are added as adapters, not branches. All transport shells out
//! to `curl` (consistent with the rest of deck-feeds).

use anyhow::{Context, Result};
use serde::Deserialize;

use deck_core::store::Release;

/// Common interface for a pollable origin.
pub trait Source {
    fn id(&self) -> &str;
    fn fetch(&self) -> Result<Vec<Release>>;
}

// ---------------------------------------------------------------- HF org feed
// Polls the HF `/api/models?author=` endpoint for each watched org and emits
// one `Release` per model. Identity is `hf:<id>@<sha|created_at>` — stable
// across re-polls, so the store dedups.

#[derive(Debug, Clone)]
pub struct HfSource {
    pub orgs: Vec<String>,
    pub limit: usize,
}

impl Default for HfSource {
    fn default() -> Self {
        Self {
            orgs: crate::watchlist::default_watchlist(),
            limit: 20,
        }
    }
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct HfRow {
    id: String,
    #[serde(default)]
    sha: Option<String>,
    #[serde(default)]
    createdAt: Option<String>,
    #[serde(default)]
    lastModified: Option<String>,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    likes: u64,
    #[serde(default)]
    pipeline_tag: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Source for HfSource {
    fn id(&self) -> &str {
        "hf"
    }
    fn fetch(&self) -> Result<Vec<Release>> {
        let mut out = Vec::new();
        for org in &self.orgs {
            let url = format!(
                "https://huggingface.co/api/models?author={}&sort=lastModified&direction=-1&limit={}",
                crate::probe::simple_encode(org),
                self.limit
            );
            let body = crate::probe::fetch_url(&url, 20)
                .with_context(|| format!("HF feed for '{org}' failed"))?;
            let rows: Vec<HfRow> = serde_json::from_str(&body).with_context(|| "HF feed JSON parse")?;
            for r in rows {
                let rev = r
                    .sha
                    .clone()
                    .or_else(|| r.lastModified.clone())
                    .or_else(|| r.createdAt.clone())
                    .unwrap_or_else(|| r.id.clone());
                let published = r.createdAt.clone().unwrap_or_default();
                let payload = serde_json::to_string(&serde_json::json!({
                    "id": r.id, "sha": r.sha, "createdAt": r.createdAt,
                    "lastModified": r.lastModified, "downloads": r.downloads,
                    "likes": r.likes, "pipeline_tag": r.pipeline_tag, "tags": r.tags
                }))
                .unwrap_or_else(|_| "{}".into());
                out.push(Release {
                    source: "hf".into(),
                    repo: r.id.clone(),
                    rev,
                    kind: "model".into(),
                    title: r.id.clone(),
                    url: format!("https://huggingface.co/{}", r.id),
                    published_at: published,
                    payload_json: payload,
                    fetched_at: now_secs(),
                });
            }
        }
        Ok(out)
    }
}

// -------------------------------------------------------------- GitHub releases
// Polls `api.github.com/repos/{repo}/releases` per configured repo. Rev =
// `tag_name` (stable). Requires no auth for public repos; if `GITHUB_TOKEN`
// is set we send it to raise the rate limit.

#[derive(Debug, Clone)]
pub struct GithubSource {
    pub repos: Vec<String>, // "org/repo"
    pub per_repo: usize,
}

impl Default for GithubSource {
    fn default() -> Self {
        Self {
            repos: vec![
                "ggml-org/llama.cpp".into(),
                "ollama/ollama".into(),
                // FreeToken upstream varies; keep as optional — failures are skipped per-repo
                "FuelLabs/FreeToken".into(),
            ],
            per_repo: 10,
        }
    }
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    body: Option<String>,
}

fn github_token() -> Option<String> {
    std::env::var("GITHUB_TOKEN")
        .or_else(|_| std::env::var("GH_TOKEN"))
        .ok()
        .filter(|s| !s.trim().is_empty())
}

fn fetch_github_releases(repo: &str, limit: usize) -> Result<Vec<GhRelease>> {
    let url = format!("https://api.github.com/repos/{repo}/releases?per_page={limit}");
    let mut cmd = std::process::Command::new("curl");
    cmd.args(["-sSL", "--fail", "--show-error", "--max-time", "20", "-H", "Accept: application/vnd.github+json", "-H", "X-GitHub-Api-Version: 2022-11-28"]);
    if let Some(tok) = github_token() {
        cmd.args(["-H", &format!("Authorization: Bearer {tok}")]);
    }
    // GitHub requires a User-Agent
    cmd.args(["-H", "User-Agent: cyberdeck-feeds/0.1", &url]);
    let out = cmd.output().context("failed to spawn curl for GitHub")?;
    if !out.status.success() {
        anyhow::bail!(
            "GitHub releases for '{repo}' failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let body = String::from_utf8_lossy(&out.stdout).into_owned();
    Ok(serde_json::from_str::<Vec<GhRelease>>(&body)?)
}

impl Source for GithubSource {
    fn id(&self) -> &str {
        "github"
    }
    fn fetch(&self) -> Result<Vec<Release>> {
        let mut out = Vec::new();
        for repo in &self.repos {
            let releases = match fetch_github_releases(repo, self.per_repo) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[feeds] github {repo} failed: {e:#}");
                    continue;
                }
            };
            for r in releases {
                let payload = serde_json::to_string(&serde_json::json!({
                    "repo": repo, "tag_name": r.tag_name, "name": r.name,
                    "html_url": r.html_url, "published_at": r.published_at, "body": r.body
                }))
                .unwrap_or_else(|_| "{}".into());
                out.push(Release {
                    source: "github".into(),
                    repo: repo.clone(),
                    rev: r.tag_name.clone(),
                    kind: "release".into(),
                    title: r.name.clone().unwrap_or_else(|| r.tag_name.clone()),
                    url: r.html_url.clone().unwrap_or_else(|| format!("https://github.com/{repo}/releases/tag/{}", r.tag_name)),
                    published_at: r.published_at.clone().unwrap_or_default(),
                    payload_json: payload,
                    fetched_at: now_secs(),
                });
            }
        }
        Ok(out)
    }
}

// -------------------------------------------------------------- registry helper
/// Poll the requested sources and upsert into the releases catalog. Returns
/// `(fetched, inserted)` counts. `sources` filters by adapter id (`hf`,
/// `github`); empty means all.
pub fn poll(sources: &[String]) -> Result<(usize, usize)> {
    let want = |id: &str| sources.is_empty() || sources.iter().any(|s| s == id);
    let mut fetched: Vec<Release> = Vec::new();

    if want("hf") {
        // Use actual watchlist orgs if present, else defaults.
        let orgs = crate::watchlist::open()
            .and_then(|c| crate::watchlist::list_watchlist(&c))
            .unwrap_or_else(|_| crate::watchlist::default_watchlist());
        let src = HfSource { orgs, limit: 20 };
        match src.fetch() {
            Ok(mut v) => fetched.append(&mut v),
            Err(e) => eprintln!("[feeds] hf fetch failed: {e:#}"),
        }
    }
    if want("github") {
        let src = GithubSource::default();
        match src.fetch() {
            Ok(mut v) => fetched.append(&mut v),
            Err(e) => eprintln!("[feeds] github fetch failed: {e:#}"),
        }
    }

    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    let total = fetched.len();
    let mut inserted = 0usize;
    for r in &fetched {
        if deck_core::store::insert_release(&conn, r)? {
            inserted += 1;
        }
    }
    Ok((total, inserted))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hf_row_parses_sha_and_fallbacks() {
        let json = r#"[{"id":"unsloth/Qwen-GGUF","sha":"abc123","createdAt":"2026-08-13T00:00:00Z","downloads":10,"likes":1}]"#;
        let rows: Vec<HfRow> = serde_json::from_str(json).unwrap();
        assert_eq!(rows[0].sha.as_deref(), Some("abc123"));
    }

    #[test]
    fn github_release_parses_tag() {
        let json = r#"[{"tag_name":"b1234","name":"b1234","html_url":"https://github.com/ggml-org/llama.cpp/releases/tag/b1234","published_at":"2026-08-20T00:00:00Z","body":"notes"}]"#;
        let rows: Vec<GhRelease> = serde_json::from_str(json).unwrap();
        assert_eq!(rows[0].tag_name, "b1234");
    }
}
