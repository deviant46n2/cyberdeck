//! Probe the live engine over loopback: /health liveness, /metrics fetch and
//! tps parsing, plus the headless bring-up verification on a test port.

use std::io::BufRead;
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use deck_core::profile::Profile;

use crate::unit::build_args;

fn agent(timeout: Duration) -> ureq::Agent {
    let config = ureq::config::Config::builder()
        .timeout_global(Some(timeout))
        .build();
    config.new_agent()
}

/// Waits for the engine's OpenAI-compatible /health endpoint to come up.
pub fn health_wait(host: &str, port: u16, timeout: Duration) -> bool {
    let url = format!("http://{host}:{port}/health");
    let agent = agent(Duration::from_secs(2));
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Ok(r) = agent.get(&url).call() {
            if r.status() == 200 {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    false
}

/// Fast liveness check against the engine's /health endpoint. Returns false on
/// any error or non-200 (including connection refused). Used by the HUD status
/// pills so the UI can show which engines are live without blocking.
pub fn health_ok(host: &str, port: u16) -> bool {
    let url = format!("http://{host}:{port}/health");
    let agent = agent(Duration::from_secs(2));
    matches!(agent.get(&url).call(), Ok(r) if r.status() == 200)
}

/// Liveness check that accepts whichever endpoint the engine happens to expose
/// (/health for llama.cpp, /v1/models for FreeToken). Used by the loadout test
/// harness so a healthy engine isn't misread as a timeout.
pub fn health_ok_any(host: &str, port: u16) -> bool {
    let agent = agent(Duration::from_secs(2));
    for path in ["/health", "/v1/models"] {
        let url = format!("http://{host}:{port}{path}");
        if matches!(agent.get(&url).call(), Ok(r) if r.status() == 200) {
            return true;
        }
    }
    false
}

/// Pulls the raw Prometheus text from a running engine's /metrics endpoint.
pub fn fetch_metrics(host: &str, port: u16) -> anyhow::Result<String> {
    let url = format!("http://{host}:{port}/metrics");
    let agent = agent(Duration::from_secs(5));
    let resp = agent
        .get(&url)
        .call()
        .map_err(|e| anyhow::anyhow!("metrics fetch failed: {e}"))?;
    let body = resp
        .into_body()
        .read_to_string()
        .map_err(|e| anyhow::anyhow!("reading metrics body: {e}"))?;
    Ok(body)
}

/// Extracts a generation throughput (tokens/sec) gauge from Prometheus text.
/// Prefers `generation_tokens_per_second`; falls back to any `*_tokens_per_second`
/// line. Returns None if the endpoint exposes no usable gauge.
pub fn parse_tps(text: &str) -> Option<f64> {
    let lines: Vec<&str> = text.lines().collect();
    // Pass 1: an explicit generation throughput gauge.
    for line in &lines {
        let l = line.trim();
        if l.starts_with('#')
            || !(l.contains("generation") && l.contains("tok") && l.contains("per_sec"))
        {
            continue;
        }
        if let Some(v) = l
            .split_whitespace()
            .last()
            .and_then(|s| s.parse::<f64>().ok())
        {
            return Some(v);
        }
    }
    // Pass 2: any tokens/sec gauge (llama.cpp exports
    // `*_tokens_per_second`; FreeToken exports `tok_per_sec`).
    for line in &lines {
        let l = line.trim();
        if l.starts_with('#') || !(l.contains("tok") && l.contains("per_sec")) {
            continue;
        }
        if let Some(v) = l
            .split_whitespace()
            .last()
            .and_then(|s| s.parse::<f64>().ok())
        {
            return Some(v);
        }
    }
    None
}

/// Keywords that indicate the engine failed to allocate VRAM rather than a clean
/// exit or a logic error. Scanned from the engine's stderr/stdout during a
/// headless bring-up verification.
pub const OOM_MARKERS: &[&str] = &[
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

/// Outcome of a headless bring-up verification on a test port.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BringupOutcome {
    /// The ctx that was verified (already reduced via the ladder if the max OOM'd).
    pub ctx: u32,
    pub verdict: String,
    pub summary: String,
    /// Generation throughput (tok/s) sampled after health, if `/metrics` exposes one.
    pub tok_per_sec: Option<f64>,
}

/// Headlessly verify a draft loadout on a dedicated test port **without
/// touching the live service**, watching for OOM and health. If the max-ctx
/// candidate OOMs or fails to serve, walks the profile's ctx ladder down and
/// retries. Returns the first config that actually serves (or the first error /
/// full-ladder-exhausted outcome).
///
/// This is the "safe bring-up" path: the live engine on `p.port` keeps running;
/// only the verified-good profile is ever installed/started by the caller.
pub fn verify_on_test_port(p: &Profile, test_port: u16, timeout: Duration) -> BringupOutcome {
    let mut draft = p.clone();
    draft.port = test_port;
    let host = draft.host.clone();

    for ctx in draft.active_ladder() {
        // (re-)apply ladder ctx to the draft.
        draft.ctx_size = ctx;
        let args = build_args(&draft);
        let mut child = match Command::new(&draft.bin)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                return BringupOutcome {
                    ctx,
                    verdict: "ERROR".into(),
                    summary: format!("failed to spawn {}: {e}", draft.bin.display()),
                    tok_per_sec: None,
                };
            }
        };
        let stderr = child.stderr.take().expect("stderr");
        let oom = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let oom_t = oom.clone();
        std::thread::spawn(move || {
            let reader = std::io::BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                let low = line.to_lowercase();
                if OOM_MARKERS.iter().any(|m| low.contains(m)) {
                    oom_t.store(true, Ordering::SeqCst);
                }
            }
        });

        let start = Instant::now();
        let mut verdict = (
            "TIMEOUT",
            "never reported healthy within the timeout".to_string(),
        );
        loop {
            if oom.load(Ordering::SeqCst) {
                verdict = (
                    "OOM",
                    "engine logged an out-of-memory / allocation failure".into(),
                );
                break;
            }
            if let Some(s) = child.try_wait().ok().flatten() {
                verdict = ("CRASH", format!("engine exited early with status {s}"));
                break;
            }
            if health_ok_any(&host, test_port) {
                verdict = (
                    "RUNNING",
                    "engine loaded and is serving on the test port".into(),
                );
                break;
            }
            if start.elapsed() > timeout {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }

        // Always tear down the test child before moving on.
        let _ = child.kill();
        let _ = child.wait();

        match verdict.0 {
            "RUNNING" => {
                let tps = fetch_metrics(&host, test_port)
                    .ok()
                    .and_then(|m| parse_tps(&m));
                return BringupOutcome {
                    ctx,
                    verdict: "RUNNING".into(),
                    summary: verdict.1,
                    tok_per_sec: tps,
                };
            }
            "OOM" | "CRASH" | "TIMEOUT" => {
                eprintln!(
                    "[bringup] ctx={ctx} {}: {} — walking ladder down",
                    verdict.0, verdict.1
                );
                continue;
            }
            _ => {}
        }
    }

    BringupOutcome {
        ctx: p.ctx_size,
        verdict: "FAIL".into(),
        summary: "all ctx-ladder candidates failed to serve on the test port".into(),
        tok_per_sec: None,
    }
}
