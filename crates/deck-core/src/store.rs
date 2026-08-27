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
