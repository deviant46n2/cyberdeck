use anyhow::Result;
use rusqlite::Connection;

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
    #[serde(default)]
    pub workflow_id: Option<String>,
}

/// One role's accumulated bench (Phase 8e): best/avg tok/s across the models the
/// canvas has run against that role, so a canvas can show "which model best at
/// which node" and Phase 4 recommend has per-role signal. Aggregated from
/// `matrix_runs` where `role_id` is set.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RoleBenchRow {
    pub role_id: String,
    pub engine: String,
    pub model: String,
    pub runs: u64,
    pub best_tps: f64,
    pub avg_tps: f64,
    /// Tok/s of the most recent run in this role/model group.
    pub last_tps: f64,
    pub last_wall_ms: u64,
    pub last_ttft_ms: Option<u64>,
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
        ("workflow_id", "TEXT"),
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
             workload_id, hardware_profile_id, engine_version, prompt_tps, ttft_ms, peak_vram_mb, model_rev, sampling_json, role_id, workflow_id)
          VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24)",
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
            row.workflow_id,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Aggregate per-role bench for the given role ids (Phase 8e). Only rows with a
/// measurable `tok_s` count (stateless engine runs); returns best/avg/last per
/// role+model, ordered role then best tok/s desc, so "which model best at which
/// node" is one query.
pub fn per_role_bench(conn: &Connection, roles: &[&str]) -> Result<Vec<RoleBenchRow>> {
    ensure_matrix_schema(conn)?;
    if roles.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", roles.len()).collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT role_id, engine, model,
                COUNT(*),
                MAX(tok_s),
                AVG(tok_s),
                (SELECT tok_s FROM matrix_runs m2
                  WHERE m2.role_id = matrix_runs.role_id AND m2.model = matrix_runs.model
                    AND m2.engine = matrix_runs.engine
                  ORDER BY m2.at DESC, m2.id DESC LIMIT 1) AS last_tps,
                (SELECT wall_ms FROM matrix_runs m2
                  WHERE m2.role_id = matrix_runs.role_id AND m2.model = matrix_runs.model
                    AND m2.engine = matrix_runs.engine
                  ORDER BY m2.at DESC, m2.id DESC LIMIT 1) AS last_wall,
                (SELECT ttft_ms FROM matrix_runs m2
                  WHERE m2.role_id = matrix_runs.role_id AND m2.model = matrix_runs.model
                    AND m2.engine = matrix_runs.engine
                  ORDER BY m2.at DESC, m2.id DESC LIMIT 1) AS last_ttft
         FROM matrix_runs
         WHERE role_id IN ({placeholders}) AND tok_s IS NOT NULL
         GROUP BY role_id, engine, model
         ORDER BY role_id, MAX(tok_s) DESC",
        placeholders = placeholders
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(roles.iter().copied()), |r| {
        Ok(RoleBenchRow {
            role_id: r.get(0)?,
            engine: r.get(1)?,
            model: r.get(2)?,
            runs: r.get(3)?,
            best_tps: r.get(4)?,
            avg_tps: r.get(5)?,
            last_tps: r.get(6)?,
            last_wall_ms: r.get(7)?,
            last_ttft_ms: r.get(8)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Whole-loop bench: aggregate over a workflow's runs (Phase 8 unified canvas).
#[derive(Debug, Clone, serde::Serialize)]
pub struct LoopBenchRow {
    pub workflow_id: String,
    pub runs: u64,
    pub best_tps: f64,
    pub avg_tps: f64,
    pub last_tps: f64,
    pub last_wall_ms: u64,
    pub last_gen_tokens: u64,
}

pub fn workflow_loop_bench(conn: &Connection, workflow_id: &str) -> Result<Option<LoopBenchRow>> {
    ensure_matrix_schema(conn)?;
    let mut stmt = conn.prepare(
        "SELECT at, SUM(COALESCE(gen_tokens,0)), SUM(wall_ms),
                CASE WHEN SUM(wall_ms)>0 THEN (SUM(COALESCE(gen_tokens,0))*1000.0/SUM(wall_ms)) ELSE 0 END as loop_tps
         FROM matrix_runs WHERE workflow_id = ?1 AND tok_s IS NOT NULL GROUP BY at ORDER BY at DESC",
    )?;
    let runs: Vec<(i64, u64, u64, f64)> = stmt
        .query_map([workflow_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as u64, r.get::<_, i64>(2)? as u64, r.get::<_, f64>(3)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    if runs.is_empty() {
        return Ok(None);
    }
    let tps: Vec<f64> = runs.iter().map(|(_, _, _, t)| *t).collect();
    let best = tps.iter().copied().fold(0.0_f64, f64::max);
    let avg = tps.iter().sum::<f64>() / tps.len() as f64;
    let (last_at, last_gen, last_wall, last_tps) = runs[0];
    let _ = last_at;
    Ok(Some(LoopBenchRow {
        workflow_id: workflow_id.to_string(),
        runs: runs.len() as u64,
        best_tps: best,
        avg_tps: avg,
        last_tps,
        last_wall_ms: last_wall,
        last_gen_tokens: last_gen,
    }))
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

impl BenchRow {
    /// Create a BenchRow with hardware profile auto-captured.
    /// `engine_version` is passed through (detect it from the running engine).
    pub fn with_provenance(
        conn: &Connection,
        engine: &str,
        host: &str,
        port: u16,
        model: &str,
        ctx: u32,
        tps: f64,
        at: i64,
        engine_version: Option<String>,
        prompt_tps: Option<f64>,
        ttft_ms: Option<u64>,
    ) -> Self {
        let hardware_profile_id = super::capture_hardware_profile(conn).ok();
        BenchRow {
            id: 0,
            engine: engine.to_string(),
            host: host.to_string(),
            port,
            model: model.to_string(),
            ctx,
            tps,
            at,
            hardware_profile_id,
            engine_version,
            prompt_tps,
            ttft_ms,
        }
    }
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
