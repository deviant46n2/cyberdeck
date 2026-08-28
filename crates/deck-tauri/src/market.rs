//! Market discovery + SIGNALS watchlist: HF probes and the feeds sqlite store.

use serde::Serialize;

#[derive(Serialize)]
pub struct SignalRow {
    pub id: String,
    pub author: String,
    pub created_at: String,
    pub downloads: u64,
    pub likes: u64,
    pub pipeline_tag: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Serialize)]
pub struct MarketHit {
    pub id: String,
    pub downloads: u64,
    pub likes: u64,
    pub pipeline_tag: Option<String>,
    pub tags: Vec<String>,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct MarketFileRow {
    pub rfilename: String,
    pub size: Option<u64>,
}

/// Run a SIGNALS check: poll watched orgs and return only new models.
pub fn signals_check(limit: usize) -> anyhow::Result<Vec<SignalRow>> {
    let conn = deck_feeds::open()?;
    deck_feeds::ensure_seeds(&conn)?;
    let news = deck_feeds::check(&conn, limit)?;
    Ok(news
        .into_iter()
        .map(|m| SignalRow {
            id: m.id,
            author: m.author,
            created_at: m.created_at,
            downloads: m.downloads,
            likes: m.likes,
            pipeline_tag: m.pipeline_tag,
            tags: m.tags,
        })
        .collect())
}

pub fn watchlist() -> anyhow::Result<Vec<String>> {
    let conn = deck_feeds::open()?;
    deck_feeds::ensure_seeds(&conn)?;
    deck_feeds::list_watchlist(&conn)
}

pub fn watch_add(org: &str) -> anyhow::Result<()> {
    let conn = deck_feeds::open()?;
    deck_feeds::add_org(&conn, org)
}

pub fn watch_remove(org: &str) -> anyhow::Result<()> {
    let conn = deck_feeds::open()?;
    deck_feeds::remove_org(&conn, org)
}

/// Search HuggingFace models by free-text query.
pub fn market_search(query: &str, limit: usize) -> anyhow::Result<Vec<MarketHit>> {
    Ok(deck_feeds::search_models(query, limit)?
        .into_iter()
        .map(|h| MarketHit {
            id: h.id,
            downloads: h.downloads,
            likes: h.likes,
            pipeline_tag: h.pipeline_tag,
            tags: h.tags,
            created_at: h.created_at,
        })
        .collect())
}

/// Fetch recent models authored by `org` from HuggingFace.
pub fn browse_org(org: &str, limit: usize) -> anyhow::Result<Vec<MarketHit>> {
    Ok(deck_feeds::fetch_org(org, limit)?
        .into_iter()
        .map(|h| MarketHit {
            id: h.id,
            downloads: h.downloads,
            likes: h.likes,
            pipeline_tag: h.pipeline_tag,
            tags: h.tags,
            created_at: h.created_at,
        })
        .collect())
}

/// List downloadable model files in a repo (GGUF + safetensors), resolving
/// each file's size via a HEAD request.
pub fn market_files(repo_id: &str) -> anyhow::Result<Vec<MarketFileRow>> {
    Ok(deck_feeds::model_files(repo_id)?
        .into_iter()
        .map(|f| MarketFileRow {
            rfilename: f.rfilename,
            size: f.size,
        })
        .collect())
}
