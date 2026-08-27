//! SQLite inventory index. Single table; models keyed by path.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::Connection;

use crate::dedup::DupGroup;
use crate::model::{ModelFormat, ModelMeta};

pub fn default_db_path() -> PathBuf {
    let dir = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|h| h.join(".local/share"))
                .unwrap_or_else(|| PathBuf::from("."))
        });
    dir.join("cyberdeck/cyberdeck.db")
}

pub fn open(path: &std::path::Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS models (
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL UNIQUE,
            format TEXT NOT NULL,
            name TEXT,
            arch TEXT,
            quant TEXT,
            params INTEGER,
            n_layers INTEGER,
            n_embd INTEGER,
            ctx_train INTEGER,
            vocab INTEGER,
            weight_size INTEGER,
            footprint INTEGER,
            scanned_at INTEGER
        );",
    )?;
    Ok(conn)
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn upsert_many(conn: &mut Connection, models: &[ModelMeta]) -> Result<usize> {
    let tx = conn.transaction()?;
    let stamp = now();
    for m in models {
        tx.execute(
            "INSERT INTO models
                (path, format, name, arch, quant, params, n_layers, n_embd,
                 ctx_train, vocab, weight_size, footprint, scanned_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
             ON CONFLICT(path) DO UPDATE SET
                format=?2, name=?3, arch=?4, quant=?5, params=?6, n_layers=?7,
                n_embd=?8, ctx_train=?9, vocab=?10, weight_size=?11,
                footprint=?12, scanned_at=?13",
            rusqlite::params![
                m.path.display().to_string(),
                match m.format {
                    ModelFormat::Gguf => "gguf",
                    ModelFormat::SafetensorsDir => "safetensors-dir",
                },
                m.name,
                m.arch,
                m.quant,
                m.params,
                m.n_layers,
                m.n_embd,
                m.ctx_train,
                m.vocab,
                m.weight_size as i64,
                m.footprint as i64,
                stamp,
            ],
        )?;
    }
    tx.commit()?;
    Ok(models.len())
}

pub fn list(conn: &Connection) -> Result<Vec<ModelMeta>> {
    let mut stmt = conn.prepare(
        "SELECT path, format, name, arch, quant, params, n_layers, n_embd,
                ctx_train, vocab, weight_size, footprint FROM models",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<i64>>(5)?,
            r.get::<_, Option<i64>>(6)?,
            r.get::<_, Option<i64>>(7)?,
            r.get::<_, Option<i64>>(8)?,
            r.get::<_, Option<i64>>(9)?,
            r.get::<_, i64>(10)?,
            r.get::<_, i64>(11)?,
        ))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (path, format, name, arch, quant, params, nl, ne, ctx, vocab, ws, fp) = row?;
        out.push(ModelMeta {
            path: PathBuf::from(path),
            format: if format == "gguf" {
                ModelFormat::Gguf
            } else {
                ModelFormat::SafetensorsDir
            },
            name: name.unwrap_or_default(),
            arch,
            quant,
            params: params.map(|v| v as u64),
            n_layers: nl.map(|v| v as u64),
            n_embd: ne.map(|v| v as u64),
            ctx_train: ctx.map(|v| v as u64),
            vocab: vocab.map(|v| v as u64),
            weight_size: ws as u64,
            footprint: fp as u64,
        });
    }
    Ok(out)
}

pub fn duplicates(conn: &Connection) -> Result<Vec<DupGroup>> {
    let models = list(conn)?;
    Ok(crate::dedup::find_duplicates(&models))
}

/// Remove a single model from the index. If `delete_file` is true the file is
/// unlinked from disk (for local/GGUF files only — safe to skip for ollama/hub
/// paths that the user should manage externally).
pub fn delete_model(conn: &Connection, path: &str, delete_file: bool) -> Result<usize> {
    if delete_file {
        let p = std::path::Path::new(path);
        if p.is_file() {
            let _ = std::fs::remove_file(p);
        }
    }
    let n = conn.execute("DELETE FROM models WHERE path = ?1", [path])?;
    Ok(n)
}

/// Delete all duplicate copies in a group except the cheapest one (the one with
/// the smallest footprint). Returns the number of rows removed.
pub fn dedup_delete(conn: &Connection, identity: &str, delete_file: bool) -> Result<usize> {
    let mut stmt = conn.prepare("SELECT path, footprint FROM models WHERE arch = ?1 ORDER BY footprint ASC")?;
    let rows: Vec<(String, i64)> = stmt
        .query_map([identity], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;

    if rows.len() < 2 {
        return Ok(0);
    }

    // Keep the first (cheapest), delete the rest.
    let mut removed = 0;
    for (path, _) in rows.into_iter().skip(1) {
        if delete_file {
            let p = std::path::Path::new(&path);
            if p.is_file() {
                let _ = std::fs::remove_file(p);
            }
        }
        conn.execute("DELETE FROM models WHERE path = ?1", [&path])?;
        removed += 1;
    }
    Ok(removed)
}

// ---------------------------------------------------------------- profiles

pub fn ensure_profile_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS profiles (
            name TEXT PRIMARY KEY,
            engine TEXT NOT NULL,
            body TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )?;
    Ok(())
}

pub fn upsert_profile(conn: &Connection, profile: &crate::profile::Profile) -> Result<()> {
    let body = serde_json::to_string(profile)?;
    let engine = match profile.engine {
        crate::profile::Engine::LlamaCpp => "llamacpp",
        crate::profile::Engine::FreeToken => "freetoken",
    };
    conn.execute(
        "INSERT INTO profiles (name, engine, body) VALUES (?1,?2,?3)
         ON CONFLICT(name) DO UPDATE SET engine=?2, body=?3",
        rusqlite::params![profile.name, engine, body],
    )?;
    Ok(())
}

pub fn list_profiles(conn: &Connection) -> Result<Vec<crate::profile::Profile>> {
    let mut stmt = conn.prepare("SELECT body FROM profiles ORDER BY name")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for body in rows.flatten() {
        if let Ok(p) = serde_json::from_str::<crate::profile::Profile>(&body) {
            out.push(p);
        }
    }
    Ok(out)
}

pub fn get_profile(conn: &Connection, name: &str) -> Result<Option<crate::profile::Profile>> {
    let mut stmt = conn.prepare("SELECT body FROM profiles WHERE name = ?1")?;
    let mut rows = stmt.query_map([name], |r| r.get::<_, String>(0))?;
    if let Some(body) = rows.next().transpose()? {
        return Ok(Some(serde_json::from_str::<crate::profile::Profile>(
            &body,
        )?));
    }
    Ok(None)
}

pub fn delete_profile(conn: &Connection, name: &str) -> Result<()> {
    conn.execute("DELETE FROM profiles WHERE name = ?1", [name])?;
    Ok(())
}

pub fn set_active(conn: &Connection, name: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('active_profile', ?1)
         ON CONFLICT(key) DO UPDATE SET value=?1",
        [name],
    )?;
    Ok(())
}

pub fn active_profile(conn: &Connection) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM meta WHERE key = 'active_profile'")?;
    let mut rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.next().transpose()?)
}

/// Removes rows whose path is not in `keep`. Keeps the index honest when a
/// model is deleted or moved between scans.
pub fn prune(conn: &Connection, keep: &[String]) -> Result<usize> {
    let existing: Vec<String> = {
        let mut stmt = conn.prepare("SELECT path FROM models")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.flatten().collect()
    };
    let keep_set: std::collections::HashSet<&str> = keep.iter().map(|s| s.as_str()).collect();
    let mut removed = 0;
    for path in existing {
        if !keep_set.contains(path.as_str()) {
            conn.execute("DELETE FROM models WHERE path = ?1", [path.clone()])?;
            removed += 1;
        }
    }
    Ok(removed)
}

// ---------------------------------------------------------------- benchmark

/// A single live throughput measurement pulled from a running engine's
/// Prometheus `/metrics` endpoint.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BenchRow {
    pub id: i64,
    pub engine: String,
    pub host: String,
    pub port: u16,
    pub model: String,
    pub ctx: u32,
    /// Measured tokens/second (generation).
    pub tps: f64,
    /// Unix epoch seconds.
    pub at: i64,
}

pub fn ensure_bench_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS bench (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            engine TEXT NOT NULL,
            host TEXT NOT NULL,
            port INTEGER NOT NULL,
            model TEXT NOT NULL,
            ctx INTEGER NOT NULL,
            tps REAL NOT NULL,
            at INTEGER NOT NULL
        )",
    )?;
    Ok(())
}

pub fn insert_bench(conn: &Connection, row: &BenchRow) -> Result<i64> {
    conn.execute(
        "INSERT INTO bench (engine, host, port, model, ctx, tps, at)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        rusqlite::params![
            row.engine, row.host, row.port, row.model, row.ctx, row.tps, row.at
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn recent_bench(conn: &Connection, n: usize) -> Result<Vec<BenchRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, engine, host, port, model, ctx, tps, at
         FROM bench ORDER BY at DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([n as i64], |r| {
        Ok(BenchRow {
            id: r.get(0)?,
            engine: r.get(1)?,
            host: r.get(2)?,
            port: r.get(3)?,
            model: r.get(4)?,
            ctx: r.get(5)?,
            tps: r.get(6)?,
            at: r.get(7)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}
