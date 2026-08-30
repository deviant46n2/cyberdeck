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

/// The current schema version. Every new additive table/column bumps this so
/// tooling (CLI `deck status`, UI HUD) can tell whether the DB was migrated by
/// a newer cyberdeck. We never drop/reload; `ensure_*` helpers + `ensure_column`
/// stay idempotent and ADD-only, so an older binary opening a newer DB still
/// works (unknown tables/columns are simply unused) and a newer binary is the
/// only one that advances the version.
pub const SCHEMA_VERSION: i64 = 2;

/// Non-destructive forward migration. `ensure_schema_version` stamps the
/// version at `SCHEMA_VERSION`; no steps are wired yet because every current
/// migration is an idempotent `ensure_*`/`ensure_column` that already runs on
/// first touch. The moment we need a cross-cutting transform (Phase 6+), a
/// match arm on `from_version` goes here.
fn migrate(_conn: &Connection, _from_version: i64) -> Result<()> {
    Ok(())
}

pub fn open(path: &std::path::Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    // Guarantee the `meta` key-value table exists regardless of which schema
    // helper has run — it is the home of `schema_version` (and `active_profile`).
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )?;
    // Stamp the schema version if this DB predates it (first open of any
    // existing cyberdeck DB writes SCHEMA_VERSION with no destructive step).
    let stored: Option<String> = conn
        .prepare("SELECT value FROM meta WHERE key = 'schema_version'")
        .ok()
        .and_then(|mut stmt| {
            stmt.query_row([], |r| r.get::<_, String>(0)).ok()
        });
    if stored.is_none() {
        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
            [SCHEMA_VERSION.to_string()],
        )?;
    } else if let Ok(ver) = stored.unwrap().parse::<i64>() {
        if ver < SCHEMA_VERSION {
            // Older DB opened by a newer binary → run forward migrations.
            migrate(&conn, ver)?;
            conn.execute(
                "UPDATE meta SET value=?1 WHERE key='schema_version'",
                [SCHEMA_VERSION.to_string()],
            )?;
        }
    }
    // Existing schema-creation logic continues below this point.
    // Existing schema-creation logic continues below this point.
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

/// Read the stamped schema version (defaults to `SCHEMA_VERSION` for a fresh
/// DB that simply hasn't been opened yet). Used by CLI/UI to surface
/// staleness. Returns `None` only if the `meta` read itself fails.
pub fn schema_version(conn: &Connection) -> Option<i64> {
    let mut stmt = conn
        .prepare("SELECT value FROM meta WHERE key = 'schema_version'")
        .ok()?;
    stmt.query_row([], |r| r.get::<_, String>(0))
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
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
        } else if p.is_dir() {
            let _ = std::fs::remove_dir_all(p);
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
            } else if p.is_dir() {
                let _ = std::fs::remove_dir_all(p);
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    // --- Phase 1 provenance (NULL for old rows) ---
    #[serde(default)]
    pub workload_id: Option<String>,
    #[serde(default)]
    pub hardware_profile_id: Option<i64>,
    #[serde(default)]
    pub engine_version: Option<String>,
    #[serde(default)]
    pub prompt_tps: Option<f64>,
    #[serde(default)]
    pub ttft_ms: Option<u64>,
    #[serde(default)]
    pub peak_vram_mb: Option<u64>,
    #[serde(default)]
    pub model_rev: Option<String>,
    #[serde(default)]
    pub sampling_json: Option<String>,
    /// The canvas Role id this benchmark row fed, if any (Phase 8c).
    #[serde(default)]
    pub role_id: Option<String>,
}

fn ensure_column(conn: &Connection, table: &str, col: &str, ddl: &str) -> Result<()> {
    let mut stmt = conn.prepare(&format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name='{col}'"))?;
    let exists: i64 = stmt.query_row([], |r| r.get(0))?;
    if exists == 0 {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {col} {ddl}"))?;
    }
    Ok(())
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
    // Phase 1 additive provenance — history survives (NULL for old rows)
    for (col, ddl) in [
        ("workload_id", "TEXT"),
        ("hardware_profile_id", "INTEGER"),
        ("engine_version", "TEXT"),
        ("prompt_tps", "REAL"),
        ("ttft_ms", "INTEGER"),
        ("peak_vram_mb", "INTEGER"),
        ("model_rev", "TEXT"),
        ("sampling_json", "TEXT"),
        ("role_id", "TEXT"),
    ] {
        ensure_column(conn, "matrix_runs", col, ddl)?;
    }
    Ok(())
}

pub fn insert_matrix_run(conn: &Connection, row: &MatrixRow) -> Result<i64> {
    ensure_matrix_schema(conn)?;
    conn.execute(
        "INSERT INTO matrix_runs
            (engine, model, ctx, task, run, verdict, summary, gen_tokens,
             prompt_tokens, tok_s, tok_s_kind, wall_ms, output, at,
             workload_id, hardware_profile_id, engine_version, prompt_tps, ttft_ms, peak_vram_mb, model_rev, sampling_json, role_id)
          VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23)",
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
            row.workload_id,
            row.hardware_profile_id,
            row.engine_version,
            row.prompt_tps,
            row.ttft_ms,
            row.peak_vram_mb,
            row.model_rev,
            row.sampling_json,
            row.role_id,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// A single live throughput measurement pulled from a running engine's
/// Prometheus `/metrics` endpoint.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    // Phase 1 provenance (NULL for old rows)
    #[serde(default)]
    pub hardware_profile_id: Option<i64>,
    #[serde(default)]
    pub engine_version: Option<String>,
    #[serde(default)]
    pub prompt_tps: Option<f64>,
    #[serde(default)]
    pub ttft_ms: Option<u64>,
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
    for (col, ddl) in [
        ("hardware_profile_id", "INTEGER"),
        ("engine_version", "TEXT"),
        ("prompt_tps", "REAL"),
        ("ttft_ms", "INTEGER"),
    ] {
        ensure_column(conn, "bench", col, ddl)?;
    }
    Ok(())
}

pub fn insert_bench(conn: &Connection, row: &BenchRow) -> Result<i64> {
    ensure_bench_schema(conn)?;
    conn.execute(
        "INSERT INTO bench (engine, host, port, model, ctx, tps, at, hardware_profile_id, engine_version, prompt_tps, ttft_ms)
          VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        rusqlite::params![
            row.engine, row.host, row.port, row.model, row.ctx, row.tps, row.at,
            row.hardware_profile_id, row.engine_version, row.prompt_tps, row.ttft_ms
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

// ------------------------------------------------------------ workloads (Phase 2)
pub fn ensure_workloads_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS workloads (
            id TEXT PRIMARY KEY,
            label TEXT NOT NULL,
            description TEXT NOT NULL,
            tasks_json TEXT NOT NULL
        )",
    )?;
    Ok(())
}

pub fn upsert_workload(conn: &Connection, w: &crate::workload::Workload) -> Result<()> {
    ensure_workloads_schema(conn)?;
    let tasks_json = serde_json::to_string(&w.tasks).unwrap_or_else(|_| "[]".into());
    conn.execute(
        "INSERT INTO workloads (id, label, description, tasks_json) VALUES (?1,?2,?3,?4)
         ON CONFLICT(id) DO UPDATE SET label=excluded.label, description=excluded.description, tasks_json=excluded.tasks_json",
        rusqlite::params![w.id, w.label, w.description, tasks_json],
    )?;
    Ok(())
}

pub fn list_workloads(conn: &Connection) -> Result<Vec<crate::workload::Workload>> {
    ensure_workloads_schema(conn)?;
    let mut stmt = conn.prepare("SELECT id, label, description, tasks_json FROM workloads ORDER BY id")?;
    let rows = stmt.query_map([], |r| {
        let id: String = r.get(0)?;
        let label: String = r.get(1)?;
        let description: String = r.get(2)?;
        let tasks_json: String = r.get(3)?;
        let tasks: Vec<crate::workload::WorkloadTask> = serde_json::from_str(&tasks_json).unwrap_or_default();
        Ok(crate::workload::Workload { id, label, description, tasks })
    })?;
    Ok(rows.flatten().collect())
}

pub fn get_workload(conn: &Connection, id: &str) -> Result<Option<crate::workload::Workload>> {
    ensure_workloads_schema(conn)?;
    let mut stmt = conn.prepare("SELECT id, label, description, tasks_json FROM workloads WHERE id=?1")?;
    let mut rows = stmt.query_map([id], |r| {
        let id: String = r.get(0)?;
        let label: String = r.get(1)?;
        let description: String = r.get(2)?;
        let tasks_json: String = r.get(3)?;
        let tasks: Vec<crate::workload::WorkloadTask> = serde_json::from_str(&tasks_json).unwrap_or_default();
        Ok(crate::workload::Workload { id, label, description, tasks })
    })?;
    Ok(rows.next().transpose()?)
}

pub fn ensure_seeded_workloads(conn: &Connection) -> Result<()> {
    ensure_workloads_schema(conn)?;
    for w in crate::workload::seeded() {
        upsert_workload(conn, &w)?;
    }
    Ok(())
}

// ------------------------------------------------------------ hardware_profiles (Phase 3)
pub fn ensure_hardware_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS hardware_profiles (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            gpu TEXT NOT NULL, vram_mb INTEGER NOT NULL,
            cpu TEXT NOT NULL, ram_mb INTEGER NOT NULL,
            os TEXT NOT NULL, driver TEXT NOT NULL, cuda TEXT NOT NULL,
            cyberdeck_ver TEXT NOT NULL, engines_json TEXT NOT NULL,
            captured_at INTEGER NOT NULL, content_hash TEXT NOT NULL UNIQUE
        )",
    )?;
    Ok(())
}

pub fn upsert_hardware_profile(conn: &Connection, p: &crate::hardware::HardwareProfile) -> Result<i64> {
    ensure_hardware_schema(conn)?;
    conn.execute(
        "INSERT OR IGNORE INTO hardware_profiles (gpu, vram_mb, cpu, ram_mb, os, driver, cuda, cyberdeck_ver, engines_json, captured_at, content_hash)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        rusqlite::params![p.gpu, p.vram_mb as i64, p.cpu, p.ram_mb as i64, p.os, p.driver, p.cuda, p.cyberdeck_ver, p.engines_json, p.captured_at, p.content_hash],
    )?;
    let mut stmt = conn.prepare("SELECT id FROM hardware_profiles WHERE content_hash=?1")?;
    Ok(stmt.query_row([&p.content_hash], |r| r.get(0))?)
}

pub fn capture_hardware_profile(conn: &Connection) -> Result<i64> {
    upsert_hardware_profile(conn, &crate::hardware::capture())
}

// ------------------------------------------------------------ settings + audit_log (O3)
pub fn ensure_settings_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value_json TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS audit_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ts INTEGER NOT NULL,
            actor TEXT NOT NULL,
            key TEXT NOT NULL,
            old_json TEXT,
            new_json TEXT,
            reason TEXT NOT NULL
        )",
    )?;
    Ok(())
}

pub fn settings_get(conn: &Connection, key: &str) -> Result<Option<String>> {
    ensure_settings_schema(conn)?;
    let mut stmt = conn.prepare("SELECT value_json FROM settings WHERE key=?1")?;
    let mut rows = stmt.query_map([key], |r| r.get::<_, String>(0))?;
    Ok(rows.next().transpose()?)
}

pub fn settings_set(conn: &Connection, key: &str, value_json: &str, actor: &str, reason: &str) -> Result<()> {
    ensure_settings_schema(conn)?;
    // validate JSON
    if serde_json::from_str::<serde_json::Value>(value_json).is_err() {
        anyhow::bail!("value must be valid JSON");
    }
    let old = settings_get(conn, key)?.unwrap_or_default();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    conn.execute("INSERT INTO settings(key, value_json, updated_at) VALUES (?1,?2,?3) ON CONFLICT(key) DO UPDATE SET value_json=excluded.value_json, updated_at=excluded.updated_at", rusqlite::params![key, value_json, now])?;
    conn.execute("INSERT INTO audit_log(ts, actor, key, old_json, new_json, reason) VALUES (?1,?2,?3,?4,?5,?6)", rusqlite::params![now, actor, key, if old.is_empty() { None } else { Some(old) }, value_json, reason])?;
    Ok(())
}

pub fn settings_list(conn: &Connection) -> Result<Vec<(String, String, i64)>> {
    ensure_settings_schema(conn)?;
    let mut stmt = conn.prepare("SELECT key, value_json, updated_at FROM settings ORDER BY key")?;
    Ok(stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<Vec<_>, _>>()?)
}

pub fn audit_list(conn: &Connection, limit: usize) -> Result<Vec<(i64, i64, String, String, Option<String>, Option<String>, String)>> {
    ensure_settings_schema(conn)?;
    let mut stmt = conn.prepare("SELECT id, ts, actor, key, old_json, new_json, reason FROM audit_log ORDER BY id DESC LIMIT ?1")?;
    Ok(stmt.query_map([limit as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))?.collect::<Result<Vec<_>,_>>()?)
}

pub fn settings_undo(conn: &Connection, audit_id: i64) -> Result<()> {
    ensure_settings_schema(conn)?;
    let mut stmt = conn.prepare("SELECT key, old_json, new_json FROM audit_log WHERE id=?1")?;
    let (key, old, _new): (String, Option<String>, Option<String>) = stmt.query_row([audit_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    if let Some(v) = old {
        settings_set(conn, &key, &v, "undo", &format!("revert #{audit_id}"))?;
    } else {
        conn.execute("DELETE FROM settings WHERE key=?1", [&key])?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        conn.execute("INSERT INTO audit_log(ts, actor, key, old_json, new_json, reason) VALUES (?1,?2,?3,?4,?5,?6)", rusqlite::params![now, "undo", key, _new, Option::<String>::None, format!("undo #{audit_id}")])?;
    }
    Ok(())
}

// ------------------------------------------------------------ evaluations (Phase 2)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Evaluation {
    pub id: i64,
    pub matrix_run_id: i64,
    pub method: String,
    pub passed: bool,
    pub score: f64,
    pub details_json: String,
    pub at: i64,
}

pub fn ensure_evaluations_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS evaluations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            matrix_run_id INTEGER NOT NULL,
            method TEXT NOT NULL,
            passed INTEGER NOT NULL,
            score REAL NOT NULL,
            details_json TEXT NOT NULL,
            at INTEGER NOT NULL,
            FOREIGN KEY(matrix_run_id) REFERENCES matrix_runs(id)
        );
        CREATE INDEX IF NOT EXISTS idx_evals_run ON evaluations(matrix_run_id);",
    )?;
    Ok(())
}

pub fn insert_evaluation(conn: &Connection, e: &Evaluation) -> Result<i64> {
    ensure_evaluations_schema(conn)?;
    conn.execute(
        "INSERT INTO evaluations (matrix_run_id, method, passed, score, details_json, at) VALUES (?1,?2,?3,?4,?5,?6)",
        rusqlite::params![e.matrix_run_id, e.method, e.passed as i64, e.score, e.details_json, e.at],
    )?;
    Ok(conn.last_insert_rowid())
}

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
    ensure_bench_schema(conn)?;
    let mut stmt = conn.prepare(
        "SELECT id, engine, host, port, model, ctx, tps, at,
                hardware_profile_id, engine_version, prompt_tps, ttft_ms
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
            hardware_profile_id: r.get(8)?,
            engine_version: r.get(9)?,
            prompt_tps: r.get(10)?,
            ttft_ms: r.get(11)?,
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

    #[test]
    fn workloads_seed_and_roundtrip() {
        let conn = Connection::open_in_memory().expect("mem");
        ensure_seeded_workloads(&conn).unwrap();
        let ws = list_workloads(&conn).unwrap();
        assert!(ws.iter().any(|w| w.id == "coding"));
        let coding = get_workload(&conn, "coding").unwrap().unwrap();
        assert!(!coding.tasks.is_empty());
        // upsert preserves
        upsert_workload(&conn, &coding).unwrap();
        assert!(get_workload(&conn, "coding").unwrap().is_some());
    }

    #[test]
    fn hardware_profile_dedup() {
        let conn = Connection::open_in_memory().expect("mem");
        let id1 = capture_hardware_profile(&conn).unwrap();
        let id2 = capture_hardware_profile(&conn).unwrap();
        assert_eq!(id1, id2);
        let mut stmt = conn.prepare("SELECT count(*) FROM hardware_profiles").unwrap();
        let n: i64 = stmt.query_row([], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn settings_audit_reversible() {
        let conn = Connection::open_in_memory().expect("mem");
        settings_set(&conn, "foo", "\"bar\"", "tester", "test").unwrap();
        assert_eq!(settings_get(&conn, "foo").unwrap().as_deref(), Some("\"bar\""));
        settings_set(&conn, "foo", "\"baz\"", "tester", "update").unwrap();
        let logs = audit_list(&conn, 10).unwrap();
        assert!(logs.len() >= 2);
        let last_id = logs[0].0;
        settings_undo(&conn, last_id).unwrap();
        assert_eq!(settings_get(&conn, "foo").unwrap().as_deref(), Some("\"bar\""));
    }

    #[test]
    fn evaluations_persist_per_matrix() {
        let conn = Connection::open_in_memory().expect("mem");
        ensure_matrix_schema(&conn).unwrap();
        let row = MatrixRow {
            engine: "llamacpp".into(), model: "qwen".into(), ctx: 8192, task: "humaneval".into(), run: 1,
            verdict: "RUNNING".into(), summary: "".into(), gen_tokens: Some(10), prompt_tokens: Some(5),
            tok_s: Some(50.0), tok_s_kind: "native".into(), wall_ms: 100, output: "hello world".into(), at: 0,
            workload_id: None, hardware_profile_id: None, engine_version: None, prompt_tps: None, ttft_ms: None, peak_vram_mb: None, model_rev: None, sampling_json: None, role_id: None,
        };
        let id = insert_matrix_run(&conn, &row).unwrap();
        let ev = Evaluation { id: 0, matrix_run_id: id, method: "exact".into(), passed: true, score: 1.0, details_json: "{}".into(), at: 0 };
        insert_evaluation(&conn, &ev).unwrap();
        let mut stmt = conn.prepare("SELECT count(*) FROM evaluations WHERE matrix_run_id=?1").unwrap();
        let n: i64 = stmt.query_row([id], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }
}
