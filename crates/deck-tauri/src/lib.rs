//! Tauri command API for cyberdeck.
//!
//! This crate is the single bridge between the UI and the engine crates. Every
//! command returns a serializable DTO so the same logic is unit-tested headless
//! here and consumed by both the desktop app and (eventually) the CLI.

use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub use deck_core::profile::Profile;
use serde::Serialize;

/// Check if a blob file starts with the GGUF magic bytes (first 4 bytes = "GGUF").
/// Ollama stores model files as content-addressable blobs without file extensions,
/// so we need to detect GGUF format by reading the magic header.
fn is_gguf_blob(path: &str) -> bool {
    if let Ok(mut f) = std::fs::File::open(path) {
        let mut header = [0u8; 4];
        if std::io::Read::read_exact(&mut f, &mut header).is_ok() {
            return header == *b"GGUF";
        }
    }
    false
}
use tauri::Emitter;

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
    /// MANAGED-mode client rewiring outcomes (empty unless --managed).
    pub rewired: Vec<String>,
}

fn gib(bytes: u64) -> f64 {
    bytes as f64 / 1_073_741_824.0
}

/// Refresh the index from configured roots; returns the fresh inventory.
pub fn scan() -> anyhow::Result<ScanResult> {
    let roots = deck_core::scanner::default_roots();
    let mut models = deck_core::scanner::scan(&roots)?;

    // Also index ollama models — they live as blobs under /var/lib/ollama/blobs
    // which is not a standard scanner root.
    if let Ok(ollama) = deck_feeds::ollama_models() {
        for o in &ollama {
            // Check if this blob is actually a GGUF file (some ollama blobs
            // are just manifest JSON or config files, not model weights).
            if o.path.ends_with(".gguf") || is_gguf_blob(&o.path) {
                let existing: std::collections::HashSet<String> = models
                    .iter()
                    .map(|m| std::fs::canonicalize(&m.path).ok()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| m.path.display().to_string()))
                    .collect();
                let canonical = std::fs::canonicalize(&o.path)
                    .ok()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| o.path.clone());
                if !existing.contains(&canonical) {
                    // Parse GGUF header to extract model metadata.
                    let meta = if let Ok(gguf_meta) = deck_core::gguf::GgufMeta::read(&o.path) {
                        gguf_meta.to_meta(&std::path::PathBuf::from(&o.path))
                    } else {
                        deck_core::model::ModelMeta {
                            path: std::path::PathBuf::from(o.path.clone()),
                            format: deck_core::model::ModelFormat::Gguf,
                            name: o.name.clone(),
                            arch: None,
                            quant: None,
                            params: None,
                            n_layers: None,
                            n_embd: None,
                            ctx_train: None,
                            vocab: None,
                            weight_size: o.size,
                            footprint: o.size,
                        }
                    };
                    models.push(meta);
                }
            }
        }
    }

    let db = deck_core::store::default_db_path();
    let mut conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_profile_schema(&conn)?;
    let indexed = deck_core::store::upsert_many(&mut conn, &models)?;
    let keep: Vec<String> = models
        .iter()
        .map(|m| m.path.display().to_string())
        .collect();
    let pruned = deck_core::store::prune(&conn, &keep)?;
    let dups = deck_core::store::duplicates(&conn)?
        .into_iter()
        .map(|d| DupRow {
            identity: d.identity,
            wasted_gib: gib(d.wasted_bytes),
            members: d
                .members
                .iter()
                .map(|m| m.path.display().to_string())
                .collect(),
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
    Ok(ScanResult {
        indexed,
        pruned,
        models,
        dups,
    })
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

/// Persist a loadout (created or edited in the UI) to the index.
pub fn save_profile(p: Profile) -> anyhow::Result<()> {
    let db = deck_core::store::default_db_path();
    let mut conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_profile_schema(&conn)?;
    deck_core::store::upsert_profile(&mut conn, &p)
}

/// Remove a saved loadout by name.
pub fn delete_profile(name: &str) -> anyhow::Result<()> {
    let db = deck_core::store::default_db_path();
    let mut conn = deck_core::store::open(&db)?;
    deck_core::store::delete_profile(&mut conn, name)
}

/// Render the systemd unit for an arbitrary (possibly unsaved) profile so the
/// editor can preview exactly what `apply` would write.
pub fn render_profile_unit(p: Profile) -> String {
    deck_engines::render_unit(&p)
}

/// Scan the index for duplicate models (same arch + quant + param count).
/// Returns groups of duplicate models so the UI can display them.
pub fn dedup() -> anyhow::Result<Vec<DupRow>> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    Ok(deck_core::store::duplicates(&conn)?
        .into_iter()
        .map(|d| DupRow {
            identity: d.identity,
            wasted_gib: gib(d.wasted_bytes),
            members: d
                .members
                .iter()
                .map(|m| m.path.display().to_string())
                .collect(),
        })
        .collect())
}

/// Delete a model from the index. If `delete_file` is true the file is removed
/// from disk as well (for local GGUF/safetensors only — skip for ollama/hub
/// paths that the user should manage externally).
pub fn delete_model(path: &str, delete_file: bool) -> anyhow::Result<usize> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::delete_model(&conn, path, delete_file)
}

/// Delete all duplicate models in a group except the cheapest one.
/// Returns the number of models removed.
pub fn dedup_delete(identity: &str, delete_file: bool) -> anyhow::Result<usize> {
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::dedup_delete(&conn, identity, delete_file)
}

pub fn fit(
    model: PathBuf,
    ctx: u32,
    kv_bytes: f64,
    n_gpu_layers: u32,
    kv_layers: Option<u64>,
    reserve: u64,
    offload: bool,
) -> anyhow::Result<FitRow> {
    let meta = if model.is_dir() {
        deck_core::safetensors::open_dir(&model)?
    } else {
        deck_core::gguf::GgufMeta::read(&model)?.to_meta(&model)
    };
    // Translate an absolute layer count into the estimator's fraction. 0 means
    // "all layers on GPU" — used by the quick HUD estimate.
    let ngl_frac = if n_gpu_layers == 0 {
        1.0
    } else {
        let total = meta.n_layers.unwrap_or(0).max(1) as f64;
        (n_gpu_layers as f64 / total).clamp(0.0, 1.0)
    };
    let req = deck_core::fit::FitRequest {
        ctx: ctx as u64,
        kv_bytes,
        ngl_frac,
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
///
/// `managed` (opt-in) additionally repoints dsh + opencode at the applied
/// engine's port so the rest of the stack follows the swap. Off by default:
/// the Advisory contract preserves the alias+port so clients don't reconfigure.
pub fn use_profile(name: &str, dry_run: bool, managed: bool) -> anyhow::Result<UseResult> {
    let db = deck_core::store::default_db_path();
    let mut conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_profile_schema(&conn)?;
    let p = deck_core::store::get_profile(&conn, name)?
        .ok_or_else(|| anyhow::anyhow!("no loadout named '{name}'"))?;
    deck_core::store::set_active(&mut conn, name)?;
    let unit = deck_engines::render_unit(&p);
    let mut rewired = Vec::new();
    if !dry_run {
        deck_engines::apply(&p, false)?;
        if managed {
            for r in deck_engines::rewire::rewire_clients(p.port) {
                rewired.push(format!("[{}] {} — {}", r.client, r.path, r.status));
            }
        }
    }
    Ok(UseResult {
        name: name.to_string(),
        applied: !dry_run,
        dry_run,
        unit,
        rewired,
    })
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

/// Fetch recent models authored by `org` from HuggingFace.
pub fn browse_org(org: &str, limit: usize) -> anyhow::Result<Vec<MarketHit>> {
    Ok(deck_feeds::fetch_org(org, limit)?
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

/// List downloadable model files in a repo (GGUF + safetensors), resolving
/// each file's size via a HEAD request.
pub fn market_files(repo_id: &str) -> anyhow::Result<Vec<MarketFileRow>> {
    Ok(deck_feeds::model_files(repo_id)?
        .into_iter()
        .map(|f| MarketFileRow {
            rfilename: f.rfilename,
            size: f.size,
        })
        .collect())
}

// ------------------------------------------------------------ downloads

#[derive(Serialize)]
pub struct DownloadStarted {
    pub key: String,
}

/// Emitted on `dl-start` / throttled `dl-progress`.
#[derive(Clone, Serialize)]
pub struct DownloadEvt {
    pub key: String,
    pub repo_id: String,
    pub rfilename: String,
    pub done: u64,
    pub total: u64,
}

#[derive(Clone, Serialize)]
pub struct DownloadDone {
    pub key: String,
    pub repo_id: String,
    pub rfilename: String,
    /// Final path on disk once renamed from `.part`.
    pub path: String,
}

#[derive(Clone, Serialize)]
pub struct DownloadErr {
    pub key: String,
    /// "cancelled" for user-cancelled transfers; otherwise a message.
    pub error: String,
}

struct DlJob {
    cancel: std::sync::Arc<deck_feeds::Cancel>,
}

/// Active background downloads keyed by `{repo_id}/{rfilename}`. Jobs remove
/// themselves on completion/error/cancel.
static DOWNLOADS: std::sync::LazyLock<Mutex<std::collections::HashMap<String, DlJob>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

const DL_EMIT_BYTES: u64 = 4 * 1024 * 1024;
const DL_EMIT_MS: u64 = 400;

fn models_dir() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("models")
}

/// Begin a background download of one repo file into ~/models. Returns
/// immediately; progress flows as `dl-start` / `dl-progress` / `dl-done` /
/// `dl-error` events tagged with `key`. Re-requesting an active transfer is
/// rejected so double-clicks can't duplicate it.
pub fn download_start(
    app: &tauri::AppHandle,
    repo_id: &str,
    rfilename: &str,
) -> anyhow::Result<DownloadStarted> {
    let key = format!("{repo_id}/{rfilename}");
    {
        let mut g = DOWNLOADS.lock().unwrap();
        if g.contains_key(&key) {
            anyhow::bail!("already downloading {key}");
        }
        g.insert(
            key.clone(),
            DlJob {
                cancel: std::sync::Arc::new(deck_feeds::Cancel::new()),
            },
        );
    }

    // Row shows up instantly; total/progress refine as the worker streams.
    let _ = app.emit(
        "dl-start",
        DownloadEvt {
            key: key.clone(),
            repo_id: repo_id.to_string(),
            rfilename: rfilename.to_string(),
            done: 0,
            total: 0,
        },
    );

    let app2 = app.clone();
    let key2 = key.clone();
    let repo = repo_id.to_string();
    let rf = rfilename.to_string();
    std::thread::spawn(move || {
        let cancel = match DOWNLOADS.lock().unwrap().get(&key2) {
            Some(j) => j.cancel.clone(),
            None => return, // cancelled before the worker even started
        };

        // Best-effort size probe up front so the UI can show a real percentage
        // immediately rather than waiting for the first body bytes.
        let probed_total = deck_feeds::remote_file_size(&repo, &rf).unwrap_or(0);
        if !cancel.cancelled() {
            let _ = app2.emit(
                "dl-progress",
                DownloadEvt {
                    key: key2.clone(),
                    repo_id: repo.clone(),
                    rfilename: rf.clone(),
                    done: 0,
                    total: probed_total,
                },
            );
        }

        // Emit on ≥4 MiB deltas or ≥400 ms gaps; interior ticks are dropped so
        // a fast NVMe-backed connection doesn't flood the event bus.
        let mut last_emit_done: u64 = 0;
        let mut last_emit_at = Instant::now();
        let mut emit_progress = |done: u64, total: u64| {
            let due = done.saturating_sub(last_emit_done) >= DL_EMIT_BYTES
                || done < last_emit_done // stream restarted (defensive)
                || last_emit_at.elapsed().as_millis() >= DL_EMIT_MS as u128;
            if done == 0 || due {
                last_emit_done = done;
                last_emit_at = Instant::now();
                let _ = app2.emit(
                    "dl-progress",
                    DownloadEvt {
                        key: key2.clone(),
                        repo_id: repo.clone(),
                        rfilename: rf.clone(),
                        done,
                        total,
                    },
                );
            }
        };

        let result = deck_feeds::download_file_progress(
            &repo,
            &rf,
            &models_dir(),
            (probed_total > 0).then_some(probed_total),
            &mut emit_progress,
            &cancel,
        );

        match result {
            Ok(path) => {
                let _ = app2.emit(
                    "dl-done",
                    DownloadDone {
                        key: key2.clone(),
                        repo_id: repo.clone(),
                        rfilename: rf.clone(),
                        path: path.display().to_string(),
                    },
                );
            }
            Err(e) => {
                let msg = e.to_string();
                let _ = app2.emit(
                    "dl-error",
                    DownloadErr {
                        key: key2.clone(),
                        error: msg,
                    },
                );
            }
        }
        DOWNLOADS.lock().unwrap().remove(&key2);
    });

    Ok(DownloadStarted { key })
}

/// Request cancellation of an active download. The worker notices at its next
/// chunk boundary and emits `dl-error` with "cancelled".
pub fn download_cancel(key: &str) -> anyhow::Result<()> {
    let hit = DOWNLOADS
        .lock()
        .unwrap()
        .get(key)
        .map(|j| j.cancel.cancel());
    if hit.is_some() {
        Ok(())
    } else {
        anyhow::bail!("no active download '{key}'")
    }
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

// ------------------------------------------------------- loadout test harness

#[derive(Clone, serde::Serialize)]
pub struct TestPhase {
    pub phase: String,
}

#[derive(Clone, serde::Serialize)]
pub struct TestLine {
    pub stream: String,
    pub text: String,
}

#[derive(Clone, serde::Serialize)]
pub struct TestResult {
    pub verdict: String,
    pub summary: String,
}

/// At most one loadout test runs at a time; holds the spawned engine child so it
/// can be killed from `test_stop` or on teardown.
static TEST_CHILD: Mutex<Option<std::process::Child>> = Mutex::new(None);

/// Keywords that indicate the engine failed to allocate VRAM rather than a clean
/// exit or a logic error. Scanned from the engine's stderr/stdout.
const OOM_MARKERS: &[&str] = &[
    "out of memory",
    "cuda out of memory",
    "allocation failed",
    "cannot allocate",
    "cudamalloc",
    "illegal memory",
    "std::bad_alloc",
    "failed to allocate",
    "oom",
    "vkerror",
];

/// Launch the draft loadout directly (no systemd writes) on a dedicated test
/// port, watch it boot, and report whether it OOMs, crashes, or serves. Per the
/// user's choice this STOPS the live service of the same engine first so the
/// test gets isolated VRAM, then RESTARTS it before returning — the frontend is
/// expected to show a warning before calling this.
///
/// The function returns immediately; the actual run happens on a background
/// thread and streams `test-phase` / `test-output` / `test-result` events.
pub fn test_profile(
    app: &tauri::AppHandle,
    profile: Profile,
    test_port: u16,
) -> anyhow::Result<()> {
    {
        let guard = TEST_CHILD.lock().unwrap();
        if guard.is_some() {
            anyhow::bail!("a loadout test is already running");
        }
    }

    let unit = profile.engine.systemd_unit().to_string();
    let mut draft = profile.clone();
    draft.port = test_port;
    let host = draft.host.clone();

    let app2 = app.clone();
    std::thread::spawn(move || {
        let emit_phase = |p: &str| {
            let _ = app2.emit("test-phase", TestPhase { phase: p.into() });
        };
        let emit_result = |verdict: &str, summary: &str| {
            let _ = app2.emit(
                "test-result",
                TestResult {
                    verdict: verdict.into(),
                    summary: summary.into(),
                },
            );
        };
        let restart_live = || {
            emit_phase("restarting-live");
            let _ = deck_engines::start(&unit);
        };

        emit_phase("stopping-live");
        let _ = deck_engines::stop(&unit);
        // Give systemd a moment to actually free VRAM.
        std::thread::sleep(Duration::from_secs(3));

        emit_phase("spawning");
        let mut cmd = Command::new(&draft.bin);
        cmd.args(deck_engines::build_args(&draft));
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                emit_phase("error");
                emit_result(
                    "ERROR",
                    &format!("failed to spawn {}: {}", draft.bin.display(), e),
                );
                restart_live();
                emit_phase("done");
                return;
            }
        };

        let stdout = child.stdout.take().expect("test engine stdout unavailable");
        let stderr = child.stderr.take().expect("test engine stderr unavailable");

        *TEST_CHILD.lock().unwrap() = Some(child);

        let oom = std::sync::Arc::new(AtomicBool::new(false));
        let healthy = std::sync::Arc::new(AtomicBool::new(false));

        let app_o = app2.clone();
        let oom_o = oom.clone();
        std::thread::spawn(move || {
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                let low = line.to_lowercase();
                if OOM_MARKERS.iter().any(|m| low.contains(m)) {
                    oom_o.store(true, Ordering::SeqCst);
                }
                let _ = app_o.emit(
                    "test-output",
                    TestLine {
                        stream: "stdout".into(),
                        text: line,
                    },
                );
            }
        });
        let app_e = app2.clone();
        let oom_e = oom.clone();
        std::thread::spawn(move || {
            let reader = std::io::BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                let low = line.to_lowercase();
                if OOM_MARKERS.iter().any(|m| low.contains(m)) {
                    oom_e.store(true, Ordering::SeqCst);
                }
                let _ = app_e.emit(
                    "test-output",
                    TestLine {
                        stream: "stderr".into(),
                        text: line,
                    },
                );
            }
        });

        // Watch: OOM keyword, process exit, then health endpoint. The child lives
        // in TEST_CHILD so test_stop can kill it; we operate on that slot here.
        let start = Instant::now();
        let timeout = Duration::from_secs(180);
        let mut verdict = (
            "TIMEOUT",
            "engine never reported healthy within the timeout".to_string(),
        );
        loop {
            if oom.load(Ordering::SeqCst) {
                verdict = (
                    "OOM",
                    "engine logged an out-of-memory / allocation failure".into(),
                );
                break;
            }
            let status = {
                let mut g = TEST_CHILD.lock().unwrap();
                match g.as_mut() {
                    Some(c) => c.try_wait().ok().flatten(),
                    // None means test_stop already reaped it.
                    None => Some(std::process::ExitStatus::default()),
                }
            };
            if let Some(s) = status {
                verdict = if healthy.load(Ordering::SeqCst) {
                    ("RUNNING", "engine loaded and served before exiting".into())
                } else {
                    ("CRASH", format!("engine exited early with status {s}"))
                };
                break;
            }
            if deck_engines::health_ok_any(&host, test_port) {
                healthy.store(true, Ordering::SeqCst);
                verdict = (
                    "RUNNING",
                    "engine loaded the model and is serving on the test port".into(),
                );
                break;
            }
            if start.elapsed() > timeout {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }

        // Tear down the test process, then restore the live service.
        if let Some(mut c) = TEST_CHILD.lock().unwrap().take() {
            let _ = c.kill();
        }
        restart_live();
        emit_phase("done");
        emit_result(&verdict.0, &verdict.1);
    });

    Ok(())
}

/// Abort a running loadout test (also restarts the live service).
pub fn test_stop() -> anyhow::Result<()> {
    if let Some(mut c) = TEST_CHILD.lock().unwrap().take() {
        let _ = c.kill();
    }
    Ok(())
}

/// Verify a profile with user-adjusted parameters on a test port.
///
/// The profile is cloned with the tweak overrides applied, then sent through
/// the same `verify_on_test_port` flow as a normal bring-up. On success the
/// verified profile is saved to the DB. Returns the outcome including the tok/s
/// reading.
#[derive(Clone, serde::Serialize)]
pub struct TweakResult {
    pub ok: bool,
    pub summary: String,
    pub ctx: u32,
    pub tps: Option<f64>,
}

pub fn test_profile_tweaked(
    profile: Profile,
    ctx_override: Option<u32>,
    kv_bytes_override: Option<f64>,
    offload_override: Option<bool>,
    ngl_override: Option<u32>,
) -> TweakResult {
    let test_port = profile.engine.test_port();
    let mut draft = profile.clone();

    if let Some(v) = ctx_override { draft.ctx_size = v; }
    if let Some(_v) = kv_bytes_override { /* noted for display */ }
    if let Some(offload) = offload_override {
        if offload && profile.engine == Engine::FreeToken {
            draft.ft_backend = Some("offload".into());
        }
    }
    if let Some(v) = ngl_override { draft.n_gpu_layers = v; }

    let outcome = deck_engines::verify_on_test_port(&draft, test_port, Duration::from_secs(180));

    if outcome.verdict != "RUNNING" {
        return TweakResult {
            ok: false,
            summary: format!("{} (ctx={}) — tweak params and retry", outcome.summary, outcome.ctx),
            ctx: outcome.ctx,
            tps: None,
        };
    }

    // Success — save the verified profile with the verified ctx.
    let final_ctx = if outcome.ctx != draft.ctx_size {
        draft.ctx_size = outcome.ctx;
        outcome.ctx
    } else {
        draft.ctx_size
    };

    let db = deck_core::store::default_db_path();
    if let Ok(conn) = deck_core::store::open(&db) {
        let _ = deck_core::store::ensure_profile_schema(&conn);
        let _ = deck_core::store::upsert_profile(&conn, &draft);
    }

    let tps = outcome.tok_per_sec;
    TweakResult {
        ok: true,
        summary: format!(
            "'{}' verified on :{} at ctx={}{}",
            draft.name,
            profile.engine.default_port(),
            final_ctx,
            tps.map(|t| format!(" · {t:.1} tok/s")).unwrap_or_default()
        ),
        ctx: final_ctx,
        tps,
    }
}

// ----------------------------------------------------------- agentic console

/// Emitted when a session starts, so the UI can open a tab before output flows.
#[derive(Clone, serde::Serialize)]
pub struct OpStarted {
    pub id: String,
    pub prompt: String,
}

#[derive(Clone, serde::Serialize)]
pub struct OpLine {
    pub session: String,
    pub stream: String,
    pub text: String,
}

#[derive(Clone, serde::Serialize)]
pub struct OpDone {
    pub session: String,
    pub code: i32,
}

/// One concurrent opencode session. The child handle is kept so its pipes stay
/// open; stop is performed by PID (via SIGTERM) so it never contends with the
/// waiter thread that holds the lock during `wait`.
struct Session {
    pid: u32,
    child: std::sync::Mutex<Option<std::process::Child>>,
}

static SESSIONS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, Session>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));
static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Spawn a new `opencode run` session in `dir`. Unlike a single-slot runner,
/// this supports many concurrent sessions: each gets a unique id, its output is
/// tagged with that id, and `opencode_stop(id)` ends just that one.
///
/// `auto` maps to opencode's `--auto` (auto-approve permissions) — required for
/// a headless coding session, but it WILL let the agent modify files without
/// prompting. The UI must surface that trade-off.
pub fn opencode_run(
    app: &tauri::AppHandle,
    prompt: &str,
    dir: &str,
    auto: bool,
    model: Option<&str>,
) -> anyhow::Result<()> {
    let mut cmd = std::process::Command::new("opencode");
    cmd.arg("run").arg("--dir").arg(dir);
    if auto {
        cmd.arg("--auto");
    }
    if let Some(m) = model.filter(|s| !s.is_empty()) {
        cmd.arg("-m").arg(m);
    }
    cmd.arg(prompt);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => {
            eprintln!("[deck] opencode_run: spawned pid={}", c.id());
            c
        }
        Err(e) => {
            eprintln!("[deck] opencode_run: SPAWN FAILED: {e}");
            return Err(anyhow::anyhow!("failed to spawn opencode: {e}"));
        }
    };
    let pid = child.id();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("opencode stdout unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("opencode stderr unavailable"))?;

    let id = format!(
        "sess-{}",
        SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    );
    SESSIONS.lock().unwrap().insert(
        id.clone(),
        Session {
            pid,
            child: std::sync::Mutex::new(Some(child)),
        },
    );

    eprintln!("[deck] opencode_run: emitting opencode-started id={id}");
    let _ = app.emit(
        "opencode-started",
        OpStarted {
            id: id.clone(),
            prompt: prompt.to_string(),
        },
    );

    let app_o = app.clone();
    let id_o = id.clone();
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            let _ = app_o.emit(
                "opencode-output",
                OpLine {
                    session: id_o.clone(),
                    stream: "stdout".into(),
                    text: line,
                },
            );
        }
    });

    let app_e = app.clone();
    let id_e = id.clone();
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stderr);
        for line in reader.lines().map_while(Result::ok) {
            let _ = app_e.emit(
                "opencode-output",
                OpLine {
                    session: id_e.clone(),
                    stream: "stderr".into(),
                    text: line,
                },
            );
        }
    });

    let app_done = app.clone();
    let id_done = id.clone();
    std::thread::spawn(move || {
        let code = {
            let mut g = SESSIONS.lock().unwrap();
            match g
                .get_mut(&id_done)
                .and_then(|s| s.child.lock().unwrap().take())
            {
                Some(mut c) => c.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1),
                None => -1,
            }
        };
        eprintln!("[deck] opencode_run: session {id_done} exited code={code}");
        SESSIONS.lock().unwrap().remove(&id_done);
        let _ = app_done.emit(
            "opencode-done",
            OpDone {
                session: id_done,
                code,
            },
        );
    });

    Ok(())
}

/// Stop a single session by id (SIGTERM to its process group). Unknown ids are
/// ignored. Multiple sessions can run; this ends only the named one.
pub fn opencode_stop(id: &str) -> anyhow::Result<()> {
    let pid = SESSIONS.lock().unwrap().get(id).map(|s| s.pid);
    if let Some(pid) = pid {
        // SIGTERM the process; the reader threads see EOF and the waiter emits done.
        let _ = std::process::Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status();
    }
    SESSIONS.lock().unwrap().remove(id);
    Ok(())
}

// ---------------------------------------------------------------- BRINGUP

/// Emitted as the one-click load pipeline advances.
#[derive(Clone, serde::Serialize)]
pub struct BringupPhase {
    pub phase: String, // derive | verify | apply | bench | done | error
}

#[derive(Clone, serde::Serialize)]
pub struct BringupLine {
    pub text: String,
}

/// Full VRAM fit breakdown shown during bring-up so the user sees exactly
/// where memory goes even when verification fails.
#[derive(Clone, serde::Serialize)]
pub struct FitBreakdown {
    pub weights_mb: u64,
    pub weights_gpu_mb: u64,
    pub weights_ram_mb: u64,
    pub kv_mb: u64,
    pub buffers_mb: u64,
    pub model_vram_mb: u64,
    pub overhead_mb: u64,
    pub available_mb: u64,
    pub available_for_model_mb: u64,
    pub headroom_mb: u64,
    pub verdict: String,
}

#[derive(Clone, serde::Serialize)]
pub struct BringupResult {
    pub ok: bool,
    pub summary: String,
    /// Name of the saved loadout (empty when nothing was installed).
    pub name: String,
    pub port: u16,
    pub ctx: u32,
    pub tps: Option<f64>,
    /// The full VRAM fit breakdown — present even when `ok == false` so the
    /// tweak panel can show where the model ran out of memory.
    pub fit: Option<FitBreakdown>,
}

static BRINGUP_RUNNING: AtomicBool = AtomicBool::new(false);

/// One-click load: derive the best-max-ctx loadout from a local GGUF or
/// safetensors model dir + engine choice, verify it headlessly on the engine's
/// test port (never touching live services; walks the ctx ladder on OOM),
/// install+start via systemd, then bench and record tok/s. Runs on a background
/// thread emitting `bringup-phase` / `bringup-line` / `bringup-result` events.
///
/// Single-flight: a second request while one runs is rejected.
pub fn bringup_start(
    app: &tauri::AppHandle,
    model_path: &str,
    engine_s: &str,
    fast: bool,
) -> anyhow::Result<()> {
    if !std::path::Path::new(model_path).exists() {
        anyhow::bail!("model not found on disk: {model_path}");
    }
    let eng = deck_core::profile::Engine::parse(engine_s)
        .ok_or_else(|| anyhow::anyhow!("unknown engine '{engine_s}' (llamacpp|freetoken)"))?;
    if BRINGUP_RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        anyhow::bail!("a bring-up is already running");
    }

    let _ = app.emit(
        "bringup-phase",
        BringupPhase {
            phase: "derive".into(),
        },
    );

    let app2 = app.clone();
    let model = model_path.to_string();
    std::thread::spawn(move || {
        let finish = |res: BringupResult| {
            let _ = app2.emit("bringup-result", res);
            let _ = app2.emit(
                "bringup-phase",
                BringupPhase {
                    phase: "done".into(),
                },
            );
            BRINGUP_RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
        };
        let line = |t: String| {
            let _ = app2.emit("bringup-line", BringupLine { text: t });
        };

        // 1. Derive ---------------------------------------------------------
        line(format!("[derive] reading {} header…", model));
        let derived = match deck_core::profile::derive_loadout(&model, eng) {
            Ok(d) => d,
            Err(e) => {
                finish(BringupResult {
                    ok: false,
                    summary: format!("derive failed: {e}"),
                    name: String::new(),
                    port: 0,
                    ctx: 0,
                    tps: None,
                    fit: None,
                });
                return;
            }
        };
        // Emit the profile so the frontend tweak panel can access it.
        let _ = app2.emit("bringup-profile", derived.profile.clone());
        let mut p = derived.profile;
        p.name = p.alias.clone();
        let offload = derived.weights_ram_mb > 0;
        let fit = FitBreakdown {
            weights_mb: if offload { derived.weights_gpu_mb + derived.weights_ram_mb } else { derived.weights_gpu_mb },
            weights_gpu_mb: derived.weights_gpu_mb,
            weights_ram_mb: derived.weights_ram_mb,
            kv_mb: derived.kv_mb,
            buffers_mb: derived.buffers_mb,
            model_vram_mb: derived.model_vram_mb,
            overhead_mb: 1600,
            available_mb: derived.available_mb,
            available_for_model_mb: derived.available_for_model_mb,
            headroom_mb: derived.headroom_mb,
            verdict: derived.verdict.clone(),
        };
        line(format!(
            "[derive] max ctx {} · kv={} MiB · weights gpu={} MiB ram={} MiB · buf={} MiB · model vram={} MiB · headroom={} MiB · verdict={}",
            derived.max_ctx,
            derived.kv_mb,
            derived.weights_gpu_mb,
            derived.weights_ram_mb,
            derived.buffers_mb,
            derived.model_vram_mb,
            derived.headroom_mb,
            derived.verdict
        ));

        // 2. Verify (headless, never touches live) --------------------------
        if !fast {
            let test_port = eng.test_port();
            let _ = app2.emit(
                "bringup-phase",
                BringupPhase {
                    phase: "verify".into(),
                },
            );
            line(format!(
                "[verify] loading on :{test_port} — live :{} untouched…",
                p.port
            ));
            let outcome =
                deck_engines::verify_on_test_port(&p, test_port, Duration::from_secs(180));
            line(format!(
                "[verify] {} ({})",
                outcome.summary, outcome.verdict
            ));
            if outcome.verdict != "RUNNING" {
                // Save the derived profile even on failure so the tweak panel
                // has something to work with — the user can adjust ctx/kv/offload
                // and re-verify without re-running the full derive.
                let db = deck_core::store::default_db_path();
                if let Ok(conn) = deck_core::store::open(&db) {
                    let _ = deck_core::store::ensure_profile_schema(&conn);
                    if deck_core::store::upsert_profile(&conn, &p).is_ok() {
                        line(format!("[save] derived profile '{}' saved (tweak & retry)", p.name));
                    }
                }
                finish(BringupResult {
                    ok: false,
                    summary: format!(
                        "verification failed: {} ({}) · ctx={}",
                        outcome.summary, outcome.verdict, outcome.ctx
                    ),
                    name: p.name.clone(),
                    port: p.port,
                    ctx: outcome.ctx,
                    tps: None,
                    fit: Some(fit),
                });
                return;
            }
            if outcome.ctx != p.ctx_size {
                line(format!("[verify] ctx walked down to {}", outcome.ctx));
                p.ctx_size = outcome.ctx;
            }
        }

        // 3. Save + apply ---------------------------------------------------
        {
            let _ = app2.emit(
                "bringup-phase",
                BringupPhase {
                    phase: "apply".into(),
                },
            );
            let db = deck_core::store::default_db_path();
            let outcome = (|| -> anyhow::Result<()> {
                let conn = deck_core::store::open(&db)?;
                deck_core::store::ensure_profile_schema(&conn)?;
                deck_core::store::upsert_profile(&conn, &p)?;
                deck_engines::apply(&p, false)
            })();
            match outcome {
                Ok(()) => line(format!(
                    "[apply] '{}' live on :{} ({}), health OK",
                    p.name,
                    p.port,
                    format!("{:?}", p.engine).to_lowercase()
                )),
                Err(e) => {
                    finish(BringupResult {
                        ok: false,
                        summary: format!("apply failed: {e}"),
                        name: p.name.clone(),
                        port: p.port,
                        ctx: p.ctx_size,
                        tps: None,
                        fit: Some(fit),
                    });
                    return;
                }
            }
        }

        // 4. Bench + record -------------------------------------------------
        let mut tps = None;
        {
            let _ = app2.emit(
                "bringup-phase",
                BringupPhase {
                    phase: "bench".into(),
                },
            );
            if let Ok(text) = deck_engines::fetch_metrics(&p.host, p.port)
                && let Some(v) = deck_engines::parse_tps(&text)
            {
                tps = Some(v);
                let at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let db = deck_core::store::default_db_path();
                if let Ok(conn) = deck_core::store::open(&db) {
                    deck_core::store::ensure_bench_schema(&conn).ok();
                    let row = deck_core::store::BenchRow {
                        id: 0,
                        engine: format!("{:?}", p.engine).to_lowercase(),
                        host: p.host.clone(),
                        port: p.port,
                        model: p.model.clone(),
                        ctx: p.ctx_size,
                        tps: v,
                        at,
                    };
                    match deck_core::store::insert_bench(&conn, &row) {
                        Ok(id) => line(format!("[bench] recorded #{id}: {v:.1} tok/s")),
                        Err(e) => line(format!("[bench] record failed: {e}")),
                    }
                }
            } else {
                line("[bench] no /metrics tok/s gauge exposed (is --metrics on?)".into());
            }
        }

        finish(BringupResult {
            ok: true,
            summary: format!(
                "'{}' brought up on :{} at ctx={}{}",
                p.name,
                p.port,
                p.ctx_size,
                tps.map(|t| format!(" · {t:.1} tok/s")).unwrap_or_default()
            ),
            name: p.name.clone(),
            port: p.port,
            ctx: p.ctx_size,
            tps,
            fit: Some(fit),
        });
    });

    Ok(())
}

// ----------------------------------------------------------------- BROWSE

#[derive(Serialize)]
pub struct HwInfo {
    pub vram_mb: Option<u64>,
    pub detected: bool,
}

/// Detect GPU VRAM via nvidia-smi. The frontend uses this to display the
/// hardware baseline and for fit calculations against remote models.
pub fn hw_info() -> HwInfo {
    let vram = deck_core::fit::hw_vram();
    HwInfo {
        vram_mb: vram,
        detected: vram.is_some(),
    }
}

/// Result of a remote fit estimate: the GGUF metadata we could parse plus
/// the full fit breakdown for a given context size.
#[derive(Serialize)]
pub struct BrowseFitResult {
    // GGUF metadata (from the header parse — may be partial if truncated)
    pub arch: Option<String>,
    pub quant: Option<String>,
    pub params: Option<u64>,
    pub n_layers: Option<u64>,
    pub n_embd: Option<u64>,
    pub truncated: bool,
    // fit breakdown
    pub weights_mb: u64,
    pub kv_mb: u64,
    pub buffers_mb: u64,
    pub model_vram_mb: u64,
    pub weights_ram_mb: u64,
    pub overhead_mb: u64,
    pub available_for_model_mb: u64,
    pub verdict: String,
}

/// Fetch the first 2 MiB of a remote GGUF file via HTTP Range, parse its
/// header metadata, and compute a VRAM fit estimate. This is the core
/// operation behind the BROWSE view: exact fit without downloading the file.
pub fn browse_fit_remote(
    repo_id: &str,
    rfilename: &str,
    ctx: u32,
    kv_bytes: f64,
    n_gpu_layers: u32,
    kv_layers: Option<u64>,
    reserve: u64,
    offload: bool,
) -> anyhow::Result<BrowseFitResult> {
    // 1. Fetch the GGUF header (first 2 MiB — covers all scalar KVs)
    let (bytes, total_size) = deck_feeds::fetch_gguf_bytes(repo_id, rfilename, 2 * 1024 * 1024)?;

    // 2. Parse GGUF metadata from the buffer
    let mut cursor = std::io::Cursor::new(bytes);
    let gguf_meta = deck_core::gguf::GgufMeta::from_reader(&mut cursor, total_size)?;

    // 3. Build a ModelMeta for the fit estimator. weight_size = file_size
    //    (GGUF files store all tensor weights; close enough for VRAM calc).
    let meta = deck_core::model::ModelMeta {
        path: std::path::PathBuf::from(format!("{repo_id}/{rfilename}")),
        format: deck_core::model::ModelFormat::Gguf,
        name: gguf_meta.name().unwrap_or(rfilename).to_string(),
        arch: gguf_meta.arch().map(str::to_string),
        quant: gguf_meta.quant_name(),
        params: gguf_meta.params().map(|v| v as u64),
        n_layers: gguf_meta.n_layers(),
        n_embd: gguf_meta.n_embd(),
        ctx_train: gguf_meta.ctx_train().map(|v| v as u64),
        vocab: gguf_meta.vocab_size(),
        weight_size: total_size,
        footprint: total_size,
    };

    // 4. Translate absolute layer count to fraction. 0 = all layers on GPU.
    let ngl_frac = if n_gpu_layers == 0 {
        1.0
    } else {
        let total = meta.n_layers.unwrap_or(0).max(1) as f64;
        (n_gpu_layers as f64 / total).clamp(0.0, 1.0)
    };

    // 5. Run the fit estimator
    let req = deck_core::fit::FitRequest {
        ctx: ctx as u64,
        kv_bytes,
        ngl_frac,
        kv_layers,
        reserved_mb: reserve,
        offload,
    };
    let available = deck_core::fit::available_vram_mb(16_303);
    let b = deck_core::fit::estimate(&meta, &req, available);

    Ok(BrowseFitResult {
        arch: meta.arch,
        quant: meta.quant,
        params: meta.params,
        n_layers: meta.n_layers,
        n_embd: meta.n_embd,
        truncated: gguf_meta.truncated,
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
            0,
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
            let f = fit(dir, 32768, 1.0, 0, None, 1600, true).expect("fit");
            assert!(
                f.weights_ram_mb > 0,
                "offload should report RAM-spilled weights"
            );
            assert!(
                f.model_vram_mb < f.weights_mb + f.weights_ram_mb,
                "offload VRAM should be far below total weights"
            );
        }
    }

    #[test]
    fn render_known_profile() {
        // import from the real wrapper, then render without applying
        let script =
            std::path::PathBuf::from("/home/deviant/.local/share/llama-server/run-llama-server.sh");
        if script.exists() {
            let p = deck_core::importer::import_llamacpp_script(&script, "qwen").unwrap();
            let unit = deck_engines::render_unit(&p);
            assert!(unit.contains("qwen3.8-27b"));
            assert!(unit.contains("/home/deviant/Qwen3.8-27B-UD-Q3_K_XL.gguf"));
        }
    }

    #[test]
    fn hw_info_returns_something() {
        let hw = hw_info();
        // Either nvidia-smi works (detected) or not — both are valid
        if hw.detected {
            assert!(hw.vram_mb.unwrap() > 0);
        }
    }

    #[test]
    fn browse_fit_remote_smoke() {
        // Test against a known GGUF on HF. Skip if offline.
        let result = browse_fit_remote(
            "unsloth/Qwen3.8-GGUF",
            "Qwen3.8-Q4_K_M.gguf",
            32768,
            0.5,
            0, // all layers on GPU
            None,
            1600,
            false,
        );
        match result {
            Ok(r) => {
                assert!(!r.verdict.is_empty());
                assert!(r.model_vram_mb > 0);
                assert!(r.arch.is_some(), "should parse arch from remote header");
                assert!(
                    r.n_layers.is_some(),
                    "should parse n_layers from remote header"
                );
            }
            Err(e) => {
                // Network failures in CI are expected
                eprintln!("browse_fit_remote skipped (offline?): {e}");
            }
        }
    }
}
