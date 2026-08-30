//! One-click BRINGUP pipeline: derive → headless verify → apply → bench.
//!
//! `bringup_start` and `test_model_start` share the single-flight lock,
//! the `BRINGUP_RUNNING` flag, and the `derive_and_verify` helper, and they
//! stream the same `bringup-phase` / `bringup-line` / `bringup-result` events,
//! so the frontend panel renders identically for both.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::Emitter;

use crate::{Engine, Profile};

/// Emitted as the one-click load pipeline advances.
#[derive(Clone, Serialize)]
pub struct BringupPhase {
    pub phase: String, // derive | verify | apply | bench | done | error
}

#[derive(Clone, Serialize)]
pub struct BringupLine {
    pub text: String,
}

/// Full VRAM fit breakdown shown during bring-up so the user sees exactly
/// where memory goes even when verification fails.
#[derive(Clone, Deserialize, Serialize)]
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

#[derive(Clone, Serialize)]
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

pub fn bringup_reset() {
    BRINGUP_RUNNING.store(false, Ordering::SeqCst);
}

struct BringupGuard;
impl Drop for BringupGuard {
    fn drop(&mut self) {
        BRINGUP_RUNNING.store(false, Ordering::SeqCst);
    }
}

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
    let eng = Engine::parse(engine_s)
        .ok_or_else(|| anyhow::anyhow!("unknown engine '{engine_s}' (llamacpp|freetoken)"))?;
    if BRINGUP_RUNNING.swap(true, Ordering::SeqCst) {
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
            BRINGUP_RUNNING.store(false, Ordering::SeqCst);
        };
        let line = |t: String| {
            let _ = app2.emit("bringup-line", BringupLine { text: t });
        };

        // 1+2. Derive + verify (headless, never touches live) ---------------
        let Some((p, fit, _tps)) =
            derive_and_verify(&app2, &model, eng, fast, true, &line, &finish)
        else {
            return;
        };

        // 3. Save + apply ---------------------------------------------------
        if let Err(e) = save_and_apply(&app2, &p, &line) {
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

        // 4. Bench + record -------------------------------------------------
        let tps = bench_and_record(&app2, &p, &line);

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

/// Persist the derived profile and bring it live via systemd (BRINGUP step 3).
/// Emits the apply phase and a confirmation line; the caller wraps failures
/// with the fit breakdown it holds.
fn save_and_apply(
    app2: &tauri::AppHandle,
    p: &Profile,
    line: &impl Fn(String),
) -> anyhow::Result<()> {
    let _ = app2.emit(
        "bringup-phase",
        BringupPhase {
            phase: "apply".into(),
        },
    );
    let db = deck_core::store::default_db_path();
    let conn = deck_core::store::open(&db)?;
    deck_core::store::ensure_profile_schema(&conn)?;
    deck_core::store::upsert_profile(&conn, p)?;
    deck_core::store::ensure_resident_schema(&conn).ok();
    let _ = deck_core::store::set_resident(&conn, p.engine.store_id(), &p.name, Some(true));
    deck_engines::apply(p, false)?;
    line(format!(
        "[apply] '{}' live on :{} ({}), health OK",
        p.name,
        p.port,
        format!("{:?}", p.engine).to_lowercase()
    ));
    Ok(())
}

/// Measure generation tok/s against the freshly-applied engine and record
/// a bench row (BRINGUP step 4). Uses the single measurement path
/// (`deck_engines::measure_generation_tps`: scrape, then a 192-token
/// probe generation if the idle gauge reads 0). Returns the measured
/// tok/s, or None when the probe itself fails.
fn bench_and_record(app2: &tauri::AppHandle, p: &Profile, line: &impl Fn(String)) -> Option<f64> {
    let _ = app2.emit(
        "bringup-phase",
        BringupPhase {
            phase: "bench".into(),
        },
    );
    let tps = match deck_engines::measure_generation_tps(p.engine, &p.host, p.port, &p.alias) {
        Ok(v) => {
            line(format!("[bench] probe: {v:.1} tok/s"));
            Some(v)
        }
        Err(e) => {
            line(format!("[bench] probe failed: {e}"));
            None
        }
    };
    if let Some(v) = tps {
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
                hardware_profile_id: None,
                engine_version: None,
                prompt_tps: None,
                ttft_ms: None,
            };
            match deck_core::store::insert_bench(&conn, &row) {
                Ok(id) => line(format!("[bench] recorded #{id}: {v:.1} tok/s")),
                Err(e) => line(format!("[bench] record failed: {e}")),
            }
        }
        Some(v)
    } else {
        None
    }
}

/// Shared derive → headless verify phases for BRINGUP (goes live afterwards)
/// and TEST (stops here). Runs derive, emits the derived profile for the tweak
/// panel, then verifies on the engine's test port with a ctx ladder, collecting
/// tok/s straight off the test engine. On any failure it reports through
/// `finish` and returns None; on success returns the verified profile, the fit
/// breakdown, and the measured tok/s.
///
/// `save_on_fail` mirrors bringup's trick of persisting the derived profile so
/// the tweak panel can retry; TEST leaves the loadout list alone.
#[allow(clippy::type_complexity)]
fn derive_and_verify(
    app2: &tauri::AppHandle,
    model: &str,
    eng: Engine,
    fast: bool,
    save_on_fail: bool,
    line: &impl Fn(String),
    finish: &impl Fn(BringupResult),
) -> Option<(Profile, FitBreakdown, Option<f64>)> {
    line(format!("[derive] reading {} header…", model));
    let derived = match deck_core::profile::derive_loadout(model, eng) {
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
            return None;
        }
    };
    // Emit the profile so the frontend tweak panel can access it.
    let _ = app2.emit("bringup-profile", derived.profile.clone());
    let mut p = derived.profile;
    p.name = p.alias.clone();
    // Substitute a configured engine executable when the derived default does
    // not exist on disk (e.g. /usr/bin/llama-server here).
    if let Ok(conn) = deck_core::store::open(&deck_core::store::default_db_path())
        && let Ok(p2) = deck_core::store::resolve_engine_bin(&conn, p.clone())
    {
        if p2.bin != p.bin {
            line(format!("[derive] engine binary: {}", p2.bin.display()));
        }
        p = p2;
    }
    let offload = derived.weights_ram_mb > 0;
    let fit = FitBreakdown {
        weights_mb: if offload {
            derived.weights_gpu_mb + derived.weights_ram_mb
        } else {
            derived.weights_gpu_mb
        },
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

    if !fast {
        let test_port = eng.test_port();
        let _ = app2.emit(
            "bringup-phase",
            BringupPhase {
                phase: "verify".into(),
            },
        );
        line(format!(
            "[verify] loading on :{test_port} — live :{} untouched… (this takes 10–30s for 20G+ models, check `nvidia-smi` for VRAM)",
            p.port
        ));
        line(format!("[verify] ctx={} weights GPU {} MiB + RAM {} MiB — waiting for health probe…", p.ctx_size, derived.weights_gpu_mb, derived.weights_ram_mb));
        let outcome = deck_engines::verify_on_test_port(&p, test_port, Duration::from_secs(180));
        line(format!(
            "[verify] {} ({})",
            outcome.summary, outcome.verdict
        ));
        if outcome.verdict != "RUNNING" {
            if save_on_fail {
                // Save the derived profile even on failure so the tweak panel
                // has something to work with — the user can adjust ctx/kv/offload
                // and re-verify without re-running the full derive.
                let db = deck_core::store::default_db_path();
                if let Ok(conn) = deck_core::store::open(&db) {
                    let _ = deck_core::store::ensure_profile_schema(&conn);
                    if deck_core::store::upsert_profile(&conn, &p).is_ok() {
                        line(format!(
                            "[save] derived profile '{}' saved (tweak & retry)",
                            p.name
                        ));
                    }
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
            return None;
        }
        if outcome.ctx != p.ctx_size {
            line(format!("[verify] ctx walked down to {}", outcome.ctx));
            p.ctx_size = outcome.ctx;
        }
        return Some((p, fit, outcome.tok_per_sec));
    }

    Some((p, fit, None))
}

/// Headless TEST — derive + verify on the test port, report tok/s, then stop.
/// Never saves a profile, installs a unit, or restarts the live service.
/// Shares BRINGUP's single-flight lock and event channels so the frontend
/// panel renders identically.
pub fn test_model_start(
    app: &tauri::AppHandle,
    model_path: &str,
    engine_s: &str,
) -> anyhow::Result<()> {
    if !std::path::Path::new(model_path).exists() {
        anyhow::bail!("model not found on disk: {model_path}");
    }
    let eng = Engine::parse(engine_s)
        .ok_or_else(|| anyhow::anyhow!("unknown engine '{engine_s}' (llamacpp|freetoken)"))?;
    if BRINGUP_RUNNING.swap(true, Ordering::SeqCst) {
        anyhow::bail!("a bring-up or test is already running");
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
        let _guard = BringupGuard;
        let finish = |res: BringupResult| {
            let _ = app2.emit("bringup-result", res);
            let _ = app2.emit(
                "bringup-phase",
                BringupPhase {
                    phase: "done".into(),
                },
            );
            BRINGUP_RUNNING.store(false, Ordering::SeqCst);
        };
        let line = |t: String| {
            let _ = app2.emit("bringup-line", BringupLine { text: t });
        };

        // derive + verify on the test port — the live service is never touched.
        let Some((p, fit, tps)) =
            derive_and_verify(&app2, &model, eng, false, false, &line, &finish)
        else {
            return;
        };

        // Record a bench row tagged as test-port so the score survives
        // the dry-run (the row's port distinguishes it from a live run).
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
    });

    Ok(())
}

/// Apply a previously-verified profile (e.g. from a headless TEST) —
/// save + start via systemd and bench+record, skipping re-derive and
/// re-verify. Shares the single-flight lock and event channels so the
/// frontend panel renders identically to bringup.
pub fn apply_cached_profile(
    app: &tauri::AppHandle,
    profile: Profile,
    fit: Option<FitBreakdown>,
) -> anyhow::Result<()> {
    if !std::path::Path::new(&profile.model).exists() {
        anyhow::bail!("model not found on disk: {}", profile.model);
    }
    if BRINGUP_RUNNING.swap(true, Ordering::SeqCst) {
        anyhow::bail!("a bring-up is already running");
    }
    let app2 = app.clone();
    let profile2 = profile.clone();
    std::thread::spawn(move || {
        let _guard = BringupGuard;
        let finish = |res: BringupResult| {
            let _ = app2.emit("bringup-result", res);
            let _ = app2.emit(
                "bringup-phase",
                BringupPhase {
                    phase: "done".into(),
                },
            );
            BRINGUP_RUNNING.store(false, Ordering::SeqCst);
        };
        let line = |t: String| {
            let _ = app2.emit("bringup-line", BringupLine { text: t });
        };

        // 1. Save + apply (skip derive + verify — already done) -----
        if let Err(e) = save_and_apply(&app2, &profile2, &line) {
            finish(BringupResult {
                ok: false,
                summary: format!("apply failed: {e}"),
                name: profile2.name.clone(),
                port: profile2.port,
                ctx: profile2.ctx_size,
                tps: None,
                fit,
            });
            return;
        }

        // 2. Bench + record (real measurement) -----------------------
        let tps = bench_and_record(&app2, &profile2, &line);

        finish(BringupResult {
            ok: true,
            summary: format!(
                "'{}' applied on :{} at ctx={}{}",
                profile2.name,
                profile2.port,
                profile2.ctx_size,
                tps.map(|t| format!(" · {t:.1} tok/s")).unwrap_or_default()
            ),
            name: profile2.name.clone(),
            port: profile2.port,
            ctx: profile2.ctx_size,
            tps,
            fit,
        });
    });

    Ok(())
}
