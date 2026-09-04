//! Probe the live engine over loopback: /health liveness, /metrics fetch and
//! tps parsing, plus the headless bring-up verification on a test port.

use std::io::BufRead;
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use deck_core::profile::{Engine, Profile};

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
/// (/health for llama.cpp and FreeToken, /v1/models for FreeToken's API,
/// /api/tags for Ollama). Used by the loadout test harness so a healthy engine
/// isn't misread as a timeout.
pub fn health_ok_any(host: &str, port: u16) -> bool {
    let agent = agent(Duration::from_secs(2));
    for path in ["/health", "/v1/models", "/api/tags"] {
        let url = format!("http://{host}:{port}{path}");
        if matches!(agent.get(&url).call(), Ok(r) if r.status() == 200) {
            return true;
        }
    }
    false
}

/// Detect the engine version string from a running engine.
///
/// Strategy:
/// 1. Try parsing `llama_version` from Prometheus `/metrics` (llama.cpp exports this).
/// 2. Fall back to running the engine binary with `--version`.
/// 3. For Ollama, try `/api/version`.
/// Returns None if none of these work.
pub fn detect_engine_version(engine: Engine, host: &str, port: u16) -> Option<String> {
    // Strategy 1: parse from /metrics (llama.cpp exports llama_version_info or similar)
    if let Ok(text) = fetch_metrics(host, port) {
        for line in text.lines() {
            let l = line.trim();
            // llama.cpp exports: # HELP llama_version llama.cpp version
            // or: llama_version{...} "bXXX"
            if l.contains("llama_version") && !l.starts_with('#') {
                if let Some(v) = l.split_whitespace().last().map(|s| s.trim_matches('"').to_string()) {
                    if !v.is_empty() && v != "0" {
                        return Some(v);
                    }
                }
            }
        }
    }
    // Strategy 2: Ollama /api/version
    if engine == Engine::Ollama {
        let url = format!("http://{host}:{port}/api/version");
        let agent = agent(Duration::from_secs(3));
        if let Ok(r) = agent.get(&url).call() {
            if let Ok(body) = r.into_body().read_to_string() {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                    if let Some(ver) = v.get("version").and_then(|v| v.as_str()) {
                        return Some(ver.to_string());
                    }
                }
            }
        }
    }
    // Strategy 3: run engine binary --version
    let bin_name = match engine {
        Engine::LlamaCpp => "llama-server",
        Engine::FreeToken => "ft",
        Engine::Ollama => "ollama",
    };
    if let Ok(out) = std::process::Command::new(bin_name).arg("--version").output() {
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        for line in stdout.lines().chain(stderr.lines()) {
            let l = line.trim();
            if !l.is_empty() {
                return Some(l.to_string());
            }
        }
    }
    None
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

/// Fixed probe prompt for the active fallback: long enough that the steady-
/// state decode rate dominates prefill, deterministic content.
pub const PROBE_PROMPT: &str = "Write a detailed explanation of how transformer attention works: queries, keys, values, softmax, and multi-head attention.";

/// Measure generation tok/s against a live engine: scrape the /metrics
/// throughput gauge, and when that is absent or dead (idle llama.cpp gauges
/// read 0), run one real probe generation and take its native timing. This is
/// the single measurement path for `deck bench record` and the app's
/// bench door — a 0 tok/s row must never come from a dead scrape.
pub fn measure_generation_tps(
    engine: Engine,
    host: &str,
    port: u16,
    model_id: &str,
) -> Result<f64, String> {
    let text = fetch_metrics(host, port)
        .map_err(|e| format!("could not reach {host}:{port}/metrics — is the engine running with --metrics? ({e})"))?;
    if let Some(v) = parse_tps(&text).filter(|v| *v > 0.0) {
        return Ok(v);
    }
    let sample = crate::inference::run_prompt(engine, host, port, model_id, PROBE_PROMPT, 192);
    sample.tok_s.filter(|v| *v > 0.0).ok_or_else(|| {
        format!(
            "probe generation failed: {}",
            sample.error.unwrap_or_else(|| "no tok/s".into())
        )
    })
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
    // Pass 2: any tokens/sec gauge (llama.cpp exports `*_tokens_per_second`
    // or `*_tokens_seconds` gauges; FreeToken exports `tok_per_sec`).
    // `*_seconds_total` lines are counters (cumulative seconds), not rates —
    // excluded. Prompt-processing speed is never recorded as generation speed.
    for line in &lines {
        let l = line.trim();
        if l.starts_with('#') || l.contains("_total") || l.contains("prompt") {
            continue;
        }
        if !(l.contains("tok") && (l.contains("per_sec") || l.contains("tokens_seconds"))) {
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

/// Spawn a draft loadout directly (no systemd writes) on a dedicated test port
/// and wait until it is healthy, OOMs, crashes, or times out. On success the
/// live child is returned — the caller OWNS it and MUST kill it. On failure the
/// child is already reaped and the (verdict, summary) is returned.
///
/// Ollama binds via `OLLAMA_HOST` (no `--host`/$`--port` flags), so that env is
/// set on the child here — it only exists in the rendered unit otherwise.
pub fn boot_on_test_port(
    p: &Profile,
    test_port: u16,
    timeout: Duration,
) -> Result<std::process::Child, (String, String)> {
    let mut draft = p.clone();
    draft.port = test_port;
    let host = draft.host.clone();
    let args = build_args(&draft);
    let mut cmd = Command::new(&draft.bin);
    cmd.args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if draft.engine == deck_core::profile::Engine::Ollama {
        cmd.env("OLLAMA_HOST", format!("{host}:{test_port}"));
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Err((
                "ERROR".into(),
                format!("failed to spawn {}: {e}", draft.bin.display()),
            ));
        }
    };
    let stderr = child.stderr.take().expect("test engine stderr unavailable");
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
    if verdict.0 == "RUNNING" {
        Ok(child)
    } else {
        let _ = child.kill();
        let _ = child.wait();
        Err((verdict.0.to_string(), verdict.1))
    }
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
    let host = draft.host.clone();

    for ctx in draft.active_ladder() {
        // (re-)apply ladder ctx to the draft.
        draft.ctx_size = ctx;
        match boot_on_test_port(&draft, test_port, timeout) {
            Ok(mut child) => {
                // Sample tok/s WHILE the engine is still alive (/metrics dies
                // with the process), then tear down.
                let tps = fetch_metrics(&host, test_port)
                    .ok()
                    .and_then(|m| parse_tps(&m));
                let _ = child.kill();
                let _ = child.wait();
                return BringupOutcome {
                    ctx,
                    verdict: "RUNNING".into(),
                    summary: "engine loaded and is serving on the test port".into(),
                    tok_per_sec: tps,
                };
            }
            Err((verdict, summary)) => {
                eprintln!("[bringup] ctx={ctx} {verdict}: {summary} — walking ladder down");
                continue;
            }
        }
    }

    BringupOutcome {
        ctx: p.ctx_size,
        verdict: "FAIL".into(),
        summary: "all ctx-ladder candidates failed to serve on the test port".into(),
        tok_per_sec: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tps_reads_predicted_gauge_of_local_llamacpp_build() {
        let text = "# HELP llamacpp:predicted_tokens_seconds Average generation throughput\n\
                    # TYPE llamacpp:predicted_tokens_seconds gauge\n\
                    llamacpp:predicted_tokens_seconds 47.5\n\
                    llamacpp:tokens_predicted_seconds_total 14.742\n";
        assert_eq!(parse_tps(text), Some(47.5));
    }

    #[test]
    fn tps_ignores_counters_and_prompt_rate() {
        // Only a cumulative counter + the prompt rate: nothing usable.
        let text = "llamacpp:tokens_predicted_seconds_total 14.742\n\
                    llamacpp:prompt_tokens_seconds 980.0\n";
        assert_eq!(parse_tps(text), None);
    }

    #[test]
    fn tps_still_reads_upstream_and_freetoken_names() {
        let upstream = "llamacpp:generation_tokens_per_second 12.5\n";
        assert_eq!(parse_tps(upstream), Some(12.5));
        let ft = "ft:tok_per_sec 31.0\n";
        assert_eq!(parse_tps(ft), Some(31.0));
    }
}
