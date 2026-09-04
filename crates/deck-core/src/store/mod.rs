//! SQLite inventory store. Connection plumbing + the core `models` index live
//! here; every other table is owned by a per-domain submodule that shares this
//! module's surface through the `pub use` below, so callers keep the flat
//! `store::*` paths regardless of file boundaries.

mod bench;
mod engine_bin;
mod hw;
mod profiles;
mod releases;
mod residents;
mod settings;
mod workloads;
mod agents;

pub use bench::*;
pub use engine_bin::*;
pub use hw::*;
pub use profiles::*;
pub use releases::*;
pub use residents::*;
pub use settings::*;
pub use workloads::*;
pub use agents::*;

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
pub const SCHEMA_VERSION: i64 = 3;

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
    ensure_models_table(&conn)?;
    Ok(conn)
}

/// The vault index table. Extracted so the profile store's tests (and any
/// other store touch) can create it without opening a real file-backed DB.
pub fn ensure_models_table(conn: &Connection) -> Result<()> {
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
    Ok(())
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
/// Removes rows whose path is not in `keep`. Keeps the index honest when a
/// model is deleted or moved between scans.
///
/// Because `profiles.model_id` references `models(id)` and the bundled SQLite
/// has `SQLITE_DEFAULT_FOREIGN_KEYS=1`, we must NULL out any profile links
/// before deleting the model row — otherwise the FK constraint fires error 787.
pub fn prune(conn: &Connection, keep: &[String]) -> Result<usize> {
    let existing: Vec<(i64, String)> = {
        let mut stmt = conn.prepare("SELECT id, path FROM models")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        rows.flatten().collect()
    };
    let keep_set: std::collections::HashSet<&str> = keep.iter().map(|s| s.as_str()).collect();
    let mut removed = 0;
    for (id, path) in existing {
        if !keep_set.contains(path.as_str()) {
            // Unlink any profiles that reference this model before deleting.
            conn.execute(
                "UPDATE profiles SET model_id = NULL WHERE model_id = ?1",
                [id],
            )?;
            conn.execute("DELETE FROM models WHERE path = ?1", [path])?;
            removed += 1;
        }
    }
    Ok(removed)
}

// ---------------------------------------------------- extra scan directories

/// Read the user-configured extra scan directories from the settings table.
/// Returns an empty vec when none have been added yet.
pub fn scan_dirs(conn: &Connection) -> Result<Vec<PathBuf>> {
    let json = settings_get(conn, "extra_scan_dirs")?.unwrap_or_else(|| "[]".into());
    let dirs: Vec<String> = serde_json::from_str(&json).unwrap_or_default();
    Ok(dirs.into_iter().map(PathBuf::from).collect())
}

/// Append a directory to the extra scan list (idempotent — duplicates are
/// ignored). The path is stored as-is; canonicalisation happens at scan time.
pub fn add_scan_dir(conn: &Connection, path: &str) -> Result<()> {
    let mut dirs: Vec<String> = {
        let json = settings_get(conn, "extra_scan_dirs")?.unwrap_or_else(|| "[]".into());
        serde_json::from_str(&json).unwrap_or_default()
    };
    if !dirs.iter().any(|d| d == path) {
        dirs.push(path.to_string());
        settings_set(
            conn,
            "extra_scan_dirs",
            &serde_json::to_string(&dirs)?,
            "cli",
            &format!("add scan dir: {path}"),
        )?;
    }
    Ok(())
}

/// Remove a directory from the extra scan list.
pub fn remove_scan_dir(conn: &Connection, path: &str) -> Result<bool> {
    let mut dirs: Vec<String> = {
        let json = settings_get(conn, "extra_scan_dirs")?.unwrap_or_else(|| "[]".into());
        serde_json::from_str(&json).unwrap_or_default()
    };
    let before = dirs.len();
    dirs.retain(|d| d != path);
    if dirs.len() < before {
        settings_set(
            conn,
            "extra_scan_dirs",
            &serde_json::to_string(&dirs)?,
            "cli",
            &format!("remove scan dir: {path}"),
        )?;
        Ok(true)
    } else {
        Ok(false)
    }
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
            workload_id: None, hardware_profile_id: None, engine_version: None, prompt_tps: None, ttft_ms: None, peak_vram_mb: None, model_rev: None, sampling_json: None, role_id: None, workflow_id: None,
        };
        let id = insert_matrix_run(&conn, &row).unwrap();
        let ev = Evaluation { id: 0, matrix_run_id: id, method: "exact".into(), passed: true, score: 1.0, details_json: "{}".into(), at: 0 };
        insert_evaluation(&conn, &ev).unwrap();
        let mut stmt = conn.prepare("SELECT count(*) FROM evaluations WHERE matrix_run_id=?1").unwrap();
        let n: i64 = stmt.query_row([id], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    fn matrix_row(role: &str, model: &str, tps: Option<f64>) -> MatrixRow {
        MatrixRow {
            engine: "llamacpp".into(), model: model.into(), ctx: 0, task: role.into(), run: 1,
            verdict: "ok".into(), summary: "workflow node".into(), gen_tokens: Some(100), prompt_tokens: None,
            tok_s: tps, tok_s_kind: "wall".into(), wall_ms: 500, output: String::new(), at: 0,
            workload_id: None, hardware_profile_id: None, engine_version: None, prompt_tps: None, ttft_ms: Some(40), peak_vram_mb: None, model_rev: None, sampling_json: None, role_id: Some(role.into()), workflow_id: None,
        }
    }

    #[test]
    fn per_role_bench_aggregates_best_avg_last() {
        let conn = Connection::open_in_memory().expect("mem");
        ensure_matrix_schema(&conn).unwrap();
        // role "r1" run against two models (fast then slow); role "r2" one model
        insert_matrix_run(&conn, &matrix_row("r1", "qwen-fast", Some(80.0))).unwrap();
        insert_matrix_run(&conn, &matrix_row("r1", "qwen-fast", Some(60.0))).unwrap();
        insert_matrix_run(&conn, &matrix_row("r1", "qwen-slow", Some(30.0))).unwrap();
        insert_matrix_run(&conn, &matrix_row("r2", "qwen-slow", Some(20.0))).unwrap();
        // a row with NULL tok_s must not pollute the aggregation
        insert_matrix_run(&conn, &matrix_row("r1", "agent-node", None)).unwrap();

        let rows = per_role_bench(&conn, &["r1"]).unwrap();
        // r1 has two models; r2 excluded by the role filter
        assert_eq!(rows.len(), 2);
        let fast = rows.iter().find(|r| r.model == "qwen-fast").unwrap();
        assert_eq!(fast.runs, 2);
        assert_eq!(fast.best_tps, 80.0);
        assert!((fast.avg_tps - 70.0).abs() < 1e-9);
        assert_eq!(fast.last_tps, 60.0); // most recent of the two
        let slow = rows.iter().find(|r| r.model == "qwen-slow").unwrap();
        assert_eq!(slow.runs, 1);
        // ordered best-first within the role
        assert_eq!(rows[0].role_id, "r1");
        assert!(rows[0].best_tps >= rows[1].best_tps);

        let empty = per_role_bench(&conn, &[]).unwrap();
        assert!(empty.is_empty());
    }
}
