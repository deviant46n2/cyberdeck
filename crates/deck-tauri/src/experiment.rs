//! One-click EXPERIMENT pipeline (Phase 6 first slice): download → derive →
//! verify → [apply → bench], started straight from a MARKET repo file.
//!
//! Reuses the bringup machinery (single-flight lock, `derive_and_verify`,
//! `save_and_apply`/`bench_and_record`) and streams the SAME `bringup-*`
//! events, so the frontend drawer renders the whole flow — including the new
//! `download` phase — with no extra event surface. Downloads ride the shared
//! `DownloadManager` (dl-* events keep the DOWNLOADS tab live) and are
//! skipped per-member when the file is already on disk.

use std::collections::HashMap;
use std::time::Duration;

use tauri::Emitter;

use crate::bringup::{self, BringupLine, BringupPhase, BringupResult};
use crate::downloads::{download_start, download_states};
use crate::Engine;

/// A shard set can take hours on a slow link; the poll loop only ever waits
/// for manager terminal states, so this bound just stops zombie threads.
const DOWNLOAD_WAIT: Duration = Duration::from_secs(6 * 60 * 60);
const POLL: Duration = Duration::from_millis(1000);

/// Resolve the shard set for `rfilename` within `repo` (a multi-part GGUF
/// returns every member in order; a single file returns itself).
fn shard_set(repo: &str, rfilename: &str) -> anyhow::Result<Vec<String>> {
    let files = deck_feeds::model_files(repo)?;
    let all: Vec<String> = files.iter().map(|f| f.rfilename.clone()).collect();
    if !all.iter().any(|f| f == rfilename) {
        anyhow::bail!("'{rfilename}' not found in {repo}");
    }
    Ok(deck_feeds::shard_set_of(rfilename, &all))
}

/// One-click TEST/LOAD from a repo file: queue the shard set through the
/// shared manager (already-landed members are skipped), wait for every member
/// to land, then run the bringup pipeline on the first shard. `apply=false`
/// stops after headless verification (TEST, records a bench row tagged by the
/// test port); `apply=true` installs + starts + benches (full LOAD).
///
/// Single-flight: shares BRINGUP_RUNNING with bringup/test. Runs on a
/// background thread emitting `bringup-phase` / `bringup-line` /
/// `bringup-result` events.
pub fn experiment_start(
    app: &tauri::AppHandle,
    repo_id: &str,
    rfilename: &str,
    engine_s: &str,
    apply: bool,
) -> anyhow::Result<()> {
    let eng = Engine::parse(engine_s)
        .ok_or_else(|| anyhow::anyhow!("unknown engine '{engine_s}' (llamacpp|freetoken)"))?;
    // Resolve the set up front so a bad repo/file fails fast, inline.
    let set = shard_set(repo_id, rfilename)?;
    if !bringup::try_acquire() {
        anyhow::bail!("a bring-up, test, or experiment is already running");
    }

    let _ = app.emit("bringup-phase", BringupPhase { phase: "download".into() });

    let app2 = app.clone();
    let repo = repo_id.to_string();
    std::thread::spawn(move || {
        let finish = |res: BringupResult| {
            let _ = app2.emit("bringup-result", res);
            let _ = app2.emit("bringup-phase", BringupPhase { phase: "done".into() });
            bringup::bringup_reset();
        };
        let line = |t: String| {
            let _ = app2.emit("bringup-line", BringupLine { text: t });
        };

        // 1. Land every set member (shared queue; disk hits skip the wire) --
        match land_set(&app2, &repo, &set, &line) {
            Ok(paths) => {
                // Multi-part GGUFs load from the first shard; single files are
                // themselves.
                let model = paths[0].display().to_string();
                run_after_download(&app2, &model, eng, apply, &line, &finish);
            }
            Err(e) => {
                finish(BringupResult {
                    ok: false,
                    summary: format!("download failed: {e}"),
                    name: String::new(),
                    port: 0,
                    ctx: 0,
                    tps: None,
                    fit: None,
                });
            }
        }
    });

    Ok(())
}

/// Queue the set and wait until every member's final file exists. Members
/// already on disk (final name present) skip the wire — the manager is
/// in-memory, so a model downloaded in a previous session has no row. A
/// paused row is reported once and keeps waiting (resume continues the same
/// experiment); an errored or discarded row fails the experiment.
fn land_set(
    app: &tauri::AppHandle,
    repo: &str,
    set: &[String],
    line: &impl Fn(String),
) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let models_dir = deck_core::store::models_dir();
    let file_name = |member: &str| {
        std::path::Path::new(member)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| member.to_string())
    };

    let mut pending: Vec<String> = Vec::new();
    for member in set {
        if models_dir.join(file_name(member)).exists() {
            line(format!("[download] {} already on disk", file_name(member)));
        } else {
            pending.push(member.clone());
        }
    }
    if set.len() > 1 {
        line(format!(
            "[download] {}-part set · {} to fetch (watch DOWNLOADS)",
            set.len(),
            pending.len()
        ));
    } else if let Some(m) = pending.first() {
        line(format!("[download] fetching {m} (watch DOWNLOADS)"));
    }
    let keys: Vec<String> = pending.iter().map(|m| format!("{repo}/{m}")).collect();
    for member in &pending {
        download_start(app, repo, member)?;
    }

    let deadline = std::time::Instant::now() + DOWNLOAD_WAIT;
    let mut announced_pause = false;
    loop {
        let states = download_states(&keys);
        let mut landed: HashMap<String, std::path::PathBuf> = HashMap::new();
        let mut waiting = false;
        for (key, status, path, error) in &states {
            match status {
                deck_feeds::DlStatus::Done => match path {
                    Some(p) => {
                        landed.insert(key.clone(), p.clone());
                    }
                    None => anyhow::bail!("{key}: done without a path"),
                },
                deck_feeds::DlStatus::Error => {
                    anyhow::bail!(
                        "{key}: {}",
                        error.clone().unwrap_or_else(|| "stream failed".into())
                    );
                }
                deck_feeds::DlStatus::Paused => {
                    if !announced_pause {
                        announced_pause = true;
                        line(format!(
                            "[download] {key} paused — resume it (DOWNLOADS) to continue"
                        ));
                    }
                    waiting = true;
                }
                _ => waiting = true,
            }
        }
        if states.len() < keys.len() {
            anyhow::bail!("a queued download was removed (discard)");
        }
        if !waiting && landed.len() == keys.len() {
            // Order by the shard set: on-disk members resolve from disk,
            // fetched ones from the manager's landed paths.
            let mut ordered = Vec::with_capacity(set.len());
            for member in set {
                let on_disk = models_dir.join(file_name(member));
                if on_disk.exists() {
                    ordered.push(on_disk);
                } else {
                    let key = format!("{repo}/{member}");
                    ordered.push(
                        landed
                            .remove(&key)
                            .ok_or_else(|| anyhow::anyhow!("{key}: landed path missing"))?,
                    );
                }
            }
            return Ok(ordered);
        }
        if std::time::Instant::now() > deadline {
            anyhow::bail!("download wait exceeded the time budget");
        }
        std::thread::sleep(POLL);
    }
}

/// The post-landing half of the pipeline, shared shape with bringup_start /
/// test_model_start: derive → verify → [apply → bench].
fn run_after_download(
    app2: &tauri::AppHandle,
    model: &str,
    eng: Engine,
    apply: bool,
    line: &impl Fn(String),
    finish: &impl Fn(BringupResult),
) {
    let _ = app2.emit("bringup-phase", BringupPhase { phase: "derive".into() });

    let Some((p, fit, tps)) =
        bringup::derive_and_verify(app2, model, eng, false, apply, line, finish)
    else {
        return;
    };

    if !apply {
        // TEST: record the verified score (tagged by test port) and stop.
        if let Some(v) = tps {
            let at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            if let Ok(conn) = deck_core::store::open(&deck_core::store::default_db_path()) {
                deck_core::store::ensure_bench_schema(&conn).ok();
                let _ = deck_core::store::insert_bench(
                    &conn,
                    &deck_core::store::BenchRow {
                        id: 0,
                        engine: format!("{:?}", p.engine).to_lowercase(),
                        host: p.host.clone(),
                        port: p.port,
                        model: p.model.clone(),
                        ctx: p.ctx_size,
                        tps: v,
                        at,
                        hardware_profile_id: None,
                        engine_version: None,
                        prompt_tps: None,
                        ttft_ms: None,
                    },
                );
            }
        }
        finish(BringupResult {
            ok: true,
            summary: format!(
                "TEST OK · {} · ctx={} · NOT applied",
                tps.map(|t| format!("{t:.1} tok/s"))
                    .unwrap_or_else(|| "no metrics exposed".into()),
                p.ctx_size
            ),
            name: p.name.clone(),
            port: p.port,
            ctx: p.ctx_size,
            tps,
            fit: Some(fit),
        });
        return;
    }

    // LOAD: install + start, then bench + record.
    if let Err(e) = bringup::save_and_apply(app2, &p, line) {
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
    let tps = bringup::bench_and_record(app2, &p, line);
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
}