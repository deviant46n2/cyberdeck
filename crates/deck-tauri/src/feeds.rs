use serde::Serialize;

pub use deck_core::store::Release;

#[derive(Serialize)]
pub struct RankedRelease {
    pub release: Release,
    pub score: deck_core::relevance::Score,
}

#[derive(Serialize)]
pub struct FeedsPollResult {
    pub fetched: usize,
    pub inserted: usize,
}

pub fn feeds_poll(sources: Vec<String>) -> Result<FeedsPollResult, String> {
    let (fetched, inserted) = deck_feeds::feeds_poll(&sources).map_err(|e| e.to_string())?;
    Ok(FeedsPollResult { fetched, inserted })
}

pub fn feeds_list(limit: usize) -> Result<Vec<Release>, String> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db).map_err(|e| e.to_string())?;
    deck_core::store::list_releases(&conn, limit).map_err(|e| e.to_string())
}

pub fn feeds_rank(limit: usize, workload: Option<String>) -> Result<Vec<RankedRelease>, String> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db).map_err(|e| e.to_string())?;
    let releases = deck_core::store::list_releases(&conn, 200).map_err(|e| e.to_string())?;
    let installed = deck_core::store::list(&conn)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|m| deck_core::relevance::Installed { name: m.name, arch: m.arch, quant: m.quant })
        .collect::<Vec<_>>();
    let vram = deck_core::fit::available_vram_mb(16000);
    let disk_free_mb = deck_core::fit::hw_free_disk_mb().unwrap_or(268_000);
    let best = deck_core::store::recent_bench(&conn, 20).ok().and_then(|v| v.first().map(|r| r.tps)).unwrap_or(0.0);
    let bench = deck_core::relevance::BenchBest { tok_s: if best > 0.0 { Some(best) } else { None } };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let mut weights = deck_core::relevance::Weights::default();
    if let Some(ref w) = workload {
        if w == "coding" { weights.family = 0.35; weights.hw = 0.25; }
    }
    let ranked = deck_core::relevance::rank(releases, &installed, &bench, vram, now, &weights, disk_free_mb);
    Ok(ranked.into_iter().take(limit).map(|(release, score)| RankedRelease { release, score }).collect())
}
