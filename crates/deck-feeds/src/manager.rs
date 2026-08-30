//! Shared download manager: the queue authority for REPO→`~/models` transfers.
//!
//! This is the "one truth" both doors drive — the Tauri app's
//! `deck-tauri::downloads` registry and the CLI's `deck downloads` — so the
//! download-manager contract no longer lives only in the frontend store.
//!
//! Contract (mirrors `frontend/src/lib/dl.ts`):
//!   - a bounded pump fills up to `MAX_ACTIVE` slots from the frontmost queued
//!     entries (front of the queue = runs next);
//!   - STOP parks a transfer as `paused` — the backend keeps the `.part` and
//!     START resumes it (`curl -C -`);
//!   - REMOVE cancels and drops the `.part`, then forgets the row;
//!   - done/error rows persist until `clear_finished`.
//!
//! The stream function is injectable so queue logic is tested offline (no
//! network, no curl); the real implementation is `download_file_progress`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::download::{Cancel, download_file_progress};
use crate::market::remote_file_size;
use deck_core::store::models_dir;

/// Maximum concurrently-streaming transfers (matches the frontend store).
pub const MAX_ACTIVE: usize = 2;

/// A job's lifecycle, ordered along the priority queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DlStatus {
    Queued,
    Active,
    Paused,
    Done,
    Error,
}

/// A snapshot of one job, as doors and tests observe it.
#[derive(Clone, Debug)]
pub struct DlJob {
    pub key: String,
    pub repo_id: String,
    pub rfilename: String,
    pub status: DlStatus,
    pub done: u64,
    pub total: u64,
    pub error: Option<String>,
    pub path: Option<PathBuf>,
}

/// Events the manager emits; doors translate these to their channel (Tauri
/// `dl-*` events, CLI console lines).
#[derive(Clone, Debug)]
pub enum DlEvent {
    Started {
        key: String,
        repo_id: String,
        rfilename: String,
    },
    Progress {
        key: String,
        done: u64,
        total: u64,
    },
    Done {
        key: String,
        path: PathBuf,
    },
    Error {
        key: String,
        error: String,
    },
}

/// A streamable transfer: repo id + file → destination file path. Progress is
/// monotonic done/total (`total` 0 when Content-Length unknown). Cancellation
/// is cooperative — the `.part` is kept for resume.
pub type StreamFn = Arc<
    dyn Fn(
            &str,
            &str,
            &std::path::Path,
            Option<u64>,
            &mut dyn FnMut(u64, u64),
            &Cancel,
        ) -> anyhow::Result<PathBuf>
        + Send
        + Sync,
>;

/// Optional up-front size probe (`remote_file_size`), so a door can show a real
/// percentage before the first body bytes arrive. None keeps the manager
/// offline-friendly (tests).
pub type ProbeFn = Arc<dyn Fn(&str, &str) -> Option<u64> + Send + Sync>;

/// Event sink: doors route `DlEvent`s to their channel (Tauri `dl-*`, CLI
/// console). Attached once per manager via `set_sink`.
pub type Sink = Arc<dyn Fn(&DlEvent) + Send + Sync>;

struct InnerJob {
    job: DlJob,
    cancel: Arc<Cancel>,
    /// Bumped each time the lane is relaunched (start / error-restart). A
    /// worker that finishes under a stale epoch must not write terminal state
    /// — ownership of the lane has passed to a newer transfer.
    epoch: u64,
}

struct Inner {
    /// Priority queue: front = next to start. Terminal rows sit here too until
    /// cleared (`clear_finished`), so doors can keep showing them.
    order: Vec<InnerJob>,
    sink: Option<Sink>,
}

/// The download manager: a lock-guarded registry + bounded pump + workers.
///
/// Long-running streaming happens on spawned threads, so the pump never blocks
/// on network I/O; every transition that frees a slot re-runs the pump.
pub struct DownloadManager {
    inner: Mutex<Inner>,
    stream: StreamFn,
    probe: Option<ProbeFn>,
}

const EMIT_BYTES: u64 = 4 * 1024 * 1024;

impl DownloadManager {
    /// Manager streaming through the real curl-backed implementation.
    pub fn real() -> Arc<Self> {
        Self::with_stream(Arc::new(|repo, file, dest, total, progress, cancel| {
            download_file_progress(repo, file, dest, total, progress, cancel)
        }))
        .with_probe(Arc::new(remote_file_size))
    }

    /// Manager with an injectable stream (tests; doors that pre-probe totals).
    pub fn with_stream(stream: StreamFn) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                order: Vec::new(),
                sink: None,
            }),
            stream,
            probe: None,
        })
    }

    /// Attach the event sink (e.g. the Tauri `dl-*` emitter). Replaces prior.
    pub fn set_sink(self: &Arc<Self>, sink: Sink) {
        self.inner.lock().unwrap().sink = Some(sink);
    }

    /// Attach an up-front size probe (the real one is `remote_file_size`).
    /// Consumes the only-shared handle at construction time (`real()`).
    pub fn with_probe(mut self: Arc<Self>, probe: ProbeFn) -> Arc<Self> {
        match Arc::get_mut(&mut self) {
            Some(s) => s.probe = Some(probe),
            None => unreachable!("probe set before any shared clone exists"),
        }
        self
    }

    /// The default sink: a per-job clone; the real door wires its own.
    pub fn noop_sink() -> Sink {
        Arc::new(|_| {})
    }

    fn emit(&self, evt: &DlEvent) {
        let sink = self.inner.lock().unwrap().sink.clone();
        if let Some(s) = sink {
            s(evt);
        }
    }

    fn find_job(&self, key: &str) -> Option<usize> {
        self.inner.lock().unwrap().order.iter().position(|j| j.job.key == key)
    }

    fn set_status(&self, key: &str, status: DlStatus, error: Option<String>, path: Option<PathBuf>) {
        let mut g = self.inner.lock().unwrap();
        if let Some(j) = g.order.iter_mut().find(|j| j.job.key == key) {
            j.job.status = status;
            j.job.error = error;
            if let Some(p) = path {
                j.job.path = Some(p);
            }
        }
    }

    fn set_progress(&self, key: &str, done: u64, total: u64) {
        let mut g = self.inner.lock().unwrap();
        if let Some(j) = g.order.iter_mut().find(|j| j.job.key == key) {
            j.job.done = done;
            j.job.total = total;
        }
    }

    /// Enqueue one repo file. Safe to recall — a queued/active/paused/done
    /// entry is left alone; an errored entry restarts. Returns the job key.
    pub fn enqueue(self: &Arc<Self>, repo_id: &str, rfilename: &str) -> anyhow::Result<String> {
        let key = format!("{repo_id}/{rfilename}");
        if let Some(i) = self.find_job(&key) {
            let mut g = self.inner.lock().unwrap();
            if g.order[i].job.status != DlStatus::Error {
                return Ok(key);
            }
            // errored entry restarts — fresh cancel handle so a tripped flag
            // doesn't kill the relaunch, epoch bumped so a stale worker
            // finishing under the old transfer can't clobber this lane
            g.order[i].job.status = DlStatus::Queued;
            g.order[i].job.error = None;
            g.order[i].job.done = 0;
            g.order[i].cancel = Arc::new(Cancel::new());
            g.order[i].epoch += 1;
            drop(g);
            let _ = self.pump();
            return Ok(key);
        }
        self.inner.lock().unwrap().order.push(InnerJob {
            job: DlJob {
                key: key.clone(),
                repo_id: repo_id.to_string(),
                rfilename: rfilename.to_string(),
                status: DlStatus::Queued,
                done: 0,
                total: 0,
                error: None,
                path: None,
            },
            cancel: Arc::new(Cancel::new()),
            epoch: 0,
        });
        let _ = self.pump();
        Ok(key)
    }

    /// Enqueue an ordered set of files (e.g. a shard set). Each becomes an
    /// independent queue entry under the same concurrency cap. Callers own
    /// set-awareness (index once every member has landed); the manager
    /// guarantees the members coexist in the queue.
    pub fn enqueue_all(self: &Arc<Self>, repo_id: &str, files: &[String]) -> anyhow::Result<Vec<String>> {
        files.iter().map(|f| self.enqueue(repo_id, f)).collect()
    }

    /// START / RETRY: unpark a paused or errored entry back into the queue.
    pub fn start(self: &Arc<Self>, key: &str) -> anyhow::Result<()> {
        let i = self
            .find_job(key)
            .ok_or_else(|| anyhow::anyhow!("no download '{key}'"))?;
        {
            let mut g = self.inner.lock().unwrap();
            match g.order[i].job.status {
                DlStatus::Paused | DlStatus::Error => {
                    // fresh cancel handle: a tripped flag must not cancel the
                    // resumed transfer immediately at spawn; epoch bump lets a
                    // stale worker's terminal write be ignored
                    g.order[i].job.status = DlStatus::Queued;
                    g.order[i].job.error = None;
                    g.order[i].cancel = Arc::new(Cancel::new());
                    g.order[i].epoch += 1;
                }
                _ => return Ok(()),
            }
        }
        let _ = self.pump();
        Ok(())
    }

    /// STOP: park an active/queued transfer as `paused`. The `.part` is kept
    /// so START (curl `-C -`) resumes. An already-paused row is a no-op.
    pub fn stop(&self, key: &str) -> anyhow::Result<()> {
        let i = self
            .find_job(key)
            .ok_or_else(|| anyhow::anyhow!("no download '{key}'"))?;
        let mut g = self.inner.lock().unwrap();
        match g.order[i].job.status {
            DlStatus::Active | DlStatus::Queued => {
                g.order[i].cancel.cancel();
                g.order[i].job.status = DlStatus::Paused;
            }
            _ => {}
        }
        Ok(())
    }

    /// REMOVE: cancel if running, drop the `.part` if any, forget the row.
    /// Completed files on disk are left alone (that's the VAULT's job).
    pub fn discard(&self, key: &str) -> anyhow::Result<()> {
        let i = self
            .find_job(key)
            .ok_or_else(|| anyhow::anyhow!("no download '{key}'"))?;
        let rfilename = self.inner.lock().unwrap().order[i].job.rfilename.clone();
        // Cancel an in-flight stream before dropping the row.
        if let Some(cancel) = self
            .inner
            .lock()
            .unwrap()
            .order
            .iter()
            .find(|j| j.job.key == key)
            .map(|j| j.cancel.clone())
        {
            cancel.cancel();
        }
        let name = std::path::Path::new(&rfilename)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| rfilename.clone());
        let part = models_dir().join(format!("{name}.part"));
        if part.exists() {
            let _ = std::fs::remove_file(&part);
        }
        let mut g = self.inner.lock().unwrap();
        g.order.retain(|j| j.job.key != key);
        Ok(())
    }

    /// Move an entry up/down the priority queue (front = runs next).
    pub fn reorder(&self, key: &str, dir: i8) -> anyhow::Result<()> {
        let i = self
            .find_job(key)
            .ok_or_else(|| anyhow::anyhow!("no download '{key}'"))?;
        let len = self.inner.lock().unwrap().order.len();
        if len < 2 {
            return Ok(());
        }
        let j = (i as isize + dir as isize).clamp(0, len as isize - 1) as usize;
        if i == j {
            return Ok(());
        }
        let mut g = self.inner.lock().unwrap();
        g.order.swap(i, j);
        Ok(())
    }

    /// Dismiss every finished/failed row (files already on disk, or gone).
    pub fn clear_finished(&self) {
        let mut g = self.inner.lock().unwrap();
        g.order.retain(|j| j.job.status != DlStatus::Done && j.job.status != DlStatus::Error);
    }

    /// Snapshot of queue entries in priority order (front first).
    pub fn list(&self) -> Vec<DlJob> {
        self.inner.lock().unwrap().order.iter().map(|j| j.job.clone()).collect()
    }

    /// The number of currently-streaming transfers.
    pub fn active(&self) -> usize {
        self.inner
            .lock()
            .unwrap()
            .order
            .iter()
            .filter(|j| j.job.status == DlStatus::Active)
            .count()
    }

    /// Fill up to `MAX_ACTIVE` slots from the frontmost queued entries.
    /// Workers spawned here re-run the pump on exit, so a freed slot is
    /// immediately offered to the next queued entry.
    pub fn pump(self: &Arc<Self>) -> anyhow::Result<()> {
        struct Launch {
            repo: String,
            file: String,
            key: String,
            epoch: u64,
        }
        let mut to_launch: Vec<Launch> = Vec::new();
        {
            let mut g = self.inner.lock().unwrap();
            let active = g.order.iter().filter(|j| j.job.status == DlStatus::Active).count();
            let mut slots = MAX_ACTIVE.saturating_sub(active);
            for j in &mut g.order {
                if slots == 0 {
                    break;
                }
                if j.job.status == DlStatus::Queued {
                    j.job.status = DlStatus::Active;
                    j.job.error = None;
                    slots -= 1;
                    to_launch.push(Launch {
                        repo: j.job.repo_id.clone(),
                        file: j.job.rfilename.clone(),
                        key: j.job.key.clone(),
                        epoch: j.epoch,
                    });
                }
            }
        }
        for l in to_launch {
            self.spawn_worker(l.repo, l.file, l.key, l.epoch);
        }
        Ok(())
    }

    /// Spawn one worker thread for an entry that the pump just activated.
    fn spawn_worker(self: &Arc<Self>, repo: String, file: String, key: String, epoch: u64) {
        let cancel = self
            .inner
            .lock()
            .unwrap()
            .order
            .iter()
            .find(|j| j.job.key == key)
            .map(|j| j.cancel.clone())
            .unwrap_or_default();
        if cancel.cancelled() {
            // stop()/discard() beat the worker to the race — park as paused,
            // but only if this lane wasn't relaunched under a fresh epoch.
            if self.current_epoch(&key) == Some(epoch) {
                self.set_status(&key, DlStatus::Paused, None, None);
            }
            return;
        }
        let mgr = Arc::clone(self);
        let stream = Arc::clone(&self.stream);
        let probe = self.probe.clone();
        std::thread::Builder::new()
            .name(format!("dl:{key}"))
            .spawn(move || {
                // Discarded/relaunched before we even started? Emit nothing —
                // the lane's newer transfer owns the channel.
                if mgr.current_epoch(&key) != Some(epoch) {
                    return;
                }
                mgr.emit(&DlEvent::Started {
                    key: key.clone(),
                    repo_id: repo.clone(),
                    rfilename: file.clone(),
                });
                // Up-front size probe (when attached) gives doors a real total
                // before the first body bytes arrive.
                let probed_total = probe.as_ref().and_then(|p| p(&repo, &file)).unwrap_or(0);
                if probed_total > 0 {
                    mgr.set_progress(&key, 0, probed_total);
                    mgr.emit(&DlEvent::Progress {
                        key: key.clone(),
                        done: 0,
                        total: probed_total,
                    });
                }
                let mut last_emit_done: u64 = 0;
                let mut progress = |done: u64, total: u64| {
                    if done.saturating_sub(last_emit_done) >= EMIT_BYTES || done == 0 {
                        last_emit_done = done;
                        mgr.set_progress(&key, done, total);
                        mgr.emit(&DlEvent::Progress {
                            key: key.clone(),
                            done,
                            total,
                        });
                    }
                };
                let result = stream(
                    &repo,
                    &file,
                    &models_dir(),
                    (probed_total > 0).then_some(probed_total),
                    &mut progress,
                    &cancel,
                );
                // A stale worker (lane was stopped+restarted, or discarded
                // while we streamed) must not overwrite the lane's newer state.
                if mgr.current_epoch(&key) != Some(epoch) {
                    return;
                }
                match result {
                    Ok(path) => {
                        mgr.set_status(&key, DlStatus::Done, None, Some(path.clone()));
                        mgr.emit(&DlEvent::Done { key: key.clone(), path });
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        // STOP parks the row (the `.part` is kept for resume);
                        // real failures are errors.
                        let status = if msg == "cancelled" {
                            DlStatus::Paused
                        } else {
                            DlStatus::Error
                        };
                        mgr.set_status(&key, status, Some(msg.clone()), None);
                        mgr.emit(&DlEvent::Error { key: key.clone(), error: msg });
                    }
                }
                let _ = mgr.pump();
            })
            .expect("spawn dl worker");
    }

    /// Current epoch of a lane, or None if the row is gone.
    fn current_epoch(&self, key: &str) -> Option<u64> {
        self.inner.lock().unwrap().order.iter().find(|j| j.job.key == key).map(|j| j.epoch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Deterministic fake stream: parks until cancelled (never completes), so
    /// tests can observe queued/active/paused states without racing.
    fn holding_stream() -> StreamFn {
        Arc::new(|_repo, _file, _dest, _total, progress, cancel| {
            let mut tick: u64 = 1;
            while !cancel.cancelled() {
                progress(tick * 100_000, 1_000_000);
                tick += 1;
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            anyhow::bail!("cancelled") // STOP — `.part` kept, like the real sink
        })
    }

    /// Fake stream that errors on the first call then holds (for restart tests).
    fn error_then_hold() -> StreamFn {
        let failed = AtomicBool::new(false);
        let flag = move || failed.swap(true, Ordering::SeqCst);
        Arc::new(move |_repo, _file, _dest, _total, progress, cancel| {
            if !flag() {
                return Err(anyhow::anyhow!("boom"));
            }
            let mut tick: u64 = 1;
            while !cancel.cancelled() {
                progress(tick * 100_000, 1_000_000);
                tick += 1;
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            anyhow::bail!("cancelled")
        })
    }

    

    fn mgr_with(stream: StreamFn) -> Arc<DownloadManager> {
        let m = DownloadManager::with_stream(stream);
        m.set_sink(DownloadManager::noop_sink());
        m
    }

    #[test]
    fn pump_caps_active_at_max_active() {
        let m = mgr_with(holding_stream());
        m.enqueue("r", "a").unwrap();
        m.enqueue("r", "b").unwrap();
        m.enqueue("r", "c").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert_eq!(m.active(), 2, "pump leaves the 3rd queued");
        let queued: Vec<String> = m
            .list()
            .iter()
            .filter(|j| j.status == DlStatus::Queued)
            .map(|j| j.key.clone())
            .collect();
        assert_eq!(queued, vec!["r/c"]);
    }

    #[test]
    fn stop_parks_paused_and_start_resumes() {
        let m = mgr_with(holding_stream());
        m.enqueue("r", "a").unwrap();
        let key = "r/a";
        std::thread::sleep(std::time::Duration::from_millis(10));
        m.stop(key).unwrap();
        let job = m.list().into_iter().find(|j| j.key == key).unwrap();
        assert_eq!(job.status, DlStatus::Paused);
        m.start(key).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let job = m.list().into_iter().find(|j| j.key == key).unwrap();
        assert_eq!(
            job.status,
            DlStatus::Active,
            "resumed into the queue and re-launched"
        );
    }

    #[test]
    fn discard_forgets_the_row_and_drops_part() {
        let m = mgr_with(holding_stream());
        m.enqueue("r", "a").unwrap();
        m.discard("r/a").unwrap();
        assert!(m.list().is_empty());
    }

    #[test]
    fn enqueue_is_idempotent_and_restarts_errors() {
        let m = mgr_with(error_then_hold());
        m.enqueue("r", "a").unwrap();
        m.enqueue("r", "a").unwrap();
        assert_eq!(m.list().len(), 1, "duplicate enqueue leaves the row alone");
        // The first stream attempt errors → row is Error.
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert_eq!(m.list()[0].status, DlStatus::Error);
        // Re-enqueue restarts it: it pumps and goes back to a live slot.
        m.enqueue("r", "a").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert_eq!(
            m.list()[0].status,
            DlStatus::Active,
            "error entry restarts into the queue"
        );
    }

    #[test]
    fn reorder_swaps_priority_positions() {
        let m = mgr_with(holding_stream());
        m.enqueue("r", "a").unwrap();
        m.enqueue("r", "b").unwrap();
        let back = m.list().len() - 1;
        // move the tail entry to the front
        m.reorder("r/b", -(back as i8)).unwrap();
        let order: Vec<String> = m.list().iter().map(|j| j.key.clone()).collect();
        assert_eq!(order[0], "r/b");
    }

    #[test]
    fn clear_finished_keeps_live_rows() {
        // One live (never-completing) + one errored (first-call fails) row:
        // clear drops only the errored row.
        let m = mgr_with(error_then_hold());
        m.enqueue("r", "err").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        m.enqueue("r", "live").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert_eq!(m.list().len(), 2, "both rows present");
        assert_eq!(
            m.list().iter().find(|j| j.key == "r/err").unwrap().status,
            DlStatus::Error,
            "first-call stream fails the err row"
        );
        m.clear_finished();
        let keys: Vec<String> = m.list().iter().map(|j| j.key.clone()).collect();
        assert_eq!(keys, vec!["r/live"], "errored row cleared, live row kept");
    }

    #[test]
    fn payload_snapshots_are_observable() {
        let m = mgr_with(holding_stream());
        m.enqueue("org", "q2.gguf").unwrap();
        let job = m.list().into_iter().next().unwrap();
        assert_eq!(job.repo_id, "org");
        assert_eq!(job.rfilename, "q2.gguf");
        assert_eq!(job.key, "org/q2.gguf");
        assert_eq!(m.active(), 1);
    }
}