use anyhow::Result;

pub fn poll(sources: Vec<String>) -> Result<()> {
    let (fetched, inserted) = deck_feeds::feeds_poll(&sources)?;
    println!("fetched {fetched} releases, {inserted} new");
    Ok(())
}

pub fn list(json: bool, limit: usize) -> Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    let rows = deck_core::store::list_releases(&conn, limit)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        if rows.is_empty() {
            println!("no releases yet — run `deck feeds poll`");
            return Ok(());
        }
        for r in rows {
            println!("{:<8} {:<30} {:<18} {}  {}", r.source, r.repo, r.rev, r.published_at, r.url);
        }
    }
    Ok(())
}

pub fn watch(interval: u64, once: bool) -> Result<()> {
    let (f, ins) = deck_feeds::feeds_poll(&[])?;
    println!("[watch] poll: fetched {f} inserted {ins}");
    if once { return Ok(()); }
    println!("[watch] interval {interval}s — Ctrl+C to stop");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(interval));
        match deck_feeds::feeds_poll(&[]) {
            Ok((f, ins)) => println!("[watch] poll: fetched {f} inserted {ins}"),
            Err(e) => eprintln!("[watch] poll failed: {e:#}"),
        }
    }
}

pub fn rank(limit: usize, json: bool, workload: Option<String>) -> Result<()> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    let releases = deck_core::store::list_releases(&conn, 200)?;
    let installed = deck_core::store::list(&conn)?
        .into_iter()
        .map(|m| deck_core::relevance::Installed {
            name: m.name,
            arch: m.arch,
            quant: m.quant,
        })
        .collect::<Vec<_>>();
    let vram = deck_core::fit::available_vram_mb(16000);
    let best = deck_core::store::recent_bench(&conn, 20).ok().and_then(|v| v.first().map(|r| r.tps)).unwrap_or(0.0);
    let bench = deck_core::relevance::BenchBest { tok_s: if best > 0.0 { Some(best) } else { None } };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    // workload-aware boost: if a workload hint is given, reweight family slightly
    let mut weights = deck_core::relevance::Weights::default();
    if let Some(ref w) = workload {
        // tiny nudge — keeps deterministic ranking but surfaces workload family
        if w == "coding" { weights.family = 0.35; weights.hw = 0.25; }
        if w == "reasoning" { weights.recency = 0.12; }
    }
    let ranked = deck_core::relevance::rank(releases, &installed, &bench, vram, now, &weights);
    let top = ranked.into_iter().take(limit).collect::<Vec<_>>();
    if json {
        let out: Vec<serde_json::Value> = top.iter().map(|(r, s)| serde_json::json!({"release": r, "score": s})).collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        if top.is_empty() {
            println!("no releases to rank — run `deck feeds poll`");
            return Ok(());
        }
        println!("{:<5} {:<6} {:<8} {:<30} {}  {}", "rank", "score", "fits", "repo", "rev", "why");
        for (i, (r, s)) in top.iter().enumerate() {
            println!("{:<5} {:<6.2} {:<8} {:<30} {:<12} {}", i + 1, s.total, if s.fits { "✓" } else { "✗" }, r.repo, r.rev, s.reasons.join(", "));
        }
    }
    Ok(())
}
