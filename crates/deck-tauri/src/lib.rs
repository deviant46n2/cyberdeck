//! Tauri command API for cyberdeck.
//!
//! This crate is the single bridge between the UI and the engine crates. Every
//! command returns a serializable DTO so the same logic is unit-tested headless
//! here and consumed by both the desktop app and (eventually) the CLI.

use std::path::PathBuf;

use serde::Serialize;

pub use deck_core::profile::Engine;

#[derive(Serialize)]
pub struct ModelRow {
    pub name: String,
    pub quant: Option<String>,
    pub arch: Option<String>,
    pub ctx_train: u64,
    pub footprint_gib: f64,
    pub path: String,
}

#[derive(Serialize)]
pub struct DupRow {
    pub identity: String,
    pub wasted_gib: f64,
    pub members: Vec<String>,
}

#[derive(Serialize)]
pub struct ProfileRow {
    pub name: String,
    pub engine: String,
    pub alias: String,
    pub port: u16,
    pub ctx: u32,
    pub model: String,
}

#[derive(Serialize)]
pub struct FitRow {
    pub model: String,
    pub ctx: u64,
    pub weights_mb: u64,
    pub kv_mb: u64,
    pub buffers_mb: u64,
    pub model_vram_mb: u64,
    pub weights_ram_mb: u64,
    pub overhead_mb: u64,
    pub available_for_model_mb: u64,
    pub verdict: String,
}

#[derive(Serialize)]
pub struct ScanResult {
    pub indexed: usize,
    pub pruned: usize,
    pub models: Vec<ModelRow>,
    pub dups: Vec<DupRow>,
}

#[derive(Serialize)]
pub struct UseResult {
    pub name: String,
    pub applied: bool,
    pub dry_run: bool,
    pub unit: String,
}

fn gib(bytes: u64) -> f64 {
    bytes as f64 / 1_073_741_824.0
}

/// Refresh the index from configured roots; returns the fresh inventory.
pub fn scan() -> anyhow::Result<ScanResult> {
    let roots = deck_core::scanner::default_roots();
    let models = deck_core::scanner::scan(&roots)?;
    let db = deck_core::store::default_db_path();
    let mut conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_profile_schema(&conn)?;
    let indexed = deck_core::store::upsert_many(&mut conn, &models)?;
    let keep: Vec<String> = models.iter().map(|m| m.path.display().to_string()).collect();
    let pruned = deck_core::store::prune(&conn, &keep)?;
    let dups = deck_core::store::duplicates(&conn)?
        .into_iter()
        .map(|d| DupRow {
            identity: d.identity,
            wasted_gib: gib(d.wasted_bytes),
            members: d.members.iter().map(|m| m.path.display().to_string()).collect(),
        })
        .collect();
    let models = models
        .into_iter()
        .map(|m| ModelRow {
            name: m.name,
            quant: m.quant,
            arch: m.arch,
            ctx_train: m.ctx_train.unwrap_or(0),
            footprint_gib: gib(m.footprint),
            path: m.path.display().to_string(),
        })
        .collect();
    Ok(ScanResult { indexed, pruned, models, dups })
}

pub fn list_models() -> anyhow::Result<Vec<ModelRow>> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    Ok(deck_core::store::list(&conn)?
        .into_iter()
        .map(|m| ModelRow {
            name: m.name,
            quant: m.quant,
            arch: m.arch,
            ctx_train: m.ctx_train.unwrap_or(0),
            footprint_gib: gib(m.footprint),
            path: m.path.display().to_string(),
        })
        .collect())
}

pub fn list_profiles() -> anyhow::Result<Vec<ProfileRow>> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_profile_schema(&conn)?;
    Ok(deck_core::store::list_profiles(&conn)?
        .into_iter()
        .map(|p| ProfileRow {
            name: p.name,
            engine: format!("{:?}", p.engine),
            alias: p.alias,
            port: p.port,
            ctx: p.ctx_size,
            model: p.model,
        })
        .collect())
}

pub fn dedup() -> anyhow::Result<Vec<DupRow>> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    Ok(deck_core::store::duplicates(&conn)?
        .into_iter()
        .map(|d| DupRow {
            identity: d.identity,
            wasted_gib: gib(d.wasted_bytes),
            members: d.members.iter().map(|m| m.path.display().to_string()).collect(),
        })
        .collect())
}

pub fn fit(
    model: PathBuf,
    ctx: u32,
    kv_bytes: f64,
    ngl: f64,
    kv_layers: Option<u64>,
    reserve: u64,
    offload: bool,
) -> anyhow::Result<FitRow> {
    let meta = if model.is_dir() {
        deck_core::safetensors::open_dir(&model)?
    } else {
        deck_core::gguf::GgufMeta::read(&model)?.to_meta(&model)
    };
    let req = deck_core::fit::FitRequest {
        ctx: ctx as u64,
        kv_bytes,
        ngl_frac: ngl,
        kv_layers,
        reserved_mb: reserve,
        offload,
    };
    let available = deck_core::fit::available_vram_mb(16_303);
    let b = deck_core::fit::estimate(&meta, &req, available);
    Ok(FitRow {
        model: meta.path.display().to_string(),
        ctx: req.ctx,
        weights_mb: b.weights_mb,
        kv_mb: b.kv_mb,
        buffers_mb: b.buffers_mb,
        model_vram_mb: b.model_vram_mb,
        weights_ram_mb: b.weights_ram_mb,
        overhead_mb: b.overhead_mb,
        available_for_model_mb: b.available_for_model_mb,
        verdict: b.verdict.tag().to_string(),
    })
}

/// Render (dry_run) or apply a loadout. `dry_run` returns the unit without
/// touching the live service.
pub fn use_profile(name: &str, dry_run: bool) -> anyhow::Result<UseResult> {
    let db = deck_core::store::default_db_path();
    let mut conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_profile_schema(&conn)?;
    let p = deck_core::store::get_profile(&conn, name)?
        .ok_or_else(|| anyhow::anyhow!("no loadout named '{name}'"))?;
    deck_core::store::set_active(&mut conn, name)?;
    let unit = deck_engines::render_unit(&p);
    if !dry_run {
        deck_engines::apply(&p, false)?;
    }
    Ok(UseResult { name: name.to_string(), applied: !dry_run, dry_run, unit })
}

#[derive(Serialize)]
pub struct SignalRow {
    pub id: String,
    pub author: String,
    pub created_at: String,
    pub downloads: u64,
    pub likes: u64,
    pub pipeline_tag: Option<String>,
    pub tags: Vec<String>,
}

/// Run a SIGNALS check: poll watched orgs and return only new models.
pub fn signals_check(limit: usize) -> anyhow::Result<Vec<SignalRow>> {
    let conn = deck_feeds::open()?;
    deck_feeds::ensure_seeds(&conn)?;
    let news = deck_feeds::check(&conn, limit)?;
    Ok(news
        .into_iter()
        .map(|m| SignalRow {
            id: m.id,
            author: m.author,
            created_at: m.created_at,
            downloads: m.downloads,
            likes: m.likes,
            pipeline_tag: m.pipeline_tag,
            tags: m.tags,
        })
        .collect())
}

pub fn watchlist() -> anyhow::Result<Vec<String>> {
    let conn = deck_feeds::open()?;
    deck_feeds::ensure_seeds(&conn)?;
    deck_feeds::list_watchlist(&conn)
}

pub fn watch_add(org: &str) -> anyhow::Result<()> {
    let conn = deck_feeds::open()?;
    deck_feeds::add_org(&conn, org)
}

pub fn watch_remove(org: &str) -> anyhow::Result<()> {
    let conn = deck_feeds::open()?;
    deck_feeds::remove_org(&conn, org)
}

#[derive(Serialize)]
pub struct MarketHit {
    pub id: String,
    pub downloads: u64,
    pub likes: u64,
    pub pipeline_tag: Option<String>,
    pub tags: Vec<String>,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct MarketFileRow {
    pub rfilename: String,
    pub size: Option<u64>,
}

/// Search HuggingFace models by free-text query.
pub fn market_search(query: &str, limit: usize) -> anyhow::Result<Vec<MarketHit>> {
    Ok(deck_feeds::search_models(query, limit)?
        .into_iter()
        .map(|h| MarketHit {
            id: h.id,
            downloads: h.downloads,
            likes: h.likes,
            pipeline_tag: h.pipeline_tag,
            tags: h.tags,
            created_at: h.created_at,
        })
        .collect())
}

/// List GGUF files (with sizes) for a repo.
pub fn market_files(repo_id: &str) -> anyhow::Result<Vec<MarketFileRow>> {
    Ok(deck_feeds::model_files(repo_id)?
        .into_iter()
        .map(|f| MarketFileRow { rfilename: f.rfilename, size: f.size })
        .collect())
}

fn models_dir() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("models")
}

/// Download a single repo file into ~/models, returning the saved path.
pub fn market_download(repo_id: &str, rfilename: &str) -> anyhow::Result<String> {
    let dest = deck_feeds::download_file(repo_id, rfilename, &models_dir())?;
    Ok(dest.display().to_string())
}

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
    let text = deck_engines::fetch_metrics(host, port)?;
    let tps = deck_engines::parse_tps(&text)
        .ok_or_else(|| anyhow::anyhow!("no tokens/sec gauge exposed by {host}:{port}"))?;
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let row = deck_core::store::BenchRow {
        id: 0,
        engine: engine.to_string(),
        host: host.to_string(),
        port,
        model: model.to_string(),
        ctx,
        tps,
        at,
    };
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_bench_schema(&conn)?;
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

/// Liveness of a single engine endpoint.
pub fn engine_status(engine: &str, host: &str, port: u16) -> EngineStatus {
    EngineStatus {
        engine: engine.to_string(),
        host: host.to_string(),
        port,
        up: deck_engines::health_ok(host, port),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_and_dedup_headless() {
        let r = scan().expect("scan");
        assert!(r.indexed > 0, "should find local models");
        // the known NVFP4 duplicate must surface
        let hit = r.dups.iter().any(|d| d.wasted_gib > 10.0);
        assert!(hit, "expected the real duplicate to be reported");
        let rows = list_models().expect("list");
        assert_eq!(rows.len(), r.models.len());
    }

    #[test]
    fn fit_reports_verdict() {
        let f = fit(
            PathBuf::from("/home/deviant/Qwen3.8-27B-UD-Q3_K_XL.gguf"),
            32768,
            0.5,
            1.0,
            None,
            1600,
            false,
        )
        .expect("fit");
        assert!(!f.verdict.is_empty());
        assert!(f.model_vram_mb > 0);
    }

    #[test]
    fn fit_offload_spills_weights_to_ram() {
        let dir = PathBuf::from("/home/deviant/Qwen3.6-35B-A3B-NVFP4");
        if dir.exists() {
            let f = fit(dir, 32768, 1.0, 1.0, None, 1600, true).expect("fit");
            assert!(f.weights_ram_mb > 0, "offload should report RAM-spilled weights");
            assert!(
                f.model_vram_mb < f.weights_mb + f.weights_ram_mb,
                "offload VRAM should be far below total weights"
            );
        }
    }

    #[test]
    fn render_known_profile() {
        // import from the real wrapper, then render without applying
        let script = std::path::PathBuf::from(
            "/home/deviant/.local/share/llama-server/run-llama-server.sh",
        );
        if script.exists() {
            let p = deck_core::importer::import_llamacpp_script(&script, "qwen").unwrap();
            let unit = deck_engines::render_unit(&p);
            assert!(unit.contains("qwen3.8-27b"));
            assert!(unit.contains("/home/deviant/Qwen3.8-27B-UD-Q3_K_XL.gguf"));
        }
    }
}
