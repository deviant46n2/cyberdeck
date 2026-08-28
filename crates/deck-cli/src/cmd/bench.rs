//! Benchmark a live engine: probe /metrics and record, or list history.

use anyhow::Result;

pub(crate) fn record(
    engine: String,
    host: String,
    port: u16,
    model: String,
    ctx: u32,
) -> Result<()> {
    let text = deck_engines::fetch_metrics(&host, port).map_err(|e| {
        anyhow::anyhow!(
            "could not reach {host}:{port}/metrics — is the engine running with --metrics? ({e})"
        )
    })?;
    let tps = deck_engines::parse_tps(&text)
        .ok_or_else(|| anyhow::anyhow!("no tokens/sec gauge exposed by {host}:{port}"))?;
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_bench_schema(&conn)?;
    let row = deck_core::store::BenchRow {
        id: 0,
        engine: engine.clone(),
        host: host.clone(),
        port,
        model: model.clone(),
        ctx,
        tps,
        at,
    };
    let id = deck_core::store::insert_bench(&conn, &row)?;
    println!("recorded #{id}: {tps:.1} tok/s from {engine} {host}:{port}");
    Ok(())
}

pub(crate) fn list() -> Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_bench_schema(&conn)?;
    let rows = deck_core::store::recent_bench(&conn, 50)?;
    if rows.is_empty() {
        println!("no benchmark readings yet — run `deck bench record` against a live engine");
        return Ok(());
    }
    for r in rows {
        println!(
            "{:>4}  {:<10} {:<15} {:>7.1} tok/s  {}",
            r.id,
            r.engine,
            format!("{}:{}", r.host, r.port),
            r.tps,
            chrono_like(r.at)
        );
    }
    Ok(())
}

fn chrono_like(at: i64) -> String {
    if at <= 0 {
        return "—".into();
    }
    // time-of-day in UTC (no chrono dependency)
    let rem = at.rem_euclid(86400);
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let s = rem % 60;
    format!("{h:02}:{m:02}:{s:02} UTC")
}
