//! deck-feeds: HuggingFace watchlist poller and new-release detection.
//!
//! Phase 4 (SIGNALS). Watches a set of orgs/users, fetches their most recent
//! models from the HF API, and reports only what hasn't been seen before —
//! filtered notifications, never a firehose. State (watchlist + seen ids) lives
//! in the shared cyberdeck SQLite index.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::Deserialize;

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

fn now() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default()
}

fn simple_encode(s: &str) -> String {
    s.replace(' ', "%20").replace('#', "%23")
}

/// Fetch the most recent `limit` models authored by `org` from the HF API.
pub fn fetch_org(org: &str, limit: usize) -> Result<Vec<HfModel>> {
    let url = format!(
        "https://huggingface.co/api/models?author={}&sort=createdAt&direction=-1&limit={}",
        simple_encode(org),
        limit
    );
    let agent = ureq::config::Config::builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .new_agent();
    let resp = agent
        .get(&url)
        .call()
        .with_context(|| format!("HF API request for '{org}' failed (offline?)"))?;
    let body = resp.into_body().read_to_string()?;
    parse_models(&body)
}

pub fn open() -> Result<rusqlite::Connection> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS watchlist (org TEXT PRIMARY KEY);
         CREATE TABLE IF NOT EXISTS seen (id TEXT PRIMARY KEY, org TEXT, seen_at TEXT);",
    )?;
    Ok(conn)
}

/// Default orgs to watch, per the cyberdeck lore.
pub fn default_watchlist() -> Vec<String> {
    vec![
        "unsloth".into(),
        "bartowski".into(),
        "ggml-org".into(),
        "nvidia".into(),
    ]
}

pub fn ensure_seeds(conn: &rusqlite::Connection) -> Result<()> {
    for org in default_watchlist() {
        conn.execute("INSERT OR IGNORE INTO watchlist (org) VALUES (?1)", [org])?;
    }
    Ok(())
}

pub fn list_watchlist(conn: &rusqlite::Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT org FROM watchlist ORDER BY org")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.flatten().collect())
}

pub fn add_org(conn: &rusqlite::Connection, org: &str) -> Result<()> {
    conn.execute("INSERT OR IGNORE INTO watchlist (org) VALUES (?1)", [org])?;
    Ok(())
}

pub fn remove_org(conn: &rusqlite::Connection, org: &str) -> Result<()> {
    conn.execute("DELETE FROM watchlist WHERE org = ?1", [org])?;
    Ok(())
}

/// Poll every watched org; return only models not seen before, recording them.
pub fn check(conn: &rusqlite::Connection, limit: usize) -> Result<Vec<HfModel>> {
    let orgs = list_watchlist(conn)?;
    let mut news = Vec::new();
    for org in &orgs {
        let models = fetch_org(org, limit)?;
        for m in models {
            let seen = conn
                .query_row("SELECT 1 FROM seen WHERE id = ?1", [&m.id], |_| Ok(()))
                .is_ok();
            if !seen {
                conn.execute(
                    "INSERT OR IGNORE INTO seen (id, org, seen_at) VALUES (?1, ?2, ?3)",
                    [&m.id, org, &now()],
                )?;
                news.push(m);
            }
        }
    }
    Ok(news)
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
