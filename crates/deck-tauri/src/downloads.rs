//! The download manager: registry + worker threads streaming into ~/models.

use std::sync::Mutex;
use std::time::Instant;

use serde::Serialize;
use tauri::Emitter;

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

        // Free the registry slot BEFORE emitting the terminal event so an
        // immediate STOP→START (resume) re-entry isn't rejected as a duplicate.
        DOWNLOADS.lock().unwrap().remove(&key2);

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
    });

    Ok(DownloadStarted { key })
}

/// Cancel an active transfer (keeping its `.part` for later resume) and anyhow
/// drop any partial file for `rfilename` from the model directory. Used by the
/// download manager's REMOVE action. Best-effort: succeeds even when the
/// transfer already finished or no `.part` exists.
pub fn download_remove(key: &str, rfilename: &str) -> anyhow::Result<()> {
    // No-op when not currently active (the worker already removed its slot).
    let _ = download_cancel(key);
    let name = std::path::Path::new(rfilename)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| rfilename.to_string());
    let part = models_dir().join(format!("{name}.part"));
    if part.exists() {
        std::fs::remove_file(&part)?;
    }
    Ok(())
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
