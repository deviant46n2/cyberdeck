//! `deck downloads`: the CLI door over the shared download manager.
//!
//! Mirrors the DOWNLOADS tab's contract with the queue authority in
//! `deck-feeds` — bounded concurrency (`MAX_ACTIVE`), `.part` resume points,
//! and set-aware indexing (a multi-part GGUF enters the vault only once every
//! member has landed). One truth, two doors.

use anyhow::Result;
use std::sync::Arc;
use std::time::{Duration, Instant};

use deck_core::store::models_dir;
use deck_feeds::{DlEvent, DlStatus, DownloadManager};

/// `deck downloads run <repo>`: queue a repo's picked .gguf (and its shard
/// set), stream progress, index every completed set, then exit.
pub fn run(repo: &str, file: Option<&str>, quant: Option<&str>, no_index: bool) -> Result<()> {
    let files = deck_feeds::model_files(repo)?;
    let ggufs: Vec<deck_feeds::MarketFile> = files
        .iter()
        .filter(|f| f.rfilename.ends_with(".gguf"))
        .cloned()
        .collect();
    if ggufs.is_empty() {
        anyhow::bail!("no .gguf files found in {repo}");
    }

    let picked = pick(file, quant, &ggufs)
        .ok_or_else(|| anyhow::anyhow!("no matching .gguf in {repo}"))?;
    let all: Vec<String> = ggufs.iter().map(|f| f.rfilename.clone()).collect();
    let set = deck_feeds::shard_set_of(&picked.rfilename, &all);

    let mgr = DownloadManager::real();
    let (tx, rx) = std::sync::mpsc::channel::<DlEvent>();
    mgr.set_sink(Arc::new(move |e| {
        let _ = tx.send(e.clone());
    }));

    let mut landed: Vec<String> = Vec::new();
    eprintln!(
        "queueing {} file(s) from {repo} under {} workers (dir {})",
        set.len(),
        deck_feeds::MAX_ACTIVE,
        models_dir().display()
    );
    mgr.enqueue_all(repo, &set)?;

    let mut last_emit = std::collections::HashMap::new();
    let start = Instant::now();
    while mgr
        .list()
        .iter()
        .any(|j| matches!(j.status, DlStatus::Queued | DlStatus::Active | DlStatus::Paused))
    {
        while let Ok(e) = rx.try_recv() {
            match e {
                DlEvent::Started { key, .. } => eprintln!("→ {key}"),
                DlEvent::Progress { key, done, total } => {
                    if done != 0 || total != 0 {
                        let last = last_emit.get(&key).copied().unwrap_or(0);
                        if done.saturating_sub(last) >= 4_000_000 || done == 0 {
                            *last_emit.entry(key.clone()).or_insert(0) = done;
                            let pct = if total > 0 {
                                (done as f64 / total as f64 * 100.0) as u64
                            } else {
                                0
                            };
                            eprintln!("  {key}: {done}B / {total}B ({pct}%)");
                        }
                    }
                }
                DlEvent::Done { key, path } => {
                    eprintln!("✓ {key} → {}", path.display());
                    landed.push(path.display().to_string());
                }
                DlEvent::Error { key, error } => {
                    eprintln!("✗ {key}: {error}");
                }
            }
        }
        std::thread::sleep(Duration::from_millis(150));
        if start.elapsed().as_secs() > 600 {
            anyhow::bail!("downloads did not drain within 10 minutes");
        }
    }

    if landed.is_empty() && !set.is_empty() {
        // All members errored/cancelled — nothing landed, nothing to index.
        eprintln!("no files landed");
        return Ok(());
    }

    if !no_index {
        let n = index_landed(&landed)?;
        eprintln!("indexed {n} model(s) into the vault");
    } else {
        eprintln!("skip indexing (--no-index) — files are in {}", models_dir().display());
    }
    Ok(())
}

/// `deck downloads list`: parked `.part` resume points in `~/models`. These
/// are STOP's durable surface — a `.part` is a parked resume point, never
/// indexed, and lives across CLI processes (the queue itself is in-memory).
pub fn list(json: bool) -> Result<()> {
    let dir = models_dir();
    let mut parts: Vec<(u64, std::path::PathBuf)> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".part"))
        .filter_map(|e| {
            let p = e.path();
            p.metadata().ok().map(|m| (m.len(), p))
        })
        .collect();
    parts.sort_by_key(|(_, p)| p.file_name().unwrap_or_default().to_string_lossy().into_owned());

    if json {
        let rows: Vec<serde_json::Value> = parts
            .iter()
            .map(|(size, p)| {
                serde_json::json!({
                    "name": p.file_name().unwrap_or_default().to_string_lossy(),
                    "size": size,
                    "path": p.display().to_string(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    if parts.is_empty() {
        println!("no parked downloads (no *.part files in {})", dir.display());
        return Ok(());
    }
    for (size, p) in &parts {
        let units = if *size >= 1_048_576 {
            format!("{:.1} MiB", *size as f64 / 1_048_576.0)
        } else {
            format!("{size} B")
        };
        println!("{:<32}  {:>10}", p.file_name().unwrap_or_default().to_string_lossy(), units);
    }
    Ok(())
}

/// `deck downloads discard <name>`: drop a parked `.part` (REMOVE's durable
/// side). The file name must be the bare `name.part` or `name`.
pub fn discard(name: &str) -> Result<()> {
    let dir = models_dir();
    let target = if name.ends_with(".part") {
        dir.join(name)
    } else {
        dir.join(format!("{name}.part"))
    };
    if !target.exists() {
        anyhow::bail!("no parked download '{}' in {}", name, dir.display());
    }
    std::fs::remove_file(&target)?;
    println!("discarded {}", target.display());
    Ok(())
}

/// Index a completed set's landed paths into the vault (mirrors the app's
/// `index_downloaded`: scan_paths → upsert_many, never prune).
fn index_landed(paths: &[String]) -> Result<usize> {
    let paths: Vec<&std::path::Path> = paths.iter().map(std::path::Path::new).collect();
    let models = deck_core::scanner::scan_paths(&paths)?;
    let db = deck_core::store::default_db_path();
    let mut conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_profile_schema(&conn)?;
    deck_core::store::upsert_many(&mut conn, &models)
}

/// Pick the file to resolve a shard set from, matching `deck download`'s rule:
/// explicit name/suffix wins; else quant token within the name; else largest.
fn pick<'a>(file: Option<&'a str>, quant: Option<&'a str>, ggufs: &'a [deck_feeds::MarketFile]) -> Option<&'a deck_feeds::MarketFile> {
    let mut iter = ggufs.iter();
    match (file, quant) {
        (Some(f), _) => iter.find(|m| m.rfilename == f || m.rfilename.ends_with(f)),
        (None, Some(q)) => iter.filter(|m| m.rfilename.contains(q)).max_by_key(|m| m.size.unwrap_or(0)),
        (None, None) => iter.max_by_key(|m| m.size.unwrap_or(0)),
    }
}