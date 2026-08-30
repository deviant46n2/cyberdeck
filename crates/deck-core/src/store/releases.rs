use anyhow::Result;
use rusqlite::Connection;

// ------------------------------------------------------------ releases (O1 catalog)
//
// Release catalog for online intelligence. Each row is a stable `source:repo@rev`
// identity; re-fetching the same rev is a no-op (INSERT OR IGNORE). Payload is
// the source's raw JSON so scoring can evolve without migrations.

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Release {
    pub source: String,
    pub repo: String,
    pub rev: String,
    pub kind: String,
    pub title: String,
    pub url: String,
    pub published_at: String,
    pub payload_json: String,
    pub fetched_at: i64,
}

pub fn ensure_releases_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS releases (
            source TEXT NOT NULL,
            repo TEXT NOT NULL,
            rev TEXT NOT NULL,
            kind TEXT NOT NULL DEFAULT '',
            title TEXT NOT NULL DEFAULT '',
            url TEXT NOT NULL DEFAULT '',
            published_at TEXT NOT NULL DEFAULT '',
            payload_json TEXT NOT NULL DEFAULT '{}',
            fetched_at INTEGER NOT NULL,
            PRIMARY KEY (source, repo, rev)
        );
        CREATE INDEX IF NOT EXISTS idx_releases_fetched ON releases(fetched_at DESC);
        CREATE INDEX IF NOT EXISTS idx_releases_source ON releases(source);",
    )?;
    Ok(())
}

/// Insert a release; returns true if newly inserted, false if deduped.
pub fn insert_release(conn: &Connection, r: &Release) -> Result<bool> {
    ensure_releases_schema(conn)?;
    let n = conn.execute(
        "INSERT OR IGNORE INTO releases
            (source, repo, rev, kind, title, url, published_at, payload_json, fetched_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        rusqlite::params![
            r.source, r.repo, r.rev, r.kind, r.title, r.url, r.published_at, r.payload_json, r.fetched_at
        ],
    )?;
    Ok(n == 1)
}

pub fn list_releases(conn: &Connection, limit: usize) -> Result<Vec<Release>> {
    ensure_releases_schema(conn)?;
    let mut stmt = conn.prepare(
        "SELECT source, repo, rev, kind, title, url, published_at, payload_json, fetched_at
         FROM releases ORDER BY fetched_at DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |r| {
        Ok(Release {
            source: r.get(0)?,
            repo: r.get(1)?,
            rev: r.get(2)?,
            kind: r.get(3)?,
            title: r.get(4)?,
            url: r.get(5)?,
            published_at: r.get(6)?,
            payload_json: r.get(7)?,
            fetched_at: r.get(8)?,
        })
    })?;
    Ok(rows.flatten().collect())
}

pub fn count_releases(conn: &Connection) -> Result<i64> {
    ensure_releases_schema(conn)?;
    let mut stmt = conn.prepare("SELECT COUNT(*) FROM releases")?;
    Ok(stmt.query_row([], |r| r.get(0))?)
}
