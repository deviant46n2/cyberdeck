//! The loadout TEST harness: spawn a draft engine on a test port and watch it.

use std::io::BufRead;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::Emitter;

use crate::{Engine, Profile};

#[derive(Clone, Serialize)]
pub struct TestPhase {
    pub phase: String,
}

#[derive(Clone, Serialize)]
pub struct TestLine {
    pub stream: String,
    pub text: String,
}

#[derive(Clone, Serialize)]
pub struct TestResult {
    pub verdict: String,
    pub summary: String,
}

#[derive(Clone, Serialize)]
pub struct TweakResult {
    pub ok: bool,
    pub summary: String,
    pub ctx: u32,
    pub tps: Option<f64>,
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

/// Streams one engine pipe (stdout or stderr) as `test-output` events, flagging
/// OOM keywords into `oom`. Runs until the pipe closes.
fn spawn_output_reader<R: std::io::Read + Send + 'static>(
    app: tauri::AppHandle,
    stream: &'static str,
    pipe: R,
    oom: std::sync::Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(pipe);
        for line in reader.lines().map_while(Result::ok) {
            let low = line.to_lowercase();
            if OOM_MARKERS.iter().any(|m| low.contains(m)) {
                oom.store(true, Ordering::SeqCst);
            }
            let _ = app.emit(
                "test-output",
                TestLine {
                    stream: stream.into(),
                    text: line,
                },
            );
        }
    });
}

/// Poll the spawned test child until an OOM keyword, an early exit, or the
/// health endpoint comes up. The child lives in TEST_CHILD so test_stop can
/// kill it; `None` there means test_stop already reaped it.
/// Returns the (verdict, summary) tuple.
fn watch_boot(
    host: &str,
    test_port: u16,
    timeout: Duration,
    oom: &std::sync::Arc<AtomicBool>,
    healthy: &std::sync::Arc<AtomicBool>,
) -> (&'static str, String) {
    let start = Instant::now();
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
        if deck_engines::health_ok_any(host, test_port) {
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
    verdict
}

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

        spawn_output_reader(app2.clone(), "stdout", stdout, oom.clone());
        spawn_output_reader(app2.clone(), "stderr", stderr, oom.clone());

        let (verdict, summary) =
            watch_boot(&host, test_port, Duration::from_secs(180), &oom, &healthy);

        // Tear down the test process, then restore the live service.
        if let Some(mut c) = TEST_CHILD.lock().unwrap().take() {
            let _ = c.kill();
        }
        restart_live();
        emit_phase("done");
        emit_result(&verdict, &summary);
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
pub fn test_profile_tweaked(
    profile: Profile,
    ctx_override: Option<u32>,
    kv_bytes_override: Option<f64>,
    offload_override: Option<bool>,
    ngl_override: Option<u32>,
) -> TweakResult {
    let test_port = profile.engine.test_port();
    let mut draft = profile.clone();

    if let Some(v) = ctx_override {
        draft.ctx_size = v;
    }
    if let Some(_v) = kv_bytes_override { /* noted for display */ }
    if let Some(offload) = offload_override {
        if offload && profile.engine == Engine::FreeToken {
            draft.ft_backend = Some("offload".into());
        }
    }
    if let Some(v) = ngl_override {
        draft.n_gpu_layers = v;
    }

    let outcome = deck_engines::verify_on_test_port(&draft, test_port, Duration::from_secs(180));

    if outcome.verdict != "RUNNING" {
        return TweakResult {
            ok: false,
            summary: format!(
                "{} (ctx={}) — tweak params and retry",
                outcome.summary, outcome.ctx
            ),
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
