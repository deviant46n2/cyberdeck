//! Tauri door over the shared `DownloadManager`: maps `DlEvent`s to the
//! `dl-start` / `dl-progress` / `dl-done` / `dl-error` bus and keeps the
//! command surface (`download_start` / `download_cancel` / `download_remove`)
//! byte-compatible with the frontend store's expectations.
//!
//! One truth, two doors: the queue authority lives in `deck-feeds` and is
//! shared with the CLI (`deck downloads`); this module is a thin adapter.

use std::sync::Arc;

use serde::Serialize;
use tauri::Emitter;

use deck_feeds::{DlEvent, DlStatus, DownloadManager};

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

/// The shared queue authority, streaming through the real curl implementation.
static MANAGER: std::sync::LazyLock<Arc<DownloadManager>> =
    std::sync::LazyLock::new(DownloadManager::real);

/// (repo_id, rfilename) for a key, for event payloads the manager doesn't
/// carry (progress/done/error). Falls back to empty strings when the row is
/// already gone — the frontend tolerates that on the no-entry path.
fn row_of(key: &str) -> (String, String) {
    MANAGER
        .list()
        .iter()
        .find(|j| j.key == key)
        .map(|j| (j.repo_id.clone(), j.rfilename.clone()))
        .unwrap_or_default()
}

fn key_of(evt: &DlEvent) -> &str {
    match evt {
        DlEvent::Started { key, .. }
        | DlEvent::Progress { key, .. }
        | DlEvent::Done { key, .. }
        | DlEvent::Error { key, .. } => key,
    }
}

/// Route manager events onto the frontend's `dl-*` bus. Idempotent — each
/// `download_start` re-installs the same emitter.
fn ensure_sink(app: &tauri::AppHandle) {
    let app2 = app.clone();
    MANAGER.set_sink(Arc::new(move |evt: &DlEvent| {
        let key = key_of(evt).to_string();
        let (repo_id, rfilename) = row_of(&key);
        match evt {
            DlEvent::Started { .. } => {
                let _ = app2.emit(
                    "dl-start",
                    DownloadEvt {
                        key: key.clone(),
                        repo_id,
                        rfilename,
                        done: 0,
                        total: 0,
                    },
                );
            }
            DlEvent::Progress { done, total, .. } => {
                let _ = app2.emit(
                    "dl-progress",
                    DownloadEvt {
                        key,
                        repo_id,
                        rfilename,
                        done: *done,
                        total: *total,
                    },
                );
            }
            DlEvent::Done { path, .. } => {
                let _ = app2.emit(
                    "dl-done",
                    DownloadDone {
                        key,
                        repo_id,
                        rfilename,
                        path: path.display().to_string(),
                    },
                );
            }
            DlEvent::Error { error, .. } => {
                let _ = app2.emit("dl-error", DownloadErr { key, error: error.clone() });
            }
        }
    }));
}

/// Begin a background download of one repo file into `~/models`. Returns
/// immediately; progress flows as `dl-start` / `dl-progress` / `dl-done` /
/// `dl-error` events tagged with `key`. Idempotent: a queued or running key is
/// left alone, a paused one resumes, an errored one restarts, and a completed
/// one reports its existing `dl-done` so a fresh frontend session converges.
pub fn download_start(
    app: &tauri::AppHandle,
    repo_id: &str,
    rfilename: &str,
) -> anyhow::Result<DownloadStarted> {
    ensure_sink(app);
    let key = format!("{repo_id}/{rfilename}");
    let existing = MANAGER
        .list()
        .iter()
        .find(|j| j.key == key)
        .map(|j| (j.status, j.path.clone()))
        .clone();
    match existing {
        Some((DlStatus::Done, Some(path))) => {
            // Already on disk — surface it so the frontend doesn't wait for a
            // dl-start that will never come.
            let (repo, rf) = row_of(&key);
            let _ = app.emit(
                "dl-done",
                DownloadDone {
                    key: key.clone(),
                    repo_id: repo,
                    rfilename: rf,
                    path: path.display().to_string(),
                },
            );
            return Ok(DownloadStarted { key });
        }
        Some((DlStatus::Done, None)) => {
            // Row kept after clear happened mid-flight; restart it.
            MANAGER.enqueue(repo_id, rfilename)?;
        }
        Some((DlStatus::Paused, _)) => {
            MANAGER.start(&key)?;
        }
        Some(_) => {
            // queued / active: carry on; dl-start will confirm the launch.
        }
        None => {
            MANAGER.enqueue(repo_id, rfilename)?;
        }
    }
    Ok(DownloadStarted { key })
}

/// Cancel an active transfer (keeping its `.part` for later resume) and drop
/// any partial file for `rfilename` from the model directory. Used by the
/// download manager's REMOVE action. Best-effort: succeeds even when the
/// transfer already finished or no `.part` exists.
pub fn download_remove(key: &str, rfilename: &str) -> anyhow::Result<()> {
    // Cancel + forget the row if present (in-flight workers notice the cancel).
    let _ = MANAGER.discard(key);
    let name = std::path::Path::new(rfilename)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| rfilename.to_string());
    let part = deck_core::store::models_dir().join(format!("{name}.part"));
    if part.exists() {
        std::fs::remove_file(&part)?;
    }
    Ok(())
}

/// Request cancellation of an active download. The worker notices at its next
/// chunk boundary and emits `dl-error` with "cancelled". Stopping a row that
/// isn't runnable anymore is a no-op.
pub fn download_cancel(key: &str) -> anyhow::Result<()> {
    MANAGER.stop(key)
}

/// Snapshot of specific queue rows for in-process orchestrators (the
/// experiment door waits on a shard set this way): (key, status, landed path,
/// error message when failed).
pub fn download_states(
    keys: &[String],
) -> Vec<(String, DlStatus, Option<std::path::PathBuf>, Option<String>)> {
    MANAGER
        .list()
        .into_iter()
        .filter(|j| keys.contains(&j.key))
        .map(|j| (j.key, j.status, j.path, j.error))
        .collect()
}