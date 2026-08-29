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

/// Directory where downloaded models land (`~/models`).
pub fn models_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("models")
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
            n_head: None,
            n_head_kv: None,
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
    let mut stmt =
        conn.prepare("SELECT path, footprint FROM models WHERE arch = ?1 ORDER BY footprint ASC")?;
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
    let engine = profile.engine.store_id();
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

// ------------------------------------------------------------ residents (PORT MAP)

/// What is bound to a single engine port slot. `resident = true` means the
/// profile is meant to run *alongside* other engine slots (multi-model
/// residency); `false` is a plain single swap that still records reality so
/// `deck engines status` can show what's currently bound to each slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resident {
    pub engine_id: String,
    pub profile: String,
    pub resident: bool,
}

/// Keyed by engine id (`llamacpp` / `freetoken` / `ollama`) because each
/// engine owns exactly one fixed PORT MAP slot with one unit. Presence of a row
/// = a profile is bound to that slot; the `resident` flag marks it as a
/// coexisting resident rather than a plain swap.
pub fn ensure_resident_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS residents (
            engine_id TEXT PRIMARY KEY,
            profile_name TEXT NOT NULL,
            resident INTEGER NOT NULL DEFAULT 0
        )",
    )?;
    Ok(())
}

/// Bind `profile` to `engine_id`'s slot. Keeps the existing `resident` flag if
/// unspecified so re-applying a resident profile doesn't silently demote it;
/// pass `resident` to force.
pub fn set_resident(
    conn: &Connection,
    engine_id: &str,
    profile: &str,
    resident: Option<bool>,
) -> Result<()> {
    ensure_resident_schema(conn)?;
    if let Some(r) = resident {
        conn.execute(
            "INSERT INTO residents (engine_id, profile_name, resident) VALUES (?1,?2,?3)
             ON CONFLICT(engine_id) DO UPDATE SET profile_name = excluded.profile_name,
                                                  resident = excluded.resident",
            rusqlite::params![engine_id, profile, r as i64],
        )?;
    } else {
        conn.execute(
            "INSERT INTO residents (engine_id, profile_name, resident)
             VALUES (?1,?2, COALESCE((SELECT resident FROM residents WHERE engine_id = ?1), 0))
             ON CONFLICT(engine_id) DO UPDATE SET profile_name = excluded.profile_name",
            rusqlite::params![engine_id, profile],
        )?;
    }
    Ok(())
}

pub fn get_resident(conn: &Connection, engine_id: &str) -> Result<Option<Resident>> {
    ensure_resident_schema(conn)?;
    let mut stmt =
        conn.prepare("SELECT engine_id, profile_name, resident FROM residents WHERE engine_id = ?1")?;
    let mut rows = stmt.query_map([engine_id], |r| {
        Ok(Resident {
            engine_id: r.get(0)?,
            profile: r.get(1)?,
            resident: r.get::<_, i64>(2)? != 0,
        })
    })?;
    Ok(rows.next().transpose()?)
}

pub fn list_residents(conn: &Connection) -> Result<Vec<Resident>> {
    ensure_resident_schema(conn)?;
    let mut stmt = conn.prepare("SELECT engine_id, profile_name, resident FROM residents")?;
    let rows = stmt.query_map([], |r| {
        Ok(Resident {
            engine_id: r.get(0)?,
            profile: r.get(1)?,
            resident: r.get::<_, i64>(2)? != 0,
        })
    })?;
    Ok(rows.flatten().collect())
}

pub fn clear_resident(conn: &Connection, engine_id: &str) -> Result<()> {
    ensure_resident_schema(conn)?;
    conn.execute("DELETE FROM residents WHERE engine_id = ?1", [engine_id])?;
    Ok(())
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

/// One recorded trial in the scientific model × quant × engine matrix.
/// Keeps the RAW ingredients (token counts, wall ms) so downstream math can
/// recompute derived metrics; `tok_s_kind` says whether the speed is the
/// engine's native timing or a wall-based estimate.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MatrixRow {
    pub engine: String,
    pub model: String,
    pub ctx: u32,
    pub task: String,
    pub run: u32,
    /// RUNNING when the sample was taken, else the boot verdict that ended the
    /// cell (OOM / CRASH / TIMEOUT / ERROR).
    pub verdict: String,
    pub summary: String,
    pub gen_tokens: Option<u64>,
    pub prompt_tokens: Option<u64>,
    pub tok_s: Option<f64>,
    pub tok_s_kind: String,
    pub wall_ms: u64,
    pub output: String,
    /// Unix epoch seconds.
    pub at: i64,
}

pub fn ensure_matrix_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS matrix_runs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            engine TEXT NOT NULL,
            model TEXT NOT NULL,
            ctx INTEGER NOT NULL,
            task TEXT NOT NULL,
            run INTEGER NOT NULL,
            verdict TEXT NOT NULL,
            summary TEXT NOT NULL,
            gen_tokens INTEGER,
            prompt_tokens INTEGER,
            tok_s REAL,
            tok_s_kind TEXT,
            wall_ms INTEGER,
            output TEXT,
            at INTEGER NOT NULL
        )",
    )?;
    Ok(())
}

pub fn insert_matrix_run(conn: &Connection, row: &MatrixRow) -> Result<i64> {
    conn.execute(
        "INSERT INTO matrix_runs
            (engine, model, ctx, task, run, verdict, summary, gen_tokens,
             prompt_tokens, tok_s, tok_s_kind, wall_ms, output, at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
        rusqlite::params![
            row.engine,
            row.model,
            row.ctx,
            row.task,
            row.run,
            row.verdict,
            row.summary,
            row.gen_tokens,
            row.prompt_tokens,
            row.tok_s,
            row.tok_s_kind,
            row.wall_ms,
            row.output,
            row.at,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

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

// ------------------------------------------------------------ engine bins
//
// Optional per-engine executable paths, keyed by `Engine::store_id` (e.g.
// "llamacpp"). A configured bin is used by bringup/test/matrix when a profile's
// resolved bin does not exist on disk — the one machine-specific fact a profile
// (or the CLI) should not be forced to carry. Unset means "use the engine's
// default resolution". Schema is created on first use so older databases pick it
// up without migration.

pub fn ensure_engine_bin_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS engine_bin (
            engine_id TEXT PRIMARY KEY,
            bin TEXT NOT NULL
        )",
    )?;
    Ok(())
}

pub fn get_engine_bin(conn: &Connection, store_id: &str) -> Result<Option<String>> {
    ensure_engine_bin_schema(conn)?;
    let mut stmt = conn.prepare("SELECT bin FROM engine_bin WHERE engine_id = ?1")?;
    let mut rows = stmt.query_map([store_id], |r| r.get::<_, String>(0))?;
    match rows.next() {
        Some(Ok(b)) => Ok(Some(b)),
        Some(Err(e)) => Err(e.into()),
        None => Ok(None),
    }
}

pub fn set_engine_bin(conn: &Connection, store_id: &str, bin: &str) -> Result<()> {
    ensure_engine_bin_schema(conn)?;
    conn.execute(
        "INSERT INTO engine_bin (engine_id, bin) VALUES (?1,?2)
         ON CONFLICT(engine_id) DO UPDATE SET bin = excluded.bin",
        rusqlite::params![store_id, bin],
    )?;
    Ok(())
}

pub fn clear_engine_bin(conn: &Connection, store_id: &str) -> Result<()> {
    ensure_engine_bin_schema(conn)?;
    conn.execute("DELETE FROM engine_bin WHERE engine_id = ?1", [store_id])?;
    Ok(())
}

/// Confirm a bin value will actually run: bare command names resolve through
/// PATH, so only an explicit path that is missing on disk needs substituting.
fn bin_looks_resolvable(bin: &std::path::Path) -> bool {
    let s = bin.to_string_lossy();
    !s.contains(['/', '\\']) || bin.is_file()
}

/// Substitute the configured per-engine executable path into a profile when
/// its resolved bin does not exist on disk. Leaves explicit, existing binaries
/// (and bare PATH-resolvable names like `ollama`) untouched. This is the one
/// piece of machine-specific config a profile should not carry.
pub fn resolve_engine_bin(
    conn: &Connection,
    mut p: crate::profile::Profile,
) -> Result<crate::profile::Profile> {
    if !bin_looks_resolvable(&p.bin)
        && let Some(b) = get_engine_bin(conn, p.engine.store_id())?
        && b != p.bin.to_string_lossy()
    {
        p.bin = std::path::PathBuf::from(b);
    }
    Ok(p)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{Engine, Profile};

    fn fresh() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        ensure_engine_bin_schema(&conn).expect("schema");
        conn
    }

    #[test]
    fn engine_bin_roundtrip() {
        let conn = fresh();
        assert_eq!(get_engine_bin(&conn, "llamacpp").unwrap(), None);
        set_engine_bin(&conn, "llamacpp", "/opt/llama/llama-server").unwrap();
        assert_eq!(
            get_engine_bin(&conn, "llamacpp").unwrap().as_deref(),
            Some("/opt/llama/llama-server")
        );
        clear_engine_bin(&conn, "llamacpp").unwrap();
        assert_eq!(get_engine_bin(&conn, "llamacpp").unwrap(), None);
    }

    #[test]
    fn residents_roundtrip_keeps_flag() {
        let conn = Connection::open_in_memory().expect("in-memory db");
        ensure_resident_schema(&conn).unwrap();

        assert_eq!(get_resident(&conn, "llamacpp").unwrap(), None);

        // Bind as a resident.
        set_resident(&conn, "llamacpp", "qwen", Some(true)).unwrap();
        let r = get_resident(&conn, "llamacpp").unwrap().unwrap();
        assert_eq!(r.engine_id, "llamacpp");
        assert_eq!(r.profile, "qwen");
        assert!(r.resident);

        // Re-applying without a flag preserves the resident bit (no demotion).
        set_resident(&conn, "llamacpp", "qwen2", None).unwrap();
        let r = get_resident(&conn, "llamacpp").unwrap().unwrap();
        assert_eq!(r.profile, "qwen2");
        assert!(r.resident, "flag preserved on re-bind");

        // Explicit non-resident overrides.
        set_resident(&conn, "llamacpp", "qwen2", Some(false)).unwrap();
        let r = get_resident(&conn, "llamacpp").unwrap().unwrap();
        assert!(!r.resident);

        // Two slots coexist independently.
        set_resident(&conn, "ollama", "qwen-ollama", Some(true)).unwrap();
        let all = list_residents(&conn).unwrap();
        assert_eq!(all.len(), 2);

        clear_resident(&conn, "llamacpp").unwrap();
        assert_eq!(get_resident(&conn, "llamacpp").unwrap(), None);
        assert!(list_residents(&conn).unwrap().len() == 1);
    }

    #[test]
    fn resolve_substitutes_missing_absolute_bin() {
        let conn = fresh();
        set_engine_bin(&conn, "llamacpp", "/opt/llama/llama-server").unwrap();
        let p = Profile {
            engine: Engine::LlamaCpp,
            bin: "/usr/bin/llama-server".into(),
            ..Default::default()
        };
        let resolved = resolve_engine_bin(&conn, p).unwrap();
        assert_eq!(resolved.bin, PathBuf::from("/opt/llama/llama-server"));
    }

    #[test]
    fn resolve_leaves_bare_names_and_existing_files() {
        let conn = fresh();
        set_engine_bin(&conn, "ollama", "/opt/ollama/bin/ollama").unwrap();
        // Bare PATH-resolvable name stays put even when a config exists.
        let p = Profile {
            engine: Engine::Ollama,
            bin: "ollama".into(),
            ..Default::default()
        };
        assert_eq!(
            resolve_engine_bin(&conn, p).unwrap().bin,
            PathBuf::from("ollama")
        );

        // An existing absolute path wins over configured config.
        let temp = std::env::temp_dir().join("cyberdeck-resolve-test.bin");
        std::fs::write(&temp, b"#!").unwrap();
        set_engine_bin(&conn, "llamacpp", "/opt/llama/llama-server").unwrap();
        let p = Profile {
            engine: Engine::LlamaCpp,
            bin: temp.clone(),
            ..Default::default()
        };
        assert_eq!(resolve_engine_bin(&conn, p).unwrap().bin, temp);
        let _ = std::fs::remove_file(&temp);
    }
}
