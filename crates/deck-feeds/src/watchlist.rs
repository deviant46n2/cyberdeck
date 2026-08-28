//! Watched-org state and new-model detection. The watchlist + `seen` tables
//! live in the shared cyberdeck SQLite index (deck-core's store).

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;

use crate::probe::fetch_org;

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

fn now() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default()
}

/// Poll every watched org; return only models not seen before, recording them.
pub fn check(conn: &rusqlite::Connection, limit: usize) -> Result<Vec<crate::probe::HfModel>> {
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
