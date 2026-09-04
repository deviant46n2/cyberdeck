//! Benchmark recording + engine liveness.

use serde::Serialize;

#[derive(Serialize)]
pub struct BenchRow {
    pub id: i64,
    pub engine: String,
    pub host: String,
    pub port: u16,
    pub model: String,
    pub ctx: u32,
    pub tps: f64,
    pub at: i64,
}

#[derive(Serialize)]
pub struct EngineStatus {
    pub engine: String,
    pub host: String,
    pub port: u16,
    pub up: bool,
}

/// Query a running engine's /metrics, parse generation tokens/sec, and store
/// the reading in the bench history table.
pub fn bench_now(
    engine: &str,
    host: &str,
    port: u16,
    model: &str,
    ctx: u32,
) -> anyhow::Result<BenchRow> {
    let eng = deck_core::profile::Engine::parse(engine)
        .ok_or_else(|| anyhow::anyhow!("unknown engine '{engine}'"))?;
    let tps = deck_engines::measure_generation_tps(eng, host, port, model)
        .map_err(|e| anyhow::anyhow!(e))?;
    let engine_version = deck_engines::detect_engine_version(eng, host, port);
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_bench_schema(&conn)?;
    let row = deck_core::store::BenchRow::with_provenance(
        &conn, engine, host, port, model, ctx, tps, at, engine_version, None, None,
    );
    let id = deck_core::store::insert_bench(&conn, &row)?;
    Ok(BenchRow {
        id,
        engine: row.engine,
        host: row.host,
        port: row.port,
        model: row.model,
        ctx: row.ctx,
        tps: row.tps,
        at: row.at,
    })
}

/// Return recent bench readings (newest first).
pub fn bench_history() -> anyhow::Result<Vec<BenchRow>> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_bench_schema(&conn)?;
    Ok(deck_core::store::recent_bench(&conn, 20)?
        .into_iter()
        .map(|r| BenchRow {
            id: r.id,
            engine: r.engine,
            host: r.host,
            port: r.port,
            model: r.model,
            ctx: r.ctx,
            tps: r.tps,
            at: r.at,
        })
        .collect())
}

/// Liveness of a single engine endpoint. Accepts whichever liveness endpoint
/// the runtime exposes (`health_ok_any`) so an Ollama daemon isn't misread as
/// down just because it has no `/health`.
pub fn engine_status(engine: &str, host: &str, port: u16) -> EngineStatus {
    EngineStatus {
        engine: engine.to_string(),
        host: host.to_string(),
        port,
        up: deck_engines::health_ok_any(host, port),
    }
}
